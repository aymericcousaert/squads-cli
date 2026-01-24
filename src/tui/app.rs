use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::Mutex;

use crate::api::TeamsClient;
use crate::config::Config;
use crate::types::{Chat, MailMessage, Message, Team};

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
}

impl App {
    pub fn new(client: TeamsClient) -> Self {
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
            status_message: String::from("Press ? for help | 1: Chats | 2: Channels | q to quit"),
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
        }
    }

    pub async fn load_data(&mut self) -> Result<()> {
        self.loading = true;
        self.status_message = "Loading...".to_string();

        // Load chats and teams
        match self.client.get_user_details().await {
            Ok(details) => {
                self.unread_messages = details
                    .chats
                    .iter()
                    .filter(|c| c.is_read == Some(false))
                    .count();
                self.chats = details.chats;
                self.teams = details.teams;
                // Store my user ID (from the first chat where I'm a member)
                if self.my_user_id.is_none() {
                    for chat in &self.chats {
                        // Try to find my ID by checking last_message sender
                        if chat.is_last_message_from_me == Some(true) {
                            if let Some(last_msg) = &chat.last_message {
                                if let Some(from) = &last_msg.from {
                                    // Extract user ID from MRI like "8:orgid:xxxx"
                                    if let Some(id) = from.strip_prefix("8:orgid:") {
                                        self.my_user_id = Some(id.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                self.status_message = format!("Error loading chats: {}", e);
            }
        }

        // Resolve member names for chats (collect unique IDs first)
        let mut unique_ids: Vec<String> = Vec::new();
        for chat in &self.chats {
            for member in &chat.members {
                if let Some(obj_id) = &member.object_id {
                    if !self.user_names.contains_key(obj_id)
                        && !unique_ids.contains(obj_id)
                        && Some(obj_id) != self.my_user_id.as_ref()
                    {
                        unique_ids.push(obj_id.clone());
                    }
                }
            }
        }

        // Look up user names (limit to first 20 to avoid too many API calls)
        self.status_message = format!("Resolving {} user names...", unique_ids.len().min(20));
        for user_id in unique_ids.into_iter().take(20) {
            if let Ok(Some(user)) = self.client.get_user_by_id(&user_id).await {
                if let Some(name) = user.display_name {
                    self.user_names.insert(user_id, name);
                }
            }
        }

        // Load emails (just count unread)
        match self.client.get_mail_messages(Some("inbox"), 50).await {
            Ok(msgs) => {
                self.unread_emails = msgs
                    .value
                    .iter()
                    .filter(|m| m.is_read != Some(true))
                    .count();
                self.emails = msgs.value;
            }
            Err(_) => {
                // Silently ignore mail errors
            }
        }

        self.loading = false;
        let channel_count: usize = self.teams.iter().map(|t| t.channels.len()).sum();
        self.status_message = format!(
            "{} chats | {} channels | {} unread emails | 1/2: toggle view | ? for help",
            self.chats.len(),
            channel_count,
            self.unread_emails
        );

        Ok(())
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
                // Look up name in cache
                self.user_names.get(obj_id).cloned()
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

    pub async fn load_messages(&mut self) -> Result<()> {
        if let Some(chat) = self.chats.get(self.selected_chat) {
            self.current_chat_id = Some(chat.id.clone());
            self.loading = true;
            self.status_message = "Loading messages...".to_string();

            match self.client.get_conversations(&chat.id, None).await {
                Ok(convs) => {
                    // API returns newest first, so take 50 most recent then reverse for display
                    let mut msgs: Vec<_> = convs
                        .messages
                        .into_iter()
                        .filter(|m| {
                            // Filter by message type
                            let is_content_msg = m.message_type.as_deref() == Some("RichText/Html")
                                || m.message_type.as_deref() == Some("Text");
                            // Filter out deleted messages (deletetime > 0)
                            let is_deleted = m
                                .properties
                                .as_ref()
                                .map(|p| p.deletetime > 0)
                                .unwrap_or(false);
                            is_content_msg && !is_deleted
                        })
                        .take(50)
                        .collect();
                    msgs.reverse(); // Show oldest first, newest at bottom
                    self.messages = msgs;
                    self.selected_message = self.messages.len().saturating_sub(1);
                }
                Err(e) => {
                    self.status_message = format!("Error: {}", e);
                }
            }

            self.loading = false;
            self.status_message = format!(
                "{} messages | i to compose | Enter to select",
                self.messages.len()
            );
        }
        Ok(())
    }

    pub async fn load_channel_messages(&mut self) -> Result<()> {
        if let Some(team) = self.teams.get(self.selected_team) {
            if let Some(channel) = team.channels.get(self.selected_channel) {
                self.current_team_id = Some(team.id.clone());
                self.current_channel_id = Some(channel.id.clone());
                self.current_chat_id = None; // Clear chat context
                self.loading = true;
                self.status_message = format!("Loading {} messages...", channel.display_name);

                match self
                    .client
                    .get_team_conversations(&team.id, &channel.id)
                    .await
                {
                    Ok(convs) => {
                        let mut msgs: Vec<_> = convs
                            .reply_chains
                            .into_iter()
                            .flat_map(|chain| chain.messages)
                            .filter(|m| {
                                let is_content_msg = m.message_type.as_deref()
                                    == Some("RichText/Html")
                                    || m.message_type.as_deref() == Some("Text");
                                let is_deleted = m
                                    .properties
                                    .as_ref()
                                    .map(|p| p.deletetime > 0)
                                    .unwrap_or(false);
                                is_content_msg && !is_deleted
                            })
                            .take(50)
                            .collect();
                        msgs.reverse();
                        self.messages = msgs;
                        self.selected_message = self.messages.len().saturating_sub(1);
                    }
                    Err(e) => {
                        self.status_message = format!("Error: {}", e);
                    }
                }

                self.loading = false;
                self.status_message = format!(
                    "#{} | {} messages | i to compose",
                    channel.display_name,
                    self.messages.len()
                );
            }
        }
        Ok(())
    }

    pub async fn send_message(&mut self) -> Result<()> {
        if self.input.is_empty() {
            return Ok(());
        }

        // Convert newlines to <br> for multi-line messages
        let escaped = html_escape(&self.input);
        let with_breaks = escaped.replace('\n', "<br>");
        let content = format!("<p>{}</p>", with_breaks);

        if let Some(chat_id) = &self.current_chat_id.clone() {
            // Send to chat
            match self.client.send_message(chat_id, &content, None).await {
                Ok(_) => {
                    self.status_message = "Message sent! Refreshing...".to_string();
                    self.clear_input();
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    self.load_messages().await?;
                }
                Err(e) => {
                    self.status_message = format!("Send failed: {}", e);
                }
            }
        } else if let (Some(team_id), Some(channel_id)) = (
            self.current_team_id.clone(),
            self.current_channel_id.clone(),
        ) {
            // Send to channel
            match self
                .client
                .send_channel_message(&team_id, &channel_id, &content, None)
                .await
            {
                Ok(_) => {
                    self.status_message = "Message posted! Refreshing...".to_string();
                    self.clear_input();
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    self.load_channel_messages().await?;
                }
                Err(e) => {
                    self.status_message = format!("Post failed: {}", e);
                }
            }
        } else {
            self.status_message = "No chat or channel selected".to_string();
        }
        Ok(())
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

pub async fn run(config: &Config) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let client = TeamsClient::new(config)?;
    let app = Arc::new(Mutex::new(App::new(client)));

    // Initial data load
    {
        let mut app = app.lock().await;
        app.load_data().await?;
    }

    // Main loop
    let result = run_app(&mut terminal, app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: Arc<Mutex<App>>,
) -> Result<()> {
    loop {
        // Draw UI
        {
            let app = app.lock().await;
            terminal.draw(|f| ui::draw(f, &app))?;
        }

        // Handle input with timeout for async updates
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                let mut app = app.lock().await;

                // Handle Ctrl+C always
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                }

                match app.mode {
                    Mode::Normal => {
                        match key.code {
                            KeyCode::Char('q') => app.should_quit = true,
                            KeyCode::Char('?') => {
                                app.status_message = "j/k: navigate | Enter: select | 1: chats | 2: channels | i: compose | r: refresh | q: quit".to_string();
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
                                        app.load_messages().await?;
                                    } else {
                                        app.load_channel_messages().await?;
                                    }
                                }
                            }
                            KeyCode::Char('i') => {
                                app.mode = Mode::Insert;
                                app.active_panel = Panel::Input;
                                app.status_message =
                                    "-- INSERT -- (Esc: cancel, Enter: send, F2: newline)"
                                        .to_string();
                            }
                            KeyCode::Char('r') => {
                                app.load_data().await?;
                                if app.current_chat_id.is_some() {
                                    app.load_messages().await?;
                                } else if app.current_channel_id.is_some() {
                                    app.load_channel_messages().await?;
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
                                app.status_message = "Press ? for help".to_string();
                            }
                            // Multiple ways to insert newline:
                            // 1. F2 key (universal - works on all terminals)
                            // 2. Ctrl+J (traditional Unix)
                            // 3. Ctrl+O (traditional "open line")
                            // 4. Alt+Enter / Option+Enter (macOS friendly)
                            KeyCode::F(2) => {
                                app.insert_newline();
                            }
                            KeyCode::Char('j')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                app.insert_newline();
                            }
                            KeyCode::Char('o')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
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
                                    app.send_message().await?;
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
                            KeyCode::Char('b')
                                if key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.move_cursor_word_left();
                            }
                            KeyCode::Char('f')
                                if key.modifiers.contains(KeyModifiers::ALT) =>
                            {
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
                            app.status_message = "Press ? for help".to_string();
                        }
                        KeyCode::Enter => {
                            let cmd = app.command_input.clone();
                            app.command_input.clear();
                            app.mode = Mode::Normal;

                            match cmd.as_str() {
                                "q" | "quit" => app.should_quit = true,
                                "r" | "refresh" => {
                                    app.load_data().await?;
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
