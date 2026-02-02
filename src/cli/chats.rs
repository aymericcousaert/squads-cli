use std::collections::HashMap;
use std::io::{self, Read};

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;
use crate::types::Chat;

use super::output::{print_error, print_output, print_single, print_success};
use super::utils::{html_escape, markdown_to_html, strip_html, truncate};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct ChatsCommand {
    #[command(subcommand)]
    pub command: ChatsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ChatsSubcommand {
    /// List all chats
    List {
        /// Maximum number of chats to return
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Search/filter chats by member names or title (case-insensitive, all words must match)
        #[arg(short, long)]
        search: Option<String>,
    },

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

        /// Treat content as Markdown and convert to HTML
        #[arg(short, long)]
        markdown: bool,

        /// Send raw HTML without escaping
        #[arg(long)]
        html: bool,
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

        /// Reaction type (like, heart, laugh, surprised, sad, angry, skull)
        reaction: String,

        /// Remove the reaction instead of adding it
        #[arg(long)]
        remove: bool,
    },

    /// Find messages where you are @mentioned
    Mentions {
        /// Maximum number of messages to scan per chat
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// List files shared in a chat
    Files {
        /// Chat ID
        chat_id: String,

        /// Maximum number of messages to scan for files
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Download a file from a chat
    DownloadFile {
        /// Chat ID
        chat_id: String,

        /// File URL or ID
        file_id: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },

    /// List images shared in a chat
    Images {
        /// Chat ID
        chat_id: String,

        /// Specific message ID to get images from (optional)
        #[arg(short, long)]
        message_id: Option<String>,

        /// Maximum number of messages to scan for images
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Download an image from a chat
    DownloadImage {
        /// Image URL (from images list)
        image_url: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },

    /// View reactions on a specific message
    Reactions {
        /// Chat ID
        chat_id: String,

        /// Message ID to get reactions for
        #[arg(short, long)]
        message_id: String,
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
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Reactions")]
    reactions: String,
    #[tabled(rename = "Content")]
    content: String,
}

#[derive(Debug, Serialize, Tabled)]
struct MentionRow {
    #[tabled(rename = "Chat")]
    chat: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Content")]
    content: String,
    #[tabled(rename = "Message ID")]
    message_id: String,
}

#[derive(Debug, Serialize, Tabled)]
struct FileRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    file_type: String,
    #[tabled(rename = "URL")]
    url: String,
    #[tabled(rename = "Message ID")]
    message_id: String,
}

#[derive(Debug, Serialize, Tabled)]
struct ImageRow {
    #[tabled(rename = "URL")]
    url: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Message ID")]
    message_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct MentionJson {
    chat_id: String,
    chat_name: String,
    message_id: String,
    from: String,
    time: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct FileJson {
    chat_id: String,
    message_id: String,
    file_name: String,
    file_type: String,
    file_url: String,
    share_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ImageJson {
    chat_id: String,
    message_id: String,
    image_url: String,
    from: String,
    time: String,
}

#[derive(Debug, Serialize, Tabled)]
struct ReactionRow {
    #[tabled(rename = "Reaction")]
    reaction: String,
    #[tabled(rename = "User")]
    user: String,
    #[tabled(rename = "Time")]
    time: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReactionJson {
    reaction: String,
    user_mri: String,
    user_name: Option<String>,
    timestamp: u64,
}

pub async fn execute(cmd: ChatsCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        ChatsSubcommand::List { limit, search } => list(config, limit, search, format).await,
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
            markdown,
            html,
        } => reply(config, &chat_id, &message_id, &content, markdown, html).await,
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
        ChatsSubcommand::Mentions { limit } => mentions(config, limit, format).await,
        ChatsSubcommand::Files { chat_id, limit } => files(config, &chat_id, limit, format).await,
        ChatsSubcommand::DownloadFile {
            chat_id,
            file_id,
            output,
        } => download_file(config, &chat_id, &file_id, output).await,
        ChatsSubcommand::Images {
            chat_id,
            message_id,
            limit,
        } => images(config, &chat_id, message_id, limit, format).await,
        ChatsSubcommand::DownloadImage { image_url, output } => {
            download_image(config, &image_url, output).await
        }
        ChatsSubcommand::Reactions {
            chat_id,
            message_id,
        } => reactions(config, &chat_id, &message_id, format).await,
    }
}

async fn list(
    config: &Config,
    limit: usize,
    search: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    // Get current user's ID to exclude from member names
    let my_user_id = client.get_me().await.ok().map(|me| me.id);

    // Collect unique user IDs that need name resolution
    let mut unique_ids: Vec<String> = Vec::new();
    for chat in &details.chats {
        for member in &chat.members {
            if let Some(obj_id) = &member.object_id {
                if !unique_ids.contains(obj_id) && my_user_id.as_ref() != Some(obj_id) {
                    unique_ids.push(obj_id.clone());
                }
            }
        }
    }

    // Resolve user names (limit to first 50 to avoid too many API calls)
    let mut user_names: HashMap<String, String> = HashMap::new();
    for user_id in unique_ids.into_iter().take(50) {
        if let Ok(Some(user)) = client.get_user_by_id(&user_id).await {
            if let Some(name) = user.display_name {
                user_names.insert(user_id, name);
            }
        }
    }

    // Build chat rows with resolved names
    // Split search into words for fuzzy matching (all words must match)
    let search_words: Option<Vec<String>> = search.as_ref().map(|s| {
        s.to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect()
    });

    let rows: Vec<ChatRow> = details
        .chats
        .into_iter()
        .filter_map(|chat| {
            let title = get_chat_display_name(&chat, &user_names, my_user_id.as_ref());

            // Apply search filter if provided (all words must match)
            if let Some(ref words) = search_words {
                let title_lower = title.to_lowercase();
                if !words.iter().all(|word| title_lower.contains(word)) {
                    return None;
                }
            }

            Some(ChatRow {
                id: chat.id,
                title: truncate(&title, 40),
                members: chat.members.len(),
                unread: if chat.is_read == Some(false) {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
                chat_type: chat.chat_type.unwrap_or_else(|| "chat".to_string()),
            })
        })
        .take(limit)
        .collect();

    print_output(&rows, format);
    Ok(())
}

/// Get display name for a chat based on members (similar to TUI logic)
fn get_chat_display_name(
    chat: &Chat,
    user_names: &HashMap<String, String>,
    my_user_id: Option<&String>,
) -> String {
    // If chat has a meaningful title set, use it
    if let Some(title) = &chat.title {
        if !title.is_empty()
            && title != "Direct Chat"
            && title != "Group Chat"
            && !title.starts_with("Group (")
        {
            return title.clone();
        }
    }

    // Get member names, excluding myself
    let member_names: Vec<String> = chat
        .members
        .iter()
        .filter_map(|m| {
            let obj_id = m.object_id.as_ref()?;
            // Skip if this is me
            if my_user_id == Some(obj_id) {
                return None;
            }
            // Look up name in cache
            user_names.get(obj_id).cloned()
        })
        .collect();

    if !member_names.is_empty() {
        // Join names with "&"
        if member_names.len() <= 3 {
            return member_names.join(" & ");
        } else {
            // For many members, show first 2 and count
            return format!(
                "{} & {} +{}",
                member_names[0],
                member_names[1],
                member_names.len() - 2
            );
        }
    }

    // Fallback: try to get name from last message sender (if not from me)
    if let Some(last_msg) = &chat.last_message {
        if chat.is_last_message_from_me != Some(true) {
            if let Some(name) = &last_msg.im_display_name {
                return name.clone();
            }
        }
    }

    // Final fallback
    if chat.is_one_on_one == Some(true) {
        "1:1 Chat".to_string()
    } else {
        format!("Group ({} members)", chat.members.len())
    }
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
                    let reactions = crate::api::emoji::format_reactions_summary(&msg.properties);

                    let mut status = Vec::new();
                    if let Some(props) = &msg.properties {
                        if props.deletetime > 0 {
                            status.push("DELETED");
                        }
                        if props.systemdelete {
                            status.push("SYS_DEL");
                        }
                    }
                    let status_str = if status.is_empty() {
                        "ACTIVE".to_string()
                    } else {
                        status.join("|")
                    };

                    MessageRow {
                        id: msg.id.unwrap_or_default(),
                        from: msg
                            .im_display_name
                            .unwrap_or_else(|| msg.from.unwrap_or_else(|| "Unknown".to_string())),
                        status: status_str,
                        time: msg.original_arrival_time.unwrap_or_default(),
                        reactions,
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
        markdown_to_html(&content)
    } else {
        format!("<p>{}</p>", html_escape(&content))
    };

    client.send_message(chat_id, &html_body, None).await?;
    print_success("Message sent successfully");

    Ok(())
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

async fn reply(
    config: &Config,
    chat_id: &str,
    message_id: &str,
    content: &str,
    markdown: bool,
    html: bool,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let html_body = if html {
        content.to_string()
    } else if markdown {
        markdown_to_html(content)
    } else {
        format!("<p>{}</p>", html_escape(content))
    };

    client
        .reply_to_message(chat_id, message_id, &html_body)
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

async fn mentions(config: &Config, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let me = client.get_me().await?;
    let my_id = me.id.clone();

    let details = client.get_user_details().await?;

    let mut all_mentions: Vec<MentionJson> = Vec::new();

    for chat in &details.chats {
        if let Ok(convs) = client.get_conversations(&chat.id, None).await {
            for msg in convs.messages.iter().take(limit) {
                // Check if this is a user message
                if msg.message_type.as_deref() != Some("RichText/Html")
                    && msg.message_type.as_deref() != Some("Text")
                {
                    continue;
                }

                // Check if message content contains an @mention of current user
                let content = msg.content.as_deref().unwrap_or("");

                // Look for <at id="user_mri"> pattern or user ID in content
                let is_mentioned = content.contains(&format!("8:orgid:{}", my_id))
                    || content.contains(&format!("id=\"8:orgid:{}\"", my_id))
                    || content.to_lowercase().contains(&format!(
                        "@{}",
                        me.display_name.as_deref().unwrap_or("").to_lowercase()
                    ));

                if is_mentioned {
                    let chat_name = chat
                        .title
                        .clone()
                        .unwrap_or_else(|| "Direct Chat".to_string());

                    all_mentions.push(MentionJson {
                        chat_id: chat.id.clone(),
                        chat_name: chat_name.clone(),
                        message_id: msg.id.clone().unwrap_or_default(),
                        from: msg
                            .im_display_name
                            .clone()
                            .or(msg.from.clone())
                            .unwrap_or_else(|| "Unknown".to_string()),
                        time: msg.original_arrival_time.clone().unwrap_or_default(),
                        content: strip_html(content),
                    });
                }
            }
        }
    }

    if all_mentions.is_empty() {
        println!("No mentions found.");
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            print_single(&all_mentions, format);
        }
        _ => {
            let rows: Vec<MentionRow> = all_mentions
                .into_iter()
                .map(|m| MentionRow {
                    chat: truncate(&m.chat_name, 20),
                    from: truncate(&m.from, 15),
                    time: m.time,
                    content: truncate(&m.content, 40),
                    message_id: m.message_id,
                })
                .collect();

            print_output(&rows, format);
        }
    }

    Ok(())
}

async fn files(config: &Config, chat_id: &str, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let convs = client.get_conversations(chat_id, None).await?;

    let mut all_files: Vec<FileJson> = Vec::new();

    for msg in convs.messages.iter().take(limit) {
        if let Some(props) = &msg.properties {
            if let Some(files) = &props.files {
                for file in files {
                    all_files.push(FileJson {
                        chat_id: chat_id.to_string(),
                        message_id: msg.id.clone().unwrap_or_default(),
                        file_name: file
                            .file_name
                            .clone()
                            .unwrap_or_else(|| "Unknown".to_string()),
                        file_type: file.file_type.clone().unwrap_or_else(|| "-".to_string()),
                        file_url: file.object_url.clone().unwrap_or_default(),
                        share_url: file.file_info.share_url.clone(),
                    });
                }
            }
        }
    }

    if all_files.is_empty() {
        println!("No files found in this chat.");
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            print_single(&all_files, format);
        }
        _ => {
            let rows: Vec<FileRow> = all_files
                .into_iter()
                .map(|f| FileRow {
                    name: truncate(&f.file_name, 30),
                    file_type: f.file_type,
                    url: truncate(&f.file_url, 40),
                    message_id: f.message_id,
                })
                .collect();

            print_output(&rows, format);
        }
    }

    Ok(())
}

async fn download_file(
    config: &Config,
    chat_id: &str,
    file_id: &str,
    output: Option<String>,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    // If file_id looks like a URL, download directly
    let file_url = if file_id.starts_with("http") {
        file_id.to_string()
    } else {
        // Search for the file in messages
        let convs = client.get_conversations(chat_id, None).await?;
        let mut found_url = None;

        for msg in &convs.messages {
            if let Some(props) = &msg.properties {
                if let Some(files) = &props.files {
                    for file in files {
                        if file.id.as_deref() == Some(file_id)
                            || file.item_id.as_deref() == Some(file_id)
                        {
                            if let Some(url) = &file.file_info.file_url {
                                found_url = Some(url.clone());
                                break;
                            }
                            if let Some(url) = &file.object_url {
                                found_url = Some(url.clone());
                                break;
                            }
                        }
                    }
                }
            }
            if found_url.is_some() {
                break;
            }
        }

        found_url.ok_or_else(|| anyhow::anyhow!("File not found: {}", file_id))?
    };

    let (content_type, bytes) = client.download_sharepoint_file(&file_url).await?;

    if output.as_deref() == Some("-") {
        use std::io::Write;
        io::stdout().write_all(&bytes)?;
        io::stdout().flush()?;
    } else {
        let output_path = output.unwrap_or_else(|| {
            // Try to extract filename from URL or use default
            file_url
                .split('/')
                .next_back()
                .unwrap_or("downloaded_file")
                .split('?')
                .next()
                .unwrap_or("downloaded_file")
                .to_string()
        });

        std::fs::write(&output_path, &bytes)?;
        print_success(&format!(
            "Downloaded {} ({}, {} bytes)",
            output_path,
            content_type,
            bytes.len()
        ));
    }

    Ok(())
}

async fn images(
    config: &Config,
    chat_id: &str,
    message_id: Option<String>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let convs = if let Some(msg_id) = message_id {
        // Parse message ID to u64 if possible
        let msg_id_num = msg_id.parse::<u64>().ok();
        client.get_conversations(chat_id, msg_id_num).await?
    } else {
        client.get_conversations(chat_id, None).await?
    };

    let mut all_images: Vec<ImageJson> = Vec::new();

    for msg in convs.messages.iter().take(limit) {
        if msg.message_type.as_deref() != Some("RichText/Html")
            && msg.message_type.as_deref() != Some("Text")
        {
            continue;
        }

        let content = msg.content.as_deref().unwrap_or("");

        // Extract image URLs from <img> tags
        let img_urls = extract_image_urls(content);

        for url in img_urls {
            all_images.push(ImageJson {
                chat_id: chat_id.to_string(),
                message_id: msg.id.clone().unwrap_or_default(),
                image_url: url,
                from: msg
                    .im_display_name
                    .clone()
                    .or(msg.from.clone())
                    .unwrap_or_else(|| "Unknown".to_string()),
                time: msg.original_arrival_time.clone().unwrap_or_default(),
            });
        }
    }

    if all_images.is_empty() {
        println!("No images found in this chat.");
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            print_single(&all_images, format);
        }
        _ => {
            let rows: Vec<ImageRow> = all_images
                .into_iter()
                .map(|i| ImageRow {
                    url: truncate(&i.image_url, 60),
                    from: truncate(&i.from, 15),
                    time: i.time,
                    message_id: i.message_id,
                })
                .collect();

            print_output(&rows, format);
        }
    }

    Ok(())
}

fn extract_image_urls(content: &str) -> Vec<String> {
    let mut urls = Vec::new();

    // Simple regex-like extraction of src attributes from img tags
    let mut remaining = content;
    while let Some(img_start) = remaining.find("<img") {
        remaining = &remaining[img_start..];

        if let Some(src_start) = remaining.find("src=\"") {
            let src_content = &remaining[src_start + 5..];
            if let Some(src_end) = src_content.find('"') {
                let url = &src_content[..src_end];
                // Only include AMS URLs or other image URLs
                if url.contains("ams")
                    || url.contains("teams.microsoft.com")
                    || url.contains("blob")
                    || url.starts_with("http")
                {
                    // Decode HTML entities in URL
                    let decoded_url = url
                        .replace("&amp;", "&")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">");
                    urls.push(decoded_url);
                }
            }
        }

        // Move past this img tag
        if let Some(end) = remaining.find('>') {
            remaining = &remaining[end + 1..];
        } else {
            break;
        }
    }

    urls
}

async fn download_image(config: &Config, image_url: &str, output: Option<String>) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let (content_type, bytes) = client.download_ams_image(image_url).await?;

    let extension = match content_type.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    };

    let output_path =
        output.unwrap_or_else(|| format!("image_{}.{}", chrono::Utc::now().timestamp(), extension));

    std::fs::write(&output_path, &bytes)?;
    print_success(&format!(
        "Downloaded {} ({}, {} bytes)",
        output_path,
        content_type,
        bytes.len()
    ));

    Ok(())
}

async fn reactions(
    config: &Config,
    chat_id: &str,
    message_id: &str,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let convs = client.get_conversations(chat_id, None).await?;

    // Find the specific message
    let message = convs
        .messages
        .iter()
        .find(|m| m.id.as_deref() == Some(message_id));

    let Some(msg) = message else {
        print_error(&format!("Message not found: {}", message_id));
        return Ok(());
    };

    let mut all_reactions: Vec<ReactionJson> = Vec::new();

    if let Some(props) = &msg.properties {
        if let Some(emotions) = &props.emotions {
            for emotion in emotions {
                for user in &emotion.users {
                    all_reactions.push(ReactionJson {
                        reaction: emotion.key.clone(),
                        user_mri: user.mri.clone(),
                        user_name: None, // Could resolve user names if needed
                        timestamp: user.time,
                    });
                }
            }
        }
    }

    if all_reactions.is_empty() {
        println!("No reactions on this message.");
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            print_single(&all_reactions, format);
        }
        _ => {
            let rows: Vec<ReactionRow> = all_reactions
                .into_iter()
                .map(|r| {
                    // Extract user ID from MRI (8:orgid:uuid -> uuid)
                    let user_display = r
                        .user_mri
                        .strip_prefix("8:orgid:")
                        .unwrap_or(&r.user_mri)
                        .to_string();

                    // Convert timestamp to readable time
                    let time = chrono::DateTime::from_timestamp_millis(r.timestamp as i64)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| r.timestamp.to_string());

                    ReactionRow {
                        reaction: r.reaction,
                        user: truncate(&user_display, 36),
                        time,
                    }
                })
                .collect();

            print_output(&rows, format);
        }
    }

    Ok(())
}
