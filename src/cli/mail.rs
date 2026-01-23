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

    /// Reply to an email
    Reply {
        /// Message ID to reply to
        message_id: String,

        /// Reply body
        body: String,

        /// Reply to all recipients
        #[arg(long)]
        all: bool,
    },

    /// Forward an email
    Forward {
        /// Message ID to forward
        message_id: String,

        /// Recipient email address(es), comma-separated
        #[arg(short, long)]
        to: String,

        /// Optional comment to include
        #[arg(short, long)]
        comment: Option<String>,
    },

    /// Delete an email
    Delete {
        /// Message ID to delete
        message_id: String,
    },

    /// Move an email to a folder
    Move {
        /// Message ID to move
        message_id: String,

        /// Destination folder (ID or well-known name: archive, deleteditems, drafts, inbox, junkemail, sentitems)
        #[arg(short, long)]
        to: String,
    },

    /// Mark email as read or unread
    Mark {
        /// Message ID
        message_id: String,

        /// Mark as read
        #[arg(long, conflicts_with = "unread")]
        read: bool,

        /// Mark as unread
        #[arg(long, conflicts_with = "read")]
        unread: bool,
    },

    /// List attachments of an email
    Attachments {
        /// Message ID
        message_id: String,
    },

    /// Download an attachment
    Download {
        /// Message ID
        message_id: String,

        /// Attachment ID
        attachment_id: String,

        /// Output path (default: current directory with original filename)
        #[arg(short, long)]
        output: Option<String>,
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
        MailSubcommand::Reply {
            message_id,
            body,
            all,
        } => reply(config, &message_id, &body, all).await,
        MailSubcommand::Forward {
            message_id,
            to,
            comment,
        } => forward(config, &message_id, &to, comment).await,
        MailSubcommand::Delete { message_id } => delete(config, &message_id).await,
        MailSubcommand::Move { message_id, to } => move_mail(config, &message_id, &to).await,
        MailSubcommand::Mark {
            message_id,
            read,
            unread,
        } => mark(config, &message_id, read, unread).await,
        MailSubcommand::Attachments { message_id } => {
            attachments(config, &message_id, format).await
        }
        MailSubcommand::Download {
            message_id,
            attachment_id,
            output,
        } => download(config, &message_id, &attachment_id, output).await,
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

async fn list(
    config: &Config,
    folder: Option<String>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
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
                        .map(|r| r.email_address.name.unwrap_or(r.email_address.address))
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
    let cc_list: Option<Vec<String>> = cc
        .as_ref()
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect());
    let cc_refs: Option<Vec<&str>> = cc_list
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect());

    client
        .send_mail(to_list, subject, &content, cc_refs)
        .await?;
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
                .map(|r| r.email_address.name.unwrap_or(r.email_address.address))
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

#[allow(clippy::too_many_arguments)]
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
    let cc_list: Option<Vec<String>> = cc
        .as_ref()
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect());
    let cc_refs: Option<Vec<&str>> = cc_list
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect());

    let draft = client
        .create_draft(to_list, subject, &content, cc_refs)
        .await?;

    match format {
        OutputFormat::Json => {
            print_single(&draft, format);
        }
        _ => {
            print_success(&format!(
                "Draft created with ID: {}",
                draft.id.unwrap_or_default()
            ));
            println!("To: {}", to);
            println!("Subject: {}", subject);
            if let Some(link) = draft.web_link {
                println!("Open in Outlook: {}", link);
            }
        }
    }

    Ok(())
}

async fn reply(config: &Config, message_id: &str, body: &str, reply_all: bool) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.reply_mail(message_id, body, reply_all).await?;

    if reply_all {
        print_success("Reply sent to all recipients");
    } else {
        print_success("Reply sent");
    }
    Ok(())
}

async fn forward(
    config: &Config,
    message_id: &str,
    to: &str,
    comment: Option<String>,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let to_list: Vec<&str> = to.split(',').map(|s| s.trim()).collect();
    client
        .forward_mail(message_id, to_list, comment.as_deref())
        .await?;
    print_success(&format!("Email forwarded to {}", to));
    Ok(())
}

async fn delete(config: &Config, message_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.delete_mail(message_id).await?;
    print_success("Email deleted");
    Ok(())
}

async fn move_mail(config: &Config, message_id: &str, folder: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;

    // Map well-known folder names to their IDs
    let folder_id = match folder.to_lowercase().as_str() {
        "archive" => "archive",
        "deleteditems" | "deleted" | "trash" => "deleteditems",
        "drafts" => "drafts",
        "inbox" => "inbox",
        "junkemail" | "junk" | "spam" => "junkemail",
        "sentitems" | "sent" => "sentitems",
        _ => folder, // Assume it's a folder ID
    };

    client.move_mail(message_id, folder_id).await?;
    print_success(&format!("Email moved to {}", folder));
    Ok(())
}

async fn mark(config: &Config, message_id: &str, read: bool, unread: bool) -> Result<()> {
    if !read && !unread {
        print_error("Please specify --read or --unread");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;
    let is_read = read; // If --read is set, mark as read; if --unread is set, read=false
    client.mark_mail(message_id, is_read).await?;

    if is_read {
        print_success("Email marked as read");
    } else {
        print_success("Email marked as unread");
    }
    Ok(())
}

#[derive(Debug, Serialize, Tabled)]
struct AttachmentRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    content_type: String,
    #[tabled(rename = "Size")]
    size: String,
}

async fn attachments(config: &Config, message_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let attachments = client.get_mail_attachments(message_id).await?;

    match format {
        OutputFormat::Json => {
            print_single(&attachments.value, format);
        }
        _ => {
            if attachments.value.is_empty() {
                println!("No attachments");
                return Ok(());
            }

            let rows: Vec<AttachmentRow> = attachments
                .value
                .into_iter()
                .map(|a| AttachmentRow {
                    id: truncate(&a.id.unwrap_or_default(), 20),
                    name: a.name,
                    content_type: a.content_type.unwrap_or_default(),
                    size: format_size(a.size.unwrap_or(0)),
                })
                .collect();

            print_output(&rows, format);
        }
    }
    Ok(())
}

fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;

    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

async fn download(
    config: &Config,
    message_id: &str,
    attachment_id: &str,
    output: Option<String>,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let (filename, content) = client
        .download_attachment(message_id, attachment_id)
        .await?;

    let output_path = output.unwrap_or_else(|| filename.clone());
    std::fs::write(&output_path, content)?;

    print_success(&format!(
        "Downloaded {} ({} bytes)",
        output_path,
        std::fs::metadata(&output_path)?.len()
    ));
    Ok(())
}
