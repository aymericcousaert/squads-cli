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
        #[arg(short, long)]
        message_id: String,

        /// Reply content
        content: String,

        /// Treat content as Markdown and convert to HTML
        #[arg(short, long)]
        markdown: bool,
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
    #[tabled(rename = "Subject")]
    subject: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Content")]
    content: String,
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
        } => {
            reply(
                config,
                &team_id,
                &channel_id,
                &message_id,
                &content,
                markdown,
            )
            .await
        }
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

                rows.push(MessageRow {
                    id: msg.id.unwrap_or_default(),
                    from: msg
                        .im_display_name
                        .unwrap_or_else(|| msg.from.unwrap_or_else(|| "Unknown".to_string())),
                    subject: truncate(&subject, 20),
                    time: msg.original_arrival_time.unwrap_or_default(),
                    content: truncate(&content, 40),
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
        markdown::to_html(&content)
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
) -> Result<()> {
    if content.is_empty() {
        print_error("Reply content cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    let html_body = if markdown {
        markdown::to_html(content)
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
