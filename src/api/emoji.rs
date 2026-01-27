use crate::config::Config;
use anyhow::{Context, Result};
use serde_json;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::fs;

const EMOJI_METADATA_URL: &str = "https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v1/metadata/a098bcb732fd7dd80ce11c12ad15767f/en-us.json";

static EMOJI_MAPPING: OnceLock<HashMap<String, String>> = OnceLock::new();
static REVERSE_MAPPING: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Initialize the emoji mapping by loading from cache or downloading from Microsoft
pub async fn init() -> Result<()> {
    if EMOJI_MAPPING.get().is_some() {
        return Ok(());
    }

    match try_init().await {
        Ok((mapping, reverse)) => {
            let _ = EMOJI_MAPPING.set(mapping);
            let _ = REVERSE_MAPPING.set(reverse);
        }
        Err(e) => {
            tracing::warn!("Failed to initialize emoji mapping: {}. Using fallback.", e);
            let _ = EMOJI_MAPPING.set(HashMap::new());
            let _ = REVERSE_MAPPING.set(HashMap::new());
        }
    }

    Ok(())
}

async fn try_init() -> Result<(HashMap<String, String>, HashMap<String, String>)> {
    let cache_dir = Config::cache_dir()?;
    let cache_path = cache_dir.join("teams-emoji.json");

    let mapping: HashMap<String, String> = if cache_path.exists() {
        let content = fs::read_to_string(&cache_path).await?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        // Download and parse
        let res = reqwest::get(EMOJI_METADATA_URL)
            .await
            .context("Failed to download emoji metadata")?;
        let data: serde_json::Value = res
            .json()
            .await
            .context("Failed to parse emoji metadata JSON")?;

        let mut mapping = HashMap::new();
        if let Some(categories) = data.get("categories").and_then(|v| v.as_array()) {
            for cat in categories {
                if let Some(emoticons) = cat.get("emoticons").and_then(|v| v.as_array()) {
                    for emo in emoticons {
                        if let (Some(id), Some(unicode)) = (
                            emo.get("id").and_then(|v| v.as_str()),
                            emo.get("unicode").and_then(|v| v.as_str()),
                        ) {
                            // Only insert if not already present to prefer the first key found (often more descriptive)
                            // or to maintain consistency if multiple keys exist for same unicode.
                            mapping.entry(id.to_string()).or_insert(unicode.to_string());
                        }
                    }
                }
            }
        }

        // Save to cache
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let content = serde_json::to_string_pretty(&mapping)?;
        fs::write(&cache_path, content).await?;

        mapping
    };

    let reverse: HashMap<String, String> = mapping
        .iter()
        .map(|(k, v)| (v.clone(), k.clone()))
        .collect();

    Ok((mapping, reverse))
}

/// Get emoji Unicode character by Teams key (e.g., "like" -> "👍")
pub fn get_emoji_by_key(key: &str) -> Option<&str> {
    EMOJI_MAPPING.get()?.get(key).map(|s| s.as_str())
}

/// Get Teams key by emoji Unicode character (e.g., "👍" -> "like")
pub fn get_key_by_emoji(emoji: &str) -> Option<&str> {
    REVERSE_MAPPING.get()?.get(emoji).map(|s| s.as_str())
}

/// Map a reaction string (key or emoji) to a Unicode emoji character
pub fn map_to_unicode(reaction: &str) -> String {
    let reaction_lower = reaction.to_lowercase();
    if let Some(emoji) = get_emoji_by_key(&reaction_lower) {
        return emoji.to_string();
    }

    // If it's already an emoji or we don't know the key, return as is
    reaction.to_string()
}

/// Map a reaction string (key or emoji) to a Teams internal key
pub fn map_to_key(reaction: &str) -> String {
    let reaction_lower = reaction.to_lowercase();

    // If it is already a known key, return it
    if get_emoji_by_key(&reaction_lower).is_some() {
        return reaction_lower;
    }

    // If it is an emoji, try to find its key
    if let Some(key) = get_key_by_emoji(reaction) {
        return key.to_string();
    }

    // Fallback or return lowercased if unknown
    reaction_lower
}

/// Format a summary of reactions (e.g., "👍2 ❤️1")
pub fn format_reactions_summary(props: &Option<crate::types::MessageProperties>) -> String {
    if let Some(properties) = props {
        if let Some(emotions) = &properties.emotions {
            let parts: Vec<String> = emotions
                .iter()
                .map(|e| {
                    let count = e.users.len();
                    let emoji = get_emoji_by_key(&e.key).unwrap_or(&e.key);
                    if count > 1 {
                        format!("{}{}", emoji, count)
                    } else {
                        emoji.to_string()
                    }
                })
                .collect();
            return parts.join(" ");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_emoji_init_and_mapping() {
        // Initialize mapping (will download if not cached)
        init().await.unwrap();

        // Test basic mapping
        assert_eq!(get_emoji_by_key("like"), Some("👍"));

        // Test reverse mapping
        let key_for_thumbsup = get_key_by_emoji("👍").expect("Should find a key for 👍");
        assert!(key_for_thumbsup == "like" || key_for_thumbsup == "yes");

        // Test utility functions (map_to_unicode)
        assert_eq!(map_to_unicode("like"), "👍");
        assert_eq!(map_to_unicode("👍"), "👍");
        assert_eq!(map_to_unicode("skull"), "💀");

        // Test utility functions (map_to_key)
        let mapped_key = map_to_key("👍");
        assert!(mapped_key == "like" || mapped_key == "yes");

        // Test weird/specific keys from Teams asset
        assert_eq!(map_to_unicode("meltingface"), "🫠");
        assert_eq!(map_to_unicode("1f92f_explodinghead"), "🤯");
        assert_eq!(map_to_unicode("heartpink"), "🩷");

        // Test mapping emoji characters back to keys
        assert_eq!(map_to_key("🫠"), "meltingface");
        assert_eq!(map_to_key("🤯"), "1f92f_explodinghead");
        assert_eq!(map_to_key("🩷"), "heartpink");

        // Test unknown input remains same (lowercased for key)
        assert_eq!(map_to_unicode("unknown_emoji_key"), "unknown_emoji_key");
        assert_eq!(map_to_key("unknown_emoji_key"), "unknown_emoji_key");
    }
}
