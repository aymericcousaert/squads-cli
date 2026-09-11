//! The Teams palette: Fluent's brand purple over its dark neutrals.
//!
//! Fixed RGB rather than the terminal's 16 colours, so the UI looks the same
//! everywhere and can rely on exact tone steps to separate regions instead of
//! drawing a border around each one.

use ratatui::style::Color;

/// Primary Teams purple. Readable as a background behind white text, but too
/// dark to use as text on a dark surface, which is what `BRAND_TEXT` is for.
pub const BRAND: Color = Color::Rgb(0x5B, 0x5F, 0xC7);
/// A lighter step of the same ramp, for accented text and the selection bar.
pub const BRAND_TEXT: Color = Color::Rgb(0x9E, 0xA2, 0xF5);
/// A darker step, for rules and other lines that should barely register.
pub const BRAND_MUTED: Color = Color::Rgb(0x44, 0x47, 0x91);

/// Page background, and the sidebar.
pub const BG: Color = Color::Rgb(0x1F, 0x1F, 0x1F);
/// One step up: the conversation area, header and status bar.
pub const SURFACE: Color = Color::Rgb(0x26, 0x26, 0x26);
/// Two steps up: the selected row and the compose box.
pub const RAISED: Color = Color::Rgb(0x33, 0x33, 0x33);

pub const TEXT: Color = Color::Rgb(0xF2, 0xF2, 0xF2);
pub const TEXT_DIM: Color = Color::Rgb(0xAD, 0xAD, 0xAD);
pub const TEXT_FAINT: Color = Color::Rgb(0x70, 0x70, 0x70);

/// Teams' urgent red, used here for the unread badge.
pub const ALERT: Color = Color::Rgb(0xC4, 0x31, 0x4B);

/// Braille spinner, advanced once per redraw so waiting looks continuous.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The bar marking the selected row, in place of a full-width colour block.
pub const SELECTION_BAR: &str = "▎";
