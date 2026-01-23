mod api;
mod cache;
mod cli;
mod config;
mod types;

#[cfg(feature = "tui")]
mod tui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "squads_cli=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let cli = Cli::parse();

    // Load configuration
    let config = config::Config::load()?;

    // Execute command
    match cli.command {
        Commands::Auth(cmd) => cli::auth::execute(cmd, &config).await,
        Commands::Chats(cmd) => cli::chats::execute(cmd, &config, cli.format).await,
        Commands::Teams(cmd) => cli::teams::execute(cmd, &config, cli.format).await,
        Commands::Users(cmd) => cli::users::execute(cmd, &config, cli.format).await,
        Commands::Activity(cmd) => cli::activity::execute(cmd, &config, cli.format).await,
        Commands::Mail(cmd) => cli::mail::execute(cmd, &config, cli.format).await,
        Commands::Calendar(cmd) => cli::calendar::execute(cmd, &config, cli.format).await,
        #[cfg(feature = "tui")]
        Commands::Tui => tui::run(&config).await,
    }
}
