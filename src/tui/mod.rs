#[cfg(feature = "tui")]
mod app;
#[cfg(feature = "tui")]
mod media;
#[cfg(feature = "tui")]
mod theme;
#[cfg(feature = "tui")]
mod ui;

#[cfg(feature = "tui")]
pub use app::run;
