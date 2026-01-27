use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use std::io::{self, Read};
use tabled::Tabled;

use super::output::{print_error, print_output, print_success};
use super::utils::{html_escape, markdown_to_html, strip_html, truncate};
use super::OutputFormat;
use crate::api::TeamsClient;
use crate::config::Config;

const NOTES_CHAT_ID: &str = "48:notes";

#[derive(Args, Debug)]
pub struct NotesCommand {
    #[command(subcommand)]
    pub command: NotesSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum NotesSubcommand {
    /// List recent notes
    List {
        /// Maximum number of notes to retrieve
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Add a new note
    Add {
        /// Note content
        message: Option<String>,

        /// Read message from stdin
        #[arg(short, long)]
        stdin: bool,

        /// Use markdown to HTML conversion
        #[arg(short, long)]
        markdown: bool,
    },
    /// Delete a note
    Delete {
        /// Message ID to delete
        message_id: String,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct NoteRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Content")]
    content: String,
}

pub async fn execute(cmd: NotesCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        NotesSubcommand::List { limit } => list(config, limit, format).await,
        NotesSubcommand::Add {
            message,
            stdin,
            markdown,
        } => add(config, message, stdin, markdown).await,
        NotesSubcommand::Delete { message_id } => delete(config, &message_id).await,
    }
}

async fn list(config: &Config, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let convs = client.get_conversations(NOTES_CHAT_ID, None).await?;

    let rows: Vec<NoteRow> = convs
        .messages
        .into_iter()
        .filter(|msg| {
            msg.message_type.as_deref() == Some("RichText/Html")
                || msg.message_type.as_deref() == Some("Text")
        })
        .take(limit)
        .map(|msg| {
            let content = msg.content.map(|c| strip_html(&c)).unwrap_or_default();
            NoteRow {
                id: msg.id.unwrap_or_default(),
                time: msg.original_arrival_time.unwrap_or_default(),
                content: match format {
                    OutputFormat::Json => content.clone(),
                    _ => truncate(&content, 80),
                },
            }
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

async fn add(config: &Config, message: Option<String>, stdin: bool, markdown: bool) -> Result<()> {
    let content = if let Some(msg) = message {
        msg
    } else if stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer.trim().to_string()
    } else {
        print_error("No note content provided. Use --stdin or provide message as argument.");
        return Ok(());
    };

    if content.is_empty() {
        print_error("Note cannot be empty");
        return Ok(());
    }

    let client = TeamsClient::new(config)?;
    let html_body = if markdown {
        markdown_to_html(&content)
    } else {
        format!("<p>{}</p>", html_escape(&content))
    };

    client.send_message(NOTES_CHAT_ID, &html_body, None).await?;
    print_success("Note added successfully");

    Ok(())
}

async fn delete(config: &Config, message_id: &str) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.delete_message(NOTES_CHAT_ID, message_id).await?;
    print_success("Note deleted");
    Ok(())
}
