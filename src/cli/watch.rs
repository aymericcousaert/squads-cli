use anyhow::Result;
use clap::{Args, ValueEnum};
use colored::Colorize;
use std::collections::HashSet;
use std::time::Duration;

use crate::api::{SessionEnd, TeamsClient, TrouterEvent};
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

    /// Use real-time push (Trouter websocket) instead of polling. Chats only.
    #[arg(short, long)]
    pub push: bool,

    /// Emit each new message as a JSON line (for scripts / agents). Implies --push.
    #[arg(long)]
    pub json: bool,
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

    // --json is a push-only output mode; imply push so it never silently no-ops.
    if cmd.push || cmd.json {
        return watch_push(&client, &cmd).await;
    }

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

/// Real-time push watch via the Trouter websocket, with auto-reconnect.
async fn watch_push(client: &TeamsClient, cmd: &WatchCommand) -> Result<()> {
    use std::io::Write;

    let me = client.get_me().await.ok();
    let my_mri = me.as_ref().map(|p| format!("8:orgid:{}", p.id));

    if matches!(cmd.source, WatchSource::Mail) {
        eprintln!("note: --push is chats-only; --source mail is ignored");
    }

    // Dedup across the session: Trouter redelivers un-acked events and replays recent
    // messages on every reconnect, so without this the same message fires twice.
    let mut seen: HashSet<String> = HashSet::new();

    if !cmd.json {
        println!(
            "{}",
            "Starting push watch (Trouter real-time)...".cyan().bold()
        );
        if cmd.notify {
            println!("Desktop notifications: {}", "enabled".green());
        }
        println!("Connected. Waiting for messages. Press Ctrl+C to stop.");
        println!();
    }

    let debug = std::env::var("SQUADS_TROUTER_DEBUG").is_ok();
    let mut backoff = BACKOFF_MIN;
    loop {
        let res = client
            .trouter_listen(|ev: TrouterEvent| {
                // Other event kinds are parsed but not surfaced yet: the --json output is
                // a stable contract for other tools.
                let m = match ev {
                    TrouterEvent::NewMessage(m) => m,
                    other => {
                        if debug {
                            eprintln!("[watch] ignored event: {other:?}");
                        }
                        return;
                    }
                };
                // Each skip says why, so a message that never printed can be told
                // apart from one that never arrived.
                let skip = |reason: &str| {
                    if debug {
                        eprintln!("[watch] skipped {}: {reason}", m.message_id);
                    }
                };
                // skip our own messages
                if let Some(mri) = &my_mri {
                    if &m.from_mri == mri {
                        skip("own message");
                        return;
                    }
                }
                // optional chat filter
                if !cmd.chat.is_empty() && !cmd.chat.contains(&m.chat_id) {
                    skip("chat filtered out");
                    return;
                }
                // skip messages we've already delivered (reconnect replay / redelivery)
                if !m.message_id.is_empty() && !seen.insert(m.message_id.clone()) {
                    skip("already delivered");
                    return;
                }
                if seen.len() > 10_000 {
                    seen.clear();
                }
                let content = strip_html(&m.content);
                if content.trim().is_empty() {
                    skip("empty after html strip");
                    return;
                }

                if cmd.json {
                    let obj = serde_json::json!({
                        "chat_id": m.chat_id,
                        "message_id": m.message_id,
                        "from": m.from,
                        "from_mri": m.from_mri,
                        "time": chrono::Utc::now().to_rfc3339(),
                        "content": content,
                        "source": "push",
                    });
                    println!("{}", serde_json::to_string(&obj).unwrap_or_default());
                    let _ = std::io::stdout().flush();
                } else if !cmd.quiet {
                    let time = chrono::Local::now().format("%H:%M:%S").to_string();
                    println!(
                        "{} 💬 {} {}",
                        format!("[{}]", time).dimmed(),
                        format!("{}:", m.from).cyan().bold(),
                        truncate(&content, 80)
                    );
                }

                if cmd.notify {
                    send_notification(
                        &format!("Teams: {}", m.from),
                        &truncate(&content, 100),
                        "teams",
                    );
                }
            })
            .await;

        let end = match res {
            Ok(end) => PushEnd::Session(end),
            Err(e) => {
                eprintln!("push connection error: {e}");
                if is_transport_error(&e) {
                    PushEnd::Transport
                } else {
                    PushEnd::Failed
                }
            }
        };
        let (wait, next) = next_backoff(backoff, end);
        backoff = next;
        if debug {
            eprintln!("[watch] push session ended ({end:?}), reconnecting in {wait}s");
        }
        if wait > 0 {
            tokio::time::sleep(Duration::from_secs(wait)).await;
        }
    }
}

/// First pause between push reconnects, in seconds.
const BACKOFF_MIN: u64 = 1;

/// Longest pause between push reconnects, in seconds.
const BACKOFF_MAX: u64 = 60;

/// A transport failure usually means the network is gone, so give it time.
const BACKOFF_TRANSPORT_FLOOR: u64 = 5;

/// Why a push session came back, as far as the retry loop cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushEnd {
    /// The session ran and ended. How far it got decides the pause.
    Session(SessionEnd),
    /// We could not reach the service at all.
    Transport,
    /// The service or a token refused us.
    Failed,
}

/// Seconds to wait before the next connect, and the backoff to carry forward.
fn next_backoff(backoff: u64, end: PushEnd) -> (u64, u64) {
    let wait = match end {
        // We asked for this close, so there is nothing to wait for.
        PushEnd::Session(SessionEnd::Clean) => return (0, BACKOFF_MIN),
        // The service was talking to us a moment ago. How long the last outage
        // lasted says nothing about this one.
        PushEnd::Session(SessionEnd::Live) => BACKOFF_MIN,
        PushEnd::Session(SessionEnd::NeverLive) | PushEnd::Failed => backoff,
        PushEnd::Transport => backoff.max(BACKOFF_TRANSPORT_FLOOR),
    };
    (wait, (wait * 2).min(BACKOFF_MAX))
}

/// True when we never reached the service: no DNS, no route, no TLS. A protocol
/// or auth error is the service answering, and deserves the usual backoff.
fn is_transport_error(err: &anyhow::Error) -> bool {
    use tokio_tungstenite::tungstenite::error::UrlError;
    use tokio_tungstenite::tungstenite::Error as WsError;
    if let Some(e) = err.downcast_ref::<WsError>() {
        return matches!(
            e,
            WsError::Io(_) | WsError::Tls(_) | WsError::Url(UrlError::UnableToConnect(_))
        );
    }
    if let Some(e) = err.downcast_ref::<reqwest::Error>() {
        return e.is_connect() || e.is_timeout();
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_io_error() -> anyhow::Error {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::other("no such host")).into()
    }

    #[test]
    fn a_clean_return_reconnects_at_once() {
        assert_eq!(
            next_backoff(32, PushEnd::Session(SessionEnd::Clean)),
            (0, 1)
        );
    }

    #[test]
    fn a_live_session_resets_the_backoff() {
        assert_eq!(next_backoff(32, PushEnd::Session(SessionEnd::Live)), (1, 2));
    }

    #[test]
    fn a_session_that_never_came_up_backs_off() {
        let steps: Vec<u64> = std::iter::successors(Some(BACKOFF_MIN), |b| {
            Some(next_backoff(*b, PushEnd::Session(SessionEnd::NeverLive)).1)
        })
        .take(8)
        .collect();
        assert_eq!(steps, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn a_transport_failure_waits_at_least_five_seconds() {
        assert_eq!(next_backoff(1, PushEnd::Transport), (5, 10));
        assert_eq!(next_backoff(32, PushEnd::Transport), (32, 60));
    }

    #[test]
    fn a_refused_connect_keeps_the_plain_backoff() {
        assert_eq!(next_backoff(4, PushEnd::Failed), (4, 8));
    }

    #[test]
    fn only_unreachable_counts_as_transport() {
        assert!(is_transport_error(&ws_io_error()));
        assert!(!is_transport_error(&anyhow::anyhow!(
            "registrar returned 401"
        )));
        assert!(!is_transport_error(
            &tokio_tungstenite::tungstenite::Error::ConnectionClosed.into()
        ));
    }

    #[test]
    fn a_transport_failure_is_found_under_context() {
        let err = ws_io_error().context("connecting to trouter");
        assert!(is_transport_error(&err));
    }
}
