//! Squads CLI - Microsoft Teams client for AI agents and terminal users
//!
//! This library provides programmatic access to Microsoft Teams functionality.

pub mod api;
pub mod cache;
pub mod config;
pub mod types;

pub use api::client::TeamsClient;
pub use config::Config;
