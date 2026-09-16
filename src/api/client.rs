use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;

use super::region::{self, Region};
use super::{
    gen_skype_token, gen_token, renew_refresh_token, SCOPE_CHATSVCAGG, SCOPE_GRAPH, SCOPE_IC3,
    SCOPE_SPACES,
};
use crate::cache::{Cache, REGION_FILE, TOKENS_FILE};
use crate::config::Config;
use crate::types::*;

fn get_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Whether a failed renewal means the refresh token itself is finished, so
/// that only a new login can help. `invalid_grant` is the OAuth answer for it;
/// AADSTS70043 is the sign-in frequency check that expires one early.
fn is_dead_refresh_token(complaint: &str) -> bool {
    complaint.contains("invalid_grant") || complaint.contains("AADSTS70043")
}

/// Simple HTML stripper for quoted messages
fn strip_html_simple(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

/// The blockquote Teams puts at the top of a reply.
///
/// Every attribute matters: without `itemtype` it is a plain quote, and without
/// the two `itemid`s the client cannot jump to the message being answered.
fn reply_quote(message_id: &str, from_mri: &str, author: &str, content: &str) -> String {
    let preview = strip_html_simple(content);
    let preview = if preview.chars().count() > REPLY_PREVIEW_CHARS {
        let cut: String = preview.chars().take(REPLY_PREVIEW_CHARS - 1).collect();
        format!("{cut}\u{2026}")
    } else {
        preview
    };
    format!(
        concat!(
            r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="{id}">"#,
            r#"<strong itemprop="mri" itemid="{mri}">{author}</strong>"#,
            r#"<span itemprop="time" itemid="{id}"></span>"#,
            r#"<p itemprop="preview">{preview}</p></blockquote>"#,
        ),
        id = escape_attribute(message_id),
        mri = escape_attribute(from_mri),
        author = escape_text(author),
        preview = escape_text(&preview),
    )
}

/// How much of the answered message the quote repeats. Teams shows a line, not
/// the message again.
///
/// The ellipsis is one of them: Teams keeps 199 characters and a single `…`,
/// never 200 and three dots. Measured against replies its own clients sent.
const REPLY_PREVIEW_CHARS: usize = 200;

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

/// Microsoft Teams API client
pub struct TeamsClient {
    tokens: Arc<RwLock<TokenStore>>,
    tenant: String,
    pub(crate) http: Client,
    cache: Cache,
    /// Own profile, kept after the first lookup. Sending a message needs it,
    /// and re-fetching it added a Graph round trip to every send.
    me: Arc<RwLock<Option<Profile>>>,
    /// Tenant region, `None` until discovery lands. Readers fall back to the
    /// default, so no call has to wait for it.
    region: Arc<RwLock<Option<Region>>>,
    /// Trouter endpoint id. Stable for the life of the client: a fresh one
    /// registers a second endpoint with the service on every reconnect.
    epid: String,
    /// Trouter URL to reconnect through, as handed to us by the service.
    trouter_reconnect_url: Arc<RwLock<Option<String>>>,
    /// Base URL the current Trouter session listens on. A subscription has to
    /// name it, and it changes with every session.
    trouter_surl: Arc<RwLock<Option<String>>>,
    /// Users we want presence for. A subscription dies with the endpoint, so the
    /// list is kept and re-sent on every reconnect.
    presence_users: Arc<RwLock<Vec<String>>>,
    /// Tenant GUID, read out of a token on first ask.
    tenant_id: Arc<RwLock<Option<String>>>,
}

/// A file uploaded to OneDrive and shared, ready for a chat message to point at.
#[derive(Debug, Clone)]
pub struct SharedFile {
    /// GUID taken from the driveItem eTag: what a reference attachment keys on.
    pub id: String,
    pub name: String,
    /// Organisation-scoped link, without which the recipient cannot open the file.
    pub url: String,
}

/// Above this size a single PUT is refused and Graph wants an upload session.
const MAX_SIMPLE_UPLOAD: u64 = 4 * 1024 * 1024;

/// The folder Teams uses for files sent in a chat, in an English drive. Its real
/// name is localised per user, so it is only the fallback when none is found.
const CHAT_FILES_FOLDER: &str = "Microsoft Teams Chat Files";

/// What a caller that names no page size gets, and the most the service will
/// hand back in one response.
pub const DEFAULT_PAGE_SIZE: usize = 200;

/// Whose profile photo is being asked for. Graph keeps people and groups on
/// separate collections and answers 404 when an id is looked up under the wrong
/// one, so the caller has to say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoSubject {
    Person,
    Group,
}

impl PhotoSubject {
    fn path(self) -> &'static str {
        match self {
            PhotoSubject::Person => "users",
            PhotoSubject::Group => "groups",
        }
    }
}

/// Bots and apps have no profile photo, and their MRI is not a Graph id at all.
pub const BOT_MRI_PREFIX: &str = "28:";

/// The Graph object id inside a Teams MRI, or the id unchanged when it is
/// already one. `None` for a bot, which Graph knows nothing about.
pub fn photo_object_id(id: &str) -> Option<&str> {
    if id.starts_with(BOT_MRI_PREFIX) {
        return None;
    }
    let bare = id
        .strip_prefix("8:orgid:")
        .or_else(|| id.strip_prefix("8:lync:"))
        .unwrap_or(id);
    (!bare.is_empty()).then_some(bare)
}

/// The `tid` claim of a JWT: the tenant the token was issued for. The payload
/// is the middle part, base64url without padding.
pub fn tenant_from_token(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let tid = claims.get("tid")?.as_str()?;
    (!tid.is_empty()).then(|| tid.to_string())
}

/// Pull the GUID out of a driveItem eTag, which looks like `"{GUID},1"`.
fn etag_guid(etag: &str) -> Option<String> {
    let start = etag.find('{')? + 1;
    let end = etag.find('}')?;
    (start < end).then(|| etag[start..end].to_string())
}

impl TeamsClient {
    /// Create a new Teams client
    pub fn new(config: &Config) -> Result<Self> {
        let cache = Cache::new()?;
        let tokens: TokenStore = cache.load(TOKENS_FILE)?.unwrap_or_default();
        let env_region = region::env_region();
        let persisted: Option<String> = cache.load(REGION_FILE).unwrap_or_default();
        let region = region::initial_region(env_region.as_deref(), persisted.as_deref());

        Ok(Self {
            tokens: Arc::new(RwLock::new(tokens)),
            tenant: config.auth.tenant.clone(),
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            cache,
            me: Arc::new(RwLock::new(None)),
            region: Arc::new(RwLock::new(region)),
            epid: uuid::Uuid::new_v4().to_string(),
            trouter_reconnect_url: Arc::new(RwLock::new(None)),
            trouter_surl: Arc::new(RwLock::new(None)),
            presence_users: Arc::new(RwLock::new(Vec::new())),
            tenant_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Check if the client is authenticated
    pub fn is_authenticated(&self) -> bool {
        self.tokens.read().unwrap().refresh_token().is_some()
    }

    /// Save tokens to cache
    fn save_tokens(&self) -> Result<()> {
        let tokens = self.tokens.read().unwrap();
        self.cache.save(TOKENS_FILE, &*tokens)
    }

    /// Store refresh token after authentication
    pub fn store_refresh_token(&self, token: AccessToken) -> Result<()> {
        {
            let mut tokens = self.tokens.write().unwrap();
            tokens.insert("refresh_token".to_string(), token);
        }
        self.save_tokens()
    }

    /// Clear all tokens (logout)
    pub fn clear_tokens(&self) -> Result<()> {
        {
            let mut tokens = self.tokens.write().unwrap();
            tokens.tokens.clear();
        }
        // The next login may be a tenant in another region, so discover again.
        *self.region.write().unwrap() =
            region::initial_region(region::env_region().as_deref(), None);
        let _ = self.cache.delete(REGION_FILE);
        self.cache.delete(TOKENS_FILE)
    }

    /// Get or generate an access token for a scope
    pub async fn get_token(&self, scope: &str) -> Result<AccessToken> {
        // Check if refresh token needs renewal
        let refresh_token = {
            let tokens = self.tokens.read().unwrap();
            tokens.refresh_token().cloned()
        };

        let refresh_token = match refresh_token {
            Some(token) if token.expires < get_epoch_s() => {
                let new_token = match renew_refresh_token(&token, &self.tenant).await {
                    Ok(token) => token,
                    // The token is dead, not the request: a sign-in frequency
                    // check, a revoked session, or ninety days of silence.
                    // Keeping it would fail the same way on every command.
                    Err(e) if is_dead_refresh_token(&e.to_string()) => {
                        self.clear_tokens()?;
                        return Err(anyhow!(
                            "Not authenticated. Run 'squads-cli auth login' first."
                        ));
                    }
                    Err(e) => return Err(e),
                };
                {
                    let mut tokens = self.tokens.write().unwrap();
                    tokens.insert("refresh_token".to_string(), new_token.clone());
                }
                self.save_tokens()?;
                new_token
            }
            Some(token) => token,
            None => {
                return Err(anyhow!(
                    "Not authenticated. Run 'squads-cli auth login' first."
                ))
            }
        };

        // Check if we have a valid token for this scope
        let existing_token = {
            let tokens = self.tokens.read().unwrap();
            tokens.get(scope).cloned()
        };

        if let Some(token) = existing_token {
            if token.expires >= get_epoch_s() {
                return Ok(token);
            }
        }

        // Generate new token
        let new_token = gen_token(&refresh_token, scope, &self.tenant).await?;
        {
            let mut tokens = self.tokens.write().unwrap();
            tokens.insert(scope.to_string(), new_token.clone());
        }
        self.save_tokens()?;

        Ok(new_token)
    }

    /// Get or generate a Skype token
    pub async fn get_skype_token(&self) -> Result<AccessToken> {
        // Check if we have a valid skype token
        let existing_token = {
            let tokens = self.tokens.read().unwrap();
            tokens.skype_token().cloned()
        };

        if let Some(token) = existing_token {
            if token.expires >= get_epoch_s() {
                return Ok(token);
            }
        }

        // Get spaces token first
        let spaces_token = self.get_token(SCOPE_SPACES).await?;

        // Generate skype token
        let new_token = gen_skype_token(&spaces_token).await?;
        {
            let mut tokens = self.tokens.write().unwrap();
            tokens.insert("skype_token".to_string(), new_token.clone());
        }
        self.save_tokens()?;

        Ok(new_token)
    }

    /// Region to build a URL with, probing first when we have not learned one.
    /// Never call it from `ensure_region`: the probe URL carries no region.
    pub(crate) async fn regional(&self) -> Region {
        self.ensure_region().await;
        self.region.read().unwrap().clone().unwrap_or_default()
    }

    /// Endpoint id Trouter and the registrar know us by.
    pub(crate) fn trouter_epid(&self) -> &str {
        &self.epid
    }

    /// Reconnect URL kept from the last session, if the service gave one.
    pub(crate) fn trouter_reconnect_url(&self) -> Option<String> {
        self.trouter_reconnect_url.read().unwrap().clone()
    }

    pub(crate) fn set_trouter_reconnect_url(&self, url: Option<String>) {
        *self.trouter_reconnect_url.write().unwrap() = url;
    }

    /// Base URL of the live Trouter session, `None` between sessions.
    pub(crate) fn trouter_surl(&self) -> Option<String> {
        self.trouter_surl.read().unwrap().clone()
    }

    pub(crate) fn set_trouter_surl(&self, surl: Option<String>) {
        *self.trouter_surl.write().unwrap() = surl;
    }

    /// Users the presence subscription covers.
    pub(crate) fn presence_users(&self) -> Vec<String> {
        self.presence_users.read().unwrap().clone()
    }

    pub(crate) fn set_presence_users(&self, users: Vec<String>) {
        *self.presence_users.write().unwrap() = users;
    }

    /// The one place a region is learned. Takes any text holding Teams URLs:
    /// a redirect target, or a payload full of conversation links.
    fn observe_region(&self, text: &str) {
        if self.region.read().unwrap().is_some() {
            return;
        }
        let Some(found) = region::extract_region_from_url(text) else {
            return;
        };
        *self.region.write().unwrap() = Some(found.clone());
        // Losing the write only costs one probe on the next run.
        let _ = self.cache.save(REGION_FILE, &found.name().to_string());
    }

    /// Ask the region-less csa endpoint who we are. Teams answers with a
    /// redirect naming the tenant's region. A failure is not fatal.
    async fn ensure_region(&self) {
        if self.region.read().unwrap().is_some() {
            return;
        }
        let Ok(token) = self.get_token(SCOPE_CHATSVCAGG).await else {
            return;
        };
        let res = self
            .http
            .get(region::CSA_PROBE_URL)
            .header("authorization", format!("Bearer {}", token.value))
            .send()
            .await;
        if let Ok(res) = res {
            if let Some(location) = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            {
                self.observe_region(location);
            }
        }
    }

    /// Get current user's teams and chats
    pub async fn get_user_details(&self) -> Result<UserDetails> {
        let token = self.get_token(SCOPE_CHATSVCAGG).await?;
        let url = format!("{}/users/me", self.regional().await.csa_base());

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self
            .http
            .get(url)
            .headers(headers)
            .query(&[
                ("isPrefetch", "false"),
                ("enableMembershipSummary", "true"),
                ("enableRC2Fetch", "false"),
            ])
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            // Chat payloads carry regional links, so this covers a failed probe.
            self.observe_region(&body);
            serde_json::from_str(&body).context("Failed to parse user details")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get user details: {} - {}", status, body))
        }
    }

    /// Get current user profile, from memory after the first call.
    pub async fn get_me(&self) -> Result<Profile> {
        if let Some(me) = self.me.read().unwrap().clone() {
            return Ok(me);
        }
        let me = self.fetch_me().await?;
        *self.me.write().unwrap() = Some(me.clone());
        Ok(me)
    }

    async fn fetch_me(&self) -> Result<Profile> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse profile")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get profile: {} - {}", status, body))
        }
    }

    /// Get organization users
    pub async fn get_users(&self, params: Option<&str>) -> Result<Users> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = match params {
            Some(p) => format!("https://graph.microsoft.com/v1.0/users?{}", p),
            None => "https://graph.microsoft.com/v1.0/users?$top=100".to_string(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse users")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get users: {} - {}", status, body))
        }
    }

    /// Search users by display name or email (uses advanced query capabilities)
    pub async fn search_users(&self, query: &str, limit: usize) -> Result<Users> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        // Use $search with displayName for partial matching
        let url = format!(
            "https://graph.microsoft.com/v1.0/users?$search=\"displayName:{}\" OR \"mail:{}\"&$top={}&$orderby=displayName",
            query, query, limit
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        // Required for $search queries
        headers.insert(
            HeaderName::from_static("consistencylevel"),
            HeaderValue::from_static("eventual"),
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse user search results")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to search users: {} - {}", status, body))
        }
    }

    /// Get a user by their ID (object_id from MRI)
    pub async fn get_user_by_id(&self, user_id: &str) -> Result<Option<Profile>> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/users/{}?$select=id,displayName,mail",
            user_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            Ok(Some(
                serde_json::from_str(&body).context("Failed to parse user")?,
            ))
        } else if res.status() == 404 {
            Ok(None)
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get user: {} - {}", status, body))
        }
    }

    /// Download a picture from a message.
    ///
    /// Teams keeps its own attachments behind the chat service and needs the
    /// IC3 token; anything else (a Giphy link, a CDN) is public and must be
    /// fetched bare, since sending a bearer token to a third party would leak
    /// it.
    pub async fn fetch_picture(&self, url: &str) -> Result<Vec<u8>> {
        let mut request = self.http.get(url);
        if is_microsoft_host(url) {
            let token = self.get_token(SCOPE_IC3).await?;
            request = request.header("authorization", format!("Bearer {}", token.value));
        }

        let res = request.send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("Failed to fetch picture: {}", res.status()));
        }
        Ok(res.bytes().await?.to_vec())
    }

    /// A profile photo and its content type, or `None` when there is none.
    ///
    /// Most people never set one, and Graph answers 404 for them as well as for
    /// an id it cannot see at all. Neither is a failure worth an error, so both
    /// come back as `None` and the caller decides what to say.
    pub async fn fetch_profile_photo(
        &self,
        id: &str,
        subject: PhotoSubject,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/{}/{}/photo/$value",
            subject.path(),
            id
        );

        let res = self
            .http
            .get(&url)
            .header("authorization", format!("Bearer {}", token.value))
            .send()
            .await?;

        // 403 joins 404: a tenant that hides its members' photos is no more a
        // failure than a person who never set one.
        if res.status() == 404 || res.status() == 403 {
            return Ok(None);
        }
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await?;
            return Err(anyhow!(
                "Failed to fetch profile photo: {} - {}",
                status,
                body
            ));
        }

        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|| "image/jpeg".to_string());
        let bytes = res.bytes().await?.to_vec();

        // An empty 200 would otherwise be cached as a zero-byte picture.
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some((content_type, bytes)))
    }

    /// The tenant the signed-in account belongs to, read out of a token.
    ///
    /// Nothing serves it as a field, and a custom emote's image URL is built
    /// from it. Kept after the first read: it never changes while signed in.
    pub async fn tenant_id(&self) -> Result<String> {
        if let Some(known) = self.tenant_id.read().unwrap().clone() {
            return Ok(known);
        }
        let token = self.get_token(SCOPE_IC3).await?;
        let found = tenant_from_token(&token.value)
            .ok_or_else(|| anyhow!("No tenant in the access token"))?;
        *self.tenant_id.write().unwrap() = Some(found.clone());
        Ok(found)
    }

    /// A custom emote's animated image and its content type, or `None` when the
    /// tenant has no such object.
    ///
    /// Like a profile photo, "there is none" is not a failure: an emote deleted
    /// since someone reacted with it leaves the key behind on the message.
    pub async fn fetch_custom_emote(&self, object_id: &str) -> Result<Option<(String, Vec<u8>)>> {
        let tenant = self.tenant_id().await?;
        let token = self.get_token(SCOPE_IC3).await?;
        let url = self.regional().await.custom_emoji_url(&tenant, object_id);

        let res = self
            .http
            .get(&url)
            .header("authorization", format!("Bearer {}", token.value))
            .send()
            .await?;

        if res.status() == 404 || res.status() == 403 {
            return Ok(None);
        }
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await?;
            return Err(anyhow!(
                "Failed to fetch custom emote: {} - {}",
                status,
                body
            ));
        }

        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|| "image/gif".to_string());
        let bytes = res.bytes().await?.to_vec();
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some((content_type, bytes)))
    }

    /// Everyone's read marker in a chat, so a message you sent can say whether
    /// it has been read.
    ///
    /// Its own request: no list endpoint carries another member's marker, and
    /// the push socket only reports one when somebody moves it, which says
    /// nothing about what happened before the window opened.
    pub async fn get_consumption_horizons(&self, chat_id: &str) -> Result<ConsumptionHorizons> {
        let token = self.get_token(SCOPE_IC3).await?;
        let url = format!(
            "{}/threads/{}/consumptionhorizons",
            self.regional().await.chatsvc_thread_base(),
            urlencoding::encode(chat_id)
        );

        let res = self
            .http
            .get(&url)
            .header("authorization", format!("Bearer {}", token.value))
            .send()
            .await?;

        let status = res.status();
        if !status.is_success() {
            return Err(anyhow!("Failed to get read receipts: {status}"));
        }
        Ok(res.json().await?)
    }

    /// Move the chat's read watermark to now, which is what clears its unread
    /// state in Teams. `isRead` is derived from this server side, so without it
    /// a chat stays unread everywhere no matter how often you open it.
    ///
    /// The horizon is `originalArrivalTime;timeStamp;clientMessageId`. Passing
    /// the current time as the arrival time marks everything up to now as read,
    /// which avoids needing the exact id of the newest message.
    pub async fn mark_chat_read(&self, chat_id: &str, last_message_id: Option<&str>) -> Result<()> {
        let token = self.get_token(SCOPE_IC3).await?;
        let now = chrono::Utc::now().timestamp_millis();
        let fallback = now.to_string();
        let horizon = format!("{};0;{}", now, last_message_id.unwrap_or(&fallback));

        let url = format!(
            "{}/conversations/{}/properties?name=consumptionhorizon",
            self.regional().await.chatsvc_base(),
            chat_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self
            .http
            .put(&url)
            .headers(headers)
            .json(&serde_json::json!({ "consumptionhorizon": horizon }))
            .send()
            .await?;

        if res.status().is_success() {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to mark chat read: {} - {}", status, body))
        }
    }

    /// Resolve several display names from a single chat, keyed by user object ID.
    /// One message fetch covers every member of the chat.
    ///
    /// Graph `/users/{id}` fails for cross-tenant users, but every message
    /// carries `imdisplayname` for its sender.
    pub async fn resolve_names_from_messages(
        &self,
        chat_id: &str,
        user_ids: &[String],
    ) -> Result<HashMap<String, String>> {
        let convs = self.get_conversations(chat_id, None).await?;
        // Deduplicated: a repeated ID would make the "found them all" check below
        // unreachable, so every message would be scanned for nothing.
        let mut wanted: Vec<(&String, String)> = Vec::new();
        for id in user_ids {
            if !wanted.iter().any(|(seen, _)| *seen == id) {
                wanted.push((id, format!("8:orgid:{}", id)));
            }
        }

        let mut names: HashMap<String, String> = HashMap::new();
        for msg in &convs.messages {
            let (Some(from), Some(name)) = (&msg.from, &msg.im_display_name) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            for (id, mri_suffix) in &wanted {
                if !names.contains_key(*id) && from.contains(mri_suffix) {
                    names.insert((*id).clone(), name.clone());
                }
            }
            if names.len() == wanted.len() {
                break;
            }
        }
        Ok(names)
    }

    /// Get conversations/messages from a chat
    pub async fn get_conversations(
        &self,
        thread_id: &str,
        message_id: Option<u64>,
    ) -> Result<Conversations> {
        self.get_conversations_page(thread_id, message_id, DEFAULT_PAGE_SIZE)
            .await
    }

    /// Get conversations/messages from a chat, asking the service for at most
    /// `page_size` of them. A caller that shows twenty messages pays for twenty.
    pub async fn get_conversations_page(
        &self,
        thread_id: &str,
        message_id: Option<u64>,
        page_size: usize,
    ) -> Result<Conversations> {
        let token = self.get_token(SCOPE_IC3).await?;

        let thread_part = match message_id {
            Some(msg_id) => format!("{};messageid={}", thread_id, msg_id),
            None => thread_id.to_string(),
        };

        let url = format!(
            "{}/conversations/{}/messages?pageSize={}",
            self.regional().await.chatsvc_base(),
            thread_part,
            page_size.clamp(1, DEFAULT_PAGE_SIZE)
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse conversations")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get conversations: {} - {}",
                status,
                body
            ))
        }
    }

    /// Get team channel conversations
    pub async fn get_team_conversations(
        &self,
        team_id: &str,
        channel_id: &str,
    ) -> Result<TeamConversations> {
        let token = self.get_token(SCOPE_CHATSVCAGG).await?;
        let url = format!(
            "{}/{}/channels/{}",
            self.regional().await.csa_base(),
            team_id,
            channel_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse team conversations")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get team conversations: {} - {}",
                status,
                body
            ))
        }
    }

    /// Debug: Print thread structure to understand root vs reply messages
    pub async fn debug_thread_structure(&self, team_id: &str, channel_id: &str) -> Result<()> {
        let conversations = self.get_team_conversations(team_id, channel_id).await?;

        eprintln!("\n=== Thread Structure Debug ===");
        eprintln!(
            "Total threads (reply_chains): {}\n",
            conversations.reply_chains.len()
        );

        for (i, chain) in conversations.reply_chains.iter().enumerate().take(10) {
            eprintln!("--- Thread {} ---", i);
            eprintln!("  Chain ID: {}", chain.id);
            eprintln!("  Container ID: {}", chain.container_id);
            eprintln!("  Message count: {}", chain.messages.len());

            for (j, msg) in chain.messages.iter().take(3).enumerate() {
                let content_preview = msg
                    .content
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect::<String>();
                eprintln!("    [{}] ID: {:?}", j, msg.id);
                eprintln!("        From: {:?}", msg.im_display_name);
                eprintln!("        Content: {}...", content_preview);
            }
            eprintln!();
        }

        Ok(())
    }

    /// Find the root message ID of a thread containing the given message
    /// If the message_id is already a root, returns it as-is
    /// If the message_id is a reply within a thread, returns the thread's root message ID
    pub async fn find_thread_root(
        &self,
        team_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<String> {
        let conversations = self.get_team_conversations(team_id, channel_id).await?;

        // Search all chains to find which one contains this message
        for chain in &conversations.reply_chains {
            // Check if message_id matches the chain ID (it's already the root)
            if chain.id == message_id {
                return Ok(chain.id.clone());
            }

            // Check if message_id is in this chain's messages
            for msg in &chain.messages {
                if msg.id.as_deref() == Some(message_id) {
                    // Found the message - return the chain ID (root)
                    return Ok(chain.id.clone());
                }
            }
        }

        // Message not found in any thread - it might be a root message itself
        // or the conversations haven't been fetched yet, so return as-is
        Ok(message_id.to_string())
    }

    /// Process @mentions in content and return (processed_content, mentions_json)
    /// Looks up user by name and replaces @Name with proper Teams mention spans
    pub async fn process_mentions(&self, content: &str) -> Result<(String, String)> {
        let mut mentions: Vec<serde_json::Value> = Vec::new();
        let mut user_mention_ids: std::collections::HashMap<String, i32> =
            std::collections::HashMap::new();
        let mut processed = content.to_string();
        let mut next_mention_id = 0;

        // Find @Name patterns - capture first name + optional last name (uppercase start)
        let re_pattern =
            regex::Regex::new(r"@([A-Za-zÀ-ÿ][-A-Za-zÀ-ÿ]*)(?:\s+([A-ZÀ-Ý][-A-Za-zÀ-ÿ]*))?").ok();

        // Common words to exclude from being treated as last names
        let common_words: std::collections::HashSet<&str> = [
            "And", "Or", "The", "Is", "Was", "Are", "Were", "Has", "Have", "Had", "For", "With",
            "From", "This", "That", "Here", "There", "When", "Where", "Et", "Ou", "Le", "La",
            "Les", "Est", "Sont", "Avec", "Pour", "Dans",
        ]
        .iter()
        .cloned()
        .collect();

        if let Some(re) = re_pattern {
            let matches: Vec<_> = re
                .captures_iter(content)
                .map(|cap| {
                    let full_match = cap.get(0).unwrap().as_str().to_string();
                    let first_name = cap.get(1).unwrap().as_str().to_string();
                    let last_name = cap.get(2).map(|m| m.as_str().to_string());
                    let last_name = last_name.filter(|ln| !common_words.contains(ln.as_str()));
                    let full_match = if last_name.is_none() && cap.get(2).is_some() {
                        format!("@{}", first_name)
                    } else {
                        full_match
                    };
                    (full_match, first_name, last_name)
                })
                .collect();

            for (full_match, first_name, last_name) in matches {
                let (search_name, display_text) = if let Some(ref last) = last_name {
                    let full_name = format!("{} {}", first_name, last);
                    match self.search_users(&full_name, 1).await {
                        Ok(users) if !users.value.is_empty() => (full_name.clone(), full_name),
                        _ => (first_name.clone(), format!("{} {}", first_name, last)),
                    }
                } else {
                    (first_name.clone(), first_name.clone())
                };

                if let Ok(users) = self.search_users(&search_name, 1).await {
                    if let Some(user) = users.value.first() {
                        let user_id = user.id.clone();

                        // Reuse same mention ID for same user (Teams limitation)
                        let mention_id = if let Some(&id) = user_mention_ids.get(&user_id) {
                            id
                        } else {
                            let id = next_mention_id;
                            next_mention_id += 1;
                            user_mention_ids.insert(user_id.clone(), id);
                            // Only add to mentions array once per user
                            let mention = serde_json::json!({
                                "id": id,
                                "mri": format!("8:orgid:{}", user_id),
                                "displayName": display_text
                            });
                            mentions.push(mention);
                            id
                        };

                        let mention_span = format!(
                            "<span itemtype=\"http://schema.skype.com/Mention\" itemscope=\"\" itemid=\"{}\">{}</span>",
                            mention_id, display_text
                        );
                        processed = processed.replacen(&full_match, &mention_span, 1);
                    }
                }
            }
        }

        let mentions_json = serde_json::to_string(&mentions)?;
        Ok((processed, mentions_json))
    }

    /// Send a message to a team channel (uses Teams internal API)
    pub async fn send_channel_message(
        &self,
        _team_id: &str,
        channel_id: &str,
        content: &str,
        subject: Option<&str>,
    ) -> Result<serde_json::Value> {
        let token = self.get_token(SCOPE_IC3).await?;
        let me = self.get_me().await?;

        // Process mentions in content
        let (processed_content, mentions_json) = self.process_mentions(content).await?;

        // Use the channel ID as the conversation ID for the Teams internal API
        let url = format!(
            "{}/conversations/{}/messages",
            self.regional().await.chatsvc_base(),
            channel_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        // Generate random message ID
        let message_id: u64 = rand::random();
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();

        let body = serde_json::json!({
            "id": "-1",
            "type": "Message",
            "conversationid": channel_id,
            "conversation_link": format!("blah/{}", channel_id),
            "from": format!("8:orgid:{}", me.id),
            "composetime": now,
            "originalarrivaltime": now,
            "content": processed_content,
            "messagetype": "RichText/Html",
            "contenttype": "Html",
            "imdisplayname": me.display_name,
            "clientmessageid": message_id.to_string(),
            "call_id": "",
            "state": 0,
            "version": "0",
            "amsreferences": [],
            "properties": {
                "importance": "",
                "subject": subject,
                "title": "",
                "cards": "[]",
                "links": "[]",
                "mentions": mentions_json,
                "onbehalfof": null,
                "files": "[]",
                "policy_violation": null,
                "format_variant": "TEAMS"
            },
            "post_type": "Standard",
            "cross_post_channels": []
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(body.to_string())
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            let body = res.text().await?;
            Ok(serde_json::json!({"status": "sent", "response": body}))
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to send channel message: {} - {}",
                status,
                body
            ))
        }
    }

    /// Reply to a message in a team channel using Teams internal API
    /// Note: This will find the thread root if the given message_id is a reply within a thread
    pub async fn reply_channel_message(
        &self,
        team_id: &str,
        channel_id: &str,
        parent_message_id: &str,
        content: &str,
    ) -> Result<serde_json::Value> {
        let token = self.get_token(SCOPE_IC3).await?;
        let me = self.get_me().await?;

        // Find the thread root message ID
        // The provided message_id might be a reply within a thread, not the root
        let root_message_id = self
            .find_thread_root(team_id, channel_id, parent_message_id)
            .await
            .unwrap_or_else(|_| parent_message_id.to_string());

        // Process mentions in content
        let (processed_content, mentions_json) = self.process_mentions(content).await?;

        // For channel thread replies, post to the thread conversation
        // The thread ID format is: {channel_id};messageid={root_message_id}
        let thread_id = format!("{};messageid={}", channel_id, root_message_id);
        let url = format!(
            "{}/conversations/{}/messages",
            self.regional().await.chatsvc_base(),
            thread_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        // Generate random message ID
        let message_id: u64 = rand::random();
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();

        let body = serde_json::json!({
            "id": "-1",
            "type": "Message",
            "conversationid": thread_id,
            "conversation_link": format!("blah/{}", thread_id),
            "from": format!("8:orgid:{}", me.id),
            "composetime": now,
            "originalarrivaltime": now,
            "content": processed_content,
            "messagetype": "RichText/Html",
            "contenttype": "Html",
            "imdisplayname": me.display_name,
            "clientmessageid": message_id.to_string(),
            "call_id": "",
            "state": 0,
            "version": "0",
            "amsreferences": [],
            "properties": {
                "importance": "",
                "subject": null,
                "title": "",
                "cards": "[]",
                "links": "[]",
                "mentions": mentions_json,
                "onbehalfof": null,
                "files": "[]",
                "policy_violation": null,
                "format_variant": "TEAMS"
            },
            "post_type": "Standard",
            "cross_post_channels": []
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(body.to_string())
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            let body = res.text().await?;
            Ok(serde_json::json!({"status": "sent", "response": body}))
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to reply to channel message: {} - {}",
                status,
                body
            ))
        }
    }

    /// Name of the drive folder Teams keeps chat files in.
    ///
    /// It is localised: a French account has "Fichiers de conversation Microsoft
    /// Teams". Hardcoding the English name creates a second folder next to the
    /// real one, which is where files sent from here would go to die.
    async fn chat_files_folder(&self) -> String {
        self.find_chat_files_folder()
            .await
            .unwrap_or_else(|| CHAT_FILES_FOLDER.to_string())
    }

    /// Root folders of the sender's drive, followed across pages.
    ///
    /// A drive root with many items comes back paged, and the folder we want can
    /// sit on any page.
    async fn find_chat_files_folder(&self) -> Option<String> {
        let token = self.get_token(SCOPE_GRAPH).await.ok()?;
        let mut next = Some(
            "https://graph.microsoft.com/v1.0/me/drive/root/children?$select=name,folder&$top=200"
                .to_string(),
        );
        let mut names: Vec<String> = Vec::new();
        let mut pages = 0;

        while let Some(url) = next.take() {
            pages += 1;
            if pages > 20 {
                break;
            }
            let res = self.http.get(&url).bearer_auth(&token.value).send().await;
            let Ok(res) = res else { break };
            let Ok(body) = res.json::<serde_json::Value>().await else {
                break;
            };
            let Some(items) = body["value"].as_array() else {
                break;
            };
            names.extend(
                items
                    .iter()
                    .filter(|i| i.get("folder").is_some())
                    .filter_map(|i| i["name"].as_str())
                    .map(|n| n.to_string()),
            );
            next = body["@odata.nextLink"].as_str().map(|s| s.to_string());
        }

        // Other Teams folders ("Microsoft Teams Data") also live in the root and
        // the listing order is arbitrary, so take the exact name when it is there.
        names
            .iter()
            .find(|n| n.as_str() == CHAT_FILES_FOLDER)
            // Every translation keeps the product name in it.
            .or_else(|| names.iter().find(|n| n.contains("Microsoft Teams")))
            .cloned()
    }

    /// Upload a file to the sender's OneDrive, then share it with the organisation.
    ///
    /// A chat attachment is a reference, never the bytes: Teams keeps the file in
    /// the sender's drive and the message only points at it. Without the sharing
    /// link the recipient sees an attachment they cannot open.
    pub async fn upload_chat_file(&self, path: &std::path::Path) -> Result<SharedFile> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| anyhow!("Not a file: {}", path.display()))?;
        // Check the size first: reading a huge file only to reject it would load
        // the whole thing into memory.
        let size = std::fs::metadata(path)
            .with_context(|| format!("Cannot read {}", path.display()))?
            .len();
        if size > MAX_SIMPLE_UPLOAD {
            return Err(anyhow!(
                "{} is {:.1} MB; files over 4 MB need an upload session, which is not implemented",
                name,
                size as f64 / 1024.0 / 1024.0
            ));
        }
        let bytes =
            std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;

        let token = self.get_token(SCOPE_GRAPH).await?;
        let folder = self.chat_files_folder().await;
        // Rename on conflict: replacing would rewrite the file an earlier message
        // already points at.
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/drive/root:/{}/{}:/content?@microsoft.graph.conflictBehavior=rename",
            urlencoding::encode(&folder),
            urlencoding::encode(&name)
        );

        let res = self
            .http
            .put(&url)
            .bearer_auth(&token.value)
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to upload {}: {} - {}", name, status, body));
        }

        let item: serde_json::Value = res.json().await.context("Failed to parse driveItem")?;
        let item_id = item["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Upload returned no item id"))?
            .to_string();
        let web_url = item["webUrl"].as_str().unwrap_or_default().to_string();
        // The drive renames on conflict, so use the name it gave back.
        let name = item["name"].as_str().unwrap_or(&name).to_string();
        // The attachment is keyed on the eTag GUID; the item id is the fallback.
        let id = item["eTag"]
            .as_str()
            .and_then(etag_guid)
            .unwrap_or_else(|| item_id.clone());

        // An attachment with no URL is one the recipient cannot open, so only
        // fall back to the drive URL, never to nothing.
        let url = match self.share_with_organization(&item_id).await {
            Ok(link) => link,
            Err(e) if !web_url.is_empty() => {
                eprintln!("Warning: no sharing link for {} ({}); using the drive URL, which only members with access can open", name, e);
                web_url
            }
            Err(e) => return Err(e),
        };

        Ok(SharedFile { id, name, url })
    }

    /// Sharing link scoped to the tenant, so any member of the chat can open it.
    async fn share_with_organization(&self, item_id: &str) -> Result<String> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/drive/items/{}/createLink",
            item_id
        );
        let res = self
            .http
            .post(&url)
            .bearer_auth(&token.value)
            .json(&serde_json::json!({ "type": "view", "scope": "organization" }))
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to share file: {} - {}", status, body));
        }

        let permission: serde_json::Value = res.json().await?;
        permission["link"]["webUrl"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Sharing link came back without a URL"))
    }

    /// Send a message carrying file attachments.
    ///
    /// Goes through Graph rather than the internal chat service used by
    /// `send_message`: the reference-attachment shape is documented there, while
    /// the internal `properties.files` payload is not.
    pub async fn send_message_with_files(
        &self,
        chat_id: &str,
        content: &str,
        files: &[SharedFile],
    ) -> Result<String> {
        let token = self.get_token(SCOPE_GRAPH).await?;

        // Each attachment needs its own placeholder in the body, or Teams shows
        // the message without the file.
        let mut body = content.to_string();
        for file in files {
            body.push_str(&format!("<attachment id=\"{}\"></attachment>", file.id));
        }

        let attachments: Vec<serde_json::Value> = files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id,
                    "contentType": "reference",
                    "contentUrl": f.url,
                    "name": f.name,
                })
            })
            .collect();

        let url = format!(
            "https://graph.microsoft.com/v1.0/chats/{}/messages",
            chat_id
        );
        let res = self
            .http
            .post(&url)
            .bearer_auth(&token.value)
            .json(&serde_json::json!({
                "body": { "contentType": "html", "content": body },
                "attachments": attachments,
            }))
            .send()
            .await?;

        if res.status().is_success() {
            res.text().await.context("Failed to read response")
        } else {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            Err(anyhow!("Failed to send message: {} - {}", status, body))
        }
    }

    /// Send a message to a conversation
    pub async fn send_message(
        &self,
        conversation_id: &str,
        content: &str,
        subject: Option<&str>,
    ) -> Result<String> {
        let token = self.get_token(SCOPE_IC3).await?;
        let me = self.get_me().await?;

        let url = format!(
            "{}/conversations/{}/messages",
            self.regional().await.chatsvc_base(),
            conversation_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        // Generate random message ID
        let message_id: u64 = rand::random();
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();

        let body = serde_json::json!({
            "id": "-1",
            "type": "Message",
            "conversationid": conversation_id,
            "conversation_link": format!("blah/{}", conversation_id),
            "from": format!("8:orgid:{}", me.id),
            "composetime": now,
            "originalarrivaltime": now,
            "content": content,
            "messagetype": "RichText/Html",
            "contenttype": "Html",
            "imdisplayname": me.display_name,
            "clientmessageid": message_id.to_string(),
            "call_id": "",
            "state": 0,
            "version": "0",
            "amsreferences": [],
            "properties": {
                "importance": "",
                "subject": subject,
                "title": "",
                "cards": "[]",
                "links": "[]",
                "mentions": "[]",
                "onbehalfof": null,
                "files": "[]",
                "policy_violation": null,
                "format_variant": "TEAMS"
            },
            "post_type": "Standard",
            "cross_post_channels": []
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(body.to_string())
            .send()
            .await?;

        if res.status().is_success() {
            res.text().await.context("Failed to read response")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to send message: {} - {}", status, body))
        }
    }

    /// Create a new chat (1:1 or group) using Graph API
    pub async fn create_chat(&self, members: Vec<&str>, topic: Option<&str>) -> Result<GraphChat> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let me = self.get_me().await?;
        let url = "https://graph.microsoft.com/v1.0/chats";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let chat_type = if members.len() == 1 {
            "oneOnOne"
        } else {
            "group"
        };

        // Build members list including self
        let mut all_members: Vec<serde_json::Value> = vec![serde_json::json!({
            "@odata.type": "#microsoft.graph.aadUserConversationMember",
            "roles": ["owner"],
            "user@odata.bind": format!("https://graph.microsoft.com/v1.0/users('{}')", me.id)
        })];

        for member in members {
            all_members.push(serde_json::json!({
                "@odata.type": "#microsoft.graph.aadUserConversationMember",
                "roles": ["owner"],
                "user@odata.bind": format!("https://graph.microsoft.com/v1.0/users('{}')", member)
            }));
        }

        let mut body = serde_json::json!({
            "chatType": chat_type,
            "members": all_members
        });

        if let Some(t) = topic {
            body["topic"] = serde_json::json!(t);
        }

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse created chat")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to create chat: {} - {}", status, body))
        }
    }

    /// Delete a message from a chat
    pub async fn delete_message(&self, conversation_id: &str, message_id: &str) -> Result<()> {
        let token = self.get_token(SCOPE_IC3).await?;
        let url = format!(
            "{}/conversations/{}/messages/{}",
            self.regional().await.chatsvc_base(),
            conversation_id,
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.delete(&url).headers(headers).send().await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to delete message: {} - {}", status, body))
        }
    }

    /// Delete a message from a team channel
    pub async fn delete_channel_message(
        &self,
        _team_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_IC3).await?;
        let url = format!(
            "{}/conversations/{}/messages/{}",
            self.regional().await.chatsvc_base(),
            channel_id,
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.delete(&url).headers(headers).send().await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to delete channel message: {} - {}",
                status,
                body
            ))
        }
    }

    /// Send a reply in a thread
    /// Note: Graph API replies don't work for 1:1 chats, so we fall back to
    /// sending a regular message with quoted content
    /// Reply to a chat message, quoting it the way Teams does.
    ///
    /// A reply is an ordinary message whose HTML opens with a
    /// `schema.skype.com/Reply` blockquote naming the message it answers.
    /// There is no reply endpoint for a chat: Graph has one for a channel
    /// (`teams reply`), and pointing it at a chat answers 404.
    pub async fn reply_to_message(
        &self,
        chat_id: &str,
        reply_to_id: &str,
        content: &str,
    ) -> Result<()> {
        let conversations = self.get_conversations(chat_id, None).await?;
        let original = conversations
            .messages
            .iter()
            .find(|m| m.id.as_deref() == Some(reply_to_id))
            .ok_or_else(|| anyhow!("Message {reply_to_id} is not in the last page of this chat"))?;

        let body = format!(
            "{}{}",
            reply_quote(
                reply_to_id,
                original.from.as_deref().unwrap_or_default(),
                original.im_display_name.as_deref().unwrap_or("Someone"),
                original.content.as_deref().unwrap_or_default(),
            ),
            content
        );
        self.send_message(chat_id, &body, None).await?;
        Ok(())
    }

    /// Send a reaction to a chat message
    pub async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        reaction: &str,
        remove: bool,
    ) -> Result<()> {
        let key = super::emoji::map_to_key(reaction);

        // Graph takes a Unicode character. A custom emote has none, so it goes
        // the way the web client sends every reaction.
        if super::emoji::custom_emote(&key).is_some() {
            return self
                .set_emotion(conversation_id, message_id, &key, remove)
                .await;
        }

        let token = self.get_token(SCOPE_GRAPH).await?;
        let unicode = super::emoji::map_to_unicode(reaction);

        let action = if remove {
            "unsetReaction"
        } else {
            "setReaction"
        };
        let url = format!(
            "https://graph.microsoft.com/v1.0/chats/{}/messages/{}/{}",
            conversation_id, message_id, action
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let body = serde_json::json!({
            "reactionType": unicode
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(body.to_string())
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to {} reaction: {} - {}",
                if remove { "remove" } else { "send" },
                status,
                body
            ))
        }
    }

    /// Send a reaction to a Teams channel message using IC3 API
    /// Uses the same endpoint as the Teams web client
    pub async fn send_team_reaction(
        &self,
        _team_id: &str,
        channel_id: &str,
        message_id: &str,
        reaction: &str,
        remove: bool,
    ) -> Result<()> {
        let key = super::emoji::map_to_key(reaction);
        self.set_emotion(channel_id, message_id, &key, remove).await
    }

    /// Set or clear one reaction on any conversation, chat or channel, through
    /// the chat service. This is the only route that takes a custom emote: its
    /// key is a name and an object id, not a character Graph could accept.
    ///
    /// Clearing is the same call as DELETE. A PUT carrying a zero time answers
    /// 200 and leaves the reaction in place, stamped with the time of the call.
    async fn set_emotion(
        &self,
        conversation_id: &str,
        message_id: &str,
        key: &str,
        remove: bool,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_IC3).await?;

        let url = format!(
            "{}/conversations/{}/messages/{}/properties?name=emotions",
            self.regional().await.chatsvc_cloud_base(),
            urlencoding::encode(conversation_id),
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        // Body format from the web client. The time is ignored on a DELETE.
        let body = serde_json::json!({
            "emotions": { "key": key, "value": chrono::Utc::now().timestamp_millis() }
        });

        let request = if remove {
            self.http.delete(&url)
        } else {
            self.http.put(&url)
        };
        let res = request
            .headers(headers)
            .body(body.to_string())
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to {} reaction: {} - {}",
                if remove { "remove" } else { "send" },
                status,
                body
            ))
        }
    }

    /// Get activity feed
    pub async fn get_activities(&self) -> Result<Conversations> {
        self.get_conversations("48:notifications", None).await
    }

    /// Get current user's presence
    pub async fn get_my_presence(&self) -> Result<GraphPresence> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/presence";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse presence")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get presence: {} - {}", status, body))
        }
    }

    /// Get presence for multiple users by their IDs
    pub async fn get_presence(&self, user_ids: Vec<&str>) -> Result<GraphPresences> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/communications/getPresencesByUserId";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let body = serde_json::json!({
            "ids": user_ids
        });

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse presences")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get presences: {} - {}", status, body))
        }
    }

    // ==================== OUTLOOK MAIL ====================

    /// Get mail folders
    pub async fn get_mail_folders(&self) -> Result<MailFolders> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/mailFolders";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse mail folders")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get mail folders: {} - {}", status, body))
        }
    }

    /// Get mail messages from inbox or a specific folder
    pub async fn get_mail_messages(
        &self,
        folder: Option<&str>,
        limit: usize,
    ) -> Result<MailMessages> {
        let token = self.get_token(SCOPE_GRAPH).await?;

        let url = match folder {
            Some(f) => format!(
                "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$top={}&$orderby=receivedDateTime desc",
                f, limit
            ),
            None => format!(
                "https://graph.microsoft.com/v1.0/me/messages?$top={}&$orderby=receivedDateTime desc",
                limit
            ),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse mail messages")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get mail messages: {} - {}",
                status,
                body
            ))
        }
    }

    /// Get a specific mail message
    pub async fn get_mail_message(&self, message_id: &str) -> Result<MailMessage> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse mail message")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get mail message: {} - {}", status, body))
        }
    }

    /// Send an email
    pub async fn send_mail(
        &self,
        to: Vec<&str>,
        subject: &str,
        body: &str,
        cc: Option<Vec<&str>>,
        content_type: &str,
        attachments: Option<Vec<FileAttachment>>,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/sendMail";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let to_recipients: Vec<Recipient> = to
            .iter()
            .map(|email| Recipient {
                email_address: EmailAddress {
                    address: email.to_string(),
                    name: None,
                },
            })
            .collect();

        let cc_recipients: Option<Vec<Recipient>> = cc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        let request = SendMailRequest {
            message: SendMailMessage {
                subject: subject.to_string(),
                body: ItemBody {
                    content_type: content_type.to_string(),
                    content: body.to_string(),
                },
                to_recipients,
                cc_recipients,
                attachments,
            },
            save_to_sent_items: true,
        };

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 202 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to send mail: {} - {}", status, body))
        }
    }

    /// Search mail messages
    pub async fn search_mail(&self, query: &str, limit: usize) -> Result<MailMessages> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages?$search=\"{}\"\u{0026}$top={}",
            query, limit
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse mail search results")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to search mail: {} - {}", status, body))
        }
    }

    /// Search calendar events specifically
    pub async fn search_calendar(&self, query: &str, limit: usize) -> Result<CalendarEvents> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        // Calendar events don't support $search well, so we use $filter with contains
        // Using lowercase for case-insensitive contains if supported by the endpoint,
        // or just providing the query as is.
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/events?$filter=contains(subject, '{}')&$top={}",
            query, limit
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse calendar search results")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to search calendar: {} - {}", status, body))
        }
    }

    /// Create a draft email message
    pub async fn create_draft(
        &self,
        to: Vec<&str>,
        subject: &str,
        body: &str,
        cc: Option<Vec<&str>>,
        content_type: &str,
        attachments: Option<Vec<FileAttachment>>,
    ) -> Result<MailMessage> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/messages";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let to_recipients: Vec<Recipient> = to
            .iter()
            .map(|email| Recipient {
                email_address: EmailAddress {
                    address: email.to_string(),
                    name: None,
                },
            })
            .collect();

        let cc_recipients: Option<Vec<Recipient>> = cc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        let request = CreateDraftRequest {
            subject: subject.to_string(),
            body: ItemBody {
                content_type: content_type.to_string(),
                content: body.to_string(),
            },
            to_recipients,
            cc_recipients,
            attachments,
        };

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse draft response")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to create draft: {} - {}", status, body))
        }
    }

    /// Reply to an email
    pub async fn reply_mail(
        &self,
        message_id: &str,
        body: &str,
        content_type: &str,
        reply_all: bool,
        cc: Option<Vec<&str>>,
        bcc: Option<Vec<&str>>,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let endpoint = if reply_all { "replyAll" } else { "reply" };
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/{}",
            message_id, endpoint
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        // Build CC recipients if provided
        let cc_recipients: Option<Vec<Recipient>> = cc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        // Build BCC recipients if provided
        let bcc_recipients: Option<Vec<Recipient>> = bcc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        // Build request with optional CC and BCC
        let mut message = serde_json::json!({
            "body": {
                "contentType": content_type,
                "content": body
            }
        });

        if let Some(cc) = cc_recipients {
            message["ccRecipients"] = serde_json::json!(cc);
        }
        if let Some(bcc) = bcc_recipients {
            message["bccRecipients"] = serde_json::json!(bcc);
        }

        let request = serde_json::json!({
            "message": message
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 202 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to reply to mail: {} - {}", status, body))
        }
    }

    /// Create a draft reply to an email
    ///
    /// Two-step process: first create the draft reply (which includes the quoted
    /// original message), then PATCH the draft body to prepend the reply content.
    pub async fn create_reply_draft(
        &self,
        message_id: &str,
        body: &str,
        content_type: &str,
        reply_all: bool,
        cc: Option<Vec<&str>>,
        bcc: Option<Vec<&str>>,
    ) -> Result<MailMessage> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let endpoint = if reply_all {
            "createReplyAll"
        } else {
            "createReply"
        };
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/{}",
            message_id, endpoint
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        // Step 1: Create draft reply with CC/BCC but no body
        // This gives us a draft that includes the quoted original message
        let mut request = serde_json::json!({});

        // Build CC recipients if provided
        let cc_recipients: Option<Vec<Recipient>> = cc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        // Build BCC recipients if provided
        let bcc_recipients: Option<Vec<Recipient>> = bcc.map(|emails| {
            emails
                .iter()
                .map(|email| Recipient {
                    email_address: EmailAddress {
                        address: email.to_string(),
                        name: None,
                    },
                })
                .collect()
        });

        if cc_recipients.is_some() || bcc_recipients.is_some() {
            let mut message = serde_json::json!({});
            if let Some(cc) = cc_recipients {
                message["ccRecipients"] = serde_json::json!(cc);
            }
            if let Some(bcc) = bcc_recipients {
                message["bccRecipients"] = serde_json::json!(bcc);
            }
            request["message"] = message;
        }

        let res = self
            .http
            .post(&url)
            .headers(headers.clone())
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await?;
            return Err(anyhow!(
                "Failed to create reply draft: {} - {}",
                status,
                err_body
            ));
        }

        let draft: MailMessage = res.json().await?;
        let draft_id = draft
            .id
            .as_deref()
            .ok_or_else(|| anyhow!("Draft reply created but no ID returned"))?;

        // Step 2: Prepend reply content to the draft body (which has the quoted original)
        let original_body = draft
            .body
            .as_ref()
            .map(|b| b.content.as_str())
            .unwrap_or("");

        let reply_html = if content_type == "HTML" {
            body.to_string()
        } else {
            // Convert plain text to HTML
            body.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('\n', "<br>\n")
        };

        // Wrap reply content in Outlook-standard font, then append the quoted original
        let combined_body = format!(
            "<div style=\"font-family: Aptos, Calibri, sans-serif; font-size: 12pt;\">{}</div><br>\n{}",
            reply_html, original_body
        );

        let patch_url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", draft_id);

        let patch_body = serde_json::json!({
            "body": {
                "contentType": "HTML",
                "content": combined_body
            }
        });

        let res = self
            .http
            .patch(&patch_url)
            .headers(headers)
            .body(serde_json::to_string(&patch_body)?)
            .send()
            .await?;

        if res.status().is_success() {
            let updated_draft: MailMessage = res.json().await?;
            Ok(updated_draft)
        } else {
            let status = res.status();
            let err_body = res.text().await?;
            Err(anyhow!(
                "Draft created but failed to update body: {} - {}",
                status,
                err_body
            ))
        }
    }

    /// Forward an email
    pub async fn forward_mail(
        &self,
        message_id: &str,
        to: Vec<&str>,
        comment: Option<&str>,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/forward",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let to_recipients: Vec<serde_json::Value> = to
            .iter()
            .map(|email| {
                serde_json::json!({
                    "emailAddress": {
                        "address": email
                    }
                })
            })
            .collect();

        let request = serde_json::json!({
            "comment": comment.unwrap_or(""),
            "toRecipients": to_recipients
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 202 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to forward mail: {} - {}", status, body))
        }
    }

    /// Delete an email
    pub async fn delete_mail(&self, message_id: &str) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.delete(&url).headers(headers).send().await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to delete mail: {} - {}", status, body))
        }
    }

    /// Move an email to a folder
    pub async fn move_mail(&self, message_id: &str, folder_id: &str) -> Result<MailMessage> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/move",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let request = serde_json::json!({
            "destinationId": folder_id
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse moved message")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to move mail: {} - {}", status, body))
        }
    }

    /// Mark email as read or unread
    pub async fn mark_mail(&self, message_id: &str, is_read: bool) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let request = serde_json::json!({
            "isRead": is_read
        });

        let res = self
            .http
            .patch(&url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to mark mail: {} - {}", status, body))
        }
    }

    /// Get email attachments
    pub async fn get_mail_attachments(&self, message_id: &str) -> Result<MailAttachments> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/attachments",
            message_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse attachments")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get attachments: {} - {}", status, body))
        }
    }

    /// Download an attachment
    pub async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>)> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages/{}/attachments/{}",
            message_id, attachment_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            let attachment: MailAttachment = serde_json::from_str(&body)?;
            let filename = attachment.name.clone();
            let content = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                attachment.content_bytes.unwrap_or_default(),
            )?;
            Ok((filename, content))
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to download attachment: {} - {}",
                status,
                body
            ))
        }
    }

    // ==================== CALENDAR ====================

    /// Get calendar events for today
    pub async fn get_calendar_today(&self) -> Result<CalendarEvents> {
        let now = chrono::Utc::now();
        let start = now.format("%Y-%m-%dT00:00:00Z").to_string();
        let end = now.format("%Y-%m-%dT23:59:59Z").to_string();
        self.get_calendar_events(&start, &end).await
    }

    /// Get calendar events for this week
    pub async fn get_calendar_week(&self) -> Result<CalendarEvents> {
        let now = chrono::Utc::now();
        let start = now.format("%Y-%m-%dT00:00:00Z").to_string();
        let end = (now + chrono::Duration::days(7))
            .format("%Y-%m-%dT23:59:59Z")
            .to_string();
        self.get_calendar_events(&start, &end).await
    }

    /// Get calendar events in a date range
    /// Get schedule/free-busy for a list of users
    pub async fn get_schedule(
        &self,
        users: Vec<&str>,
        start: &str,
        end: &str,
    ) -> Result<serde_json::Value> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/calendar/getSchedule";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            HeaderName::from_static("prefer"),
            HeaderValue::from_static("outlook.timezone=\"UTC\""),
        );

        let body = serde_json::json!({
            "schedules": users,
            "startTime": {
                "dateTime": start,
                "timeZone": "UTC"
            },
            "endTime": {
                "dateTime": end,
                "timeZone": "UTC"
            },
            "availabilityViewInterval": 30
        });

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            Ok(serde_json::from_str(&body)?)
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get schedule: {} - {}", status, body))
        }
    }
    pub async fn get_calendar_groups(&self) -> Result<serde_json::Value> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/calendarGroups";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;
        let body = res.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// Get all accessible calendars including those in groups
    pub async fn get_all_calendars(&self) -> Result<Vec<Calendar>> {
        let mut all_calendars = Vec::new();

        // 1. Get primary calendars
        if let Ok(calendars) = self.get_calendars().await {
            all_calendars.extend(calendars.value);
        }

        // 2. Get calendars from groups
        if let Ok(groups) = self.get_calendar_groups().await {
            if let Some(groups_val) = groups.get("value").and_then(|v| v.as_array()) {
                for group in groups_val {
                    if let Some(group_id) = group.get("id").and_then(|i| i.as_str()) {
                        let group_name = group
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("Unknown Group");
                        if let Ok(group_calendars) = self.get_group_calendars(group_id).await {
                            for mut c in group_calendars.value {
                                if let Some(ref mut name) = c.name {
                                    *name = format!("{} ({})", name, group_name);
                                }
                                all_calendars.push(c);
                            }
                        }
                    }
                }
            }
        }

        Ok(all_calendars)
    }

    /// Get calendars for a specific group
    pub async fn get_group_calendars(&self, group_id: &str) -> Result<CalendarList> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/calendarGroups/{}/calendars",
            group_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;
        let body = res.text().await?;
        Ok(serde_json::from_str(&body)?)
    }
    pub async fn get_calendars(&self) -> Result<CalendarList> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/calendars";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse calendars")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get calendars: {} - {}", status, body))
        }
    }

    /// Get calendar events for a specific user (if shared)
    pub async fn get_user_calendar_view(
        &self,
        user_id: &str,
        start: &str,
        end: &str,
    ) -> Result<CalendarEvents> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/users/{}/calendar/calendarView?startDateTime={}&endDateTime={}&$orderby=start/dateTime&$top=50",
            user_id, start, end
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse user calendar events")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get user calendar events: {} - {}",
                status,
                body
            ))
        }
    }
    pub async fn get_calendar_events_for_id(
        &self,
        calendar_id: &str,
        start: &str,
        end: &str,
    ) -> Result<CalendarEvents> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/calendars/{}/calendarView?startDateTime={}&endDateTime={}&$orderby=start/dateTime&$top=50",
            calendar_id, start, end
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse calendar events")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get calendar events: {} - {}",
                status,
                body
            ))
        }
    }

    /// Get calendar events in a date range for primary calendar
    pub async fn get_calendar_events(&self, start: &str, end: &str) -> Result<CalendarEvents> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/calendarView?startDateTime={}&endDateTime={}&$orderby=start/dateTime&$top=50",
            start, end
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse calendar events")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get calendar events: {} - {}",
                status,
                body
            ))
        }
    }

    /// Get a specific calendar event
    pub async fn get_calendar_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!("https://graph.microsoft.com/v1.0/me/events/{}", event_id);

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse calendar event")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!(
                "Failed to get calendar event: {} - {}",
                status,
                body
            ))
        }
    }

    /// Create a calendar event
    pub async fn create_calendar_event(
        &self,
        request: CreateEventRequest,
    ) -> Result<CalendarEvent> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = "https://graph.microsoft.com/v1.0/me/events";

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse created event")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to create event: {} - {}", status, body))
        }
    }

    /// RSVP to a calendar event
    pub async fn rsvp_calendar_event(
        &self,
        event_id: &str,
        response: &str,
        comment: Option<&str>,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let endpoint = match response.to_lowercase().as_str() {
            "accept" | "yes" => "accept",
            "decline" | "no" => "decline",
            "tentative" | "maybe" => "tentativelyAccept",
            _ => {
                return Err(anyhow!(
                    "Invalid response. Use: accept, decline, or tentative"
                ))
            }
        };
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/events/{}/{}",
            event_id, endpoint
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );

        let body = serde_json::json!({
            "comment": comment.unwrap_or(""),
            "sendResponse": true
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 202 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to RSVP: {} - {}", status, body))
        }
    }

    /// Delete a calendar event
    pub async fn delete_calendar_event(&self, event_id: &str) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!("https://graph.microsoft.com/v1.0/me/events/{}", event_id);

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.delete(&url).headers(headers).send().await?;

        if res.status().is_success() || res.status().as_u16() == 204 {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to delete event: {} - {}", status, body))
        }
    }

    /// Download an image from Teams AMS (Azure Media Services) URL
    pub async fn download_ams_image(&self, image_url: &str) -> Result<(String, Vec<u8>)> {
        // Try with chatsvcagg token first (works for chat images)
        let token = self.get_token(SCOPE_CHATSVCAGG).await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(image_url).headers(headers).send().await?;

        if res.status().is_success() {
            let content_type = res
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = res.bytes().await?.to_vec();
            return Ok((content_type, bytes));
        }

        // If chatsvcagg fails, try with skype token (works for Teams channel images)
        let skype_token = self.get_skype_token().await?;

        // Try different auth header formats for skype token
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("skype_token {}", skype_token.value))?,
        );

        let res = self.http.get(image_url).headers(headers).send().await?;

        if res.status().is_success() {
            let content_type = res
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = res.bytes().await?.to_vec();
            return Ok((content_type, bytes));
        }

        // Try with IC3 token as last resort
        let ic3_token = self.get_token(SCOPE_IC3).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", ic3_token.value))?,
        );

        let res = self.http.get(image_url).headers(headers).send().await?;

        if res.status().is_success() {
            let content_type = res
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = res.bytes().await?.to_vec();
            Ok((content_type, bytes))
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to download image: {} - {}", status, body))
        }
    }

    /// Download a file from SharePoint/OneDrive using its share URL
    pub async fn download_sharepoint_file(&self, file_url: &str) -> Result<(String, Vec<u8>)> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        // Try to use Graph API shares endpoint first as it's more reliable for shared files
        // logic: base64 encode URL, replace chars, prepend u!
        if file_url.contains("sharepoint.com") || file_url.contains("1drv.ms") {
            let encoded_url = STANDARD.encode(file_url);
            let encoded_url = "u!".to_string()
                + &encoded_url
                    .trim_end_matches('=')
                    .replace('/', "_")
                    .replace('+', "-");
            let graph_url = format!(
                "https://graph.microsoft.com/v1.0/shares/{}/driveItem/content",
                encoded_url
            );

            let token = self.get_token(super::SCOPE_GRAPH).await?;
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_static("authorization"),
                HeaderValue::from_str(&format!("Bearer {}", token.value))?,
            );

            let res = self.http.get(&graph_url).headers(headers).send().await?;

            if res.status().is_success() || res.status() == reqwest::StatusCode::FOUND {
                let final_res = if res.status() == reqwest::StatusCode::FOUND {
                    if let Some(location) = res.headers().get("Location") {
                        let location_url = location.to_str().unwrap_or_default().to_string();
                        // Pre-signed URLs usually don't need auth headers
                        self.http.get(&location_url).send().await?
                    } else {
                        res
                    }
                } else {
                    res
                };

                if final_res.status().is_success() {
                    let content_type = final_res
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    let bytes = final_res.bytes().await?.to_vec();
                    return Ok((content_type, bytes));
                }
            }
        }

        Err(anyhow!(
            "Failed to download file: URL is not a supported SharePoint/OneDrive share link"
        ))
    }

    // ===== SharePoint & Excel (Sheets) API =====

    /// Helper to build Graph API headers with authorization
    fn graph_headers(&self, token: &AccessToken) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );
        Ok(headers)
    }

    /// Helper to build Graph API headers with authorization and JSON content type
    fn graph_json_headers(&self, token: &AccessToken) -> Result<HeaderMap> {
        let mut headers = self.graph_headers(token)?;
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        Ok(headers)
    }

    /// Search SharePoint sites
    pub async fn search_sites(&self, query: &str) -> Result<Sites> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/sites?search={}",
            urlencoding::encode(query)
        );
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse sites")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to search sites: {} - {}", status, body))
        }
    }

    /// List drives (document libraries) for a site
    pub async fn list_drives(&self, site_id: &str) -> Result<Drives> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!("https://graph.microsoft.com/v1.0/sites/{}/drives", site_id);
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse drives")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to list drives: {} - {}", status, body))
        }
    }

    /// List user's OneDrive root items
    pub async fn list_my_drive_items(&self, folder_id: Option<&str>) -> Result<DriveItems> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = match folder_id {
            Some(id) => format!(
                "https://graph.microsoft.com/v1.0/me/drive/items/{}/children",
                id
            ),
            None => "https://graph.microsoft.com/v1.0/me/drive/root/children".to_string(),
        };
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse drive items")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to list drive items: {} - {}", status, body))
        }
    }

    /// List items in a drive (root or folder)
    pub async fn list_drive_items(
        &self,
        drive_id: &str,
        folder_id: Option<&str>,
    ) -> Result<DriveItems> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = match folder_id {
            Some(id) => format!(
                "https://graph.microsoft.com/v1.0/drives/{}/items/{}/children",
                drive_id, id
            ),
            None => format!(
                "https://graph.microsoft.com/v1.0/drives/{}/root/children",
                drive_id
            ),
        };
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse drive items")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to list drive items: {} - {}", status, body))
        }
    }

    /// List worksheets in an Excel workbook
    pub async fn list_worksheets(&self, drive_id: &str, item_id: &str) -> Result<Worksheets> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/worksheets",
            drive_id, item_id
        );
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse worksheets")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to list worksheets: {} - {}", status, body))
        }
    }

    /// Read a range from an Excel worksheet
    pub async fn read_sheet_range(
        &self,
        drive_id: &str,
        item_id: &str,
        sheet: &str,
        range: Option<&str>,
    ) -> Result<ExcelRange> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        // Percent-encode OData single quotes as %27 to prevent the url crate
        // from re-interpreting special chars (like !) elsewhere in the path
        let sheet_clean = sheet.replace('\'', "''");
        let escaped_sheet = urlencoding::encode(&sheet_clean);
        let url = match range {
            Some(r) => {
                let range_clean = r.replace('\'', "''");
                let escaped_range = urlencoding::encode(&range_clean);
                format!(
                    "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/worksheets(%27{}%27)/range(address=%27{}%27)",
                    drive_id, item_id, escaped_sheet, escaped_range
                )
            }
            None => format!(
                "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/worksheets(%27{}%27)/usedRange",
                drive_id, item_id, escaped_sheet
            ),
        };
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse range")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to read range: {} - {}", status, body))
        }
    }

    /// Update a range in an Excel worksheet
    pub async fn update_sheet_range(
        &self,
        drive_id: &str,
        item_id: &str,
        sheet: &str,
        range: &str,
        values: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExcelRange> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let sheet_clean = sheet.replace('\'', "''");
        let escaped_sheet = urlencoding::encode(&sheet_clean);
        let range_clean = range.replace('\'', "''");
        let escaped_range = urlencoding::encode(&range_clean);
        let url = format!(
            "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/worksheets(%27{}%27)/range(address=%27{}%27)",
            drive_id, item_id, escaped_sheet, escaped_range
        );
        let headers = self.graph_json_headers(&token)?;
        let body = serde_json::to_string(&UpdateRangeRequest { values })?;

        let res = self
            .http
            .patch(&url)
            .headers(headers)
            .body(body)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse updated range")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to update range: {} - {}", status, body))
        }
    }

    /// List tables in an Excel workbook
    pub async fn list_tables(&self, drive_id: &str, item_id: &str) -> Result<ExcelTables> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/tables",
            drive_id, item_id
        );
        let headers = self.graph_headers(&token)?;
        let res = self.http.get(&url).headers(headers).send().await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse tables")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to list tables: {} - {}", status, body))
        }
    }

    /// Append rows to an Excel table
    pub async fn append_table_rows(
        &self,
        drive_id: &str,
        item_id: &str,
        table: &str,
        values: Vec<Vec<serde_json::Value>>,
    ) -> Result<serde_json::Value> {
        let token = self.get_token(SCOPE_GRAPH).await?;
        let table_clean = table.replace('\'', "''");
        let escaped_table = urlencoding::encode(&table_clean);
        let url = format!(
            "https://graph.microsoft.com/v1.0/drives/{}/items/{}/workbook/tables(%27{}%27)/rows/add",
            drive_id, item_id, escaped_table
        );
        let headers = self.graph_json_headers(&token)?;
        let body = serde_json::to_string(&AddTableRowsRequest { values })?;

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await?;

        if res.status().is_success() {
            let body = res.text().await?;
            serde_json::from_str(&body).context("Failed to parse table row response")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to append rows: {} - {}", status, body))
        }
    }
}

/// Whether a picture URL is served by Microsoft, and so needs our token.
/// Matches on the host only: a path or query containing the domain must not
/// be enough to send the token somewhere else.
fn is_microsoft_host(url: &str) -> bool {
    const DOMAINS: [&str; 4] = [
        "teams.microsoft.com",
        "skype.com",
        "sharepoint.com",
        "office.net",
    ];

    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{}", domain)))
}

#[cfg(test)]
mod picture_host_tests {
    use super::{is_microsoft_host, photo_object_id, tenant_from_token, PhotoSubject};

    #[test]
    fn teams_attachments_are_ours() {
        assert!(is_microsoft_host(
            "https://eu-api.asyncgw.teams.microsoft.com/v1/objects/0-eu-d1/views/imgo"
        ));
        assert!(is_microsoft_host(
            "https://statics.teams.cdn.office.net/x.png"
        ));
        assert!(is_microsoft_host("https://acme.sharepoint.com/a.jpg"));
    }

    #[test]
    fn third_parties_get_no_token() {
        assert!(!is_microsoft_host(
            "https://media4.giphy.com/media/x/giphy.gif"
        ));
        assert!(!is_microsoft_host("https://example.com/a.png"));
    }

    /// A look-alike host must not collect our bearer token.
    #[test]
    fn lookalike_hosts_get_no_token() {
        for url in [
            "https://evil.com/teams.microsoft.com/x.png",
            "https://evil.com/?u=teams.microsoft.com",
            "https://teams.microsoft.com.evil.com/x.png",
            "https://notskype.com/x.png",
            "https://evil.com#teams.microsoft.com",
            "https://evil.com@teams.microsoft.com.attacker.net/x",
        ] {
            assert!(!is_microsoft_host(url), "leaked token to {}", url);
        }
    }

    /// Nothing in Teams serves the tenant GUID, and the custom emote URL needs
    /// one. It is only ever read out of a token this client already holds.
    #[test]
    fn the_tenant_is_read_out_of_a_token() {
        use base64::Engine;
        let claim = |body: &str| {
            format!(
                "header.{}.signature",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body)
            )
        };
        assert_eq!(
            tenant_from_token(&claim(r#"{"tid":"7f1e2d3c-4b5a-6978-8765-4321fedcba98"}"#)),
            Some("7f1e2d3c-4b5a-6978-8765-4321fedcba98".to_string())
        );
        assert_eq!(tenant_from_token(&claim(r#"{"tid":""}"#)), None);
        assert_eq!(tenant_from_token(&claim(r#"{"oid":"x"}"#)), None);
        assert_eq!(tenant_from_token("not.a.token"), None);
        assert_eq!(tenant_from_token("nodots"), None);
    }

    #[test]
    fn a_photo_id_is_taken_out_of_an_mri() {
        assert_eq!(
            photo_object_id("8:orgid:1f2e3d4c-5b6a-4789-9012-3456789abcde"),
            Some("1f2e3d4c-5b6a-4789-9012-3456789abcde")
        );
        assert_eq!(
            photo_object_id("8:lync:1f2e3d4c-5b6a-4789-9012-3456789abcde"),
            Some("1f2e3d4c-5b6a-4789-9012-3456789abcde")
        );
        assert_eq!(
            photo_object_id("1f2e3d4c-5b6a-4789-9012-3456789abcde"),
            Some("1f2e3d4c-5b6a-4789-9012-3456789abcde")
        );
    }

    /// A bot MRI is not a Graph id, so asking for its photo would be a wasted
    /// round trip that always fails.
    #[test]
    fn a_bot_has_no_photo_to_ask_for() {
        assert_eq!(
            photo_object_id("28:0d8b9b4e-4e0e-4f00-8000-000000000000"),
            None
        );
        assert_eq!(photo_object_id(""), None);
    }

    #[test]
    fn people_and_groups_sit_on_different_graph_paths() {
        assert_eq!(PhotoSubject::Person.path(), "users");
        assert_eq!(PhotoSubject::Group.path(), "groups");
    }
}

#[cfg(test)]
mod token_tests {
    use super::is_dead_refresh_token;

    #[test]
    fn an_expired_refresh_token_only_a_login_can_fix() {
        assert!(is_dead_refresh_token(
            r#"Failed to renew refresh token: 400 Bad Request - {"error":"invalid_grant","error_description":"AADSTS70043: The refresh token has expired"}"#
        ));
    }

    #[test]
    fn a_request_that_failed_for_another_reason_keeps_the_token() {
        assert!(!is_dead_refresh_token(
            "Failed to renew refresh token: 503 Service Unavailable - "
        ));
        assert!(!is_dead_refresh_token("error sending request: timed out"));
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;

    /// The shape a real Teams reply carries, attribute for attribute. Taken
    /// from a message the web client sent; ids and names are invented.
    #[test]
    fn reply_quote_matches_what_teams_sends() {
        assert_eq!(
            reply_quote(
                "1789025289038",
                "8:orgid:11111111-1111-4111-8111-111111111111",
                "Ada Fenwick",
                "<p>ah bon wtf</p>",
            ),
            concat!(
                r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" "#,
                r#"itemid="1789025289038">"#,
                r#"<strong itemprop="mri" itemid="8:orgid:11111111-1111-4111-8111-111111111111">"#,
                r#"Ada Fenwick</strong>"#,
                r#"<span itemprop="time" itemid="1789025289038"></span>"#,
                r#"<p itemprop="preview">ah bon wtf</p></blockquote>"#,
            )
        );
    }

    /// The quote is one line. A whole message repeated in it would be read
    /// twice.
    #[test]
    fn a_long_message_is_cut_in_the_quote() {
        let long = "widget ".repeat(80);
        let quote = reply_quote("1", "8:orgid:u1", "Ada", &format!("<p>{long}</p>"));
        let preview = quote
            .split(r#"<p itemprop="preview">"#)
            .nth(1)
            .unwrap()
            .trim_end_matches("</p></blockquote>");

        // Exactly what Teams writes: one ellipsis, and 200 characters counting it.
        assert!(preview.ends_with('\u{2026}'));
        assert!(!preview.ends_with("..."));
        assert_eq!(preview.chars().count(), REPLY_PREVIEW_CHARS);
    }

    /// A name or a message with a bracket in it must not close the tag it
    /// sits in.
    #[test]
    fn markup_in_a_name_or_message_is_escaped() {
        let quote = reply_quote("1", r#"8:orgid:"u1"#, "<script>", "<p>a &amp; b &lt; c</p>");

        assert!(quote.contains("&lt;script&gt;"));
        assert!(quote.contains(r#"itemid="8:orgid:&quot;u1""#));
        assert!(!quote.contains("<script>"));
    }
}
