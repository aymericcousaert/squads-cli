use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
        KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, layout::Rect, widgets::ListState, Terminal};
use tokio::sync::Mutex;

use crate::api::{TeamsClient, SCOPE_GRAPH};
use crate::cli::utils::message;
use crate::config::Config;
use crate::names::{cached_member_names, resolve_member_names};
use crate::overview;
use crate::types::{Chat, MailMessage, Message, Team};

use super::media::Media;
use super::ui;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Panel {
    Chats,
    Messages,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeftPanelView {
    Chats,
    Channels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

/// Where the last frame put things, so a mouse event can be mapped back to a
/// row. Filled in by the renderer, which is the only place that knows the
/// layout and the scroll offsets the list widgets settled on.
#[derive(Default)]
pub struct Hits {
    pub tabs: Rect,
    pub sidebar: Rect,
    pub messages: Rect,
    pub compose: Rect,
    /// Screen row to chat index, for the rows the sidebar actually painted.
    pub chat_rows: Vec<(u16, usize)>,
    /// Screen row to (team, channel), for the channel view.
    pub channel_rows: Vec<(u16, (usize, usize))>,
    /// Screen row to message index.
    pub message_rows: Vec<(u16, usize)>,
}

impl Hits {
    fn at<T: Copy>(rows: &[(u16, T)], row: u16) -> Option<T> {
        rows.iter()
            .find(|(r, _)| *r == row)
            .map(|(_, value)| *value)
    }
}

pub struct App {
    pub client: Arc<TeamsClient>,
    pub chats: Vec<Chat>,
    pub messages: Vec<Message>,
    pub emails: Vec<MailMessage>,
    pub selected_chat: usize,
    pub selected_message: usize,
    pub active_panel: Panel,
    pub mode: Mode,
    pub input: String,
    pub input_cursor: usize, // Cursor position in input (character index)
    pub command_input: String,
    pub status_message: String,
    pub should_quit: bool,
    pub unread_emails: usize,
    pub unread_messages: usize,
    pub loading: bool,
    pub current_chat_id: Option<String>,
    // Teams channels support
    pub left_panel_view: LeftPanelView,
    pub teams: Vec<Team>,
    pub selected_team: usize,
    pub selected_channel: usize,
    pub current_team_id: Option<String>,
    pub current_channel_id: Option<String>,
    // User name cache (user_id -> display_name)
    pub user_names: HashMap<String, String>,
    pub my_user_id: Option<String>,
    // Scroll offsets. The list widgets write these back on every render, so
    // they have to outlive the frame or the panels jump back to the top.
    /// Own display name, so a message shows the right sender before the
    /// server copy comes back.
    pub my_display_name: Option<String>,
    /// Title of whatever is actually loaded in the conversation panel. The
    /// sidebar selection can move away from it, so the two are not the same.
    pub open_title: Option<String>,
    /// Inline pictures, keyed by URL.
    pub media: Media,
    /// The picture viewer, when it is open.
    pub preview: Option<Preview>,
    /// Picture URLs claimed by the last load and not yet handed to a task.
    pending_pictures: Vec<String>,
    /// Advanced once per redraw, so the spinner animates.
    pub tick: u64,
    pub hits: Hits,
    pub chats_state: ListState,
    pub channels_state: ListState,
    pub messages_state: ListState,
}

impl App {
    pub fn new(client: TeamsClient, media: Media) -> Self {
        Self {
            client: Arc::new(client),
            chats: Vec::new(),
            messages: Vec::new(),
            emails: Vec::new(),
            selected_chat: 0,
            selected_message: 0,
            active_panel: Panel::Chats,
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            command_input: String::new(),
            status_message: String::from("? for help"),
            should_quit: false,
            unread_emails: 0,
            unread_messages: 0,
            loading: false,
            current_chat_id: None,
            // Teams channels
            left_panel_view: LeftPanelView::Chats,
            teams: Vec::new(),
            selected_team: 0,
            selected_channel: 0,
            current_team_id: None,
            current_channel_id: None,
            // User cache
            user_names: HashMap::new(),
            my_user_id: None,
            media,
            preview: None,
            pending_pictures: Vec::new(),
            my_display_name: None,
            open_title: None,
            tick: 0,
            hits: Hits::default(),
            chats_state: ListState::default(),
            channels_state: ListState::default(),
            messages_state: ListState::default(),
        }
    }

    /// Paint the lists the last run saved, so the panels are usable before the
    /// first request comes back. Fresh data replaces them a moment later.
    fn apply_cached_overview(&mut self) {
        let cached = overview::load();
        if cached.chats.is_empty() && cached.teams.is_empty() {
            return;
        }
        self.my_user_id = cached.my_user_id;
        self.user_names = cached_member_names();
        self.unread_messages = cached
            .chats
            .iter()
            .filter(|c| c.is_read == Some(false))
            .count();
        self.chats = cached.chats;
        self.teams = cached.teams;
    }

    /// Show the chat and channel lists. Applied before the slower lookups
    /// below so the panels are usable while those are still running.
    fn apply_overview(&mut self, chats: Vec<Chat>, teams: Vec<Team>, my_user_id: Option<String>) {
        // Note what the cursor is on before the lists are replaced: this may be
        // swapping out a cached list the user is already scrolling, and the
        // fresh one can order things differently.
        let chat_id = self.chats.get(self.selected_chat).map(|c| c.id.clone());
        let team = self.teams.get(self.selected_team);
        let team_id = team.map(|t| t.id.clone());
        let channel_id = team
            .and_then(|t| t.channels.get(self.selected_channel))
            .map(|c| c.id.clone());

        self.unread_messages = chats.iter().filter(|c| c.is_read == Some(false)).count();
        self.chats = chats;
        self.teams = teams;
        if self.my_user_id.is_none() {
            self.my_user_id = my_user_id;
        }

        if let Some(index) = chat_id.and_then(|id| self.chats.iter().position(|c| c.id == id)) {
            self.selected_chat = index;
        }
        self.selected_chat = self.selected_chat.min(self.chats.len().saturating_sub(1));

        if let Some(index) = team_id.and_then(|id| self.teams.iter().position(|t| t.id == id)) {
            self.selected_team = index;
        }
        self.selected_team = self.selected_team.min(self.teams.len().saturating_sub(1));
        if let Some(channels) = self.teams.get(self.selected_team).map(|t| &t.channels) {
            if let Some(index) = channel_id.and_then(|id| channels.iter().position(|c| c.id == id))
            {
                self.selected_channel = index;
            }
            self.selected_channel = self.selected_channel.min(channels.len().saturating_sub(1));
        }

        self.status_message = format!(
            "{} chats   ·   {} channels   ·   resolving names",
            self.chats.len(),
            self.channel_count()
        );
    }

    /// Fill in the member names and the mail count, finishing the load.
    fn apply_details(&mut self, names: HashMap<String, String>, emails: Vec<MailMessage>) {
        // Extend, never replace: names resolved while reading a chat stay.
        self.user_names.extend(names);
        self.unread_emails = emails.iter().filter(|m| m.is_read != Some(true)).count();
        self.emails = emails;
        self.loading = false;
        // The unread counts live in the header bar now.
        self.status_message = format!(
            "{} chats   ·   {} channels   ·   ? for help",
            self.chats.len(),
            self.channel_count()
        );
    }

    /// Report a failed load without clearing the panels: whatever is already on
    /// screen is better than empty lists.
    fn fail_load(&mut self, message: String) {
        self.loading = false;
        self.status_message = message;
    }

    fn channel_count(&self) -> usize {
        self.teams.iter().map(|t| t.channels.len()).sum()
    }

    /// Get display name for a chat based on members
    pub fn get_chat_display_name(&self, chat: &Chat) -> String {
        // If chat has a title set, use it
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
                if self.my_user_id.as_ref() == Some(obj_id) {
                    return None;
                }
                // Look up name in cache, then try displayName from API response
                self.user_names
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

    /// Point the app at a chat and clear the panel, ready for its messages.
    fn begin_open_chat(&mut self, chat_id: String, title: String) {
        self.open_title = Some(title);
        self.current_chat_id = Some(chat_id);
        self.current_team_id = None;
        self.current_channel_id = None;
        self.messages.clear();
        self.selected_message = 0;
        self.loading = true;
        self.status_message = "Loading messages".to_string();
    }

    fn begin_open_channel(&mut self, team_id: String, channel_id: String, name: &str) {
        self.open_title = Some(format!("# {}", name));
        self.current_chat_id = None;
        self.current_team_id = Some(team_id);
        self.current_channel_id = Some(channel_id);
        self.messages.clear();
        self.selected_message = 0;
        self.loading = true;
        self.status_message = format!("Loading #{}", name);
    }

    fn apply_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.pending_pictures = self.queue_pictures();
        self.selected_message = self.messages.len().saturating_sub(1);
        self.loading = false;
        self.status_message = format!("{} messages   ·   i to compose", self.messages.len());
    }

    /// URLs in the loaded messages that nothing has started fetching yet.
    fn queue_pictures(&mut self) -> Vec<String> {
        let urls: Vec<String> = self
            .messages
            .iter()
            .filter_map(|m| m.content.as_deref())
            .flat_map(message::pictures)
            .map(|(url, _)| url)
            .collect();
        urls.into_iter()
            .filter(|url| self.media.claim(url))
            .collect()
    }

    /// Drop the unread dot now, without waiting for the round trip that makes
    /// it stick server side.
    fn mark_read_locally(&mut self, chat_id: &str) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.is_read = Some(true);
        }
        self.unread_messages = self
            .chats
            .iter()
            .filter(|c| c.is_read == Some(false))
            .count();
    }

    /// Show a message the moment it is typed, before the send completes.
    /// Returns the placeholder id so a failed send can take it back out.
    fn append_own_message(&mut self, text: &str) -> String {
        let id = format!("local-{}", Utc::now().timestamp_millis());
        self.messages.push(Message {
            content: Some(text.to_string()),
            from: self.my_user_id.as_ref().map(|id| format!("8:orgid:{}", id)),
            im_display_name: self.my_display_name.clone(),
            message_type: Some("RichText/Html".to_string()),
            properties: None,
            compose_time: None,
            original_arrival_time: Some(Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
            conversation_link: None,
            id: Some(id.clone()),
            container_id: None,
        });
        self.selected_message = self.messages.len().saturating_sub(1);
        id
    }

    /// Open the selected message's pictures full screen.
    ///
    /// Only the ones that finished downloading: a picture still on its way has
    /// nothing to show, and including it would give the viewer a blank page.
    fn open_preview(&mut self) {
        let all: Vec<String> = self
            .messages
            .get(self.selected_message)
            .and_then(|m| m.content.as_deref())
            .map(message::pictures)
            .unwrap_or_default()
            .into_iter()
            .map(|(url, _)| url)
            .collect();
        let urls = self.media.ready(&all);

        if urls.is_empty() {
            self.status_message = "No picture on this message".to_string();
            return;
        }
        self.preview = Some(Preview { urls, index: 0 });
    }

    fn remove_message(&mut self, id: &str) {
        self.messages.retain(|m| m.id.as_deref() != Some(id));
        self.selected_message = self.messages.len().saturating_sub(1);
    }

    pub fn next_chat(&mut self) {
        if !self.chats.is_empty() {
            self.selected_chat = (self.selected_chat + 1) % self.chats.len();
        }
    }

    pub fn previous_chat(&mut self) {
        if !self.chats.is_empty() {
            self.selected_chat = self
                .selected_chat
                .checked_sub(1)
                .unwrap_or(self.chats.len() - 1);
        }
    }

    pub fn next_message(&mut self) {
        if !self.messages.is_empty() {
            self.selected_message = (self.selected_message + 1).min(self.messages.len() - 1);
        }
    }

    pub fn previous_message(&mut self) {
        if !self.messages.is_empty() {
            self.selected_message = self.selected_message.saturating_sub(1);
        }
    }

    /// Navigate to next channel (within and across teams)
    pub fn next_channel(&mut self) {
        if self.teams.is_empty() {
            return;
        }

        if let Some(team) = self.teams.get(self.selected_team) {
            if self.selected_channel + 1 < team.channels.len() {
                // Next channel in same team
                self.selected_channel += 1;
            } else if self.selected_team + 1 < self.teams.len() {
                // First channel in next team
                self.selected_team += 1;
                self.selected_channel = 0;
            }
            // else: at the end, stay put
        }
    }

    /// Navigate to previous channel (within and across teams)
    pub fn previous_channel(&mut self) {
        if self.teams.is_empty() {
            return;
        }

        if self.selected_channel > 0 {
            // Previous channel in same team
            self.selected_channel -= 1;
        } else if self.selected_team > 0 {
            // Last channel in previous team
            self.selected_team -= 1;
            if let Some(team) = self.teams.get(self.selected_team) {
                self.selected_channel = team.channels.len().saturating_sub(1);
            }
        }
        // else: at the beginning, stay put
    }

    pub fn delete_word(&mut self) {
        // Delete word before cursor
        if self.input_cursor == 0 {
            return;
        }

        let chars: Vec<char> = self.input.chars().collect();
        let mut new_cursor = self.input_cursor;

        // Skip spaces before cursor
        while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
            new_cursor -= 1;
        }
        // Skip non-spaces (the word)
        while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
            new_cursor -= 1;
        }

        // Remove characters from new_cursor to input_cursor
        let before: String = chars[..new_cursor].iter().collect();
        let after: String = chars[self.input_cursor..].iter().collect();
        self.input = before + &after;
        self.input_cursor = new_cursor;
    }

    pub fn insert_char(&mut self, c: char) {
        let chars: Vec<char> = self.input.chars().collect();
        let before: String = chars[..self.input_cursor].iter().collect();
        let after: String = chars[self.input_cursor..].iter().collect();
        self.input = before + &c.to_string() + &after;
        self.input_cursor += 1;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn delete_char_before_cursor(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.input.chars().collect();
        let before: String = chars[..self.input_cursor - 1].iter().collect();
        let after: String = chars[self.input_cursor..].iter().collect();
        self.input = before + &after;
        self.input_cursor -= 1;
    }

    pub fn move_cursor_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
        }
    }

    pub fn move_cursor_right(&mut self) {
        let len = self.input.chars().count();
        if self.input_cursor < len {
            self.input_cursor += 1;
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        if self.input_cursor == 0 {
            return;
        }

        let chars: Vec<char> = self.input.chars().collect();
        let mut new_cursor = self.input_cursor;

        // Skip spaces before cursor
        while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
            new_cursor -= 1;
        }
        // Skip non-spaces (the word)
        while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
            new_cursor -= 1;
        }

        self.input_cursor = new_cursor;
    }

    pub fn move_cursor_word_right(&mut self) {
        let chars: Vec<char> = self.input.chars().collect();
        let len = chars.len();

        if self.input_cursor >= len {
            return;
        }

        let mut new_cursor = self.input_cursor;

        // Skip current word
        while new_cursor < len && chars[new_cursor] != ' ' {
            new_cursor += 1;
        }
        // Skip spaces
        while new_cursor < len && chars[new_cursor] == ' ' {
            new_cursor += 1;
        }

        self.input_cursor = new_cursor;
    }

    pub fn move_cursor_to_start(&mut self) {
        self.input_cursor = 0;
    }

    pub fn move_cursor_to_end(&mut self) {
        self.input_cursor = self.input.chars().count();
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// The pictures of one message, open full screen.
pub struct Preview {
    /// Only the ones that finished downloading: there is nothing to show for
    /// the others.
    urls: Vec<String>,
    index: usize,
}

impl Preview {
    pub fn current(&self) -> &str {
        // `open_preview` never builds one from an empty list.
        &self.urls[self.index]
    }

    /// Position in the message, for the caption.
    pub fn position(&self) -> (usize, usize) {
        (self.index + 1, self.urls.len())
    }

    fn next(&mut self) {
        self.index = (self.index + 1) % self.urls.len();
    }

    fn previous(&mut self) {
        self.index = (self.index + self.urls.len() - 1) % self.urls.len();
    }
}

/// What a click asked for beyond changing the selection.
enum Click {
    Nothing,
    OpenChat,
    OpenChannel,
}

fn within(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

/// Map a mouse event onto whatever the last frame drew under the pointer.
fn on_mouse(app: &mut App, event: MouseEvent) -> Click {
    let (column, row) = (event.column, event.row);
    match event.kind {
        MouseEventKind::ScrollDown => scroll(app, column, row, 1),
        MouseEventKind::ScrollUp => scroll(app, column, row, -1),
        MouseEventKind::Down(MouseButton::Left) => return click(app, column, row),
        _ => {}
    }
    Click::Nothing
}

/// The wheel moves the selection rather than the view: the list widget pulls
/// the selected row back into sight on every render, so a bare offset change
/// would not survive the next frame.
fn scroll(app: &mut App, column: u16, row: u16, direction: isize) {
    const SIDEBAR_STEP: isize = 3;

    if within(app.hits.messages, column, row) && !app.messages.is_empty() {
        app.active_panel = Panel::Messages;
        let last = app.messages.len() as isize - 1;
        let next = app.selected_message as isize + direction;
        app.selected_message = next.clamp(0, last.max(0)) as usize;
        return;
    }

    if within(app.hits.sidebar, column, row) {
        app.active_panel = Panel::Chats;
        match app.left_panel_view {
            LeftPanelView::Chats => {
                if app.chats.is_empty() {
                    return;
                }
                let last = app.chats.len() as isize - 1;
                let next = app.selected_chat as isize + direction * SIDEBAR_STEP;
                app.selected_chat = next.clamp(0, last) as usize;
            }
            LeftPanelView::Channels => {
                for _ in 0..SIDEBAR_STEP {
                    if direction > 0 {
                        app.next_channel();
                    } else {
                        app.previous_channel();
                    }
                }
            }
        }
    }
}

fn click(app: &mut App, column: u16, row: u16) -> Click {
    // The two tab labels sit side by side on the tab row.
    if within(app.hits.tabs, column, row) {
        let on_channels = column >= app.hits.tabs.x.saturating_add(8);
        app.left_panel_view = if on_channels {
            LeftPanelView::Channels
        } else {
            LeftPanelView::Chats
        };
        app.active_panel = Panel::Chats;
        return Click::Nothing;
    }

    if within(app.hits.compose, column, row) {
        app.mode = Mode::Insert;
        app.active_panel = Panel::Input;
        app.status_message = "Esc cancel   ·   Enter send   ·   F2 newline".to_string();
        return Click::Nothing;
    }

    if within(app.hits.sidebar, column, row) {
        app.active_panel = Panel::Chats;
        match app.left_panel_view {
            LeftPanelView::Chats => {
                if let Some(index) = Hits::at(&app.hits.chat_rows, row) {
                    app.selected_chat = index;
                    app.active_panel = Panel::Messages;
                    return Click::OpenChat;
                }
            }
            LeftPanelView::Channels => {
                if let Some((team, channel)) = Hits::at(&app.hits.channel_rows, row) {
                    app.selected_team = team;
                    app.selected_channel = channel;
                    app.active_panel = Panel::Messages;
                    return Click::OpenChannel;
                }
            }
        }
        return Click::Nothing;
    }

    if within(app.hits.messages, column, row) {
        app.active_panel = Panel::Messages;
        if let Some(index) = Hits::at(&app.hits.message_rows, row) {
            app.selected_message = index;
        }
    }

    Click::Nothing
}

/// Fetch and decode the pictures the last load claimed, one task each, and
/// store them as the terminal's graphics protocol wants them.
///
/// Decoding happens off the UI thread; only the handover is done under the
/// lock, so a slow GIF never stalls a redraw.
fn spawn_pictures(app: Arc<Mutex<App>>) {
    tokio::spawn(async move {
        let (client, urls) = {
            let mut guard = app.lock().await;
            if guard.pending_pictures.is_empty() {
                return;
            }
            (
                guard.client.clone(),
                std::mem::take(&mut guard.pending_pictures),
            )
        };

        for url in urls {
            let app = app.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let decoded = match client.fetch_picture(&url).await {
                    // Decoding is CPU work on bytes we already hold, so keep it
                    // off the runtime's worker threads.
                    Ok(bytes) => tokio::task::spawn_blocking(move || {
                        image::load_from_memory(&bytes).map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string())),
                    Err(e) => Err(e.to_string()),
                };

                let mut guard = app.lock().await;
                match decoded {
                    Ok(image) => guard.media.insert(&url, image),
                    Err(e) => {
                        // The alt text stays on screen; nothing else changes.
                        tracing::debug!("picture {} unavailable: {}", url, e);
                        guard.media.fail(&url);
                    }
                }
            });
        }
    });
}

/// Messages worth showing: real posts, minus anything deleted, newest last.
fn displayable(messages: impl Iterator<Item = Message>) -> Vec<Message> {
    let mut kept: Vec<Message> = messages
        .filter(|m| {
            let is_post = matches!(
                m.message_type.as_deref(),
                Some("RichText/Html") | Some("Text")
            );
            let deleted = m.properties.as_ref().is_some_and(|p| p.deletetime > 0);
            is_post && !deleted
        })
        .take(50)
        .collect();
    kept.reverse();
    kept
}

async fn fetch_chat_messages(client: &TeamsClient, chat_id: &str) -> Result<Vec<Message>> {
    let convs = client.get_conversations(chat_id, None).await?;
    Ok(displayable(convs.messages.into_iter()))
}

async fn fetch_channel_messages(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
) -> Result<Vec<Message>> {
    let convs = client.get_team_conversations(team_id, channel_id).await?;
    Ok(displayable(
        convs.reply_chains.into_iter().flat_map(|c| c.messages),
    ))
}

/// Open the selected chat: fetch its messages off the UI thread, then tell
/// Teams it has been read.
fn spawn_open_chat(app: Arc<Mutex<App>>) {
    tokio::spawn(async move {
        let prepared = {
            let mut guard = app.lock().await;
            let Some(chat) = guard.chats.get(guard.selected_chat) else {
                return;
            };
            let chat_id = chat.id.clone();
            let was_unread = chat.is_read == Some(false);
            let title = guard.get_chat_display_name(chat);
            guard.begin_open_chat(chat_id.clone(), title);
            if was_unread {
                guard.mark_read_locally(&chat_id);
            }
            (guard.client.clone(), chat_id, was_unread)
        };
        let (client, chat_id, was_unread) = prepared;

        match fetch_chat_messages(&client, &chat_id).await {
            Ok(messages) => {
                let newest = messages.last().and_then(|m| m.id.clone());
                app.lock().await.apply_messages(messages);
                spawn_pictures(app.clone());
                if was_unread {
                    // Best effort: the dot has already gone locally, and a
                    // failure here only means it comes back on the next load.
                    if let Err(e) = client.mark_chat_read(&chat_id, newest.as_deref()).await {
                        tracing::warn!("could not mark {} read: {}", chat_id, e);
                    }
                }
            }
            Err(e) => app.lock().await.fail_load(format!("Error: {}", e)),
        }
    });
}

fn spawn_open_channel(app: Arc<Mutex<App>>) {
    tokio::spawn(async move {
        let prepared = {
            let mut guard = app.lock().await;
            let Some(team) = guard.teams.get(guard.selected_team) else {
                return;
            };
            let Some(channel) = team.channels.get(guard.selected_channel) else {
                return;
            };
            let ids = (team.id.clone(), channel.id.clone());
            let name = channel.display_name.clone();
            guard.begin_open_channel(ids.0.clone(), ids.1.clone(), &name);
            (guard.client.clone(), ids)
        };
        let (client, (team_id, channel_id)) = prepared;

        match fetch_channel_messages(&client, &team_id, &channel_id).await {
            Ok(messages) => {
                app.lock().await.apply_messages(messages);
                spawn_pictures(app.clone());
            }
            Err(e) => app.lock().await.fail_load(format!("Error: {}", e)),
        }
    });
}

/// Send what is in the compose box. The message appears immediately and is
/// taken back out if the send fails, so typing never waits on the network.
fn spawn_send(app: Arc<Mutex<App>>) {
    tokio::spawn(async move {
        enum Target {
            Chat(String),
            Channel(String, String),
        }

        let prepared = {
            let mut guard = app.lock().await;
            if guard.input.trim().is_empty() {
                return;
            }
            let target = match (
                guard.current_chat_id.clone(),
                guard.current_team_id.clone(),
                guard.current_channel_id.clone(),
            ) {
                (Some(chat), _, _) => Target::Chat(chat),
                (None, Some(team), Some(channel)) => Target::Channel(team, channel),
                _ => {
                    guard.status_message = "No chat or channel selected".to_string();
                    return;
                }
            };
            let text = guard.input.clone();
            let html = format!("<p>{}</p>", html_escape(&text).replace('\n', "<br>"));
            guard.clear_input();
            let placeholder = guard.append_own_message(&text);
            guard.status_message = "Sending".to_string();
            guard.loading = true;
            (guard.client.clone(), target, html, placeholder)
        };
        let (client, target, html, placeholder) = prepared;

        let sent = match &target {
            Target::Chat(chat) => client.send_message(chat, &html, None).await.map(|_| ()),
            Target::Channel(team, channel) => client
                .send_channel_message(team, channel, &html, None)
                .await
                .map(|_| ()),
        };

        if let Err(e) = sent {
            let mut guard = app.lock().await;
            guard.remove_message(&placeholder);
            guard.fail_load(format!("Send failed: {}", e));
            return;
        }

        // Reconcile with the server copy, which carries the real id and time.
        let refreshed = match &target {
            Target::Chat(chat) => fetch_chat_messages(&client, chat).await,
            Target::Channel(team, channel) => fetch_channel_messages(&client, team, channel).await,
        };
        let mut guard = app.lock().await;
        match refreshed {
            Ok(messages) => {
                guard.apply_messages(messages);
                drop(guard);
                spawn_pictures(app.clone());
            }
            // The send worked, so keep the local copy on screen.
            Err(_) => {
                guard.loading = false;
                guard.status_message = "Message sent".to_string();
            }
        }
    });
}

/// Load the overview in the background, applying it in two stages: the chat and
/// channel lists first, then the member names and the mail count. The lists
/// arrive in one request, the rest takes about as long again, so showing them
/// early roughly halves the wait before the UI is usable.
///
/// The app is locked only to store each stage, so the main loop keeps drawing
/// throughout.
fn spawn_load(app: Arc<Mutex<App>>) {
    tokio::spawn(async move {
        let client = {
            let mut app = app.lock().await;
            if app.loading {
                return;
            }
            app.loading = true;
            // A list is already on screen when it came from the cache, or when
            // this is a manual refresh. Say so instead of "Loading...".
            app.status_message = if app.chats.is_empty() {
                "Loading...".to_string()
            } else {
                format!(
                    "{} chats   ·   {} channels   ·   refreshing",
                    app.chats.len(),
                    app.channel_count()
                )
            };
            app.client.clone()
        };

        let details = match client.get_user_details().await {
            Ok(details) => details,
            Err(e) => {
                app.lock()
                    .await
                    .fail_load(format!("Error loading chats: {}", e));
                return;
            }
        };

        // Who I am, so my own name is left out of the chat titles built from
        // member names. Asking the profile API is exact and costs nothing after
        // the first call; reading an MRI off a message I sent is the fallback
        // for when that request fails.
        let me = client.get_me().await.ok();
        let my_user_id = me.as_ref().map(|profile| profile.id.clone()).or_else(|| {
            details
                .chats
                .iter()
                .filter(|chat| chat.is_last_message_from_me == Some(true))
                .filter_map(|chat| chat.last_message.as_ref()?.from.as_ref())
                .find_map(|from| from.strip_prefix("8:orgid:"))
                .map(str::to_string)
        });

        let chats = details.chats.clone();
        let teams = details.teams.clone();
        {
            let mut guard = app.lock().await;
            guard.my_display_name = me.and_then(|profile| profile.display_name);
            guard.apply_overview(details.chats, details.teams, my_user_id.clone());
        }

        // Saved for the next run, which paints them before its first request.
        let _ = overview::save(&chats, &teams, my_user_id.as_deref());

        // Mint the Graph token before fanning out: both halves below need it,
        // and two requests minting it at once can each rewrite the token cache.
        let _ = client.get_token(SCOPE_GRAPH).await;
        let chat_refs: Vec<&Chat> = chats.iter().collect();
        let (names, mail) = tokio::join!(
            resolve_member_names(&client, &chat_refs, my_user_id.as_deref()),
            client.get_mail_messages(Some("inbox"), 50),
        );

        let emails = mail.map(|msgs| msgs.value).unwrap_or_default();
        app.lock().await.apply_details(names, emails);
    });
}

pub async fn run(config: &Config) -> Result<()> {
    // Build what can fail before touching the terminal: a `?` once raw mode is
    // on would return without restoring it and leave the shell unusable.
    let client = TeamsClient::new(config)?;
    let mut app = App::new(client, Media::new());
    app.apply_cached_overview();

    let mut terminal = setup_terminal()?;
    install_panic_hook();

    // Paint the cached lists before probing the terminal: the probe can take
    // a moment on a terminal that ignores it, and there is no reason to make
    // the first frame wait for it.
    terminal.draw(|f| ui::draw(f, &mut app))?;
    app.media.probe();

    let app = Arc::new(Mutex::new(app));
    spawn_load(app.clone());
    let result = run_app(&mut terminal, app).await;
    restore_terminal(&mut terminal);

    result
}

/// Leave the alternate screen before a panic prints. Without this the message
/// lands on top of the UI in a raw-mode terminal, and the shell is left
/// unusable afterwards.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        previous(info);
    }));
}

/// Enter the alternate screen, undoing the steps that succeeded if a later one
/// fails.
fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    // Without this a lone Esc is held back until the next key arrives, because
    // crossterm cannot tell it apart from the start of an escape sequence. The
    // terminal reports it unambiguously once asked.
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }

    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(e) => {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            let _ = disable_raw_mode();
            Err(e.into())
        }
    }
}

/// Best effort: a failure here must not hide the error we are leaving on.
fn restore_terminal(terminal: &mut Tui) {
    let _ = disable_raw_mode();
    // Harmless if nothing was pushed.
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
}

async fn run_app(terminal: &mut Tui, app: Arc<Mutex<App>>) -> Result<()> {
    // The key handlers below shadow `app` with the lock guard, so the spawner
    // needs its own handle on the shared state.
    let shared = app.clone();

    loop {
        // Draw UI
        {
            let mut app = app.lock().await;
            app.tick = app.tick.wrapping_add(1);
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }

        // Handle input with timeout for async updates
        if event::poll(Duration::from_millis(100))? {
            let incoming = event::read()?;

            if let Event::Mouse(mouse) = incoming {
                let action = {
                    let mut app = app.lock().await;
                    on_mouse(&mut app, mouse)
                };
                match action {
                    Click::OpenChat => spawn_open_chat(shared.clone()),
                    Click::OpenChannel => spawn_open_channel(shared.clone()),
                    Click::Nothing => {}
                }
            }

            if let Event::Key(key) = incoming {
                let mut app = app.lock().await;
                // Handle Ctrl+C always
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                }

                // The picture viewer covers the screen, so while it is open it
                // takes every key: nothing underneath it should react.
                if app.preview.is_some() {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') | KeyCode::Enter => {
                            app.preview = None;
                            app.status_message = "? for help".to_string();
                        }
                        KeyCode::Char('n') | KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => {
                            if let Some(preview) = app.preview.as_mut() {
                                preview.next();
                            }
                        }
                        KeyCode::Char('p') | KeyCode::Char('h') | KeyCode::Left => {
                            if let Some(preview) = app.preview.as_mut() {
                                preview.previous();
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                match app.mode {
                    Mode::Normal => {
                        match key.code {
                            KeyCode::Char('q') => app.should_quit = true,
                            KeyCode::Char('?') => {
                                app.status_message = format!(
                                    "j/k move   ·   Enter open   ·   1 chats   ·   2 channels   ·   i compose   ·   r refresh   ·   v pictures   ·   q quit   ·   images: {}",
                                    app.media.protocol_name()
                                );
                            }
                            // View switching with 1 and 2
                            KeyCode::Char('1') => {
                                app.left_panel_view = LeftPanelView::Chats;
                                app.active_panel = Panel::Chats;
                            }
                            KeyCode::Char('2') => {
                                app.left_panel_view = LeftPanelView::Channels;
                                app.active_panel = Panel::Chats;
                            }
                            KeyCode::Char('j') | KeyCode::Down => match app.active_panel {
                                Panel::Chats => {
                                    if app.left_panel_view == LeftPanelView::Chats {
                                        app.next_chat();
                                    } else {
                                        app.next_channel();
                                    }
                                }
                                Panel::Messages => app.next_message(),
                                _ => {}
                            },
                            KeyCode::Char('k') | KeyCode::Up => match app.active_panel {
                                Panel::Chats => {
                                    if app.left_panel_view == LeftPanelView::Chats {
                                        app.previous_chat();
                                    } else {
                                        app.previous_channel();
                                    }
                                }
                                Panel::Messages => app.previous_message(),
                                _ => {}
                            },
                            KeyCode::Char('g') => {
                                // Go to top
                                match app.active_panel {
                                    Panel::Chats => {
                                        if app.left_panel_view == LeftPanelView::Chats {
                                            app.selected_chat = 0;
                                        } else {
                                            app.selected_team = 0;
                                            app.selected_channel = 0;
                                        }
                                    }
                                    Panel::Messages => app.selected_message = 0,
                                    _ => {}
                                }
                            }
                            KeyCode::Char('G') => {
                                // Go to bottom
                                match app.active_panel {
                                    Panel::Chats => {
                                        if app.left_panel_view == LeftPanelView::Chats {
                                            app.selected_chat = app.chats.len().saturating_sub(1);
                                        } else if !app.teams.is_empty() {
                                            let last_team_idx = app.teams.len() - 1;
                                            let last_channel_idx = app.teams[last_team_idx]
                                                .channels
                                                .len()
                                                .saturating_sub(1);
                                            app.selected_team = last_team_idx;
                                            app.selected_channel = last_channel_idx;
                                        }
                                    }
                                    Panel::Messages => {
                                        app.selected_message = app.messages.len().saturating_sub(1)
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Tab => {
                                app.active_panel = match app.active_panel {
                                    Panel::Chats => Panel::Messages,
                                    Panel::Messages => Panel::Input,
                                    Panel::Input => Panel::Chats,
                                };
                            }
                            KeyCode::Char('h') | KeyCode::Left => {
                                app.active_panel = Panel::Chats;
                            }
                            KeyCode::Char('l') | KeyCode::Right => {
                                app.active_panel = Panel::Messages;
                            }
                            KeyCode::Enter => {
                                if app.active_panel == Panel::Chats {
                                    app.active_panel = Panel::Messages;
                                    if app.left_panel_view == LeftPanelView::Chats {
                                        spawn_open_chat(shared.clone());
                                    } else {
                                        spawn_open_channel(shared.clone());
                                    }
                                }
                            }
                            KeyCode::Char('v') => {
                                if app.preview.is_some() {
                                    app.preview = None;
                                } else {
                                    app.open_preview();
                                }
                            }
                            KeyCode::Char('i') => {
                                app.mode = Mode::Insert;
                                app.active_panel = Panel::Input;
                                app.status_message =
                                    "Esc cancel   ·   Enter send   ·   F2 newline".to_string();
                            }
                            KeyCode::Char('r') => {
                                spawn_load(shared.clone());
                                if app.current_chat_id.is_some() {
                                    spawn_open_chat(shared.clone());
                                } else if app.current_channel_id.is_some() {
                                    spawn_open_channel(shared.clone());
                                }
                            }
                            KeyCode::Char(':') => {
                                app.mode = Mode::Command;
                                app.command_input.clear();
                                app.status_message = ":".to_string();
                            }
                            _ => {}
                        }
                    }
                    Mode::Insert => {
                        match key.code {
                            KeyCode::Esc => {
                                app.mode = Mode::Normal;
                                app.status_message = "? for help".to_string();
                            }
                            // Multiple ways to insert newline:
                            // 1. F2 key (universal - works on all terminals)
                            // 2. Ctrl+J (traditional Unix)
                            // 3. Ctrl+O (traditional "open line")
                            // 4. Alt+Enter / Option+Enter (macOS friendly)
                            KeyCode::F(2) => {
                                app.insert_newline();
                            }
                            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.insert_newline();
                            }
                            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.insert_newline();
                            }
                            KeyCode::Enter => {
                                // Alt+Enter (Option+Enter on macOS): insert newline
                                // Shift+Enter or Ctrl+Enter: also insert newline
                                // Note: Many terminals don't pass Shift+Enter correctly
                                if key.modifiers.contains(KeyModifiers::ALT)
                                    || key.modifiers.contains(KeyModifiers::SHIFT)
                                    || key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    app.insert_newline();
                                } else {
                                    // Enter: send message
                                    spawn_send(shared.clone());
                                    app.mode = Mode::Normal;
                                }
                            }
                            KeyCode::Backspace => {
                                if key.modifiers.contains(KeyModifiers::ALT) {
                                    // Alt+Backspace: delete word
                                    app.delete_word();
                                } else {
                                    app.delete_char_before_cursor();
                                }
                            }
                            KeyCode::Left => {
                                if key.modifiers.contains(KeyModifiers::ALT)
                                    || key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    // Alt+Left or Ctrl+Left: move word left
                                    app.move_cursor_word_left();
                                } else {
                                    app.move_cursor_left();
                                }
                            }
                            KeyCode::Right => {
                                if key.modifiers.contains(KeyModifiers::ALT)
                                    || key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    // Alt+Right or Ctrl+Right: move word right
                                    app.move_cursor_word_right();
                                } else {
                                    app.move_cursor_right();
                                }
                            }
                            // Also support Ctrl+B/F for word navigation (emacs style)
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                                app.move_cursor_word_left();
                            }
                            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                                app.move_cursor_word_right();
                            }
                            KeyCode::Home => {
                                app.move_cursor_to_start();
                            }
                            KeyCode::End => {
                                app.move_cursor_to_end();
                            }
                            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Ctrl+A: go to start
                                app.move_cursor_to_start();
                            }
                            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Ctrl+E: go to end
                                app.move_cursor_to_end();
                            }
                            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Ctrl+W: delete word (vim style)
                                app.delete_word();
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Ctrl+U: clear line
                                app.clear_input();
                            }
                            KeyCode::Char(c) => {
                                app.insert_char(c);
                            }
                            _ => {}
                        }
                    }
                    Mode::Command => match key.code {
                        KeyCode::Esc => {
                            app.mode = Mode::Normal;
                            app.command_input.clear();
                            app.status_message = "? for help".to_string();
                        }
                        KeyCode::Enter => {
                            let cmd = app.command_input.clone();
                            app.command_input.clear();
                            app.mode = Mode::Normal;

                            match cmd.as_str() {
                                "q" | "quit" => app.should_quit = true,
                                "r" | "refresh" => {
                                    spawn_load(shared.clone());
                                }
                                "mail" | "m" => {
                                    app.status_message =
                                        format!("{} unread emails", app.unread_emails);
                                }
                                _ => {
                                    app.status_message = format!("Unknown command: {}", cmd);
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            app.command_input.pop();
                            app.status_message = format!(":{}", app.command_input);
                        }
                        KeyCode::Char(c) => {
                            app.command_input.push(c);
                            app.status_message = format!(":{}", app.command_input);
                        }
                        _ => {}
                    },
                }

                if app.should_quit {
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod preview_tests {
    use super::Preview;

    fn viewer(count: usize) -> Preview {
        Preview {
            urls: (0..count).map(|i| format!("u{}", i)).collect(),
            index: 0,
        }
    }

    #[test]
    fn a_single_picture_stays_put() {
        let mut p = viewer(1);
        p.next();
        assert_eq!(p.current(), "u0");
        p.previous();
        assert_eq!(p.current(), "u0");
        assert_eq!(p.position(), (1, 1));
    }

    #[test]
    fn next_walks_forward_and_wraps() {
        let mut p = viewer(3);
        assert_eq!(p.position(), (1, 3));
        p.next();
        assert_eq!(p.current(), "u1");
        p.next();
        assert_eq!((p.current(), p.position()), ("u2", (3, 3)));
        p.next();
        assert_eq!(p.current(), "u0", "wraps to the first");
    }

    #[test]
    fn previous_walks_back_and_wraps() {
        let mut p = viewer(3);
        p.previous();
        assert_eq!((p.current(), p.position()), ("u2", (3, 3)));
        p.previous();
        assert_eq!(p.current(), "u1");
    }
}
