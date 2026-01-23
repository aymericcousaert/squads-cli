use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::print_output;
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct ActivityCommand {
    #[command(subcommand)]
    pub command: ActivitySubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ActivitySubcommand {
    /// List activity feed
    List {
        /// Maximum number of activities to retrieve
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct ActivityRow {
    #[tabled(rename = "Type")]
    activity_type: String,
    #[tabled(rename = "From")]
    from: String,
    #[tabled(rename = "Preview")]
    preview: String,
    #[tabled(rename = "Thread")]
    thread: String,
    #[tabled(rename = "Time")]
    time: String,
}

pub async fn execute(cmd: ActivityCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        ActivitySubcommand::List { limit } => list(config, limit, format).await,
    }
}

async fn list(config: &Config, limit: usize, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let activities = client.get_activities().await?;

    let rows: Vec<ActivityRow> = activities
        .messages
        .into_iter()
        .filter_map(|msg| {
            let props = msg.properties?;
            let activity = props.activity?;

            Some(ActivityRow {
                activity_type: activity.activity_type,
                from: activity.source_user_im_display_name.unwrap_or_else(|| {
                    activity.source_user_id.clone()
                }),
                preview: truncate(&activity.message_preview, 40),
                thread: activity.source_thread_topic.unwrap_or_else(|| {
                    truncate(&activity.source_thread_id, 20)
                }),
                time: activity.activity_timestamp,
            })
        })
        .take(limit)
        .collect();

    print_output(&rows, format);
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s.to_string()
    }
}
