use serde::{Deserialize, Serialize};

/// Email address with optional name
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAddress {
    pub address: String,
    pub name: Option<String>,
}

/// Recipient wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipient {
    pub email_address: EmailAddress,
}

/// Email body
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemBody {
    pub content_type: String,
    pub content: String,
}

/// Email message from Outlook
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    pub id: Option<String>,
    pub subject: Option<String>,
    pub body_preview: Option<String>,
    pub body: Option<ItemBody>,
    pub from: Option<Recipient>,
    pub to_recipients: Option<Vec<Recipient>>,
    pub cc_recipients: Option<Vec<Recipient>>,
    pub received_date_time: Option<String>,
    pub sent_date_time: Option<String>,
    pub is_read: Option<bool>,
    pub is_draft: Option<bool>,
    pub has_attachments: Option<bool>,
    pub importance: Option<String>,
    pub web_link: Option<String>,
}

/// Mail messages list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessages {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    pub value: Vec<MailMessage>,
}

/// Mail folder
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailFolder {
    pub id: String,
    pub display_name: String,
    pub parent_folder_id: Option<String>,
    pub child_folder_count: Option<i32>,
    pub unread_item_count: Option<i32>,
    pub total_item_count: Option<i32>,
}

/// Mail folders list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailFolders {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<MailFolder>,
}

/// Message to send
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMailRequest {
    pub message: SendMailMessage,
    pub save_to_sent_items: bool,
}

/// Message content for sending
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMailMessage {
    pub subject: String,
    pub body: ItemBody,
    pub to_recipients: Vec<Recipient>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_recipients: Option<Vec<Recipient>>,
}

/// Request to create a draft message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDraftRequest {
    pub subject: String,
    pub body: ItemBody,
    pub to_recipients: Vec<Recipient>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_recipients: Option<Vec<Recipient>>,
}

/// Email attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachment {
    pub id: Option<String>,
    pub name: String,
    pub content_type: Option<String>,
    pub size: Option<i64>,
    pub is_inline: Option<bool>,
    pub content_bytes: Option<String>,
}

/// Mail attachments list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAttachments {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<MailAttachment>,
}
