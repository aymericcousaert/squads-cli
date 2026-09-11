//! Inline pictures, decoded off the UI thread and drawn over reserved rows.

use std::collections::HashMap;
use std::time::Duration;

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::{
    picker::{cap_parser::QueryStdioOptions, Picker, ProtocolType},
    protocol::StatefulProtocol,
    Resize,
};

/// Tallest an inline picture may be drawn, so one GIF cannot take the panel.
const MAX_ROWS: u16 = 10;
/// Widest, so a picture never crowds out the text beside it.
const MAX_COLS: u16 = 48;

/// A decoded picture, boxed because it dwarfs the other states and every
/// entry would otherwise be sized to hold one.
struct Decoded {
    protocol: StatefulProtocol,
    /// Size in cells at full resolution, before any cap is applied. Kept
    /// rather than the capped size because the cap depends on how much room
    /// the message has left.
    natural: Size,
}

/// What the renderer should put where a picture goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Slot {
    /// Reserve this much room and draw the picture over it.
    Draw(Size),
    /// Not downloaded yet, or it failed: a line of text goes here.
    Waiting,
    /// Ready, but the message has no room left for it.
    Hidden,
}

/// Room for each of a message's pictures, in the order they appear.
#[derive(Debug)]
pub struct Plan {
    pub slots: Vec<Slot>,
    /// How many ready pictures did not fit.
    pub not_shown: usize,
}

#[derive(PartialEq, Eq)]
pub enum State {
    Pending,
    Ready,
    Failed,
}

enum Entry {
    /// Queued or downloading. The message keeps showing its alt text.
    Pending,
    Ready(Box<Decoded>),
    /// Unreachable, undecodable, or not a picture. Alt text stands in.
    Failed,
}

pub struct Media {
    /// How the terminal draws pictures. Falls back to half-blocks, which work
    /// anywhere that has 24-bit colour.
    picker: Picker,
    entries: HashMap<String, Entry>,
}

/// How long to wait for the terminal to say which graphics protocol it
/// speaks. A terminal that supports one answers in a few milliseconds; the
/// crate's own default of two seconds is all spent on terminals that will
/// never answer, and it is time the UI cannot accept keys.
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);

impl Media {
    /// Start with half-blocks, which need no terminal support at all. Call
    /// `probe` to find out whether the terminal can do better.
    pub fn new() -> Self {
        Self {
            picker: Picker::halfblocks(),
            entries: HashMap::new(),
        }
    }

    /// Ask the terminal which graphics protocol it speaks.
    ///
    /// Must run with raw mode already on. The query happens on a thread that
    /// snapshots the terminal state, clears echo, and later puts the snapshot
    /// back — and on a terminal that never answers, it does that *after* this
    /// call has already returned. Probing before raw mode was enabled meant
    /// that thread restored cooked mode mid-session, and the UI silently
    /// stopped receiving keys a couple of seconds in.
    pub fn probe(&mut self) {
        let options = QueryStdioOptions {
            timeout: PROBE_TIMEOUT,
            ..QueryStdioOptions::default()
        };
        if let Ok(picker) = Picker::from_query_stdio_with_options(options) {
            self.picker = picker;
        }
    }

    /// Claim a URL for downloading. False means it is already known, so the
    /// caller must not start a second request for it.
    pub fn claim(&mut self, url: &str) -> bool {
        if self.entries.contains_key(url) {
            return false;
        }
        self.entries.insert(url.to_string(), Entry::Pending);
        true
    }

    pub fn insert(&mut self, url: &str, image: DynamicImage) {
        let natural = Resize::natural_size(&image, self.picker.font_size());
        let entry = Entry::Ready(Box::new(Decoded {
            protocol: self.picker.new_resize_protocol(image),
            natural,
        }));
        self.entries.insert(url.to_string(), entry);
    }

    pub fn fail(&mut self, url: &str) {
        self.entries.insert(url.to_string(), Entry::Failed);
    }

    /// How far along a picture is, so the message can say something useful
    /// while it waits.
    pub fn state(&self, url: &str) -> State {
        match self.entries.get(url) {
            Some(Entry::Ready(_)) => State::Ready,
            Some(Entry::Failed) => State::Failed,
            _ => State::Pending,
        }
    }

    /// The area a ready picture wants, in cells, given how many rows the
    /// message can still spare. `None` while it is pending or if it failed,
    /// which is what keeps a line of text in its place instead.
    pub fn size_within(&self, url: &str, rows_left: u16) -> Option<Size> {
        match self.entries.get(url) {
            Some(Entry::Ready(decoded)) => Some(fit(decoded.natural, rows_left.min(MAX_ROWS))),
            _ => None,
        }
    }

    /// Decide how much room each of a message's pictures gets.
    ///
    /// They share one budget, in order. A picture that is not ready yet takes
    /// a line of text instead and spends nothing; once the budget is gone the
    /// rest are only counted, so the message can say how many it is holding
    /// back rather than growing past the height of the panel.
    pub fn plan(&self, urls: &[String], budget: u16) -> Plan {
        let mut slots = Vec::with_capacity(urls.len());
        let mut left = budget;
        let mut not_shown = 0;

        for url in urls {
            match self.size_within(url, left) {
                Some(size) if left > 0 => {
                    left = left.saturating_sub(size.height);
                    slots.push(Slot::Draw(size));
                }
                Some(_) => {
                    not_shown += 1;
                    slots.push(Slot::Hidden);
                }
                None => slots.push(Slot::Waiting),
            }
        }

        Plan { slots, not_shown }
    }

    /// The pictures of a message that can be shown right now, in order.
    ///
    /// The viewer needs these rather than the whole list: a picture still
    /// downloading would give it a blank page to sit on.
    pub fn ready(&self, urls: &[String]) -> Vec<String> {
        urls.iter()
            .filter(|url| self.state(url) == State::Ready)
            .cloned()
            .collect()
    }

    /// Which graphics protocol the terminal accepted. `Halfblocks` means no
    /// real graphics support was detected, so pictures are drawn as coloured
    /// block characters and will look coarse.
    pub fn protocol_name(&self) -> &'static str {
        match self.picker.protocol_type() {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Iterm2 => "iterm2",
            ProtocolType::Halfblocks => "halfblocks",
        }
    }

    /// Drawing mutates the protocol: it caches the encoding for the area it
    /// was last asked to fill.
    pub fn protocol(&mut self, url: &str) -> Option<&mut StatefulProtocol> {
        match self.entries.get_mut(url) {
            Some(Entry::Ready(decoded)) => Some(&mut decoded.protocol),
            _ => None,
        }
    }
}

/// Scale a natural size down to at most `max_rows` tall and `MAX_COLS` wide,
/// keeping the aspect ratio. Never scales up: a small picture stays small
/// rather than turning blocky.
fn fit(natural: Size, max_rows: u16) -> Size {
    let width = natural.width.max(1) as f32;
    let height = natural.height.max(1) as f32;
    let scale = (MAX_COLS as f32 / width)
        .min(max_rows as f32 / height)
        .min(1.0);
    Size {
        width: ((width * scale) as u16).max(1),
        height: ((height * scale) as u16).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u16, height: u16) -> Size {
        Size { width, height }
    }

    /// A 1x2 image, so its natural size is taller than it is wide.
    fn tall() -> DynamicImage {
        DynamicImage::new_rgb8(1, 2)
    }

    #[test]
    fn a_small_picture_is_left_alone() {
        assert_eq!(fit(size(20, 5), MAX_ROWS), size(20, 5));
    }

    #[test]
    fn a_tall_picture_is_capped_by_height() {
        let out = fit(size(40, 80), MAX_ROWS);
        assert_eq!(out.height, MAX_ROWS);
        assert!(out.width < 40, "width must shrink with the height");
    }

    #[test]
    fn a_wide_picture_is_capped_by_width() {
        let out = fit(size(200, 20), MAX_ROWS);
        assert_eq!(out.width, MAX_COLS);
        assert!(out.height <= MAX_ROWS);
    }

    #[test]
    fn aspect_ratio_survives_the_cap() {
        let out = fit(size(100, 50), MAX_ROWS);
        // 2:1 in, 2:1 out, within a cell of rounding
        assert!((out.width as f32 / out.height as f32 - 2.0).abs() < 0.35);
    }

    #[test]
    fn a_degenerate_size_stays_drawable() {
        assert_eq!(fit(size(0, 0), MAX_ROWS), size(1, 1));
    }

    /// Three tall pictures must not add up to more than the budget, however
    /// much each of them would like.
    #[test]
    fn several_pictures_share_one_budget() {
        let mut media = Media::new();
        let urls: Vec<String> = (0..3).map(|i| format!("u{}", i)).collect();
        for url in &urls {
            media.entries.insert(
                url.clone(),
                Entry::Ready(Box::new(Decoded {
                    protocol: media.picker.new_resize_protocol(tall()),
                    natural: size(40, 80),
                })),
            );
        }

        let plan = media.plan(&urls, 16);
        let drawn: u16 = plan
            .slots
            .iter()
            .map(|s| match s {
                Slot::Draw(size) => size.height,
                _ => 0,
            })
            .sum();
        assert!(drawn <= 16, "budget overspent: {} rows", drawn);
        assert_eq!(plan.slots.len(), 3, "every picture gets a slot");
        assert!(
            plan.slots.iter().any(|s| matches!(s, Slot::Draw(_))),
            "at least one must be drawn"
        );
    }

    #[test]
    fn every_ready_picture_is_offered_to_the_viewer() {
        let mut media = Media::new();
        let urls: Vec<String> = (0..3).map(|i| format!("u{}", i)).collect();
        // two ready, one still on its way
        for url in [&urls[0], &urls[2]] {
            media.entries.insert(
                url.clone(),
                Entry::Ready(Box::new(Decoded {
                    protocol: media.picker.new_resize_protocol(tall()),
                    natural: size(40, 80),
                })),
            );
        }
        media.entries.insert(urls[1].clone(), Entry::Pending);

        assert_eq!(media.ready(&urls), vec!["u0".to_string(), "u2".to_string()]);
    }

    #[test]
    fn a_failed_picture_is_not_offered() {
        let mut media = Media::new();
        media.entries.insert("bad".to_string(), Entry::Failed);
        assert!(media.ready(&["bad".to_string()]).is_empty());
    }

    /// A picture still downloading holds a line of text and spends nothing.
    #[test]
    fn a_waiting_picture_costs_no_budget() {
        let media = Media::new();
        let urls = vec!["never-claimed".to_string()];
        let plan = media.plan(&urls, 16);
        assert_eq!(plan.slots, vec![Slot::Waiting]);
        assert_eq!(plan.not_shown, 0);
    }

    #[test]
    fn a_message_with_no_pictures_plans_nothing() {
        let plan = Media::new().plan(&[], 16);
        assert!(plan.slots.is_empty());
        assert_eq!(plan.not_shown, 0);
    }

    /// Several pictures in one message share the room, so each is asked to
    /// fit whatever rows are left.
    #[test]
    fn a_tight_budget_shrinks_the_picture() {
        let out = fit(size(40, 80), 4);
        assert_eq!(out.height, 4);
        assert!(out.width <= 2, "aspect kept: half as wide as tall");
    }

    #[test]
    fn no_room_left_still_yields_a_drawable_cell() {
        let out = fit(size(40, 80), 0);
        assert_eq!(out, size(1, 1));
    }
}
