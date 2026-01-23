#[cfg(feature = "tui")]
mod app;
#[cfg(feature = "tui")]
mod ui;

#[cfg(feature = "tui")]
pub use app::run;
