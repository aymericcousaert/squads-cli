use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;

use super::{gen_skype_token, gen_token, renew_refresh_token, SCOPE_CHATSVCAGG, SCOPE_GRAPH, SCOPE_IC3, SCOPE_SPACES};
use crate::cache::{Cache, TOKENS_FILE};
use crate::config::Config;
use crate::types::*;

fn get_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Microsoft Teams API client
pub struct TeamsClient {
    tokens: Arc<RwLock<TokenStore>>,
    tenant: String,
    http: Client,
    cache: Cache,
}

impl TeamsClient {
    /// Create a new Teams client
    pub fn new(config: &Config) -> Result<Self> {
        let cache = Cache::new()?;
        let tokens: TokenStore = cache.load(TOKENS_FILE)?.unwrap_or_default();

        Ok(Self {
            tokens: Arc::new(RwLock::new(tokens)),
            tenant: config.auth.tenant.clone(),
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            cache,
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
                let new_token = renew_refresh_token(&token, &self.tenant).await?;
                {
                    let mut tokens = self.tokens.write().unwrap();
                    tokens.insert("refresh_token".to_string(), new_token.clone());
                }
                self.save_tokens()?;
                new_token
            }
            Some(token) => token,
            None => return Err(anyhow!("Not authenticated. Run 'squads-cli auth login' first.")),
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

    /// Get current user's teams and chats
    pub async fn get_user_details(&self) -> Result<UserDetails> {
        let token = self.get_token(SCOPE_CHATSVCAGG).await?;
        let url = "https://teams.microsoft.com/api/csa/emea/api/v2/teams/users/me";

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
            serde_json::from_str(&body).context("Failed to parse user details")
        } else {
            let status = res.status();
            let body = res.text().await?;
            Err(anyhow!("Failed to get user details: {} - {}", status, body))
        }
    }

    /// Get current user profile
    pub async fn get_me(&self) -> Result<Profile> {
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

    /// Get conversations/messages from a chat
    pub async fn get_conversations(&self, thread_id: &str, message_id: Option<u64>) -> Result<Conversations> {
        let token = self.get_token(SCOPE_IC3).await?;

        let thread_part = match message_id {
            Some(msg_id) => format!("{};messageid={}", thread_id, msg_id),
            None => thread_id.to_string(),
        };

        let url = format!(
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/conversations/{}/messages?pageSize=200",
            thread_part
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
            Err(anyhow!("Failed to get conversations: {} - {}", status, body))
        }
    }

    /// Get team channel conversations
    pub async fn get_team_conversations(&self, team_id: &str, channel_id: &str) -> Result<TeamConversations> {
        let token = self.get_token(SCOPE_CHATSVCAGG).await?;
        let url = format!(
            "https://teams.microsoft.com/api/csa/emea/api/v2/teams/{}/channels/{}",
            team_id, channel_id
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
            Err(anyhow!("Failed to get team conversations: {} - {}", status, body))
        }
    }

    /// Send a message to a conversation
    pub async fn send_message(&self, conversation_id: &str, content: &str, subject: Option<&str>) -> Result<String> {
        let token = self.get_token(SCOPE_IC3).await?;
        let me = self.get_me().await?;

        let url = format!(
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/conversations/{}/messages",
            conversation_id
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        // Generate random message ID
        let message_id: u64 = rand::random();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

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
            "contenttype": "Text",
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

    /// Get activity feed
    pub async fn get_activities(&self) -> Result<Conversations> {
        self.get_conversations("48:notifications", None).await
    }
}
