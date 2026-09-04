use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::{Args, Subcommand};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::api::{TeamsClient, SCOPE_GRAPH, SCOPE_IC3};
use crate::cache::{Cache, USERS_FILE};
use crate::config::Config;
use crate::types::Chat;

use super::notes::NOTES_CHAT_ID;
use super::output::{print_error, print_output, print_single, print_success};
use super::utils::{html_escape, markdown_to_html, strip_html, truncate};
use super::OutputFormat;

/// Parallel Graph user lookups when listing chats.
const GRAPH_CONCURRENCY: usize = 8;
/// Parallel chat message fetches when falling back to sender names.
const MESSAGES_CONCURRENCY: usize = 4;
/// How long a cached display name stays usable. People rename themselves, and
/// nothing else clears this cache.
const NAME_CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// A display name on disk, with the time it was resolved.
#[derive(Serialize, Deserialize)]
struct CachedName {
    name: String,
    at: u64,
}

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
        /// Chat ID (or message if --to is used)
        chat_id_or_message: Option<String>,

        /// Message content (when chat_id is provided)
        message: Option<String>,

        /// Send to a user by name or email (finds or creates 1:1 chat)
        #[arg(short, long)]
        to: Option<String>,

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

        /// Attach file(s) (can be specified multiple times)
        #[arg(short, long = "attachment")]
        attachments: Vec<String>,
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
            chat_id_or_message,
            message,
            to,
            stdin,
            file,
            markdown,
            html,
            attachments,
        } => {
            send(
                config,
                chat_id_or_message,
                to,
                message,
                stdin,
                file,
                markdown,
                html,
                &attachments,
            )
            .await
        }
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

    // Without a search filter only the first `limit` chats can reach the output,
    // so resolving names for the rest is wasted work. An account can have
    // hundreds of chats.
    let candidates: Vec<&Chat> = if search.is_some() {
        details.chats.iter().collect()
    } else {
        details.chats.iter().take(limit).collect()
    };
    let user_names = resolve_member_names(&client, &candidates, my_user_id.as_deref()).await;

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

/// The chat's own title, when Teams set a real one instead of a placeholder.
/// A chat with a real title needs no member name resolved.
fn chat_title(chat: &Chat) -> Option<&str> {
    let title = chat.title.as_deref()?;
    let placeholder = title.is_empty()
        || title == "Direct Chat"
        || title == "Group Chat"
        || title.starts_with("Group (");
    (!placeholder).then_some(title)
}

/// Resolve the display names of chat members, keyed by user object ID.
///
/// Sources are tried cheapest first: the on-disk cache, then Graph, then chat
/// messages for the users Graph cannot see (cross-tenant guests). Lookups run
/// in parallel and newly found names are cached for the next run, until they
/// reach `NAME_CACHE_TTL_SECS`.
async fn resolve_member_names(
    client: &TeamsClient,
    chats: &[&Chat],
    my_user_id: Option<&str>,
) -> HashMap<String, String> {
    let now = epoch_secs();
    let cache = Cache::new().ok();
    let mut cached: HashMap<String, CachedName> = cache
        .as_ref()
        .and_then(|c| c.load(USERS_FILE).ok().flatten())
        .unwrap_or_default();
    let loaded_count = cached.len();
    cached.retain(|_, entry| now.saturating_sub(entry.at) < NAME_CACHE_TTL_SECS);

    let mut names: HashMap<String, String> = cached
        .iter()
        .map(|(id, entry)| (id.clone(), entry.name.clone()))
        .collect();

    // Chats Teams gave a real title to need no member name resolved.
    let untitled: Vec<&&Chat> = chats
        .iter()
        .filter(|chat| chat_title(chat).is_none())
        .collect();

    let mut missing: Vec<String> = Vec::new();
    for chat in &untitled {
        for id in member_ids(chat, my_user_id) {
            if !names.contains_key(&id) && !missing.contains(&id) {
                missing.push(id);
            }
        }
    }

    // Mint the token before fanning out: parallel requests would each mint
    // their own and each rewrite the token cache, which can corrupt it. Without
    // a token the lookups would all fail anyway, so give up on this source.
    if !missing.is_empty() && client.get_token(SCOPE_GRAPH).await.is_ok() {
        let found = stream::iter(&missing)
            .map(|id| async move {
                let name = client
                    .get_user_by_id(id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|user| user.display_name);
                (id.clone(), name)
            })
            .buffer_unordered(GRAPH_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for (id, name) in found {
            if let Some(name) = name {
                names.insert(id, name);
            }
        }
    }

    // Graph knows nothing about cross-tenant users, but their messages carry
    // `imdisplayname`. One fetch per chat covers all of its members. Members the
    // chat payload already names are skipped: that name is fresher than a
    // sender name taken from an old message, and it costs no request.
    let pending: Vec<(&str, Vec<String>)> = untitled
        .iter()
        .filter_map(|chat| {
            let ids: Vec<String> = chat
                .members
                .iter()
                .filter(|member| {
                    member
                        .display_name
                        .as_deref()
                        .is_none_or(|name| name.is_empty())
                })
                .filter_map(|member| member.object_id.clone())
                .filter(|id| my_user_id != Some(id.as_str()))
                .filter(|id| !names.contains_key(id))
                .collect();
            if ids.is_empty() {
                return None;
            }
            Some((chat.id.as_str(), ids))
        })
        .collect();

    if !pending.is_empty() && client.get_token(SCOPE_IC3).await.is_ok() {
        let found = stream::iter(&pending)
            .map(|(chat_id, ids)| async move {
                client
                    .resolve_names_from_messages(chat_id, ids)
                    .await
                    .unwrap_or_default()
            })
            .buffer_unordered(MESSAGES_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for chat_names in found {
            for (id, name) in chat_names {
                names.entry(id).or_insert(name);
            }
        }
    }

    // Rewrite only when something moved: new names found, or expired ones dropped.
    if names.len() != cached.len() || cached.len() != loaded_count {
        if let Some(cache) = &cache {
            let entries: HashMap<&str, CachedName> = names
                .iter()
                .map(|(id, name)| {
                    let at = cached.get(id).map_or(now, |entry| entry.at);
                    (
                        id.as_str(),
                        CachedName {
                            name: name.clone(),
                            at,
                        },
                    )
                })
                .collect();
            let _ = cache.save(USERS_FILE, &entries);
        }
    }

    names
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Object IDs of a chat's members, without the current user.
fn member_ids(chat: &Chat, my_user_id: Option<&str>) -> Vec<String> {
    chat.members
        .iter()
        .filter_map(|member| member.object_id.clone())
        .filter(|id| my_user_id != Some(id.as_str()))
        .collect()
}

/// Get display name for a chat based on members (similar to TUI logic)
fn get_chat_display_name(
    chat: &Chat,
    user_names: &HashMap<String, String>,
    my_user_id: Option<&String>,
) -> String {
    if let Some(title) = chat_title(chat) {
        return title.to_string();
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
            // Look up name in cache, then try displayName from API response
            user_names
                .get(obj_id)
                .cloned()
                .or_else(|| m.display_name.clone())
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

#[allow(clippy::too_many_arguments)]
async fn send(
    config: &Config,
    chat_id_or_message: Option<String>,
    to: Option<String>,
    message: Option<String>,
    stdin: bool,
    file: Option<String>,
    markdown: bool,
    html: bool,
    attachments: &[String],
) -> Result<()> {
    // When --to is used, the first positional arg is the message, not chat_id
    let (chat_id, actual_message) = if to.is_some() {
        (None, chat_id_or_message)
    } else {
        (chat_id_or_message, message)
    };

    let content = if let Some(msg) = actual_message {
        msg
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else if !attachments.is_empty() {
        // A file on its own is a message.
        String::new()
    } else {
        print_error("No message provided. Use --stdin or --file, or provide message as argument.");
        return Ok(());
    };

    if content.is_empty() && attachments.is_empty() {
        print_error("Message cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    // Resolve the chat ID
    let destination = if let Some(to_query) = to {
        // Search for user by name or email
        resolve_user_to_chat(&client, &to_query).await?
    } else if let Some(id) = chat_id {
        Destination {
            label: format!("chat {}", id),
            chat_id: id,
        }
    } else {
        print_error("Either chat_id or --to must be provided");
        return Ok(());
    };

    let html_body = if content.is_empty() {
        String::new()
    } else if html {
        content
    } else if markdown {
        markdown_to_html(&content)
    } else {
        format!("<p>{}</p>", html_escape(&content))
    };

    if attachments.is_empty() {
        client
            .send_message(&destination.chat_id, &html_body, None)
            .await?;
        print_success(&format!("Message sent to {}", destination.label));
        return Ok(());
    }

    // The Notes chat only exists on the internal chat service, so Graph cannot
    // post to it. Stop before uploading anything.
    if destination.chat_id == NOTES_CHAT_ID {
        print_error("Attachments are not supported in your Notes chat");
        return Ok(());
    }

    // Check every path before the first upload: bailing halfway would leave the
    // earlier files sitting in OneDrive with nothing pointing at them.
    let paths: Vec<&Path> = attachments.iter().map(Path::new).collect();
    if let Some(missing) = paths.iter().find(|p| !p.is_file()) {
        print_error(&format!("Attachment not found: {}", missing.display()));
        return Ok(());
    }

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        files.push(client.upload_chat_file(path).await?);
    }

    client
        .send_message_with_files(&destination.chat_id, &html_body, &files)
        .await?;
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    print_success(&format!(
        "Message sent to {} with {}",
        destination.label,
        names.join(", ")
    ));

    Ok(())
}

/// A chat that was positively identified as the target of a send
struct Destination {
    chat_id: String,
    /// Who the message goes to, for the success line
    label: String,
}

/// Resolve a user query (name or email) to the chat to send to.
///
/// Never falls back to an arbitrary chat: if the query does not identify one
/// person, this returns an error instead of picking a chat.
async fn resolve_user_to_chat(client: &TeamsClient, query: &str) -> Result<Destination> {
    // Search for users matching the query
    let users_response = client.search_users(query, 10).await?;
    let users = &users_response.value;

    if users.is_empty() {
        anyhow::bail!("No users found matching \"{}\"", query);
    }

    // Case-insensitive and accent-safe: to_lowercase() folds non-ASCII letters,
    // eq_ignore_ascii_case() does not
    let wanted = query.to_lowercase();
    let user = if users.len() == 1 {
        &users[0]
    } else if let Some(user) = users.iter().find(|u| {
        u.mail
            .as_ref()
            .is_some_and(|e: &String| e.to_lowercase() == wanted)
    }) {
        user
    } else if let Some(user) = users.iter().find(|u| {
        u.display_name
            .as_ref()
            .is_some_and(|n: &String| n.to_lowercase() == wanted)
    }) {
        user
    } else {
        // Multiple matches, show them to the user
        eprintln!("Multiple users found matching \"{}\":", query);
        for (i, user) in users.iter().enumerate() {
            let name = user.display_name.as_deref().unwrap_or("Unknown");
            let email = user.mail.as_deref().unwrap_or("no email");
            eprintln!("  {}. {} ({})", i + 1, name, email);
        }
        anyhow::bail!("Please use full name or email to be more specific");
    };

    let name = user
        .display_name
        .clone()
        .or_else(|| user.mail.clone())
        .unwrap_or_else(|| user.id.clone());

    let me = client.get_me().await?;

    // A chat with yourself is the Notes chat, not a 1:1 chat
    if me.id.eq_ignore_ascii_case(&user.id) {
        return Ok(Destination {
            chat_id: NOTES_CHAT_ID.to_string(),
            label: format!("your Notes ({})", name),
        });
    }

    let chat_id = find_one_on_one_chat(client, &user.id, &me.id).await?;
    Ok(Destination {
        chat_id,
        label: name,
    })
}

/// Find the existing 1:1 chat with a user, or create one.
///
/// A chat only matches when the single other member is that user. Matching on
/// "the user is a member" alone would match every 1:1 chat when the target is
/// yourself, and send to whichever chat the API listed first.
async fn find_one_on_one_chat(
    client: &TeamsClient,
    user_id: &str,
    my_user_id: &str,
) -> Result<String> {
    let details = client.get_user_details().await?;

    for chat in &details.chats {
        if chat.members.len() != 2 {
            continue;
        }
        let others: Vec<&str> = chat
            .members
            .iter()
            .filter_map(|m| m.object_id.as_deref())
            .filter(|id| !id.eq_ignore_ascii_case(my_user_id))
            .collect();

        if others.len() == 1 && others[0].eq_ignore_ascii_case(user_id) {
            return Ok(chat.id.clone());
        }
    }

    // No existing chat found, create a new one
    let new_chat = client.create_chat(vec![user_id], None).await?;
    Ok(new_chat.id)
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
