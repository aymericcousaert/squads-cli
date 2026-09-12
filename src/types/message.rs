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
    /// Missing on some attachments. While it was required, one such file failed
    /// the whole conversation.
    pub file_info: Option<FileInfo>,
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
    pub card_client_id: Option<String>,
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
    /// `default` is not redundant: with `deserialize_with`, serde stops treating
    /// a missing field as `None`, and system messages carry no `from`.
    #[serde(default)]
    #[serde(deserialize_with = "strip_url_opt")]
    pub from: Option<String>,
    #[serde(alias = "imdisplayname")]
    pub im_display_name: Option<String>,
    #[serde(alias = "messagetype")]
    pub message_type: Option<String>,
    pub properties: Option<MessageProperties>,
    /// chatsvc spells this `composetime`, the aggregator `composeTime`.
    #[serde(alias = "composetime")]
    pub compose_time: Option<String>,
    #[serde(alias = "originalarrivaltime")]
    pub original_arrival_time: Option<String>,
    pub conversation_link: Option<String>,
    pub id: Option<String>,
    pub container_id: Option<String>,
}

/// Same as `strip_url`: keep the MRI, drop whatever regional URL wraps it.
fn strip_url_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.map(|url| super::last_segment(&url).to_string()))
}

/// Decode messages one by one, dropping the ones that fail.
///
/// A whole conversation that will not load costs the caller far more than a
/// missing message, and Teams keeps inventing message shapes.
pub fn deserialize_messages<'de, D>(deserializer: D) -> Result<Vec<Message>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<Value>::deserialize(deserializer)?;
    let mut kept: Vec<Message> = Vec::with_capacity(raw.len());
    let mut skipped = 0usize;

    for value in raw {
        match Message::deserialize(&value) {
            Ok(message) => kept.push(message),
            Err(e) => {
                skipped += 1;
                // Name the message: a silent drop is the expensive kind.
                let id = value.get("id").and_then(Value::as_str).unwrap_or("unknown");
                tracing::warn!("skipped message {id}: {e}");
            }
        }
    }

    if skipped > 0 {
        tracing::warn!("skipped {skipped} of {} messages", skipped + kept.len());
    }
    Ok(kept)
}

/// Conversations response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversations {
    #[serde(deserialize_with = "deserialize_messages")]
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

/// Graph API Chat response (from creating a chat)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphChat {
    pub id: String,
    pub topic: Option<String>,
    pub created_date_time: Option<String>,
    pub chat_type: Option<String>,
    pub web_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_mri(url: &str) -> Option<String> {
        let json = format!(r#"{{"from":"{url}"}}"#);
        serde_json::from_str::<Message>(&json)
            .expect("payload should parse")
            .from
    }

    /// A chat message as chatsvc sends one, trimmed to the fields a chat UI
    /// reads: an attachment, a reaction, an edit and a reply.
    const CHATSVC_MESSAGE: &str = r##"{
        "id": "1700000000000",
        "from": "8:orgid:u1",
        "composetime": "2030-01-01T10:00:00.000Z",
        "originalarrivaltime": "2030-01-01T10:00:00.000Z",
        "messagetype": "RichText/Html",
        "imdisplayname": "Ada Fenwick",
        "content": "<blockquote itemtype=\"http://schema.skype.com/Reply\" itemid=\"1699999999999\"></blockquote><p>hi</p>",
        "properties": {
            "edittime": "1700000000005",
            "deletetime": "0",
            "systemdelete": "false",
            "files": "[{\"id\":\"f1\",\"fileName\":\"y.docx\",\"fileType\":\"docx\",\"objectUrl\":\"https://x/y.docx\",\"fileInfo\":{\"fileUrl\":\"https://x/y.docx\"}}]",
            "emotions": [{"key":"like","users":[{"mri":"8:orgid:u2","time":1700000000009,"value":"1700000000009"}]}]
        }
    }"##;

    fn chatsvc_message_json() -> serde_json::Value {
        let msg: Message = serde_json::from_str(CHATSVC_MESSAGE).expect("payload should parse");
        serde_json::to_value(&msg).expect("message should serialise")
    }

    /// chatsvc sends `composetime`, so without the alias the field serialised
    /// as null on every message.
    #[test]
    fn compose_time_survives_the_lowercase_spelling() {
        assert_eq!(
            chatsvc_message_json()["composeTime"],
            "2030-01-01T10:00:00.000Z"
        );
    }

    /// What a client renders a conversation from. Dropping any of it would send
    /// the caller back to the API for every message.
    #[test]
    fn the_json_output_carries_what_a_chat_ui_needs() {
        let out = chatsvc_message_json();
        let props = &out["properties"];
        assert_eq!(props["files"][0]["fileName"], "y.docx");
        assert_eq!(props["files"][0]["fileInfo"]["fileUrl"], "https://x/y.docx");
        assert_eq!(props["emotions"][0]["key"], "like");
        assert_eq!(props["emotions"][0]["users"][0]["mri"], "8:orgid:u2");
        assert_eq!(props["edittime"], 1_700_000_000_005i64);
        assert_eq!(props["deletetime"], 0);
        assert_eq!(props["systemdelete"], false);
        // Reply and inline images live in the html, so it must stay unstripped.
        assert!(out["content"].as_str().unwrap().contains("itemid="));
    }

    /// A conversation is worth more than its worst message: the client shows
    /// what decoded instead of an error.
    #[test]
    fn one_malformed_message_costs_only_itself() {
        let payload = format!(
            r#"{{"messages":[{CHATSVC_MESSAGE},{{"id":"bad","content":42}},{CHATSVC_MESSAGE}]}}"#
        );
        let convs: Conversations =
            serde_json::from_str(&payload).expect("conversation should parse");
        assert_eq!(convs.messages.len(), 2);
        assert!(convs
            .messages
            .iter()
            .all(|m| m.id.as_deref() == Some("1700000000000")));
    }

    /// System messages carry no sender, and with `deserialize_with` serde needs
    /// `default` to accept that.
    #[test]
    fn a_message_without_from_still_decodes() {
        let msg: Message = serde_json::from_str(
            r#"{"id":"1","messagetype":"ThreadActivity/AddMember","content":"<addmember/>"}"#,
        )
        .expect("payload should parse");
        assert_eq!(msg.from, None);
        assert_eq!(
            msg.message_type.as_deref(),
            Some("ThreadActivity/AddMember")
        );
    }

    #[test]
    fn a_file_without_file_info_still_decodes() {
        let msg: Message = serde_json::from_str(
            r#"{"id":"1","properties":{"files":"[{\"id\":\"f1\",\"fileName\":\"y.docx\"}]"}}"#,
        )
        .expect("payload should parse");
        let files = msg.properties.expect("properties").files.expect("files");
        assert_eq!(files[0].file_name.as_deref(), Some("y.docx"));
        assert!(files[0].file_info.is_none());
    }

    #[test]
    fn message_from_keeps_the_mri_whatever_the_region() {
        for region in ["emea", "amer", "apac"] {
            let url = format!(
                "https://teams.microsoft.com/api/chatsvc/{region}/v1/users/ME/contacts/8:orgid:u1"
            );
            assert_eq!(from_mri(&url).as_deref(), Some("8:orgid:u1"));
        }
    }

    #[test]
    fn message_from_handles_notifications_hosts_and_bare_mris() {
        assert_eq!(
            from_mri("https://emea.notifications.skype.net/v1/users/ME/contacts/8:orgid:u1")
                .as_deref(),
            Some("8:orgid:u1")
        );
        assert_eq!(
            from_mri("https://notifications.skype.net/v1/users/ME/contacts/8:orgid:u1").as_deref(),
            Some("8:orgid:u1")
        );
        assert_eq!(from_mri("8:orgid:u1").as_deref(), Some("8:orgid:u1"));
    }
}
