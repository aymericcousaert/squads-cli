use std::io::Write;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;
use tabled::Tabled;

use crate::api::{emoji, TeamsClient};
use crate::config::Config;

use super::output::{print_error, print_output, print_single, print_success};
use super::OutputFormat;

/// What the command exits with when the tenant has no such emote. Distinct from
/// 1, the same way `users photo` separates "nobody set one" from a real failure.
pub const EXIT_NO_EMOTE: i32 = 3;

#[derive(Args, Debug)]
pub struct EmojiCommand {
    #[command(subcommand)]
    pub command: EmojiSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum EmojiSubcommand {
    /// List the built-in Teams emoji, in the order Teams orders them
    List {
        /// Only those whose key or name contains this
        #[arg(short, long)]
        search: Option<String>,

        /// Maximum number to print. All of them when not given.
        #[arg(short, long)]
        limit: Option<usize>,
    },

    /// Download a custom emote's image
    ///
    /// Takes the reaction key a custom emote arrives as, `<name>;<object id>`,
    /// or the object id on its own.
    Image {
        /// Reaction key or object ID
        key: String,

        /// Output file path, or `-` for stdout
        #[arg(short, long)]
        output: Option<String>,
    },
}

#[derive(Debug, Serialize, Tabled)]
struct EmojiRow {
    #[tabled(rename = "Key")]
    key: String,
    #[tabled(rename = "Emoji")]
    character: String,
    #[tabled(rename = "Name")]
    name: String,
}

pub async fn execute(cmd: EmojiCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        EmojiSubcommand::List { search, limit } => list(search, limit, format),
        EmojiSubcommand::Image { key, output } => image(config, &key, output, format).await,
    }
}

fn list(search: Option<String>, limit: Option<usize>, format: OutputFormat) -> Result<()> {
    let needle = search.map(|s| s.to_lowercase());
    let rows: Vec<EmojiRow> = emoji::catalogue()
        .iter()
        .filter(|e| match &needle {
            Some(needle) => {
                e.key.to_lowercase().contains(needle) || e.name.to_lowercase().contains(needle)
            }
            None => true,
        })
        .take(limit.unwrap_or(usize::MAX))
        .map(|e| EmojiRow {
            key: e.key.clone(),
            character: e.character.clone(),
            name: e.name.clone(),
        })
        .collect();

    print_output(&rows, format);
    Ok(())
}

#[derive(Debug, Serialize)]
struct EmoteResult {
    key: String,
    /// False when the tenant has no such emote. The bytes fields are absent then.
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
}

/// Write a custom emote's image to a file, or to stdout with `-o -`.
///
/// Shaped like `users photo`, down to the exit code: an emote deleted since
/// someone reacted with it is as ordinary as a person with no photo, and a
/// caller must not treat either as a breakage.
async fn image(
    config: &Config,
    key: &str,
    output: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let object_id = object_id(key).to_string();
    let client = TeamsClient::new(config)?;

    let Some((content_type, bytes)) = client.fetch_custom_emote(&object_id).await? else {
        return missing(key, &object_id, format);
    };

    let to_stdout = output.as_deref() == Some("-");
    if to_stdout {
        std::io::stdout().write_all(&bytes)?;
        std::io::stdout().flush()?;
        eprintln!("Wrote {} bytes ({}) to stdout", bytes.len(), content_type);
        return Ok(());
    }

    let path =
        output.unwrap_or_else(|| format!("emote_{}.{}", object_id, extension_for(&content_type)));
    std::fs::write(&path, &bytes)?;

    let result = EmoteResult {
        key: key.to_string(),
        found: true,
        object_id: Some(object_id),
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

fn missing(key: &str, object_id: &str, format: OutputFormat) -> Result<()> {
    let result = EmoteResult {
        key: key.to_string(),
        found: false,
        object_id: Some(object_id.to_string()),
        output: None,
        content_type: None,
        bytes: None,
    };
    match format {
        OutputFormat::Json => print_single(&result, format),
        _ => print_error(&format!("No custom emote for {}", key)),
    }
    std::process::exit(EXIT_NO_EMOTE);
}

/// The object id inside a reaction key, or the argument unchanged when it is
/// already one. A caller has the key to hand and should not have to split it.
fn object_id(key: &str) -> &str {
    emoji::custom_emote(key).map_or(key, |emote| emote.object_id)
}

fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "gif",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn image_args(args: &[&str]) -> (String, Option<String>) {
        match Cli::parse_from(args).command {
            Commands::Emoji(cmd) => match cmd.command {
                EmojiSubcommand::Image { key, output } => (key, output),
                other => panic!("not an image command: {:?}", other),
            },
            _ => panic!("not an emoji command"),
        }
    }

    #[test]
    fn an_emote_is_asked_for_by_key_and_written_where_told() {
        let (key, output) = image_args(&[
            "squads-cli",
            "emoji",
            "image",
            "hurray-acme;0-weu-d7-cccccccccccccccccccccccccccccc01",
            "-o",
            "e.gif",
        ]);
        assert_eq!(key, "hurray-acme;0-weu-d7-cccccccccccccccccccccccccccccc01");
        assert_eq!(output.as_deref(), Some("e.gif"));
    }

    /// The key off a message and the object id inside it both name one emote.
    #[test]
    fn the_object_id_is_taken_out_of_a_reaction_key() {
        assert_eq!(
            object_id("hurray-acme;0-weu-d7-cccccccccccccccccccccccccccccc01"),
            "0-weu-d7-cccccccccccccccccccccccccccccc01"
        );
        assert_eq!(
            object_id("0-weu-d7-cccccccccccccccccccccccccccccc01"),
            "0-weu-d7-cccccccccccccccccccccccccccccc01"
        );
    }

    /// Teams serves these animated, so a GIF is what an unlabelled one is.
    #[test]
    fn the_written_file_is_named_after_what_teams_sent() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/gif"), "gif");
        assert_eq!(extension_for("application/octet-stream"), "gif");
    }
}
