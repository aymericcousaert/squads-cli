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

    /// Search users by name or email (uses advanced search)
    Search {
        /// Search query (name or email)
        query: String,

        /// Maximum number of results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Check user presence/availability status
    Presence {
        /// Specific user email or ID to check (omit for own presence)
        #[arg(short, long)]
        user: Option<String>,

        /// Multiple user emails or IDs, comma-separated
        #[arg(long)]
        users: Option<String>,
    },
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

#[derive(Debug, Serialize, Tabled)]
struct PresenceRow {
    #[tabled(rename = "User ID")]
    id: String,
    #[tabled(rename = "Availability")]
    availability: String,
    #[tabled(rename = "Activity")]
    activity: String,
    #[tabled(rename = "Status Message")]
    status_message: String,
}

pub async fn execute(cmd: UsersCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        UsersSubcommand::List { search, limit } => list(config, search, limit, format).await,
        UsersSubcommand::Show { user_id } => show(config, &user_id, format).await,
        UsersSubcommand::Me => me(config, format).await,
        UsersSubcommand::Search { query, limit } => search(config, &query, limit, format).await,
        UsersSubcommand::Presence { user, users } => presence(config, user, users, format).await,
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
    let users = client
        .get_users(Some(&format!("$filter=id eq '{}'", user_id)))
        .await?;

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

async fn search(config: &Config, query: &str, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let users = client.search_users(query, limit).await?;

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

    if rows.is_empty() {
        print_error(&format!("No users found matching '{}'", query));
    } else {
        print_output(&rows, format);
    }
    Ok(())
}

async fn presence(
    config: &Config,
    user: Option<String>,
    users: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    if let Some(user_ids_str) = users {
        // Multiple users - resolve emails to IDs first
        let user_list: Vec<&str> = user_ids_str.split(',').map(|s| s.trim()).collect();
        let mut resolved_ids: Vec<String> = Vec::new();

        for u in &user_list {
            // Check if it looks like an email (contains @) or is already an ID
            if u.contains('@') {
                // Search for user by email to get their ID
                let search_result = client
                    .get_users(Some(&format!("$filter=mail eq '{}'", u)))
                    .await;
                if let Ok(users) = search_result {
                    if let Some(user) = users.value.into_iter().next() {
                        resolved_ids.push(user.id);
                    }
                }
            } else {
                resolved_ids.push(u.to_string());
            }
        }

        if resolved_ids.is_empty() {
            print_error("No valid users found");
            return Ok(());
        }

        let id_refs: Vec<&str> = resolved_ids.iter().map(|s| s.as_str()).collect();
        let presences = client.get_presence(id_refs).await?;

        let rows: Vec<PresenceRow> = presences
            .value
            .into_iter()
            .map(|p| PresenceRow {
                id: p.id.unwrap_or_default(),
                availability: format_availability(p.availability.as_deref()),
                activity: p.activity.unwrap_or_else(|| "-".to_string()),
                status_message: p
                    .status_message
                    .and_then(|sm| sm.message)
                    .and_then(|m| m.content)
                    .unwrap_or_else(|| "-".to_string()),
            })
            .collect();

        print_output(&rows, format);
    } else if let Some(user_id) = user {
        // Single specific user
        let user_id_for_error = user_id.clone();
        let resolved_id = if user_id.contains('@') {
            let search_result = client
                .get_users(Some(&format!("$filter=mail eq '{}'", user_id)))
                .await;
            if let Ok(users) = search_result {
                users.value.into_iter().next().map(|u| u.id)
            } else {
                None
            }
        } else {
            Some(user_id)
        };

        if let Some(id) = resolved_id {
            let presences = client.get_presence(vec![&id]).await?;
            if let Some(p) = presences.value.into_iter().next() {
                match format {
                    OutputFormat::Json => {
                        print_single(&p, format);
                    }
                    _ => {
                        let row = PresenceRow {
                            id: p.id.unwrap_or_default(),
                            availability: format_availability(p.availability.as_deref()),
                            activity: p.activity.unwrap_or_else(|| "-".to_string()),
                            status_message: p
                                .status_message
                                .and_then(|sm| sm.message)
                                .and_then(|m| m.content)
                                .unwrap_or_else(|| "-".to_string()),
                        };
                        print_output(&[row], format);
                    }
                }
            } else {
                print_error("Could not get presence for user");
            }
        } else {
            print_error(&format!("User not found: {}", user_id_for_error));
        }
    } else {
        // Current user's presence
        let p = client.get_my_presence().await?;

        match format {
            OutputFormat::Json => {
                print_single(&p, format);
            }
            _ => {
                let row = PresenceRow {
                    id: p.id.unwrap_or_else(|| "me".to_string()),
                    availability: format_availability(p.availability.as_deref()),
                    activity: p.activity.unwrap_or_else(|| "-".to_string()),
                    status_message: p
                        .status_message
                        .and_then(|sm| sm.message)
                        .and_then(|m| m.content)
                        .unwrap_or_else(|| "-".to_string()),
                };
                print_output(&[row], format);
            }
        }
    }

    Ok(())
}

fn format_availability(availability: Option<&str>) -> String {
    match availability {
        Some("Available") => "🟢 Available".to_string(),
        Some("Away") => "🟡 Away".to_string(),
        Some("BeRightBack") => "🟡 Be Right Back".to_string(),
        Some("Busy") => "🔴 Busy".to_string(),
        Some("DoNotDisturb") => "🔴 Do Not Disturb".to_string(),
        Some("Offline") => "⚫ Offline".to_string(),
        Some("PresenceUnknown") => "❓ Unknown".to_string(),
        Some(other) => other.to_string(),
        None => "-".to_string(),
    }
}
