// Suppress warnings for API constants and functions kept for future use
#![allow(dead_code)]

pub mod auth;
pub mod client;
pub mod emoji;

pub use auth::*;
pub use client::*;

// API scopes
pub const SCOPE_IC3: &str = "https://ic3.teams.office.com/.default";
pub const SCOPE_CHATSVCAGG: &str = "https://chatsvcagg.teams.microsoft.com/.default";
pub const SCOPE_GRAPH: &str = "https://graph.microsoft.com/.default";
pub const SCOPE_SPACES: &str = "https://api.spaces.skype.com/Authorization.ReadWrite";

// Teams client ID (same as official Teams client)
pub const TEAMS_CLIENT_ID: &str = "1fec8e78-bce4-4aaf-ab1b-5451cc387264";
