use anyhow::{anyhow, Context as _, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::TeamsClient;
use crate::config::Config;

use super::output::{print_output, print_single, print_success};
use super::utils::truncate;
use super::OutputFormat;

#[derive(Args, Debug)]
pub struct SheetsCommand {
    #[command(subcommand)]
    pub command: SheetsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum SheetsSubcommand {
    /// Search SharePoint sites
    Sites {
        /// Search query
        query: String,
    },

    /// List drives (document libraries) for a SharePoint site
    Drives {
        /// Site ID
        #[arg(long)]
        site: String,
    },

    /// List files in a drive or folder
    Files {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Folder ID (omit for root)
        #[arg(long)]
        folder: Option<String>,
    },

    /// List files in your OneDrive
    MyFiles {
        /// Folder ID (omit for root)
        #[arg(long)]
        folder: Option<String>,
    },

    /// List worksheets in an Excel workbook
    Worksheets {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Item (file) ID
        #[arg(long)]
        item: String,
    },

    /// Read a range from an Excel worksheet
    Read {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Item (file) ID
        #[arg(long)]
        item: String,

        /// Worksheet name
        #[arg(long)]
        sheet: String,

        /// Range address (e.g. A1:D10). Omit to read entire used range
        #[arg(long)]
        range: Option<String>,
    },

    /// Write values to a range in an Excel worksheet
    Write {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Item (file) ID
        #[arg(long)]
        item: String,

        /// Worksheet name
        #[arg(long)]
        sheet: String,

        /// Range address (e.g. A1:B2)
        #[arg(long)]
        range: String,

        /// JSON array of arrays (e.g. '[[1,"hello"],[2,"world"]]')
        #[arg(long)]
        values: String,
    },

    /// List tables in an Excel workbook
    Tables {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Item (file) ID
        #[arg(long)]
        item: String,
    },

    /// Append rows to an Excel table
    Append {
        /// Drive ID
        #[arg(long)]
        drive: String,

        /// Item (file) ID
        #[arg(long)]
        item: String,

        /// Table name or ID
        #[arg(long)]
        table: String,

        /// JSON array of arrays (e.g. '[["val1","val2"],["val3","val4"]]')
        #[arg(long)]
        values: String,
    },
}

// === Table row types for display ===

#[derive(Debug, Serialize, Tabled)]
struct SiteRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "URL")]
    url: String,
}

#[derive(Debug, Serialize, Tabled)]
struct DriveRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    drive_type: String,
    #[tabled(rename = "URL")]
    url: String,
}

#[derive(Debug, Serialize, Tabled)]
struct FileRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    item_type: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Modified")]
    modified: String,
}

#[derive(Debug, Serialize, Tabled)]
struct WorksheetRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Position")]
    position: String,
    #[tabled(rename = "Visibility")]
    visibility: String,
}

#[derive(Debug, Serialize, Tabled)]
struct TableRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Headers")]
    show_headers: String,
}

pub async fn execute(cmd: SheetsCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        SheetsSubcommand::Sites { query } => sites(config, &query, format).await,
        SheetsSubcommand::Drives { site } => drives(config, &site, format).await,
        SheetsSubcommand::Files { drive, folder } => {
            files(config, &drive, folder.as_deref(), format).await
        }
        SheetsSubcommand::MyFiles { folder } => my_files(config, folder.as_deref(), format).await,
        SheetsSubcommand::Worksheets { drive, item } => {
            worksheets(config, &drive, &item, format).await
        }
        SheetsSubcommand::Read {
            drive,
            item,
            sheet,
            range,
        } => read(config, &drive, &item, &sheet, range.as_deref(), format).await,
        SheetsSubcommand::Write {
            drive,
            item,
            sheet,
            range,
            values,
        } => write(config, &drive, &item, &sheet, &range, &values, format).await,
        SheetsSubcommand::Tables { drive, item } => tables(config, &drive, &item, format).await,
        SheetsSubcommand::Append {
            drive,
            item,
            table,
            values,
        } => append(config, &drive, &item, &table, &values, format).await,
    }
}

async fn sites(config: &Config, query: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let sites = client.search_sites(query).await?;

    match format {
        OutputFormat::Json => print_single(&sites.value, format),
        _ => {
            if sites.value.is_empty() {
                println!("No sites found");
                return Ok(());
            }
            let rows: Vec<SiteRow> = sites
                .value
                .into_iter()
                .map(|s| SiteRow {
                    id: s.id,
                    name: s.display_name.unwrap_or_default(),
                    url: truncate(&s.web_url.unwrap_or_default(), 60),
                })
                .collect();
            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn drives(config: &Config, site_id: &str, format: OutputFormat) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let drives = client.list_drives(site_id).await?;

    match format {
        OutputFormat::Json => print_single(&drives.value, format),
        _ => {
            if drives.value.is_empty() {
                println!("No drives found");
                return Ok(());
            }
            let rows: Vec<DriveRow> = drives
                .value
                .into_iter()
                .map(|d| DriveRow {
                    id: d.id,
                    name: d.name.unwrap_or_default(),
                    drive_type: d.drive_type.unwrap_or_default(),
                    url: truncate(&d.web_url.unwrap_or_default(), 50),
                })
                .collect();
            print_output(&rows, format);
        }
    }
    Ok(())
}

fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn drive_items_to_rows(items: Vec<crate::types::DriveItem>) -> Vec<FileRow> {
    items
        .into_iter()
        .map(|item| {
            let item_type = if item.folder.is_some() {
                "Folder".to_string()
            } else {
                item.file
                    .as_ref()
                    .and_then(|f| f.mime_type.clone())
                    .unwrap_or_else(|| "File".to_string())
            };
            FileRow {
                id: item.id,
                name: truncate(&item.name.unwrap_or_default(), 40),
                item_type: truncate(&item_type, 30),
                size: item.size.map(format_size).unwrap_or_default(),
                modified: item
                    .last_modified_date_time
                    .map(|d| truncate(&d, 19))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

async fn files(
    config: &Config,
    drive_id: &str,
    folder_id: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let items = client.list_drive_items(drive_id, folder_id).await?;

    match format {
        OutputFormat::Json => print_single(&items.value, format),
        _ => {
            if items.value.is_empty() {
                println!("No files found");
                return Ok(());
            }
            let rows = drive_items_to_rows(items.value);
            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn my_files(
    config: &Config,
    folder_id: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let items = client.list_my_drive_items(folder_id).await?;

    match format {
        OutputFormat::Json => print_single(&items.value, format),
        _ => {
            if items.value.is_empty() {
                println!("No files found");
                return Ok(());
            }
            let rows = drive_items_to_rows(items.value);
            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn worksheets(
    config: &Config,
    drive_id: &str,
    item_id: &str,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let worksheets = client.list_worksheets(drive_id, item_id).await?;

    match format {
        OutputFormat::Json => print_single(&worksheets.value, format),
        _ => {
            if worksheets.value.is_empty() {
                println!("No worksheets found");
                return Ok(());
            }
            let rows: Vec<WorksheetRow> = worksheets
                .value
                .into_iter()
                .map(|w| WorksheetRow {
                    id: w.id.unwrap_or_default(),
                    name: w.name,
                    position: w.position.map(|p| p.to_string()).unwrap_or_default(),
                    visibility: w.visibility.unwrap_or_else(|| "Visible".to_string()),
                })
                .collect();
            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn read(
    config: &Config,
    drive_id: &str,
    item_id: &str,
    sheet: &str,
    range: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let result = client
        .read_sheet_range(drive_id, item_id, sheet, range)
        .await?;

    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => {
            println!(
                "Range: {}",
                result.address.as_deref().unwrap_or("(unknown)")
            );
            if let (Some(rows), Some(cols)) = (result.row_count, result.column_count) {
                println!("Rows: {}, Columns: {}", rows, cols);
            }
            println!("---");
            // Prefer text (formatted strings) over raw values for display
            let display_values = result.text.as_ref().or(result.values.as_ref());
            if let Some(values) = display_values {
                for row in values {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Null => "".to_string(),
                            other => other.to_string(),
                        })
                        .collect();
                    println!("{}", cells.join("\t"));
                }
            } else {
                println!("(empty range)");
            }
        }
    }
    Ok(())
}

async fn write(
    config: &Config,
    drive_id: &str,
    item_id: &str,
    sheet: &str,
    range: &str,
    values_json: &str,
    format: OutputFormat,
) -> Result<()> {
    let values: Vec<Vec<serde_json::Value>> = serde_json::from_str(values_json)
        .context("Invalid JSON for values. Expected array of arrays, e.g. [[1,\"hello\"],[2,\"world\"]]")?;

    if values.is_empty() {
        return Err(anyhow!("Values cannot be empty"));
    }

    let client = TeamsClient::new(config)?;
    let result = client
        .update_sheet_range(drive_id, item_id, sheet, range, values)
        .await?;

    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => {
            print_success(&format!(
                "Updated range {} ({} rows x {} cols)",
                result.address.as_deref().unwrap_or(range),
                result.row_count.unwrap_or(0),
                result.column_count.unwrap_or(0),
            ));
        }
    }
    Ok(())
}

async fn tables(
    config: &Config,
    drive_id: &str,
    item_id: &str,
    format: OutputFormat,
) -> Result<()> {
    let client = TeamsClient::new(config)?;
    let tables = client.list_tables(drive_id, item_id).await?;

    match format {
        OutputFormat::Json => print_single(&tables.value, format),
        _ => {
            if tables.value.is_empty() {
                println!("No tables found");
                return Ok(());
            }
            let rows: Vec<TableRow> = tables
                .value
                .into_iter()
                .map(|t| TableRow {
                    id: t.id.unwrap_or_default(),
                    name: t.name.unwrap_or_default(),
                    show_headers: t
                        .show_headers
                        .map(|b| if b { "Yes" } else { "No" }.to_string())
                        .unwrap_or_default(),
                })
                .collect();
            print_output(&rows, format);
        }
    }
    Ok(())
}

async fn append(
    config: &Config,
    drive_id: &str,
    item_id: &str,
    table: &str,
    values_json: &str,
    format: OutputFormat,
) -> Result<()> {
    let values: Vec<Vec<serde_json::Value>> = serde_json::from_str(values_json)
        .context("Invalid JSON for values. Expected array of arrays, e.g. [[\"val1\",\"val2\"]]")?;

    if values.is_empty() {
        return Err(anyhow!("Values cannot be empty"));
    }

    let row_count = values.len();
    let client = TeamsClient::new(config)?;
    let result = client
        .append_table_rows(drive_id, item_id, table, values)
        .await?;

    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => {
            print_success(&format!("Appended {} row(s) to table '{}'", row_count, table));
        }
    }
    Ok(())
}
