use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};

use ratatui_image::{FilterType, Resize, StatefulImage};

use crate::cli::utils::message;

use super::app::{App, LeftPanelView, Mode, Panel};
use super::media;
use super::theme;

/// Rows kept visible past the selection, so the view starts moving before the
/// cursor reaches the edge of the panel.
const SCROLL_PADDING: usize = 2;
/// Rows one message may spend on pictures, however many it carries.
const PICTURE_ROWS_PER_MESSAGE: u16 = 16;
/// Sidebar width once there is room for it; narrow terminals get a third.
const SIDEBAR_WIDTH: u16 = 34;

pub fn draw(f: &mut Frame, app: &mut App) {
    let newline_count = app.input.matches('\n').count();
    let input_height = (newline_count as u16 + 2).clamp(2, 10);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // header
            Constraint::Min(6),               // sidebar + conversation
            Constraint::Length(input_height), // compose
            Constraint::Length(1),            // status
        ])
        .split(f.area());

    let sidebar_width = if rows[1].width >= 100 {
        SIDEBAR_WIDTH
    } else {
        (rows[1].width / 3).max(18)
    };
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Min(20)])
        .split(rows[1]);

    app.hits.sidebar = body[0];
    app.hits.compose = rows[2];

    draw_header(f, app, rows[0]);
    draw_sidebar(f, app, body[0]);
    draw_conversation(f, app, body[1]);
    draw_compose(f, app, rows[2]);
    draw_status(f, app, rows[3]);

    // Last, so it covers everything drawn above it.
    if app.preview.is_some() {
        draw_preview(f, app, f.area());
    }
}

/// Which item each painted row belongs to, given the offset the list widget
/// settled on and the height of every item. Items can be several lines tall, so
/// a click cannot be mapped by arithmetic alone.
fn row_map(area: Rect, offset: usize, heights: &[usize]) -> Vec<(u16, usize)> {
    let mut map = Vec::new();
    let bottom = area.y.saturating_add(area.height);
    let mut row = area.y;
    for (index, height) in heights.iter().enumerate().skip(offset) {
        for _ in 0..*height {
            if row >= bottom {
                return map;
            }
            map.push((row, index));
            row = row.saturating_add(1);
        }
    }
    map
}

/// Paint a region in one tone and hand back the area inside its padding.
/// Regions are separated by tone alone, which is why no panel has a border.
fn surface(f: &mut Frame, area: Rect, style: Style, padding: Padding) -> Rect {
    let block = Block::default().style(style).padding(padding);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::SURFACE),
        Padding::horizontal(2),
    );

    let mut left = vec![
        Span::styled(
            "squads",
            Style::default()
                .fg(theme::BRAND_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
    ];
    if !app.chats.is_empty() {
        left.push(Span::styled(
            format!("{} chats", app.chats.len()),
            Style::default().fg(theme::TEXT_DIM),
        ));
        left.push(Span::styled(
            format!("  ·  {} channels", channel_count(app)),
            Style::default().fg(theme::TEXT_FAINT),
        ));
    }

    let right = counters(app);
    let used: usize = left.iter().chain(right.iter()).map(span_width).sum();
    let gap = (inner.width as usize).saturating_sub(used);

    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// Unread counts, plus the spinner while a refresh is in flight.
fn counters(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if app.unread_messages > 0 {
        spans.push(Span::styled(
            format!("● {} ", app.unread_messages),
            Style::default().fg(theme::ALERT),
        ));
    }
    if app.unread_emails > 0 {
        spans.push(Span::styled(
            format!("✉ {} ", app.unread_emails),
            Style::default().fg(theme::TEXT_DIM),
        ));
    }
    if app.loading {
        let frame = theme::SPINNER[(app.tick as usize) % theme::SPINNER.len()];
        spans.push(Span::styled(
            frame.to_string(),
            Style::default().fg(theme::BRAND_TEXT),
        ));
    }
    spans
}

fn channel_count(app: &App) -> usize {
    app.teams.iter().map(|t| t.channels.len()).sum()
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::BG),
        Padding::new(1, 1, 1, 0),
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);

    app.hits.tabs = rows[0];
    draw_tabs(f, app, rows[0]);
    match app.left_panel_view {
        LeftPanelView::Chats => draw_chat_rows(f, app, rows[1]),
        LeftPanelView::Channels => draw_channel_rows(f, app, rows[1]),
    }
}

/// Chats / Channels, with the active one accented. Replaces the `[1]` and
/// `[2]` prefixes that used to sit in the panel titles.
fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let tab = |label: &'static str, selected: bool| {
        if selected {
            Span::styled(
                label,
                Style::default()
                    .fg(theme::BRAND_TEXT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(label, Style::default().fg(theme::TEXT_FAINT))
        }
    };
    let on_chats = app.left_panel_view == LeftPanelView::Chats;
    let line = Line::from(vec![
        tab("Chats", on_chats),
        Span::styled("   ", Style::default()),
        tab("Channels", !on_chats),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_chat_rows(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width as usize;
    let is_active = app.active_panel == Panel::Chats;

    let items: Vec<ListItem> = app
        .chats
        .iter()
        .enumerate()
        .map(|(i, chat)| {
            let selected = i == app.selected_chat;
            let unread = chat.is_read == Some(false);
            let name = app.get_chat_display_name(chat);

            let text = Style::default().fg(if selected || unread {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            });
            let text = if unread {
                text.add_modifier(Modifier::BOLD)
            } else {
                text
            };

            ListItem::new(Line::from(row_spans(
                &name, width, selected, is_active, unread, text,
            )))
            .style(row_background(selected, is_active))
        })
        .collect();

    app.chats_state.select(Some(app.selected_chat));
    let count = items.len();
    f.render_stateful_widget(
        List::new(items).scroll_padding(SCROLL_PADDING),
        area,
        &mut app.chats_state,
    );
    app.hits.chat_rows = row_map(area, app.chats_state.offset(), &vec![1; count]);
}

fn draw_channel_rows(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width as usize;
    let is_active = app.active_panel == Panel::Chats;
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_flat = None;
    // A team header is not selectable, so it maps to nothing.
    let mut targets: Vec<Option<(usize, usize)>> = Vec::new();

    for (team_index, team) in app.teams.iter().enumerate() {
        items.push(
            ListItem::new(Line::from(Span::styled(
                truncate(&team.display_name, width),
                Style::default()
                    .fg(theme::TEXT_FAINT)
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(theme::BG)),
        );
        targets.push(None);

        for (channel_index, channel) in team.channels.iter().enumerate() {
            let selected = team_index == app.selected_team && channel_index == app.selected_channel;
            if selected {
                selected_flat = Some(items.len());
            }
            let text = Style::default().fg(if selected {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            });
            items.push(
                ListItem::new(Line::from(row_spans(
                    &format!("# {}", channel.display_name),
                    width,
                    selected,
                    is_active,
                    false,
                    text,
                )))
                .style(row_background(selected, is_active)),
            );
            targets.push(Some((team_index, channel_index)));
        }
    }

    app.channels_state.select(selected_flat);
    let count = items.len();
    f.render_stateful_widget(
        List::new(items).scroll_padding(SCROLL_PADDING),
        area,
        &mut app.channels_state,
    );
    app.hits.channel_rows = row_map(area, app.channels_state.offset(), &vec![1; count])
        .into_iter()
        .filter_map(|(row, index)| targets.get(index).copied().flatten().map(|t| (row, t)))
        .collect();
}

/// One sidebar row: selection bar, name, and the unread dot pushed right.
fn row_spans(
    name: &str,
    width: usize,
    selected: bool,
    panel_active: bool,
    unread: bool,
    text: Style,
) -> Vec<Span<'static>> {
    let bar = if selected {
        Span::styled(
            theme::SELECTION_BAR,
            Style::default().fg(if panel_active {
                theme::BRAND_TEXT
            } else {
                theme::BRAND_MUTED
            }),
        )
    } else {
        Span::raw(" ")
    };

    let badge = if unread { "●" } else { " " };
    let room = width.saturating_sub(4);
    let name = truncate(name, room);
    let pad = room.saturating_sub(name.chars().count());

    vec![
        bar,
        Span::raw(" "),
        Span::styled(name, text),
        Span::raw(" ".repeat(pad + 1)),
        Span::styled(badge.to_string(), Style::default().fg(theme::ALERT)),
    ]
}

fn row_background(selected: bool, panel_active: bool) -> Style {
    if selected && panel_active {
        Style::default().bg(theme::RAISED)
    } else if selected {
        Style::default().bg(theme::SURFACE)
    } else {
        Style::default().bg(theme::BG)
    }
}

fn draw_conversation(f: &mut Frame, app: &mut App, area: Rect) {
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::SURFACE),
        Padding::new(2, 2, 1, 0),
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);

    // What is loaded, not what is highlighted: the sidebar cursor moves freely
    // while a conversation stays open.
    let title = app
        .open_title
        .clone()
        .unwrap_or_else(|| "No conversation".to_string());
    let mut heading = vec![Span::styled(
        truncate(&title, rows[0].width as usize),
        Style::default()
            .fg(theme::TEXT)
            .add_modifier(Modifier::BOLD),
    )];
    if !app.messages.is_empty() {
        heading.push(Span::styled(
            format!("   {} messages", app.messages.len()),
            Style::default().fg(theme::TEXT_FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(heading)), rows[0]);

    app.hits.messages = rows[1];
    if app.messages.is_empty() {
        app.hits.message_rows.clear();
        let hint = Paragraph::new("Click a chat, or press Enter to read it.")
            .style(Style::default().fg(theme::TEXT_FAINT));
        f.render_widget(hint, rows[1]);
        return;
    }

    draw_messages(f, app, rows[1]);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let is_active = app.active_panel == Panel::Messages;
    let width = area.width as usize;
    let mut previous_sender: Option<String> = None;
    let mut reserved: Vec<Reserved> = Vec::new();

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
            // Repeat the name only when the speaker changes, the way a chat
            // client groups a run of messages under one heading.
            let same_speaker = previous_sender.as_deref() == Some(sender.as_str());
            previous_sender = Some(sender.clone());

            let from_me = app
                .my_user_id
                .as_deref()
                .is_some_and(|id| msg.from.as_deref().is_some_and(|from| from.contains(id)));
            let time = msg
                .original_arrival_time
                .as_deref()
                .map(|t| if t.len() > 16 { &t[11..16] } else { t })
                .unwrap_or("")
                .to_string();

            let selected = i == app.selected_message && is_active;
            let mut lines = Vec::new();
            // A blank row ahead of each new speaker is what separates one run
            // of messages from the next.
            if !same_speaker && i > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(heading_spans(
                &sender,
                &time,
                width,
                same_speaker,
                from_me,
            )));

            let content_starts = lines.len();
            let html = msg.content.as_deref().unwrap_or_default();
            let body = message::text(html);
            if !body.trim().is_empty() {
                for line in wrap_text(&body, width) {
                    lines.push(Line::from(Span::styled(
                        line,
                        Style::default().fg(theme::TEXT),
                    )));
                }
            }

            // A picture that has arrived gets blank rows reserved for it, and is
            // drawn over them once the list has been rendered. One that has not
            // keeps its alt text, so nothing jumps until there is something to
            // put there.
            // The pictures of one message share a row budget, so a message
            // holding several cannot grow past the panel: the list scrolls by
            // whole messages, and anything below the fold would be unreachable.
            let pictures = message::pictures(html);
            let urls: Vec<String> = pictures.iter().map(|(url, _)| url.clone()).collect();
            let plan = app.media.plan(&urls, PICTURE_ROWS_PER_MESSAGE);
            let faint = Style::default()
                .fg(theme::TEXT_FAINT)
                .add_modifier(Modifier::ITALIC);

            for (slot, (url, alt)) in plan.slots.iter().zip(&pictures) {
                match slot {
                    media::Slot::Draw(size) => {
                        reserved.push(Reserved {
                            url: url.clone(),
                            item: i,
                            row: lines.len() as u16,
                            size: *size,
                        });
                        lines.extend((0..size.height).map(|_| Line::default()));
                    }
                    media::Slot::Waiting => lines.push(Line::from(Span::styled(
                        placeholder(&app.media.state(url), alt),
                        faint,
                    ))),
                    media::Slot::Hidden => {}
                }
            }

            if plan.not_shown > 0 {
                lines.push(Line::from(Span::styled(
                    format!("+{} more, v to view", plan.not_shown),
                    faint,
                )));
            }

            if lines.len() == content_starts {
                // Nothing but a heading: a reaction, or markup that stripped to
                // nothing. Say so rather than leaving a bare timestamp.
                lines.push(Line::from(Span::styled(
                    "no content",
                    Style::default()
                        .fg(theme::TEXT_FAINT)
                        .add_modifier(Modifier::ITALIC),
                )));
            }

            let background = if selected {
                Style::default().bg(theme::RAISED)
            } else {
                Style::default().bg(theme::SURFACE)
            };
            ListItem::new(lines).style(background)
        })
        .collect();

    app.messages_state.select(Some(app.selected_message));
    let heights: Vec<usize> = items.iter().map(|item| item.height()).collect();
    f.render_stateful_widget(
        List::new(items).scroll_padding(SCROLL_PADDING),
        area,
        &mut app.messages_state,
    );
    let offset = app.messages_state.offset();
    app.hits.message_rows = row_map(area, offset, &heights);
    draw_pictures(f, app, area, offset, &heights, &reserved);
}

/// The picture viewer: one picture, as large as the window allows.
///
/// Unlike the inline copy this scales up as well as down, since asking to
/// enlarge a small picture should actually enlarge it.
fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let Some((url, (shown, total))) = app
        .preview
        .as_ref()
        .map(|p| (p.current().to_string(), p.position()))
    else {
        return;
    };

    f.render_widget(Clear, area);
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::BG),
        Padding::new(2, 2, 1, 0),
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if let Some(protocol) = app.media.protocol(&url) {
        let image = StatefulImage::default().resize(Resize::Scale(Some(FilterType::Lanczos3)));
        f.render_stateful_widget(image, rows[0], protocol);
    }

    let key = Style::default().fg(theme::BRAND_TEXT);
    let label = Style::default().fg(theme::TEXT_FAINT);
    let mut caption = Vec::new();
    if total > 1 {
        caption.push(Span::styled(format!("{} of {}", shown, total), label));
        caption.push(Span::styled("   ", label));
        caption.push(Span::styled("n", key));
        caption.push(Span::styled("/", label));
        caption.push(Span::styled("p", key));
        caption.push(Span::styled(" next, previous   ", label));
    }
    caption.push(Span::styled("Esc", key));
    caption.push(Span::styled(" close", label));
    f.render_widget(Paragraph::new(Line::from(caption)), rows[1]);
}

/// What to show in place of a picture that is not drawable yet. Never echoes a
/// useless alt like "image": it says what the app is doing instead.
fn placeholder(state: &media::State, alt: &str) -> String {
    let named = message::useful_alt(alt);
    match (state, named) {
        (media::State::Failed, Some(alt)) => format!("{} — could not load", alt),
        (media::State::Failed, None) => "picture could not be loaded".to_string(),
        (_, Some(alt)) => format!("{} — loading", alt),
        (_, None) => "loading picture".to_string(),
    }
}

/// Where a picture was given room inside a message.
struct Reserved {
    url: String,
    /// Index of the message in the list.
    item: usize,
    /// Row of the reserved block, counted from the top of that message.
    row: u16,
    size: Size,
}

/// Draw the pictures over the rows reserved for them.
///
/// This runs after the list so the graphics land on top of the blank rows
/// instead of being painted over by them. The list always starts at an item
/// boundary, so walking the heights from the offset gives each message's exact
/// first row.
fn draw_pictures(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    offset: usize,
    heights: &[usize],
    reserved: &[Reserved],
) {
    if reserved.is_empty() {
        return;
    }
    let bottom = area.y.saturating_add(area.height);
    let mut top = area.y;

    for (item, height) in heights.iter().enumerate().skip(offset) {
        if top >= bottom {
            break;
        }
        for block in reserved.iter().filter(|r| r.item == item) {
            let y = top.saturating_add(block.row);
            if y >= bottom {
                continue;
            }
            let rect = Rect {
                x: area.x,
                y,
                width: block.size.width.min(area.width),
                // Clip rather than overflow where a picture meets the edge.
                height: block.size.height.min(bottom - y),
            };
            if let Some(protocol) = app.media.protocol(&block.url) {
                // Not `Resize::Fit(None)`: that defaults to nearest-neighbour,
                // which aliases badly on the downscale every picture gets here.
                let image =
                    StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3)));
                f.render_stateful_widget(image, rect, protocol);
            }
        }
        top = top.saturating_add(*height as u16);
    }
}

/// Sender on the left, time on the right. A follow-up from the same speaker
/// keeps only the time, which is what gives each run its gap.
fn heading_spans(
    sender: &str,
    time: &str,
    width: usize,
    same_speaker: bool,
    from_me: bool,
) -> Vec<Span<'static>> {
    let name = if same_speaker {
        String::new()
    } else {
        truncate(sender, width.saturating_sub(time.chars().count() + 2))
    };
    let pad = width
        .saturating_sub(name.chars().count())
        .saturating_sub(time.chars().count());

    vec![
        Span::styled(
            name,
            Style::default()
                .fg(if from_me {
                    theme::TEXT_DIM
                } else {
                    theme::BRAND_TEXT
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(time.to_string(), Style::default().fg(theme::TEXT_FAINT)),
    ]
}

fn draw_compose(f: &mut Frame, app: &App, area: Rect) {
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::RAISED),
        Padding::new(2, 2, 0, 0),
    );

    match app.mode {
        Mode::Normal => {
            let hint = Line::from(vec![
                Span::styled("i", Style::default().fg(theme::BRAND_TEXT)),
                Span::styled(" compose   ", Style::default().fg(theme::TEXT_FAINT)),
                Span::styled("r", Style::default().fg(theme::BRAND_TEXT)),
                Span::styled(" refresh   ", Style::default().fg(theme::TEXT_FAINT)),
                Span::styled("?", Style::default().fg(theme::BRAND_TEXT)),
                Span::styled(" help   ", Style::default().fg(theme::TEXT_FAINT)),
                Span::styled("q", Style::default().fg(theme::BRAND_TEXT)),
                Span::styled(" quit", Style::default().fg(theme::TEXT_FAINT)),
            ]);
            f.render_widget(Paragraph::new(hint), inner);
        }
        Mode::Insert => {
            let text = Paragraph::new(app.input.as_str())
                .style(Style::default().fg(theme::TEXT))
                .wrap(Wrap { trim: false });
            f.render_widget(text, inner);
            place_cursor(f, app, inner);
        }
        Mode::Command => {
            let line = Line::from(vec![
                Span::styled(":", Style::default().fg(theme::BRAND_TEXT)),
                Span::styled(app.command_input.clone(), Style::default().fg(theme::TEXT)),
            ]);
            f.render_widget(Paragraph::new(line), inner);
            let column = app.command_input.chars().count() as u16 + 1;
            f.set_cursor_position((inner.x + column, inner.y));
        }
    }
}

fn place_cursor(f: &mut Frame, app: &App, inner: Rect) {
    let chars: Vec<char> = app.input.chars().collect();
    let mut row = 0u16;
    let mut column = 0u16;
    for c in &chars[..app.input_cursor.min(chars.len())] {
        if *c == '\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    // Width is zero on a degenerate layout, so divide defensively.
    if let (Some(wrapped), Some(rest)) = (
        column.checked_div(inner.width),
        column.checked_rem(inner.width),
    ) {
        row += wrapped;
        column = rest;
    }
    f.set_cursor_position((inner.x + column, inner.y + row));
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let inner = surface(
        f,
        area,
        Style::default().bg(theme::SURFACE),
        Padding::horizontal(2),
    );

    let mut spans = Vec::new();
    match app.mode {
        Mode::Insert => spans.push(Span::styled(
            " INSERT ",
            Style::default()
                .bg(theme::BRAND)
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )),
        Mode::Command => spans.push(Span::styled(
            " COMMAND ",
            Style::default()
                .bg(theme::BRAND_MUTED)
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )),
        Mode::Normal => {}
    }
    if !spans.is_empty() {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        app.status_message.clone(),
        Style::default().fg(theme::TEXT_DIM),
    ));

    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.chars().count() + 1 + word.chars().count() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        lines.push(current);
    }
    lines.retain(|line| !line.is_empty());
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_len {
        let kept: String = chars[..max_len.saturating_sub(1)].iter().collect();
        format!("{}…", kept)
    } else {
        s.to_string()
    }
}

fn span_width(span: &Span) -> usize {
    span.content.chars().count()
}
