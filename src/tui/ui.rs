use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use super::app::{App, LeftPanelView, Mode, Panel};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Main content
            Constraint::Length(3), // Input
            Constraint::Length(1), // Status bar
        ])
        .split(f.area());

    // Split main area into chats and messages
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // Chats
            Constraint::Percentage(70), // Messages
        ])
        .split(chunks[0]);

    draw_chats(f, app, main_chunks[0]);
    draw_messages(f, app, main_chunks[1]);
    draw_input(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);
}

fn draw_chats(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == Panel::Chats;
    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Calculate max width for names (area width - borders - unread marker)
    let max_name_width = area.width.saturating_sub(5) as usize;

    match app.left_panel_view {
        LeftPanelView::Chats => {
            let items: Vec<ListItem> = app
                .chats
                .iter()
                .enumerate()
                .map(|(i, chat)| {
                    let title = app.get_chat_display_name(chat);

                    let unread_marker = if chat.is_read == Some(false) {
                        "● "
                    } else {
                        "  "
                    };
                    let display = format!("{}{}", unread_marker, truncate(&title, max_name_width));

                    let style = if i == app.selected_chat && is_active {
                        Style::default()
                            .bg(Color::Cyan)
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD)
                    } else if i == app.selected_chat {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else if chat.is_read == Some(false) {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };

                    ListItem::new(display).style(style)
                })
                .collect();

            let title = format!(" [1] Chats ({}) ", app.chats.len());
            let chats = List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(title),
            );

            f.render_widget(chats, area);
        }
        LeftPanelView::Channels => {
            // Build flat list of team > channel
            let mut items: Vec<ListItem> = Vec::new();

            for (team_idx, team) in app.teams.iter().enumerate() {
                // Team header
                let team_style = Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD);
                items.push(ListItem::new(format!("▼ {}", team.display_name)).style(team_style));

                // Channels under this team
                for (chan_idx, channel) in team.channels.iter().enumerate() {
                    let is_selected =
                        team_idx == app.selected_team && chan_idx == app.selected_channel;
                    let style = if is_selected && is_active {
                        Style::default()
                            .bg(Color::Cyan)
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD)
                    } else if is_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        Style::default()
                    };

                    let display =
                        format!("  # {}", truncate(&channel.display_name, max_name_width));
                    items.push(ListItem::new(display).style(style));
                }
            }

            let channel_count: usize = app.teams.iter().map(|t| t.channels.len()).sum();
            let title = format!(" [2] Channels ({}) ", channel_count);
            let channels = List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(title),
            );

            f.render_widget(channels, area);
        }
    }
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == Panel::Messages;
    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let chat_title = app
        .chats
        .get(app.selected_chat)
        .and_then(|c| c.title.clone())
        .unwrap_or_else(|| "Messages".to_string());

    if app.messages.is_empty() {
        let empty = Paragraph::new("No messages. Press Enter on a chat to load messages.")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(format!(" {} ", chat_title)),
            );
        f.render_widget(empty, area);
        return;
    }

    // Calculate available width for content (area width - borders - padding)
    let content_width = area.width.saturating_sub(4) as usize;
    // Header takes about 25 chars (time + sender)
    let msg_width = content_width.saturating_sub(25);

    let items: Vec<ListItem> = app
        .messages
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            let sender = msg
                .im_display_name
                .clone()
                .or_else(|| msg.from.clone())
                .unwrap_or_else(|| "Unknown".to_string());

            let content = msg
                .content
                .clone()
                .map(|c| strip_html(&c))
                .unwrap_or_default();

            let time = msg
                .original_arrival_time
                .clone()
                .map(|t| {
                    // Extract just the time part
                    if t.len() > 16 {
                        t[11..16].to_string()
                    } else {
                        t
                    }
                })
                .unwrap_or_default();

            let is_self = msg
                .from
                .as_ref()
                .map(|f| f.contains("orgid:"))
                .unwrap_or(false);

            let sender_style = if is_self {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD)
            };

            let style = if i == app.selected_message && is_active {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            // Wrap content into multiple lines if needed
            let content_lines = wrap_text(&content, msg_width.max(20));
            let mut lines: Vec<Line> = Vec::new();

            for (line_idx, line_content) in content_lines.iter().enumerate() {
                if line_idx == 0 {
                    // First line with time and sender
                    lines.push(Line::from(vec![
                        Span::styled(format!("{} ", time), Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{}: ", truncate(&sender, 15)), sender_style),
                        Span::raw(line_content.clone()),
                    ]));
                } else {
                    // Continuation lines with indent
                    lines.push(Line::from(vec![
                        Span::raw("                         "), // Indent to align with message content
                        Span::raw(line_content.clone()),
                    ]));
                }
            }

            ListItem::new(lines).style(style)
        })
        .collect();

    // Use ListState for proper scrolling
    let mut list_state = ListState::default();
    list_state.select(Some(app.selected_message));

    let messages = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(
                    " {} ({}) ",
                    truncate(&chat_title, 30),
                    app.messages.len()
                )),
        )
        .highlight_style(Style::default()); // Already handled per-item

    f.render_stateful_widget(messages, area, &mut list_state);
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.chars().count() + 1 + word.chars().count() <= max_width {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == Panel::Input || app.mode == Mode::Insert;
    let border_style = if is_active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let input_title = match app.mode {
        Mode::Insert => " Compose (Enter: send, Shift+Enter: newline, Esc: cancel) ",
        Mode::Command => " Command ",
        Mode::Normal => " Press 'i' to compose ",
    };

    let display_text = match app.mode {
        Mode::Command => format!(":{}", app.command_input),
        _ => app.input.clone(),
    };

    let input = Paragraph::new(display_text.as_str())
        .style(Style::default())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(input_title),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(input, area);

    // Show cursor in insert mode
    if app.mode == Mode::Insert {
        // Calculate cursor position accounting for newlines
        let chars: Vec<char> = app.input.chars().collect();
        let chars_before_cursor = &chars[..app.input_cursor.min(chars.len())];

        // Find the last newline before cursor to determine row and column
        let mut row = 0u16;
        let mut col = 0u16;
        for c in chars_before_cursor {
            if *c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }

        // Account for wrapping within the input area width
        let inner_width = area.width.saturating_sub(2);
        if inner_width > 0 {
            row += col / inner_width;
            col %= inner_width;
        }

        f.set_cursor_position((area.x + col + 1, area.y + row + 1));
    } else if app.mode == Mode::Command {
        let char_count = app.command_input.chars().count() as u16;
        f.set_cursor_position((area.x + char_count + 2, area.y + 1));
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mode_indicator = match app.mode {
        Mode::Normal => "",
        Mode::Insert => " INSERT ",
        Mode::Command => " COMMAND ",
    };

    let mode_style = match app.mode {
        Mode::Normal => Style::default(),
        Mode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
        Mode::Command => Style::default().bg(Color::Yellow).fg(Color::Black),
    };

    let unread_info = if app.unread_emails > 0 || app.unread_messages > 0 {
        format!(
            " | 📧 {} unread | 💬 {} unread",
            app.unread_emails, app.unread_messages
        )
    } else {
        String::new()
    };

    let loading_indicator = if app.loading { " ⏳ " } else { "" };

    let status = Line::from(vec![
        Span::styled(mode_indicator, mode_style),
        Span::raw(loading_indicator),
        Span::raw(&app.status_message),
        Span::styled(unread_info, Style::default().fg(Color::Yellow)),
    ]);

    let status_bar =
        Paragraph::new(status).style(Style::default().bg(Color::DarkGray).fg(Color::White));

    f.render_widget(status_bar, area);
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
    // Handle blockquotes first - extract quoted content
    let mut result = s.to_string();

    // Simple blockquote handling: extract content between <blockquote> and </blockquote>
    if result.contains("<blockquote") {
        // Find and replace blockquotes with "> quote" format
        let mut processed = String::new();
        let mut remaining = result.as_str();

        while let Some(start_idx) = remaining.find("<blockquote") {
            // Add content before blockquote
            processed.push_str(&remaining[..start_idx]);

            // Find end of blockquote
            if let Some(end_idx) = remaining[start_idx..].find("</blockquote>") {
                let quote_content = &remaining[start_idx..start_idx + end_idx];
                // Strip tags from quote content and add as "> quote"
                let clean_quote = strip_tags_only(quote_content);
                if !clean_quote.trim().is_empty() {
                    processed.push_str(&format!("「{}」 ", truncate_quote(&clean_quote, 40)));
                }
                remaining = &remaining[start_idx + end_idx + 13..]; // 13 = </blockquote>
            } else {
                remaining = &remaining[start_idx..];
                break;
            }
        }
        processed.push_str(remaining);
        result = processed;
    }

    // Now strip remaining HTML tags
    strip_tags_only(&result)
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_tags_only(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '\n' | '\r' => {
                if !in_tag {
                    result.push(' ');
                }
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

fn truncate_quote(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > max_len {
        let truncated: String = chars[..max_len.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        trimmed.to_string()
    }
}
