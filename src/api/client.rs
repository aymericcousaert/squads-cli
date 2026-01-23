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

    /// Get activity feed
    pub async fn get_activities(&self) -> Result<Conversations> {
        self.get_conversations("48:notifications", None).await
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
}
