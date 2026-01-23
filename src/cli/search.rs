use anyhow::Result;
use clap::Args;
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_output, print_single};
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct SearchCommand {
    /// Search query
    pub query: String,

    /// Maximum number of results per category
    #[arg(short, long, default_value = "5")]
    pub limit: usize,
}

#[derive(Debug, Serialize, Tabled)]
pub struct GlobalSearchResult {
    #[tabled(rename = "Type")]
    pub result_type: String,
    #[tabled(rename = "Title/Subject")]
    pub title: String,
    #[tabled(rename = "Snippet/Date")]
    pub detail: String,
    #[tabled(rename = "ID")]
    pub id: String,
}

pub async fn execute(cmd: SearchCommand, config: &Config, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let mut all_results = Vec::new();

    // 1. Search Mail
    if let Ok(mail_results) = client.search_mail(&cmd.query, cmd.limit).await {
        for m in mail_results.value {
            all_results.push(GlobalSearchResult {
                result_type: "MAIL".to_string(),
                title: truncate(&m.subject.unwrap_or_else(|| "No Subject".to_string()), 40),
                detail: m.received_date_time.unwrap_or_default(),
                id: truncate(&m.id.unwrap_or_default(), 12),
            });
        }
    }

    // 2. Search Calendar
    if let Ok(calendar_results) = client.search_calendar(&cmd.query, cmd.limit).await {
        for e in calendar_results.value {
            let start = e.start.map(|s| s.date_time).unwrap_or_default();
            all_results.push(GlobalSearchResult {
                result_type: "CALENDAR".to_string(),
                title: truncate(&e.subject.unwrap_or_else(|| "No Subject".to_string()), 40),
                detail: start,
                id: truncate(&e.id.unwrap_or_default(), 12),
            });
        }
    }

    if all_results.is_empty() {
        println!("No results found for '{}'", cmd.query);
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            print_single(&all_results, format);
        }
        _ => {
            print_output(&all_results, format);
        }
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_len {
        let truncated: String = chars[..max_len.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}
