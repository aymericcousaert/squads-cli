pub mod activity;
pub mod auth;
pub mod calendar;
pub mod chats;
pub mod completions;
pub mod feed;
pub mod install;
pub mod mail;
pub mod output;
pub mod search;
pub mod teams;
pub mod users;
pub mod watch;

use clap::{Parser, Subcommand, ValueEnum};

/// Microsoft Teams & Outlook CLI for AI agents and terminal users
#[derive(Parser, Debug)]
#[command(name = "squads-cli")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Output format
    #[arg(short, long, value_enum, default_value = "table", global = true)]
    pub format: OutputFormat,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Authentication commands
    Auth(auth::AuthCommand),

    /// Chat operations
    Chats(chats::ChatsCommand),

    /// Teams operations
    Teams(teams::TeamsCommand),

    /// User operations
    Users(users::UsersCommand),

    /// Activity feed
    Activity(activity::ActivityCommand),

    /// Outlook mail operations
    Mail(mail::MailCommand),

    /// Outlook calendar operations
    Calendar(calendar::CalendarCommand),

    /// Global search across mail, teams, and calendar
    Search(search::SearchCommand),

    /// Unified feed of messages and emails
    Feed(feed::FeedCommand),

    /// Watch for new messages and emails in real-time
    Watch(watch::WatchCommand),

    /// Generate shell completions
    Completions(completions::CompletionsCommand),

    /// Install squads-cli to ~/.local/bin
    Install,

    /// Interactive terminal UI (requires --features tui)
    #[cfg(feature = "tui")]
    Tui,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum OutputFormat {
    /// JSON output (best for AI agents)
    Json,
    /// Table output (best for humans)
    #[default]
    Table,
    /// Plain output (minimal, for scripting)
    Plain,
}
