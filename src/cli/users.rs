use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_error, print_output, print_single};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct UsersCommand {
    #[command(subcommand)]
    pub command: UsersSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum UsersSubcommand {
    /// List users in the organization
    List {
        /// Search filter
        #[arg(short, long)]
        search: Option<String>,

        /// Maximum number of users to retrieve
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Show user details
    Show {
        /// User ID
        user_id: String,
    },

    /// Show current user profile
    Me,
}

#[derive(Debug, Serialize, Tabled)]
struct UserRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Email")]
    email: String,
    #[tabled(rename = "Job Title")]
    job_title: String,
}

pub async fn execute(cmd: UsersCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        UsersSubcommand::List { search, limit } => list(config, search, limit, format).await,
        UsersSubcommand::Show { user_id } => show(config, &user_id, format).await,
        UsersSubcommand::Me => me(config, format).await,
    }
}

async fn list(
    config: &Config,
    search: Option<String>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let params = match search {
        Some(ref s) => format!(
            "$filter=startswith(displayName,'{}') or startswith(mail,'{}')&$top={}",
            s, s, limit
        ),
        None => format!("$top={}", limit),
    };

    let users = client.get_users(Some(&params)).await?;

    let rows: Vec<UserRow> = users
        .value
        .into_iter()
        .map(|user| UserRow {
            id: user.id,
            name: user.display_name.unwrap_or_default(),
            email: user.mail.unwrap_or_default(),
            job_title: user.job_title.unwrap_or_default(),
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

async fn show(config: &Config, user_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let users = client.get_users(Some(&format!("$filter=id eq '{}'", user_id))).await?;

    if let Some(user) = users.value.into_iter().next() {
        print_single(&user, format);
    } else {
        print_error(&format!("User not found: {}", user_id));
    }

    Ok(())
}

async fn me(config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let profile = client.get_me().await?;
    print_single(&profile, format);
    Ok(())
}
