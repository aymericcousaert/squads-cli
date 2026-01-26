use anyhow::Result;
use clap::{Args, ValueEnum};
use colored::Colorize;
use std::collections::HashSet;
use std::time::Duration;

use crate::api::TeamsClient;
use crate::cli::utils::{strip_html, truncate};
use crate::config::Config;

#[derive(Args, Debug)]
pub struct WatchCommand {
    /// What to watch
    #[arg(short, long, value_enum, default_value = "all")]
    pub source: WatchSource,

    /// Poll interval in seconds
    #[arg(short, long, default_value = "10")]
    pub interval: u64,

    /// Enable desktop notifications
    #[arg(short, long)]
    pub notify: bool,

    /// Only show notifications, don't print to terminal
    #[arg(long)]
    pub quiet: bool,

    /// Specific chat ID to watch (can be repeated)
    #[arg(long)]
    pub chat: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum WatchSource {
    /// Watch both Teams messages and emails
    #[default]
    All,
    /// Watch only Teams messages
    Chats,
    /// Watch only emails
    Mail,
}

pub async fn execute(cmd: WatchCommand, config: &Config) -> Result<()> {
    let client = TeamsClient::new(config)?;

    // Get current user profile to avoid notifying on own messages
    let me = client.get_me().await.ok();
    let my_id = me.as_ref().map(|p| format!("8:orgid:{}", p.id));

    println!("{}", "Starting watch mode...".cyan().bold());
    println!(
        "Polling every {} seconds. Press Ctrl+C to stop.",
        cmd.interval
    );
    if cmd.notify {
        println!("Desktop notifications: {}", "enabled".green());
    }
    println!();

    // Track seen message/email IDs to avoid duplicates
    let mut seen_messages: HashSet<String> = HashSet::new();
    let mut seen_emails: HashSet<String> = HashSet::new();

    // Initial load to populate seen items
    if matches!(cmd.source, WatchSource::All | WatchSource::Chats) {
        if let Ok(details) = client.get_user_details().await {
            for chat in &details.chats {
                if !cmd.chat.is_empty() && !cmd.chat.contains(&chat.id) {
                    continue;
                }
                if let Ok(convs) = client.get_conversations(&chat.id, None).await {
                    for msg in convs.messages {
                        if let Some(id) = &msg.id {
                            seen_messages.insert(id.clone());
                        }
                    }
                }
            }
        }
    }

    if matches!(cmd.source, WatchSource::All | WatchSource::Mail) {
        if let Ok(emails) = client.get_mail_messages(Some("inbox"), 50).await {
            for email in emails.value {
                if let Some(id) = &email.id {
                    seen_emails.insert(id.clone());
                }
            }
        }
    }

    if !cmd.quiet {
        println!(
            "{}",
            "Initial sync complete. Watching for new items...".dimmed()
        );
        println!();
    }

    // Main watch loop
    loop {
        tokio::time::sleep(Duration::from_secs(cmd.interval)).await;

        // Check for new chat messages
        if matches!(cmd.source, WatchSource::All | WatchSource::Chats) {
            check_new_messages(&client, &mut seen_messages, &cmd, my_id.as_deref()).await;
        }

        // Check for new emails
        if matches!(cmd.source, WatchSource::All | WatchSource::Mail) {
            check_new_emails(&client, &mut seen_emails, &cmd).await;
        }
    }
}

async fn check_new_messages(
    client: &TeamsClient,
    seen: &mut HashSet<String>,
    cmd: &WatchCommand,
    my_id: Option<&str>,
) {
    let details = match client.get_user_details().await {
        Ok(d) => d,
        Err(_) => return,
    };

    for chat in &details.chats {
        // Filter by specific chat if specified
        if !cmd.chat.is_empty() && !cmd.chat.contains(&chat.id) {
            continue;
        }

        let convs = match client.get_conversations(&chat.id, None).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        for msg in convs.messages {
            let msg_id = match &msg.id {
                Some(id) => id.clone(),
                None => continue,
            };

            if seen.contains(&msg_id) {
                continue;
            }

            seen.insert(msg_id);

            // Skip messages from self
            if let Some(my_id) = my_id {
                if msg.from.as_deref() == Some(my_id) {
                    continue;
                }
            }

            // Skip non-user messages
            if msg.message_type.as_deref() != Some("RichText/Html")
                && msg.message_type.as_deref() != Some("Text")
            {
                continue;
            }

            let sender = msg
                .im_display_name
                .or(msg.from.clone())
                .unwrap_or_else(|| "Unknown".to_string());

            let content = msg.content.map(|c| strip_html(&c)).unwrap_or_default();

            let chat_name = chat
                .title
                .clone()
                .unwrap_or_else(|| "Direct Chat".to_string());

            let time = chrono::Local::now().format("%H:%M:%S").to_string();

            if !cmd.quiet {
                println!(
                    "{} 💬 {} {}",
                    format!("[{}]", time).dimmed(),
                    format!("{}:", sender).cyan().bold(),
                    truncate(&content, 80)
                );
                println!("   {}", format!("in {}", chat_name).dimmed());
            }

            if cmd.notify {
                send_notification(
                    &format!("Teams: {}", sender),
                    &truncate(&content, 100),
                    "teams",
                );
            }
        }
    }
}

async fn check_new_emails(client: &TeamsClient, seen: &mut HashSet<String>, cmd: &WatchCommand) {
    let emails = match client.get_mail_messages(Some("inbox"), 20).await {
        Ok(e) => e,
        Err(_) => return,
    };

    for email in emails.value {
        let email_id = match &email.id {
            Some(id) => id.clone(),
            None => continue,
        };

        if seen.contains(&email_id) {
            continue;
        }

        seen.insert(email_id);

        // Only notify for unread emails
        if email.is_read == Some(true) {
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

        let time = chrono::Local::now().format("%H:%M:%S").to_string();

        if !cmd.quiet {
            println!(
                "{} 📧 {} {}",
                format!("[{}]", time).dimmed(),
                format!("{}:", sender).yellow().bold(),
                truncate(&subject, 60)
            );
        }

        if cmd.notify {
            send_notification(
                &format!("Email: {}", sender),
                &truncate(&subject, 100),
                "mail",
            );
        }
    }
}

fn send_notification(title: &str, body: &str, _category: &str) {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .appname("squads-cli")
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .appname("squads-cli")
            .show();
    }
}
