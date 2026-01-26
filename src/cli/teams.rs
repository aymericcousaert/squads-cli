use std::io::{self, Read};

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_error, print_output, print_single, print_success};
use super::utils::{html_escape, markdown_to_html, strip_html, truncate};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct TeamsCommand {
    #[command(subcommand)]
    pub command: TeamsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum TeamsSubcommand {
    /// List all teams
    List,

    /// Show team details
    Show {
        /// Team ID
        team_id: String,
    },

    /// List channels in a team
    Channels {
        /// Team ID
        team_id: String,
    },

    /// Get messages from a team channel
    Messages {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Maximum number of messages to retrieve
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Post a message to a team channel
    Post {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Message content
        message: Option<String>,

        /// Message subject (optional)
        #[arg(short, long)]
        subject: Option<String>,

        /// Read message from stdin
        #[arg(long)]
        stdin: bool,

        /// Treat message as Markdown and convert to HTML
        #[arg(short, long)]
        markdown: bool,
    },

    /// Reply to a message in a team channel
    Reply {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Message ID to reply to
        #[arg(long)]
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
    /// Delete a message from a team channel
    Delete {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Message ID to delete
        #[arg(long)]
        message_id: String,
    },

    /// React to a message in a team channel
    React {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Message ID to react to
        #[arg(long)]
        message_id: String,

        /// Reaction type (like, heart, laugh, surprised, sad, angry, skull, hourglass)
        reaction: String,

        /// Remove the reaction instead of adding it
        #[arg(long)]
        remove: bool,
    },

    /// List images shared in a team channel
    Images {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,

        /// Maximum number of messages to scan for images
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Download an image from a team channel
    DownloadImage {
        /// Image URL (from images list)
        image_url: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Debug: Show thread structure (for investigating reply issues)
    DebugThreads {
        /// Team ID
        team_id: String,

        /// Channel ID
        channel_id: String,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct TeamRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Channels")]
    channels: usize,
}

#[derive(Debug, Serialize, Tabled)]
struct ChannelRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
}

#[derive(Debug, Serialize, Tabled)]
struct MessageRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Subject")]
    subject: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Reactions")]
    reactions: String,
    #[tabled(rename = "Content")]
    content: String,
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
struct ImageJson {
    team_id: String,
    channel_id: String,
    message_id: String,
    image_url: String,
    from: String,
    time: String,
}

pub async fn execute(cmd: TeamsCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        TeamsSubcommand::List => list(config, format).await,
        TeamsSubcommand::Show { team_id } => show(config, &team_id, format).await,
        TeamsSubcommand::Channels { team_id } => channels(config, &team_id, format).await,
        TeamsSubcommand::Messages {
            team_id,
            channel_id,
            limit,
        } => messages(config, &team_id, &channel_id, limit, format).await,
        TeamsSubcommand::Post {
            team_id,
            channel_id,
            message,
            subject,
            stdin,
            markdown,
        } => {
            post(
                config,
                &team_id,
                &channel_id,
                message,
                subject,
                stdin,
                markdown,
            )
            .await
        }
        TeamsSubcommand::Reply {
            team_id,
            channel_id,
            message_id,
            content,
            markdown,
            html,
        } => {
            reply(
                config,
                &team_id,
                &channel_id,
                &message_id,
                &content,
                markdown,
                html,
            )
            .await
        }
        TeamsSubcommand::Delete {
            team_id,
            channel_id,
            message_id,
        } => delete(config, &team_id, &channel_id, &message_id).await,
        TeamsSubcommand::React {
            team_id,
            channel_id,
            message_id,
            reaction,
            remove,
        } => {
            react(
                config,
                &team_id,
                &channel_id,
                &message_id,
                &reaction,
                remove,
            )
            .await
        }
        TeamsSubcommand::Images {
            team_id,
            channel_id,
            limit,
        } => images(config, &team_id, &channel_id, limit, format).await,
        TeamsSubcommand::DownloadImage { image_url, output } => {
            download_image(config, &image_url, output).await
        }
        TeamsSubcommand::DebugThreads {
            team_id,
            channel_id,
        } => debug_threads(config, &team_id, &channel_id).await,
    }
}

async fn list(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    let rows: Vec<TeamRow> = details
        .teams
        .into_iter()
        .map(|team| TeamRow {
            id: team.id,
            name: team.display_name,
            channels: team.channels.len(),
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

async fn show(config: &Config, team_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    if let Some(team) = details.teams.into_iter().find(|t| t.id == team_id) {
        print_single(&team, format);
    } else {
        print_error(&format!("Team not found: {}", team_id));
    }

    Ok(())
}

async fn channels(config: &Config, team_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let details = client.get_user_details().await?;

    if let Some(team) = details.teams.into_iter().find(|t| t.id == team_id) {
        let rows: Vec<ChannelRow> = team
            .channels
            .into_iter()
            .map(|channel| ChannelRow {
                id: channel.id,
                name: channel.display_name,
            })
            .collect();

        print_output(&rows, format);
    } else {
        print_error(&format!("Team not found: {}", team_id));
    }

    Ok(())
}

async fn messages(
    config: &Config,
    team_id: &str,
    channel_id: &str,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let conversations = client.get_team_conversations(team_id, channel_id).await?;

    let mut rows: Vec<MessageRow> = Vec::new();

    for chain in conversations.reply_chains {
        for msg in chain.messages {
            if msg.message_type.as_deref() == Some("RichText/Html")
                || msg.message_type.as_deref() == Some("Text")
            {
                let content = msg.content.map(|c| strip_html(&c)).unwrap_or_default();

                let subject = msg
                    .properties
                    .as_ref()
                    .and_then(|p| p.subject.clone())
                    .unwrap_or_default();

                let reactions = format_reactions_summary(&msg.properties);

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

                rows.push(MessageRow {
                    id: msg.id.unwrap_or_default(),
                    from: msg
                        .im_display_name
                        .unwrap_or_else(|| msg.from.unwrap_or_else(|| "Unknown".to_string())),
                    status: status_str,
                    subject: truncate(&subject, 20),
                    time: msg.original_arrival_time.unwrap_or_default(),
                    reactions,
                    content: match format {
                        OutputFormat::Json => content.clone(),
                        _ => truncate(&content, 40),
                    },
                });

                if rows.len() >= limit {
                    break;
                }
            }
        }
        if rows.len() >= limit {
            break;
        }
    }

    print_output(&rows, format);
    Ok(())
}

/// Format reactions as a summary string (e.g., "👍2 ❤️1")
fn format_reactions_summary(props: &Option<crate::types::MessageProperties>) -> String {
    if let Some(properties) = props {
        if let Some(emotions) = &properties.emotions {
            let parts: Vec<String> = emotions
                .iter()
                .map(|e| {
                    let count = e.users.len();
                    if count > 1 {
                        format!("{}{}", e.key, count)
                    } else {
                        e.key.clone()
                    }
                })
                .collect();
            return parts.join(" ");
        }
    }
    String::new()
}

async fn post(
    config: &Config,
    team_id: &str,
    channel_id: &str,
    message: Option<String>,
    subject: Option<String>,
    stdin: bool,
    markdown: bool,
) -> Result<()> {
    let content = if let Some(msg) = message {
        msg
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else {
        print_error("No message provided. Use --stdin or provide message as argument.");
        return Ok(());
    };

    if content.is_empty() {
        print_error("Message cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    let html_body = if markdown {
        markdown_to_html(&content)
    } else {
        format!("<p>{}</p>", html_escape(&content))
    };

    let result = client
        .send_channel_message(team_id, channel_id, &html_body, subject.as_deref())
        .await?;

    if let Some(id) = result.get("id").and_then(|v| v.as_str()) {
        print_success(&format!("Message posted (ID: {})", id));
    } else {
        print_success("Message posted to channel");
    }

    Ok(())
}

async fn reply(
    config: &Config,
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    content: &str,
    markdown: bool,
    html: bool,
) -> Result<()> {
    if content.is_empty() {
        print_error("Reply content cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    let html_body = if html {
        content.to_string()
    } else if markdown {
        markdown_to_html(content)
    } else {
        format!("<p>{}</p>", html_escape(content))
    };

    let result = client
        .reply_channel_message(team_id, channel_id, message_id, &html_body)
        .await?;

    if let Some(id) = result.get("id").and_then(|v| v.as_str()) {
        print_success(&format!("Reply posted (ID: {})", id));
    } else {
        print_success("Reply posted");
    }

    Ok(())
}

async fn delete(config: &Config, team_id: &str, channel_id: &str, message_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client
        .delete_channel_message(team_id, channel_id, message_id)
        .await?;
    print_success("Message deleted");
    Ok(())
}

async fn react(
    config: &Config,
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    reaction: &str,
    remove: bool,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client
        .send_team_reaction(team_id, channel_id, message_id, reaction, remove)
        .await?;
    if remove {
        print_success("Reaction removed");
    } else {
        print_success("Reaction added");
    }
    Ok(())
}

async fn images(
    config: &Config,
    team_id: &str,
    channel_id: &str,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let conversations = client.get_team_conversations(team_id, channel_id).await?;

    let mut all_images: Vec<ImageJson> = Vec::new();
    let mut count = 0;

    for chain in &conversations.reply_chains {
        for msg in &chain.messages {
            if count >= limit {
                break;
            }

            if msg.message_type.as_deref() != Some("RichText/Html")
                && msg.message_type.as_deref() != Some("Text")
            {
                continue;
            }

            let content = msg.content.as_deref().unwrap_or("");
            let img_urls = extract_image_urls(content);

            for url in img_urls {
                all_images.push(ImageJson {
                    team_id: team_id.to_string(),
                    channel_id: channel_id.to_string(),
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

            count += 1;
        }
        if count >= limit {
            break;
        }
    }

    if all_images.is_empty() {
        println!("No images found in this channel.");
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

    let mut remaining = content;
    while let Some(img_start) = remaining.find("<img") {
        remaining = &remaining[img_start..];

        if let Some(src_start) = remaining.find("src=\"") {
            let src_content = &remaining[src_start + 5..];
            if let Some(src_end) = src_content.find('"') {
                let url = &src_content[..src_end];
                if url.contains("ams")
                    || url.contains("teams.microsoft.com")
                    || url.contains("blob")
                    || url.starts_with("http")
                {
                    let decoded_url = url
                        .replace("&amp;", "&")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">");
                    urls.push(decoded_url);
                }
            }
        }

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

async fn debug_threads(config: &Config, team_id: &str, channel_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let conversations = client.get_team_conversations(team_id, channel_id).await?;

    println!("\n=== Thread Structure Debug ===");
    println!(
        "Total threads (reply_chains): {}\n",
        conversations.reply_chains.len()
    );

    for (i, chain) in conversations.reply_chains.iter().enumerate() {
        println!("--- Thread {} ---", i);
        println!("  Chain ID: {}", chain.id);
        println!("  Container ID: {}", chain.container_id);
        println!("  Message count: {}", chain.messages.len());

        for (j, msg) in chain.messages.iter().enumerate() {
            let content_preview = msg
                .content
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect::<String>()
                .replace('\n', " ");

            let mut status = Vec::new();
            if let Some(props) = &msg.properties {
                if props.deletetime > 0 {
                    status.push(format!("DELETED({})", props.deletetime));
                }
                if props.systemdelete {
                    status.push("SYSTEM_DELETE".to_string());
                }
            }
            let status_str = if status.is_empty() {
                String::new()
            } else {
                format!(" [{}]", status.join(", "))
            };

            println!("    [{}] ID: {:?}{}", j, msg.id, status_str);
            println!("        From: {:?}", msg.im_display_name);
            println!("        Content: {}...", content_preview);
        }
        println!();
    }

    Ok(())
}
