use anyhow::Result;
use clap::{Args, ValueEnum};
use colored::Colorize;
use std::collections::HashSet;
use std::time::Duration;

use crate::api::{SessionEnd, TeamsClient, TrouterEvent, TrouterMessage};
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

    /// Event kinds to put on the --json stream, comma separated. Messages only by default.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "message")]
    pub events: Vec<WatchEvent>,
}

/// An event kind the --json stream can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WatchEvent {
    /// A new chat message
    #[value(name = "message")]
    Message,
    /// An edit, or a reaction landing on a message
    #[value(name = "message_update", alias = "message-update")]
    MessageUpdate,
    /// Someone is typing in a chat
    #[value(name = "typing")]
    Typing,
    /// Someone moved their read marker in a chat
    #[value(name = "read")]
    Read,
    /// Events were dropped and the consumer has to resync
    #[value(name = "message_loss", alias = "message-loss")]
    MessageLoss,
    /// A user's availability changed
    #[value(name = "presence")]
    Presence,
    /// Every kind above
    #[value(name = "all")]
    All,
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

    if !cmd.json && cmd.events.as_slice() != [WatchEvent::Message] {
        eprintln!("note: --events shapes the --json stream only");
    }

    // Message ids already delivered this session. See the dedup step below.
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
                let kind = event_kind(&ev);
                // Each skip says why, so an event that never printed can be told
                // apart from one that never arrived.
                let skip = |reason: &str| {
                    if debug {
                        eprintln!("[watch] skipped {kind:?}: {reason}");
                    }
                };
                // The terminal output is messages only; --events widens the json stream.
                let wanted = if cmd.json {
                    wants(&cmd.events, kind)
                } else {
                    kind == WatchEvent::Message
                };
                if !wanted {
                    skip("not selected");
                    return;
                }
                // skip our own messages, and our own edits
                if let TrouterEvent::NewMessage(m) | TrouterEvent::MessageUpdate(m) = &ev {
                    if my_mri.as_deref() == Some(m.from_mri.as_str()) {
                        skip("own message");
                        return;
                    }
                }
                // optional chat filter, on the events that belong to a chat
                if let Some(chat_id) = event_chat_id(&ev) {
                    if !cmd.chat.is_empty() && !cmd.chat.iter().any(|c| c == chat_id) {
                        skip("chat filtered out");
                        return;
                    }
                }
                // Dedup and the empty-content check are for new messages only. An edit
                // reuses the message id, so deduping updates would swallow every edit.
                if let TrouterEvent::NewMessage(m) = &ev {
                    // Trouter redelivers un-acked events and replays recent messages.
                    if !m.message_id.is_empty() && !seen.insert(m.message_id.clone()) {
                        skip("already delivered");
                        return;
                    }
                    if seen.len() > 10_000 {
                        seen.clear();
                    }
                    if strip_html(&m.content).trim().is_empty() {
                        skip("empty after html strip");
                        return;
                    }
                }

                if cmd.json {
                    let line = event_line(&ev, &chrono::Utc::now().to_rfc3339());
                    println!("{}", serde_json::to_string(&line).unwrap_or_default());
                    let _ = std::io::stdout().flush();
                } else if !cmd.quiet {
                    if let TrouterEvent::NewMessage(m) = &ev {
                        let time = chrono::Local::now().format("%H:%M:%S").to_string();
                        println!(
                            "{} 💬 {} {}",
                            format!("[{}]", time).dimmed(),
                            format!("{}:", m.from).cyan().bold(),
                            truncate(&strip_html(&m.content), 80)
                        );
                    }
                }

                if cmd.notify {
                    if let TrouterEvent::NewMessage(m) = &ev {
                        send_notification(
                            &format!("Teams: {}", m.from),
                            &truncate(&strip_html(&m.content), 100),
                            "teams",
                        );
                    }
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

/// True when the stream was asked for this kind.
fn wants(selected: &[WatchEvent], kind: WatchEvent) -> bool {
    selected.contains(&WatchEvent::All) || selected.contains(&kind)
}

/// The kind of an incoming event. Never `All`.
fn event_kind(ev: &TrouterEvent) -> WatchEvent {
    match ev {
        TrouterEvent::NewMessage(_) => WatchEvent::Message,
        TrouterEvent::MessageUpdate(_) => WatchEvent::MessageUpdate,
        TrouterEvent::Typing { .. } => WatchEvent::Typing,
        TrouterEvent::ReadHorizon { .. } => WatchEvent::Read,
        TrouterEvent::MessageLoss => WatchEvent::MessageLoss,
        TrouterEvent::Presence { .. } => WatchEvent::Presence,
    }
}

/// The chat an event belongs to, when it belongs to one. Presence and message
/// loss are account-wide, so --chat must not hide them.
fn event_chat_id(ev: &TrouterEvent) -> Option<&str> {
    match ev {
        TrouterEvent::NewMessage(m) | TrouterEvent::MessageUpdate(m) => Some(&m.chat_id),
        TrouterEvent::Typing { chat_id, .. } | TrouterEvent::ReadHorizon { chat_id } => {
            Some(chat_id)
        }
        TrouterEvent::MessageLoss | TrouterEvent::Presence { .. } => None,
    }
}

/// One line of the --json stream. The clock is a parameter so the mapping stays
/// pure and can be tested.
fn event_line(ev: &TrouterEvent, time: &str) -> serde_json::Value {
    match ev {
        TrouterEvent::NewMessage(m) => message_line("message", m, time),
        TrouterEvent::MessageUpdate(m) => message_line("message_update", m, time),
        TrouterEvent::Typing { chat_id, from } => serde_json::json!({
            "event": "typing",
            "chat_id": chat_id,
            // Teams sends most Control/Typing frames with no display name.
            "from": from,
            "time": time,
            "source": "push",
        }),
        TrouterEvent::ReadHorizon { chat_id } => serde_json::json!({
            "event": "read",
            "chat_id": chat_id,
            "time": time,
            "source": "push",
        }),
        TrouterEvent::MessageLoss => serde_json::json!({
            "event": "message_loss",
            "time": time,
            "source": "push",
        }),
        TrouterEvent::Presence {
            user_id,
            availability,
        } => serde_json::json!({
            "event": "presence",
            "user_id": user_id,
            "availability": availability,
            "time": time,
            "source": "push",
        }),
    }
}

/// The message shape, unchanged since the first --json release apart from `event`.
/// Adding or renaming a field here breaks every consumer.
fn message_line(event: &str, m: &TrouterMessage, time: &str) -> serde_json::Value {
    serde_json::json!({
        "chat_id": m.chat_id,
        "message_id": m.message_id,
        "from": m.from,
        "from_mri": m.from_mri,
        "time": time,
        "content": strip_html(&m.content),
        "source": "push",
        "event": event,
    })
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

    /// A message with every field set, so a test can pin the whole shape.
    fn message() -> TrouterMessage {
        TrouterMessage {
            chat_id: "19:abc@thread.v2".to_string(),
            from_mri: "8:orgid:u1".to_string(),
            from: "Ada Fenwick".to_string(),
            content: "<p>hello <b>you</b></p>".to_string(),
            message_id: "1700000000000".to_string(),
            message_type: "RichText/Html".to_string(),
        }
    }

    fn line(ev: &TrouterEvent) -> String {
        serde_json::to_string(&event_line(ev, "2030-01-01T10:00:00+00:00")).unwrap()
    }

    #[test]
    fn a_message_keeps_the_published_shape() {
        assert_eq!(
            line(&TrouterEvent::NewMessage(message())),
            r#"{"chat_id":"19:abc@thread.v2","content":"hello you","event":"message","from":"Ada Fenwick","from_mri":"8:orgid:u1","message_id":"1700000000000","source":"push","time":"2030-01-01T10:00:00+00:00"}"#
        );
    }

    #[test]
    fn an_update_uses_the_message_shape() {
        assert_eq!(
            line(&TrouterEvent::MessageUpdate(message())),
            r#"{"chat_id":"19:abc@thread.v2","content":"hello you","event":"message_update","from":"Ada Fenwick","from_mri":"8:orgid:u1","message_id":"1700000000000","source":"push","time":"2030-01-01T10:00:00+00:00"}"#
        );
    }

    #[test]
    fn typing_carries_the_chat_even_with_no_name() {
        assert_eq!(
            line(&TrouterEvent::Typing {
                chat_id: "19:abc@thread.v2".to_string(),
                from: String::new(),
            }),
            r#"{"chat_id":"19:abc@thread.v2","event":"typing","from":"","source":"push","time":"2030-01-01T10:00:00+00:00"}"#
        );
    }

    #[test]
    fn a_read_marker_carries_the_chat() {
        assert_eq!(
            line(&TrouterEvent::ReadHorizon {
                chat_id: "19:abc@thread.v2".to_string(),
            }),
            r#"{"chat_id":"19:abc@thread.v2","event":"read","source":"push","time":"2030-01-01T10:00:00+00:00"}"#
        );
    }

    #[test]
    fn message_loss_has_no_payload() {
        assert_eq!(
            line(&TrouterEvent::MessageLoss),
            r#"{"event":"message_loss","source":"push","time":"2030-01-01T10:00:00+00:00"}"#
        );
    }

    #[test]
    fn presence_carries_the_user_and_availability() {
        assert_eq!(
            line(&TrouterEvent::Presence {
                user_id: "u1".to_string(),
                availability: "Away".to_string(),
            }),
            r#"{"availability":"Away","event":"presence","source":"push","time":"2030-01-01T10:00:00+00:00","user_id":"u1"}"#
        );
    }

    #[test]
    fn only_chat_scoped_events_can_be_filtered_by_chat() {
        assert_eq!(
            event_chat_id(&TrouterEvent::NewMessage(message())),
            Some("19:abc@thread.v2")
        );
        assert_eq!(
            event_chat_id(&TrouterEvent::ReadHorizon {
                chat_id: "19:abc@thread.v2".to_string()
            }),
            Some("19:abc@thread.v2")
        );
        assert_eq!(event_chat_id(&TrouterEvent::MessageLoss), None);
        assert_eq!(
            event_chat_id(&TrouterEvent::Presence {
                user_id: "u1".to_string(),
                availability: "Away".to_string()
            }),
            None
        );
    }

    #[test]
    fn the_default_selection_is_messages_only() {
        let default = [WatchEvent::Message];
        assert!(wants(&default, WatchEvent::Message));
        assert!(!wants(&default, WatchEvent::MessageUpdate));
        assert!(!wants(&default, WatchEvent::Typing));
    }

    #[test]
    fn all_selects_every_kind() {
        for kind in [
            WatchEvent::Message,
            WatchEvent::MessageUpdate,
            WatchEvent::Typing,
            WatchEvent::Read,
            WatchEvent::MessageLoss,
            WatchEvent::Presence,
        ] {
            assert!(wants(&[WatchEvent::All], kind), "{kind:?} not selected");
            assert!(wants(&[kind], kind), "{kind:?} not selected by name");
        }
    }

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
