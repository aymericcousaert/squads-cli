pub mod activity;
pub mod auth;
pub mod calendar;
pub mod chats;
pub mod completions;
pub mod emoji;
pub mod feed;
pub mod install;
pub mod mail;
pub mod notes;
pub mod output;
pub mod search;
pub mod sheets;
pub mod teams;
pub mod update;
pub mod users;
pub mod utils;
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

impl Cli {
    /// True when stdout is a stream for a program, not a person.
    pub fn machine_readable(&self) -> bool {
        matches!(self.format, OutputFormat::Json)
            || matches!(&self.command, Commands::Watch(cmd) if cmd.json)
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Authentication commands
    Auth(auth::AuthCommand),

    /// Chat operations
    Chats(chats::ChatsCommand),

    /// Teams operations
    Teams(teams::TeamsCommand),

    /// Emoji and custom emotes
    Emoji(emoji::EmojiCommand),

    /// User operations
    Users(users::UsersCommand),

    /// Activity feed
    Activity(activity::ActivityCommand),

    /// Outlook mail operations
    Mail(mail::MailCommand),

    /// Shortcut to personal notes
    Notes(notes::NotesCommand),

    /// Outlook calendar operations
    Calendar(calendar::CalendarCommand),

    /// SharePoint & Excel spreadsheet operations
    Sheets(sheets::SheetsCommand),

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

    /// Update squads-cli from git repo and reinstall
    Update,

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn a_json_stream_is_machine_readable() {
        assert!(cli(&["squads-cli", "--format", "json", "chats", "list"]).machine_readable());
        assert!(cli(&["squads-cli", "watch", "--json"]).machine_readable());
    }

    #[test]
    fn terminal_output_is_not_machine_readable() {
        assert!(!cli(&["squads-cli", "chats", "list"]).machine_readable());
        assert!(!cli(&["squads-cli", "watch", "--push"]).machine_readable());
    }
}
