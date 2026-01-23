use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

// Helper deserializers imported from parent module
// (Currently using local implementations)

/// File attachment information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub version: Option<i64>,
    pub id: Option<String>,
    pub base_url: Option<String>,
    #[serde(rename = "type")]
    pub title: Option<String>,
    pub object_url: Option<String>,
    #[serde(rename = "itemid")]
    pub item_id: Option<String>,
    pub file_name: Option<String>,
    pub file_type: Option<String>,
    pub file_info: FileInfo,
}

/// File info details
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub item_id: Option<String>,
    pub file_url: Option<String>,
    pub site_url: Option<String>,
    pub server_relative_url: Option<String>,
    pub share_url: Option<String>,
    pub share_id: Option<String>,
}

/// Emoji reaction user
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmotionUser {
    pub mri: String,
    pub time: u64,
    pub value: String,
}

/// Emoji reaction
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Emotion {
    pub key: String,
    pub users: Vec<EmotionUser>,
}

/// Card button
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardContentButton {
    #[serde(rename = "type")]
    pub button_type: String,
    pub title: String,
    pub value: String,
}

/// Card content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardContent {
    pub text: Option<String>,
    pub component_url: Option<String>,
    pub source_type: Option<String>,
    pub buttons: Option<Vec<CardContentButton>>,
}

/// Card attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub app_icon: Option<String>,
    pub card_client_id: String,
    pub content: CardContent,
    pub content_type: String,
    pub preview_hidden: Option<bool>,
}

/// Activity context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityContext {
    pub teams_app_id: Option<String>,
    pub location: Option<String>,
    pub template_parameter: Option<String>,
}

/// Activity information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub activity_type: String,
    pub activity_subtype: Option<String>,
    pub activity_timestamp: String,
    pub activity_id: u64,
    pub source_message_id: u64,
    pub source_reply_chain_id: Option<u64>,
    pub source_user_id: String,
    pub source_user_im_display_name: Option<String>,
    pub target_user_id: String,
    pub source_thread_id: String,
    pub message_preview: String,
    pub source_thread_topic: Option<String>,
    pub activity_context: ActivityContext,
}

/// Message properties
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageProperties {
    #[serde(default)]
    #[serde(deserialize_with = "string_to_i64_opt")]
    pub edittime: i64,
    pub subject: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_files")]
    pub files: Option<Vec<File>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_cards")]
    pub cards: Option<Vec<Card>>,
    #[serde(default)]
    #[serde(deserialize_with = "string_to_i64_opt")]
    pub deletetime: i64,
    #[serde(default)]
    #[serde(deserialize_with = "string_to_bool_opt")]
    pub systemdelete: bool,
    pub title: Option<String>,
    pub emotions: Option<Vec<Emotion>>,
    #[serde(default)]
    #[serde(rename = "isread")]
    #[serde(deserialize_with = "string_to_option_bool_opt")]
    pub is_read: Option<bool>,
    pub activity: Option<Activity>,
}

fn string_to_i64_opt<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        Some(Value::String(s)) => s.parse().map_err(serde::de::Error::custom),
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("Invalid number")),
        _ => Ok(0),
    }
}

fn string_to_bool_opt<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        Some(Value::Bool(b)) => Ok(b),
        Some(Value::String(s)) => Ok(s == "true"),
        _ => Ok(false),
    }
}

fn string_to_option_bool_opt<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(Value::String(s)) => Ok(Some(s == "true")),
        _ => Ok(None),
    }
}

fn deserialize_files<'de, D>(deserializer: D) -> Result<Option<Vec<File>>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(json_str) if json_str != "[]" => serde_json::from_str(&json_str)
            .map(Some)
            .map_err(serde::de::Error::custom),
        _ => Ok(None),
    }
}

fn deserialize_cards<'de, D>(deserializer: D) -> Result<Option<Vec<Card>>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(json_str) if json_str != "[]" => serde_json::from_str(&json_str)
            .map(Some)
            .map_err(serde::de::Error::custom),
        _ => Ok(None),
    }
}

/// Chat/Team message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub content: Option<String>,
    #[serde(deserialize_with = "strip_url_opt")]
    pub from: Option<String>,
    #[serde(alias = "imdisplayname")]
    pub im_display_name: Option<String>,
    #[serde(alias = "messagetype")]
    pub message_type: Option<String>,
    pub properties: Option<MessageProperties>,
    pub compose_time: Option<String>,
    #[serde(alias = "originalarrivaltime")]
    pub original_arrival_time: Option<String>,
    pub conversation_link: Option<String>,
    pub id: Option<String>,
    pub container_id: Option<String>,
}

fn strip_url_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.map(|url| {
        let pass1 = url
            .strip_prefix("https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/contacts/")
            .unwrap_or(&url);
        pass1
            .strip_prefix("https://notifications.skype.net/v1/users/ME/contacts/")
            .unwrap_or(pass1)
            .to_string()
    }))
}

/// Conversations response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversations {
    pub messages: Vec<Message>,
}

/// Message to send
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsMessage<'a> {
    pub id: &'a str,
    #[serde(rename = "type")]
    pub msg_type: &'a str,
    pub conversationid: &'a str,
    pub conversation_link: &'a str,
    pub from: &'a str,
    pub composetime: &'a str,
    pub originalarrivaltime: &'a str,
    pub content: &'a str,
    pub messagetype: &'a str,
    pub contenttype: &'a str,
    pub imdisplayname: Option<&'a str>,
    pub clientmessageid: &'a str,
    pub call_id: &'a str,
    pub state: i32,
    pub version: &'a str,
    pub amsreferences: Vec<&'a str>,
    pub properties: MessageProperties,
    pub post_type: &'a str,
    pub cross_post_channels: Vec<&'a str>,
}

/// Message properties for sending
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageProperties {
    pub importance: String,
    pub subject: Option<String>,
    pub title: String,
    pub cards: String,
    pub links: String,
    pub mentions: String,
    pub onbehalfof: Option<String>,
    pub files: String,
    pub policy_violation: Option<String>,
    pub format_variant: String,
}
