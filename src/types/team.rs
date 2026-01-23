use serde::{Deserialize, Deserializer, Serialize};

/// Team site information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamSiteInformation {
    pub group_id: String,
}

/// Channel within a team
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub display_name: String,
}

/// Team information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub channels: Vec<Channel>,
    pub smtp_address: Option<String>,
    pub team_site_information: TeamSiteInformation,
    pub display_name: String,
    #[serde(deserialize_with = "trim_quotes")]
    pub picture_e_tag: Option<String>,
}

fn trim_quotes<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt_s: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt_s.map(|s| s.trim_matches('"').to_string()))
}

/// User details containing teams and chats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDetails {
    pub teams: Vec<Team>,
    pub chats: Vec<Chat>,
}

/// Chat member
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMember {
    pub is_muted: Option<bool>,
    pub mri: String,
    pub object_id: Option<String>,
    pub role: Option<String>,
    pub is_identity_masked: Option<bool>,
}

/// Chat/conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub members: Vec<ChatMember>,
    pub is_read: Option<bool>,
    pub is_high_importance: Option<bool>,
    pub is_one_on_one: Option<bool>,
    pub is_conversation_deleted: Option<bool>,
    pub is_external: Option<bool>,
    pub is_messaging_disabled: Option<bool>,
    pub is_disabled: Option<bool>,
    pub title: Option<String>,
    pub last_message: Option<super::Message>,
    pub is_last_message_from_me: Option<bool>,
    pub chat_sub_type: Option<u64>,
    pub last_join_at: Option<String>,
    pub created_at: Option<String>,
    pub creator: Option<String>,
    pub hidden: Option<bool>,
    pub added_by: Option<String>,
    pub chat_type: Option<String>,
    pub picture: Option<String>,
}

/// Team conversations response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConversations {
    pub reply_chains: Vec<Conversation>,
}

/// Conversation within a team channel
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub messages: Vec<super::Message>,
    pub container_id: String,
    pub id: String,
    pub latest_delivery_time: String,
}
