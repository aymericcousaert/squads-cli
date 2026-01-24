use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;

use super::{
    gen_skype_token, gen_token, renew_refresh_token, SCOPE_CHATSVCAGG, SCOPE_GRAPH, SCOPE_IC3,
    SCOPE_SPACES,
};
use crate::cache::{Cache, TOKENS_FILE};
use crate::config::Config;
use crate::types::*;

fn get_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
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

    /// Get conversations/messages from a chat
    pub async fn get_conversations(
        &self,
        thread_id: &str,
        message_id: Option<u64>,
    ) -> Result<Conversations> {
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
            Err(anyhow!(
                "Failed to get team conversations: {} - {}",
                status,
                body
            ))
        }
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
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/conversations/{}/messages",
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

    /// Reply to a message in a team channel
    /// First tries Graph API (requires ChannelMessage.Send permission),
    /// then falls back to posting with quoted content
    pub async fn reply_channel_message(
        &self,
        team_id: &str,
        channel_id: &str,
        parent_message_id: &str,
        content: &str,
    ) -> Result<serde_json::Value> {
        // Graph API needs the team's group_id (GUID), not the thread ID format
        let details = self.get_user_details().await?;
        let team = details
            .teams
            .iter()
            .find(|t| t.id == team_id)
            .ok_or_else(|| anyhow!("Team not found: {}", team_id))?;
        let group_id = &team.team_site_information.group_id;

        let token = self.get_token(SCOPE_GRAPH).await?;

        // Try Graph API for proper thread replies
        // Format: POST /teams/{group-id}/channels/{channel-id}/messages/{message-id}/replies
        let url = format!(
            "https://graph.microsoft.com/v1.0/teams/{}/channels/{}/messages/{}/replies",
            group_id, channel_id, parent_message_id
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
            "body": {
                "contentType": "html",
                "content": content
            }
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            let body = res.text().await?;
            return serde_json::from_str(&body)
                .or_else(|_| Ok(serde_json::json!({"status": "sent"})));
        }

        // If Graph API fails (likely missing ChannelMessage.Send permission),
        // fall back to posting a new message with quoted content
        if res.status().as_u16() == 403 {
            // Get channel messages to find the original message
            let conversations = self.get_team_conversations(team_id, channel_id).await?;
            let original_msg = conversations
                .reply_chains
                .iter()
                .flat_map(|chain| &chain.messages)
                .find(|m| m.id.as_deref() == Some(parent_message_id));

            let quoted_content = if let Some(msg) = original_msg {
                let sender = msg
                    .im_display_name
                    .clone()
                    .unwrap_or_else(|| "Someone".to_string());
                let original_content = msg
                    .content
                    .clone()
                    .map(|c| strip_html_simple(&c))
                    .unwrap_or_default();
                let truncated = if original_content.len() > 100 {
                    format!("{}...", &original_content[..100])
                } else {
                    original_content
                };
                format!(
                    "<blockquote><b>{}</b>: {}</blockquote>{}",
                    sender, truncated, content
                )
            } else {
                content.to_string()
            };

            // Post as new message with quoted content
            return self
                .send_channel_message(team_id, channel_id, &quoted_content, None)
                .await;
        }

        let status = res.status();
        let body = res.text().await?;
        Err(anyhow!(
            "Failed to reply to channel message: {} - {}",
            status,
            body
        ))
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
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/conversations/{}/messages/{}",
            conversation_id, message_id
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
            "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/conversations/{}/messages/{}",
            channel_id, message_id
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
    pub async fn reply_to_message(
        &self,
        chat_id: &str,
        reply_to_id: &str,
        content: &str,
    ) -> Result<()> {
        // First try Graph API (works for channel/group chats)
        let token = self.get_token(SCOPE_GRAPH).await?;
        let url = format!(
            "https://graph.microsoft.com/v1.0/chats/{}/messages/{}/replies",
            chat_id, reply_to_id
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
            "body": {
                "content": content
            }
        });

        let res = self
            .http
            .post(&url)
            .headers(headers)
            .body(serde_json::to_string(&body)?)
            .send()
            .await?;

        if res.status().is_success() || res.status().as_u16() == 201 {
            return Ok(());
        }

        // If 405 Method Not Allowed, fall back to regular message with quote
        // This happens for 1:1 (Direct) chats where Graph API replies aren't supported
        if res.status().as_u16() == 405 {
            // Get the original message to quote
            let conversations = self.get_conversations(chat_id, None).await?;
            let original_msg = conversations
                .messages
                .iter()
                .find(|m| m.id.as_deref() == Some(reply_to_id));

            let quoted_content = if let Some(msg) = original_msg {
                let sender = msg
                    .im_display_name
                    .clone()
                    .unwrap_or_else(|| "Someone".to_string());
                let original_content = msg
                    .content
                    .clone()
                    .map(|c| strip_html_simple(&c))
                    .unwrap_or_default();
                let truncated = if original_content.len() > 100 {
                    format!("{}...", &original_content[..100])
                } else {
                    original_content
                };
                format!(
                    "<blockquote><b>{}</b>: {}</blockquote><p>{}</p>",
                    sender, truncated, content
                )
            } else {
                format!("<p>{}</p>", content)
            };

            // Send as regular message using Teams Chat Service API
            self.send_message(chat_id, &quoted_content, None).await?;
            return Ok(());
        }

        let status = res.status();
        let body = res.text().await?;
        Err(anyhow!("Failed to reply to message: {} - {}", status, body))
    }

    /// Send a reaction to a message
    pub async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        reaction: &str,
        remove: bool,
    ) -> Result<()> {
        let token = self.get_token(SCOPE_GRAPH).await?;

        // Map user-friendly names to Unicode values
        let unicode = match reaction.to_lowercase().as_str() {
            "like" | "👍" => "👍",
            "heart" | "❤️" => "❤️",
            "laugh" | "😄" => "😄",
            "surprised" | "😮" => "😮",
            "sad" | "😢" => "😢",
            "angry" | "😡" => "😡",
            "skull" | "💀" => "💀",
            _ => reaction, // Fallback to raw string
        };

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
                    content_type: "Text".to_string(),
                    content: body.to_string(),
                },
                to_recipients,
                cc_recipients,
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
                content_type: "Text".to_string(),
                content: body.to_string(),
            },
            to_recipients,
            cc_recipients,
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
    pub async fn reply_mail(&self, message_id: &str, body: &str, reply_all: bool) -> Result<()> {
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

        let request = serde_json::json!({
            "message": {
                "body": {
                    "contentType": "Text",
                    "content": body
                }
            }
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
        let token = self.get_token(SCOPE_GRAPH).await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {}", token.value))?,
        );

        let res = self.http.get(file_url).headers(headers).send().await?;

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
            Err(anyhow!("Failed to download file: {} - {}", status, body))
        }
    }
}
