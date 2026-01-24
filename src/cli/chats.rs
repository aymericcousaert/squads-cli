use std::io::{self, Read};

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_error, print_output, print_single, print_success};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct ChatsCommand {
    #[command(subcommand)]
    pub command: ChatsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ChatsSubcommand {
    /// List all chats
    List,

    /// Show chat details
    Show {
        /// Chat ID
        chat_id: String,
    },

    /// Get messages from a chat
    Messages {
        /// Chat ID
        chat_id: String,

        /// Maximum number of messages to retrieve
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Send a message to a chat
    Send {
        /// Chat ID
        chat_id: String,

        /// Message content
        message: Option<String>,

        /// Read message from stdin
        #[arg(short, long)]
        stdin: bool,

        /// Read message from file
        #[arg(long)]
        file: Option<String>,

        /// Treat message as Markdown and convert to HTML
        #[arg(short, long)]
        markdown: bool,

        /// Send raw HTML without escaping
        #[arg(long)]
        html: bool,
    },

    /// Create a new chat
    Create {
        /// User IDs or email addresses to add to the chat, comma-separated
        #[arg(short, long)]
        members: String,

        /// Chat topic (for group chats)
        #[arg(short, long)]
        topic: Option<String>,
    },

    /// Reply to a specific message in a thread
    Reply {
        /// Chat ID
        chat_id: String,

        /// Message ID to reply to
        #[arg(short, long)]
        message_id: String,

        /// Reply content
        content: String,
    },

    /// Delete a message
    Delete {
        /// Chat ID
        chat_id: String,

        /// Message ID to delete
        message_id: String,
    },

    /// React to a message
    React {
        /// Chat ID
        chat_id: String,

        /// Message ID to react to
        #[arg(short, long)]
        message_id: String,

        /// Reaction type (like, heart, laugh, surprised, sad, angry)
        reaction: String,

        /// Remove the reaction instead of adding it
        #[arg(long)]
        remove: bool,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct ChatRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Title")]
    title: String,
    #[tabled(rename = "Members")]
    members: usize,
    #[tabled(rename = "Unread")]
    unread: String,
    #[tabled(rename = "Type")]
    chat_type: String,
}

#[derive(Debug, Serialize, Tabled)]
struct MessageRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Content")]
    content: String,
}

pub async fn execute(cmd: ChatsCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        ChatsSubcommand::List => list(config, format).await,
        ChatsSubcommand::Show { chat_id } => show(config, &chat_id, format).await,
        ChatsSubcommand::Messages { chat_id, limit } => {
            messages(config, &chat_id, limit, format).await
        }
        ChatsSubcommand::Send {
            chat_id,
            message,
            stdin,
            file,
            markdown,
            html,
        } => send(config, &chat_id, message, stdin, file, markdown, html).await,
        ChatsSubcommand::Create { members, topic } => create(config, &members, topic, format).await,
        ChatsSubcommand::Reply {
            chat_id,
            message_id,
            content,
        } => reply(config, &chat_id, &message_id, &content).await,
        ChatsSubcommand::Delete {
            chat_id,
            message_id,
        } => delete(config, &chat_id, &message_id).await,
        ChatsSubcommand::React {
            chat_id,
            message_id,
            reaction,
            remove,
        } => react(config, &chat_id, &message_id, &reaction, remove).await,
    }
}

async fn list(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    let rows: Vec<ChatRow> = details
        .chats
        .into_iter()
        .map(|chat| {
            let title = chat.title.unwrap_or_else(|| {
                // For 1:1 chats, show member info
                if chat.members.len() == 2 {
                    "Direct Chat".to_string()
                } else {
                    format!("Group ({} members)", chat.members.len())
                }
            });

            ChatRow {
                id: chat.id,
                title: truncate(&title, 30),
                members: chat.members.len(),
                unread: if chat.is_read == Some(false) {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
                chat_type: chat.chat_type.unwrap_or_else(|| "chat".to_string()),
            }
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

async fn show(config: &Config, chat_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    if let Some(chat) = details.chats.into_iter().find(|c| c.id == chat_id) {
        print_single(&chat, format);
    } else {
        print_error(&format!("Chat not found: {}", chat_id));
    }

    Ok(())
}

async fn messages(
    config: &Config,
    chat_id: &str,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let conversations = client.get_conversations(chat_id, None).await?;

    let filtered_messages: Vec<_> = conversations
        .messages
        .into_iter()
        .filter(|m| {
            m.message_type.as_deref() == Some("RichText/Html")
                || m.message_type.as_deref() == Some("Text")
        })
        .take(limit)
        .collect();

    match format {
        OutputFormat::Json => {
            // Return full message data for AI agents
            print_single(&filtered_messages, format);
        }
        _ => {
            let rows: Vec<MessageRow> = filtered_messages
                .into_iter()
                .map(|msg| {
                    let content = msg.content.map(|c| strip_html(&c)).unwrap_or_default();

                    MessageRow {
                        id: msg.id.unwrap_or_default(),
                        from: msg
                            .im_display_name
                            .unwrap_or_else(|| msg.from.unwrap_or_else(|| "Unknown".to_string())),
                        time: msg.original_arrival_time.unwrap_or_default(),
                        content: truncate(&content, 50),
                    }
                })
                .collect();

            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn send(
    config: &Config,
    chat_id: &str,
    message: Option<String>,
    stdin: bool,
    file: Option<String>,
    markdown: bool,
    html: bool,
) -> Result<()> {
    let content = if let Some(msg) = message {
        msg
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else {
        print_error("No message provided. Use --stdin or --file, or provide message as argument.");
        return Ok(());
    };

    if content.is_empty() {
        print_error("Message cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    let html_body = if html {
        content
    } else if markdown {
        // Use markdown crate for proper MD -> HTML conversion
        markdown::to_html(&content)
    } else {
        // Convert plain text to simple HTML
        format!("<p>{}</p>", html_escape(&content))
    };

    client.send_message(chat_id, &html_body, None).await?;
    print_success("Message sent successfully");

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_len {
        let truncated: String = chars[..max_len.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

fn strip_html(s: &str) -> String {
    // Simple HTML stripping - remove tags
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

    // Decode common HTML entities
    result
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn create(
    config: &Config,
    members: &str,
    topic: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let member_list: Vec<&str> = members.split(',').map(|s| s.trim()).collect();

    let chat = client.create_chat(member_list, topic.as_deref()).await?;

    match format {
        OutputFormat::Json => {
            print_single(&chat, format);
        }
        _ => {
            print_success(&format!("Chat created with ID: {}", chat.id));
            if let Some(t) = chat.topic {
                println!("Topic: {}", t);
            }
            if let Some(url) = chat.web_url {
                println!("Open in Teams: {}", url);
            }
        }
    }
    Ok(())
}

async fn reply(config: &Config, chat_id: &str, message_id: &str, content: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client
        .reply_to_message(chat_id, message_id, content)
        .await?;
    print_success("Reply sent");
    Ok(())
}

async fn delete(config: &Config, chat_id: &str, message_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.delete_message(chat_id, message_id).await?;
    print_success("Message deleted");
    Ok(())
}

async fn react(
    config: &Config,
    chat_id: &str,
    message_id: &str,
    reaction: &str,
    remove: bool,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client
        .send_reaction(chat_id, message_id, reaction, remove)
        .await?;
    if remove {
        print_success("Reaction removed");
    } else {
        print_success("Reaction added");
    }
    Ok(())
}
