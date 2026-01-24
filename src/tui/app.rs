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
use crate::types::{Chat, MailMessage, Message};

use super::ui;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Panel {
    Chats,
    Messages,
    Input,
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
    pub scroll_offset: usize,
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
            scroll_offset: 0,
            active_panel: Panel::Chats,
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            command_input: String::new(),
            status_message: String::from("Press ? for help | q to quit"),
            should_quit: false,
            unread_emails: 0,
            unread_messages: 0,
            loading: false,
            current_chat_id: None,
        }
    }

    pub async fn load_data(&mut self) -> Result<()> {
        self.loading = true;
        self.status_message = "Loading...".to_string();

        // Load chats
        match self.client.get_user_details().await {
            Ok(details) => {
                self.unread_messages = details
                    .chats
                    .iter()
                    .filter(|c| c.is_read == Some(false))
                    .count();
                self.chats = details.chats;
            }
            Err(e) => {
                self.status_message = format!("Error loading chats: {}", e);
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
        self.status_message = format!(
            "Loaded {} chats | {} unread emails | Press ? for help",
            self.chats.len(),
            self.unread_emails
        );

        Ok(())
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
                            m.message_type.as_deref() == Some("RichText/Html")
                                || m.message_type.as_deref() == Some("Text")
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

    pub async fn send_message(&mut self) -> Result<()> {
        if self.input.is_empty() {
            return Ok(());
        }

        if let Some(chat_id) = &self.current_chat_id {
            // Convert newlines to <br> for multi-line messages
            let escaped = html_escape(&self.input);
            let with_breaks = escaped.replace('\n', "<br>");
            let content = format!("<p>{}</p>", with_breaks);
            match self.client.send_message(chat_id, &content, None).await {
                Ok(_) => {
                    self.status_message = "Message sent! Refreshing...".to_string();
                    self.clear_input();
                    // Delay to let the API process the message (increased for reliability)
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    // Reload messages
                    self.load_messages().await?;
                }
                Err(e) => {
                    self.status_message = format!("Send failed: {}", e);
                }
            }
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

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
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
                                app.status_message = "j/k: navigate | Enter: select | i: compose | Tab: switch panel | r: refresh | q: quit".to_string();
                            }
                            KeyCode::Char('j') | KeyCode::Down => match app.active_panel {
                                Panel::Chats => app.next_chat(),
                                Panel::Messages => app.next_message(),
                                _ => {}
                            },
                            KeyCode::Char('k') | KeyCode::Up => match app.active_panel {
                                Panel::Chats => app.previous_chat(),
                                Panel::Messages => app.previous_message(),
                                _ => {}
                            },
                            KeyCode::Char('g') => {
                                // Go to top
                                match app.active_panel {
                                    Panel::Chats => app.selected_chat = 0,
                                    Panel::Messages => app.selected_message = 0,
                                    _ => {}
                                }
                            }
                            KeyCode::Char('G') => {
                                // Go to bottom
                                match app.active_panel {
                                    Panel::Chats => {
                                        app.selected_chat = app.chats.len().saturating_sub(1)
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
                                    app.load_messages().await?;
                                }
                            }
                            KeyCode::Char('i') => {
                                app.mode = Mode::Insert;
                                app.active_panel = Panel::Input;
                                app.status_message =
                                    "-- INSERT -- (Esc to cancel, Enter to send)".to_string();
                            }
                            KeyCode::Char('r') => {
                                app.load_data().await?;
                                if app.current_chat_id.is_some() {
                                    app.load_messages().await?;
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
                            KeyCode::Enter => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    // Shift+Enter: insert newline
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
                                if key.modifiers.contains(KeyModifiers::ALT) {
                                    // Alt+Left: move word left
                                    app.move_cursor_word_left();
                                } else {
                                    app.move_cursor_left();
                                }
                            }
                            KeyCode::Right => {
                                if key.modifiers.contains(KeyModifiers::ALT) {
                                    // Alt+Right: move word right
                                    app.move_cursor_word_right();
                                } else {
                                    app.move_cursor_right();
                                }
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
