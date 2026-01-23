use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use tokio::sync::Mutex;

use crate::api::TeamsClient;
use crate::config::Config;
use crate::types::{Chat, Message, MailMessage};

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
                self.unread_messages = details.chats.iter()
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
                self.unread_emails = msgs.value.iter()
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
                    self.messages = convs.messages
                        .into_iter()
                        .filter(|m| {
                            m.message_type.as_deref() == Some("RichText/Html")
                            || m.message_type.as_deref() == Some("Text")
                        })
                        .take(50)
                        .collect();
                    self.messages.reverse(); // Show oldest first
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
            let content = format!("<p>{}</p>", html_escape(&self.input));
            match self.client.send_message(chat_id, &content, None).await {
                Ok(_) => {
                    self.status_message = "Message sent!".to_string();
                    self.input.clear();
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
            self.selected_chat = self.selected_chat.checked_sub(1).unwrap_or(self.chats.len() - 1);
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
                            KeyCode::Char('j') | KeyCode::Down => {
                                match app.active_panel {
                                    Panel::Chats => app.next_chat(),
                                    Panel::Messages => app.next_message(),
                                    _ => {}
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                match app.active_panel {
                                    Panel::Chats => app.previous_chat(),
                                    Panel::Messages => app.previous_message(),
                                    _ => {}
                                }
                            }
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
                                    Panel::Chats => app.selected_chat = app.chats.len().saturating_sub(1),
                                    Panel::Messages => app.selected_message = app.messages.len().saturating_sub(1),
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
                                app.status_message = "-- INSERT -- (Esc to cancel, Enter to send)".to_string();
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
                                app.send_message().await?;
                                app.mode = Mode::Normal;
                            }
                            KeyCode::Backspace => {
                                app.input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.input.push(c);
                            }
                            _ => {}
                        }
                    }
                    Mode::Command => {
                        match key.code {
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
                                        app.status_message = format!("{} unread emails", app.unread_emails);
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
                        }
                    }
                }

                if app.should_quit {
                    break;
                }
            }
        }
    }

    Ok(())
}
