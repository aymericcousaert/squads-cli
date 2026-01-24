use anyhow::Result;
use clap::{Args, ValueEnum};
use colored::Colorize;
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_output, print_single};
use super::utils::{strip_html, truncate};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct FeedCommand {
    /// Filter by source
    #[arg(short, long, value_enum, default_value = "all")]
    pub source: FeedSource,

    /// Maximum number of items to show
    #[arg(short, long, default_value = "30")]
    pub limit: usize,

    /// Only show unread items
    #[arg(short, long)]
    pub unread: bool,

    /// Only show items where you are @mentioned
    #[arg(long)]
    pub mentions_only: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum FeedSource {
    /// Show both Teams messages and emails
    #[default]
    All,
    /// Show only Teams messages
    Chats,
    /// Show only emails
    Mail,
}

#[derive(Debug, Clone, Serialize, Tabled)]
struct FeedItem {
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Type")]
    item_type: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Subject/Content")]
    content: String,
    #[tabled(rename = "Unread")]
    unread: String,
}

#[derive(Debug, Clone, Serialize)]
struct FeedItemJson {
    time: String,
    timestamp: i64,
    item_type: String,
    from: String,
    content: String,
    unread: bool,
    source_id: String,
    chat_id: Option<String>,
}

pub async fn execute(cmd: FeedCommand, config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;

    // Get current user info for mentions filtering
    let (my_id, my_name) = if cmd.mentions_only {
        let me = client.get_me().await?;
        (
            me.id.clone(),
            me.display_name.clone().unwrap_or_default().to_lowercase(),
        )
    } else {
        (String::new(), String::new())
    };

    let mut items: Vec<FeedItemJson> = Vec::new();

    // Collect chat messages
    if matches!(cmd.source, FeedSource::All | FeedSource::Chats) {
        if let Ok(details) = client.get_user_details().await {
            for chat in &details.chats {
                // Check if chat has unread messages
                let chat_unread = chat.is_read == Some(false);

                if cmd.unread && !chat_unread {
                    continue;
                }

                if let Ok(convs) = client.get_conversations(&chat.id, Some(10)).await {
                    for msg in convs.messages {
                        // Skip non-user messages
                        if msg.message_type.as_deref() != Some("RichText/Html")
                            && msg.message_type.as_deref() != Some("Text")
                        {
                            continue;
                        }

                        let raw_content = msg.content.clone().unwrap_or_default();

                        // Filter for mentions if requested
                        if cmd.mentions_only {
                            let is_mentioned = raw_content.contains(&format!("8:orgid:{}", my_id))
                                || raw_content.contains(&format!("id=\"8:orgid:{}\"", my_id))
                                || raw_content
                                    .to_lowercase()
                                    .contains(&format!("@{}", my_name));

                            if !is_mentioned {
                                continue;
                            }
                        }

                        let sender = msg
                            .im_display_name
                            .clone()
                            .or(msg.from.clone())
                            .unwrap_or_else(|| "Unknown".to_string());

                        let content = strip_html(&raw_content);

                        let time_str = msg.original_arrival_time.clone().unwrap_or_default();
                        let timestamp = parse_timestamp(&time_str);

                        let chat_name = chat
                            .title
                            .clone()
                            .unwrap_or_else(|| "Direct Chat".to_string());

                        items.push(FeedItemJson {
                            time: format_time(&time_str),
                            timestamp,
                            item_type: "💬 Chat".to_string(),
                            from: format!("{} ({})", sender, truncate(&chat_name, 20)),
                            content: truncate(&content, 50),
                            unread: chat_unread,
                            source_id: msg.id.clone().unwrap_or_default(),
                            chat_id: Some(chat.id.clone()),
                        });
                    }
                }
            }
        }
    }

    // Collect emails
    if matches!(cmd.source, FeedSource::All | FeedSource::Mail) {
        if let Ok(emails) = client.get_mail_messages(Some("inbox"), 50).await {
            for email in emails.value {
                let is_unread = email.is_read != Some(true);

                if cmd.unread && !is_unread {
                    continue;
                }

                let sender = email
                    .from
                    .as_ref()
                    .map(|f| {
                        f.email_address
                            .name
                            .clone()
                            .unwrap_or_else(|| f.email_address.address.clone())
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                let subject = email
                    .subject
                    .clone()
                    .unwrap_or_else(|| "(No subject)".to_string());

                let time_str = email.received_date_time.clone().unwrap_or_default();
                let timestamp = parse_timestamp(&time_str);

                items.push(FeedItemJson {
                    time: format_time(&time_str),
                    timestamp,
                    item_type: "📧 Mail".to_string(),
                    from: truncate(&sender, 25),
                    content: truncate(&subject, 50),
                    unread: is_unread,
                    source_id: email.id.clone().unwrap_or_default(),
                    chat_id: None,
                });
            }
        }
    }

    // Sort by timestamp (newest first)
    items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Limit results
    items.truncate(cmd.limit);

    match format {
        OutputFormat::Json => {
            print_single(&items, format);
        }
        _ => {
            if items.is_empty() {
                println!(
                    "{}",
                    if cmd.unread {
                        "No unread items found."
                    } else {
                        "No items found."
                    }
                );
                return Ok(());
            }

            let rows: Vec<FeedItem> = items
                .into_iter()
                .map(|i| FeedItem {
                    time: i.time,
                    item_type: i.item_type,
                    from: i.from,
                    content: i.content,
                    unread: if i.unread {
                        "●".yellow().to_string()
                    } else {
                        " ".to_string()
                    },
                })
                .collect();

            print_output(&rows, format);
        }
    }

    Ok(())
}

fn parse_timestamp(time_str: &str) -> i64 {
    // Try parsing ISO 8601 format
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(time_str) {
        return dt.timestamp();
    }

    // Try with Z suffix
    let with_z = if time_str.ends_with('Z') {
        time_str.to_string()
    } else {
        format!("{}Z", time_str)
    };

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_z) {
        return dt.timestamp();
    }

    0
}

fn format_time(time_str: &str) -> String {
    let with_z = if time_str.ends_with('Z') {
        time_str.to_string()
    } else {
        format!("{}Z", time_str)
    };

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_z) {
        let local = dt.with_timezone(&chrono::Local);
        let now = chrono::Local::now();

        // If today, show time only
        if local.date_naive() == now.date_naive() {
            return local.format("%H:%M").to_string();
        }

        // If this year, show month and day
        if local.year() == now.year() {
            return local.format("%m-%d %H:%M").to_string();
        }

        // Otherwise show full date
        return local.format("%Y-%m-%d %H:%M").to_string();
    }

    // Fallback: extract time from string
    if time_str.len() > 16 {
        time_str[11..16].to_string()
    } else {
        time_str.to_string()
    }
}

use chrono::Datelike;
