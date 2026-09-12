use std::io::Write;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::{photo_object_id, PhotoSubject, TeamsClient};
use crate::config::Config;

use super::output::{print_error, print_output, print_single, print_success};
use super::OutputFormat;

/// What the command exits with when the person simply has no photo. Distinct
/// from 1, so a caller can tell "nobody set one" from "the fetch broke".
pub const EXIT_NO_PHOTO: i32 = 3;

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

    /// Download a profile photo
    Photo {
        /// User object ID, MRI or email. With --group, a team's group ID.
        id: String,

        /// Look the ID up as a group (a team) rather than a person
        #[arg(long)]
        group: bool,

        /// Output file path, or `-` for stdout
        #[arg(short, long)]
        output: Option<String>,
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
        UsersSubcommand::Photo { id, group, output } => {
            photo(config, &id, group, output, format).await
        }
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

#[derive(Debug, Serialize)]
struct PhotoResult {
    id: String,
    /// False when nobody set a photo. The bytes fields are absent then.
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
}

/// Write a profile photo to a file, or to stdout with `-o -`.
///
/// `chats download-image` only writes files, because an inline image is a
/// message attachment someone wants to keep. An avatar is small and usually
/// piped straight on, so stdout is worth having; the confirmation goes to
/// stderr there to keep those bytes alone on stdout.
async fn photo(
    config: &Config,
    id: &str,
    group: bool,
    output: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;

    let subject = if group {
        PhotoSubject::Group
    } else {
        PhotoSubject::Person
    };

    // An email is what a person has to hand, and the other subcommands resolve
    // one the same way. A colon rules out a chat ID, which also carries an `@`.
    let resolved = if !group && id.contains('@') && !id.contains(':') {
        client
            .get_users(Some(&format!("$filter=mail eq '{}'", id)))
            .await
            .ok()
            .and_then(|users| users.value.into_iter().next())
            .map(|user| user.id)
    } else {
        photo_object_id(id).map(str::to_string)
    };

    // A bot MRI or an unknown email resolves to nothing, which is the same
    // answer as a person without a photo.
    let found = match &resolved {
        Some(object_id) => client.fetch_profile_photo(object_id, subject).await?,
        None => None,
    };

    let Some((content_type, bytes)) = found else {
        return missing(id, format);
    };

    let to_stdout = output.as_deref() == Some("-");
    if to_stdout {
        std::io::stdout().write_all(&bytes)?;
        std::io::stdout().flush()?;
        eprintln!("Wrote {} bytes ({}) to stdout", bytes.len(), content_type);
        return Ok(());
    }

    let path = output.unwrap_or_else(|| {
        format!(
            "photo_{}.{}",
            resolved.as_deref().unwrap_or(id),
            extension_for(&content_type)
        )
    });
    std::fs::write(&path, &bytes)?;

    let result = PhotoResult {
        id: id.to_string(),
        found: true,
        output: Some(path.clone()),
        content_type: Some(content_type.clone()),
        bytes: Some(bytes.len()),
    };

    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => print_success(&format!(
            "Downloaded {} ({}, {} bytes)",
            path,
            content_type,
            bytes.len()
        )),
    }

    Ok(())
}

/// Nobody has a photo here. Said plainly and with its own exit code, because
/// most people never set one and a caller must not treat that as a breakage.
fn missing(id: &str, format: OutputFormat) -> Result<()> {
    let result = PhotoResult {
        id: id.to_string(),
        found: false,
        output: None,
        content_type: None,
        bytes: None,
    };
    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => print_error(&format!("No profile photo for {}", id)),
    }
    std::process::exit(EXIT_NO_PHOTO);
}

fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn photo_args(args: &[&str]) -> (String, bool, Option<String>) {
        match Cli::parse_from(args).command {
            Commands::Users(cmd) => match cmd.command {
                UsersSubcommand::Photo { id, group, output } => (id, group, output),
                other => panic!("not a photo command: {:?}", other),
            },
            _ => panic!("not a users command"),
        }
    }

    #[test]
    fn a_photo_is_asked_for_by_id_and_written_where_told() {
        let (id, group, output) =
            photo_args(&["squads-cli", "users", "photo", "abc", "-o", "a.jpg"]);
        assert_eq!(id, "abc");
        assert!(!group);
        assert_eq!(output.as_deref(), Some("a.jpg"));
    }

    /// A team's photo hangs off a different Graph collection, so the flag has
    /// to survive parsing.
    #[test]
    fn a_group_photo_is_asked_for_the_same_way() {
        let (id, group, output) = photo_args(&["squads-cli", "users", "photo", "abc", "--group"]);
        assert_eq!(id, "abc");
        assert!(group);
        assert_eq!(output, None);
    }

    #[test]
    fn the_written_file_is_named_after_what_graph_sent() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/gif"), "gif");
        assert_eq!(extension_for("image/jpeg"), "jpg");
        // Graph has sent an unlabelled photo before; JPEG is what it is.
        assert_eq!(extension_for("application/octet-stream"), "jpg");
    }
}
