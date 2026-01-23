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
pub struct MailCommand {
    #[command(subcommand)]
    pub command: MailSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum MailSubcommand {
    /// List mail folders
    Folders,

    /// List mail messages
    List {
        /// Folder to list (inbox, sentitems, drafts, etc.)
        #[arg(long)]
        folder: Option<String>,

        /// Maximum number of messages
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Read a specific email
    Read {
        /// Message ID
        message_id: String,
    },

    /// Send an email
    Send {
        /// Recipient email address(es), comma-separated
        #[arg(short, long)]
        to: String,

        /// Email subject
        #[arg(short, long)]
        subject: String,

        /// Email body (omit to read from stdin)
        body: Option<String>,

        /// CC recipients, comma-separated
        #[arg(short, long)]
        cc: Option<String>,

        /// Read body from stdin
        #[arg(long)]
        stdin: bool,

        /// Read body from file
        #[arg(long)]
        file: Option<String>,
    },

    /// Search emails
    Search {
        /// Search query
        query: String,

        /// Maximum number of results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Create a draft email
    Draft {
        /// Recipient email address(es), comma-separated
        #[arg(short, long)]
        to: String,

        /// Email subject
        #[arg(short, long)]
        subject: String,

        /// Email body (omit to read from stdin)
        body: Option<String>,

        /// CC recipients, comma-separated
        #[arg(short, long)]
        cc: Option<String>,

        /// Read body from stdin
        #[arg(long)]
        stdin: bool,

        /// Read body from file
        #[arg(long)]
        file: Option<String>,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct FolderRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Unread")]
    unread: i32,
    #[tabled(rename = "Total")]
    total: i32,
}

#[derive(Debug, Serialize, Tabled)]
struct MailRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Subject")]
    subject: String,
    #[tabled(rename = "Date")]
    date: String,
    #[tabled(rename = "Read")]
    is_read: String,
}

pub async fn execute(cmd: MailCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        MailSubcommand::Folders => folders(config, format).await,
        MailSubcommand::List { folder, limit } => list(config, folder, limit, format).await,
        MailSubcommand::Read { message_id } => read(config, &message_id, format).await,
        MailSubcommand::Send {
            to,
            subject,
            body,
            cc,
            stdin,
            file,
        } => send(config, &to, &subject, body, cc, stdin, file).await,
        MailSubcommand::Search { query, limit } => search(config, &query, limit, format).await,
        MailSubcommand::Draft {
            to,
            subject,
            body,
            cc,
            stdin,
            file,
        } => draft(config, &to, &subject, body, cc, stdin, file, format).await,
    }
}

async fn folders(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let folders = client.get_mail_folders().await?;

    let rows: Vec<FolderRow> = folders
        .value
        .into_iter()
        .map(|f| FolderRow {
            id: f.id,
            name: f.display_name,
            unread: f.unread_item_count.unwrap_or(0),
            total: f.total_item_count.unwrap_or(0),
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

async fn list(config: &Config, folder: Option<String>, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let messages = client.get_mail_messages(folder.as_deref(), limit).await?;

    match format {
        OutputFormat::Json => {
            // For JSON, return full message data (useful for AI agents)
            print_single(&messages.value, format);
        }
        _ => {
            // For table/plain, show truncated data
            let rows: Vec<MailRow> = messages
                .value
                .into_iter()
                .map(|m| {
                    let from = m
                        .from
                        .map(|r| {
                            r.email_address
                                .name
                                .unwrap_or(r.email_address.address)
                        })
                        .unwrap_or_else(|| "Unknown".to_string());

                    MailRow {
                        id: truncate(&m.id.unwrap_or_default(), 12),
                        from: truncate(&from, 25),
                        subject: truncate(&m.subject.unwrap_or_default(), 40),
                        date: m
                            .received_date_time
                            .map(|d| truncate(&d, 19))
                            .unwrap_or_default(),
                        is_read: if m.is_read == Some(true) { "Yes" } else { "No" }.to_string(),
                    }
                })
                .collect();

            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn read(config: &Config, message_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let message = client.get_mail_message(message_id).await?;

    match format {
        OutputFormat::Json => {
            print_single(&message, format);
        }
        _ => {
            // Pretty print for table/plain
            let from = message
                .from
                .map(|r| {
                    let name = r.email_address.name.unwrap_or_default();
                    let addr = r.email_address.address;
                    if name.is_empty() {
                        addr
                    } else {
                        format!("{} <{}>", name, addr)
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let to = message
                .to_recipients
                .map(|recipients| {
                    recipients
                        .iter()
                        .map(|r| r.email_address.address.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();

            println!("From: {}", from);
            println!("To: {}", to);
            println!("Subject: {}", message.subject.unwrap_or_default());
            println!("Date: {}", message.received_date_time.unwrap_or_default());
            println!("---");

            if let Some(body) = message.body {
                if body.content_type == "text" {
                    println!("{}", body.content);
                } else {
                    // Strip HTML for display
                    println!("{}", strip_html(&body.content));
                }
            } else if let Some(preview) = message.body_preview {
                println!("{}", preview);
            }
        }
    }

    Ok(())
}

async fn send(
    config: &Config,
    to: &str,
    subject: &str,
    body: Option<String>,
    cc: Option<String>,
    stdin: bool,
    file: Option<String>,
) -> Result<()> {
    // Get the body content
    let content = if let Some(b) = body {
        b
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else {
        print_error("No body provided. Use --stdin or --file, or provide body as argument.");
        return Ok(());
    };

    if content.is_empty() {
        print_error("Email body cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    // Parse recipients
    let to_list: Vec<&str> = to.split(',').map(|s| s.trim()).collect();
    let cc_list: Option<Vec<String>> = cc.as_ref().map(|c| c.split(',').map(|s| s.trim().to_string()).collect());
    let cc_refs: Option<Vec<&str>> = cc_list.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());

    client.send_mail(to_list, subject, &content, cc_refs).await?;
    print_success("Email sent successfully");

    Ok(())
}

async fn search(config: &Config, query: &str, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let messages = client.search_mail(query, limit).await?;

    let rows: Vec<MailRow> = messages
        .value
        .into_iter()
        .map(|m| {
            let from = m
                .from
                .map(|r| {
                    r.email_address
                        .name
                        .unwrap_or(r.email_address.address)
                })
                .unwrap_or_else(|| "Unknown".to_string());

            MailRow {
                id: truncate(&m.id.unwrap_or_default(), 12),
                from: truncate(&from, 25),
                subject: truncate(&m.subject.unwrap_or_default(), 40),
                date: m
                    .received_date_time
                    .map(|d| truncate(&d, 19))
                    .unwrap_or_default(),
                is_read: if m.is_read == Some(true) { "Yes" } else { "No" }.to_string(),
            }
        })
        .collect();

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

async fn draft(
    config: &Config,
    to: &str,
    subject: &str,
    body: Option<String>,
    cc: Option<String>,
    stdin: bool,
    file: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    // Get the body content
    let content = if let Some(b) = body {
        b
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else {
        print_error("No body provided. Use --stdin or --file, or provide body as argument.");
        return Ok(());
    };

    if content.is_empty() {
        print_error("Email body cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;

    // Parse recipients
    let to_list: Vec<&str> = to.split(',').map(|s| s.trim()).collect();
    let cc_list: Option<Vec<String>> = cc.as_ref().map(|c| c.split(',').map(|s| s.trim().to_string()).collect());
    let cc_refs: Option<Vec<&str>> = cc_list.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());

    let draft = client.create_draft(to_list, subject, &content, cc_refs).await?;

    match format {
        OutputFormat::Json => {
            print_single(&draft, format);
        }
        _ => {
            print_success(&format!("Draft created with ID: {}", draft.id.unwrap_or_default()));
            println!("To: {}", to);
            println!("Subject: {}", subject);
            if let Some(link) = draft.web_link {
                println!("Open in Outlook: {}", link);
            }
        }
    }

    Ok(())
}
