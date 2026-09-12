use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::fs;

const EMOJI_METADATA_URL: &str = "https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v1/metadata/a098bcb732fd7dd80ce11c12ad15767f/en-us.json";

/// One built-in Teams emoticon, in the order the metadata lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Emoticon {
    /// What a reaction's `key` holds for this one, e.g. `like`.
    pub key: String,
    /// The Unicode character to draw.
    pub character: String,
    /// Human name, e.g. "Thumbs up".
    pub name: String,
}

/// Skin tone modifiers, in the order Teams numbers them.
const SKIN_TONES: [&str; 5] = [
    "\u{1F3FB}",
    "\u{1F3FC}",
    "\u{1F3FD}",
    "\u{1F3FE}",
    "\u{1F3FF}",
];

static CATALOGUE: OnceLock<Vec<Emoticon>> = OnceLock::new();
static EMOJI_MAPPING: OnceLock<HashMap<String, String>> = OnceLock::new();
static REVERSE_MAPPING: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Initialize the emoji mapping by loading from cache or downloading from Microsoft
pub async fn init() -> Result<()> {
    if EMOJI_MAPPING.get().is_some() {
        return Ok(());
    }

    let entries = match load().await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Failed to initialize emoji mapping: {}. Using fallback.", e);
            Vec::new()
        }
    };

    let mapping: HashMap<String, String> = entries
        .iter()
        .map(|e| (e.key.clone(), e.character.clone()))
        .collect();
    let mut reverse: HashMap<String, String> = HashMap::new();
    for entry in &entries {
        reverse
            .entry(entry.character.clone())
            .or_insert_with(|| entry.key.clone());
    }

    let _ = CATALOGUE.set(entries);
    let _ = EMOJI_MAPPING.set(mapping);
    let _ = REVERSE_MAPPING.set(reverse);
    Ok(())
}

/// Every built-in emoticon, in metadata order. Empty when the metadata could
/// not be read, which is the same answer `get_emoji_by_key` gives then.
pub fn catalogue() -> &'static [Emoticon] {
    CATALOGUE.get().map(Vec::as_slice).unwrap_or(&[])
}

async fn load() -> Result<Vec<Emoticon>> {
    let cache_path = Config::cache_dir()?.join("teams-emoji.json");

    if let Ok(content) = fs::read_to_string(&cache_path).await {
        // Releases before the catalogue cached a bare key-to-character map
        // here. It cannot answer for a name, so it is replaced rather than read.
        if let Ok(entries) = serde_json::from_str::<Vec<Emoticon>>(&content) {
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }

    let entries = download().await?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&cache_path, serde_json::to_string(&entries)?).await?;
    Ok(entries)
}

async fn download() -> Result<Vec<Emoticon>> {
    let res = reqwest::get(EMOJI_METADATA_URL)
        .await
        .context("Failed to download emoji metadata")?;
    let data: serde_json::Value = res
        .json()
        .await
        .context("Failed to parse emoji metadata JSON")?;

    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let categories = data
        .get("categories")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    for category in categories {
        let emoticons = category
            .get("emoticons")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for emoticon in emoticons {
            let (Some(id), Some(unicode)) = (
                emoticon.get("id").and_then(|v| v.as_str()),
                emoticon.get("unicode").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            if !seen.insert(id.to_string()) {
                continue;
            }
            entries.push(Emoticon {
                key: id.to_string(),
                character: unicode.to_string(),
                name: emoticon
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id)
                    .to_string(),
            });
        }
    }
    Ok(entries)
}

/// A custom emote a tenant uploaded. Its key is `<name>;<object id>`, and the
/// object id is what fetches the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmote<'a> {
    pub name: &'a str,
    pub object_id: &'a str,
}

/// Reads a reaction key as a custom emote, or `None` for a built-in one.
///
/// The semicolon is the only thing that tells them apart. A tenant is free to
/// name one `heart`, so the name alone proves nothing.
pub fn custom_emote(key: &str) -> Option<CustomEmote<'_>> {
    let (name, object_id) = key.split_once(';')?;
    (!name.is_empty() && !object_id.is_empty()).then_some(CustomEmote { name, object_id })
}

/// Get emoji Unicode character by Teams key (e.g., "like" -> "👍")
pub fn get_emoji_by_key(key: &str) -> Option<&str> {
    EMOJI_MAPPING.get()?.get(key).map(|s| s.as_str())
}

/// Get Teams key by emoji Unicode character (e.g., "👍" -> "like")
pub fn get_key_by_emoji(emoji: &str) -> Option<&str> {
    REVERSE_MAPPING.get()?.get(emoji).map(|s| s.as_str())
}

/// The character a key draws as, skin tone included. Teams sends a toned
/// reaction as `<key>-tone1` and never lists those in the metadata.
pub fn toned(key: &str) -> Option<String> {
    if let Some(character) = get_emoji_by_key(key) {
        return Some(character.to_string());
    }
    let (base, tone) = key.rsplit_once("-tone")?;
    let index = tone.parse::<usize>().ok()?;
    let modifier = SKIN_TONES.get(index.checked_sub(1)?)?;
    Some(format!("{}{}", get_emoji_by_key(base)?, modifier))
}

/// Map a reaction string (key or emoji) to a Unicode emoji character
pub fn map_to_unicode(reaction: &str) -> String {
    let reaction_lower = reaction.to_lowercase();
    if let Some(emoji) = toned(&reaction_lower) {
        return emoji;
    }

    // If it's already an emoji or we don't know the key, return as is
    reaction.to_string()
}

/// Map a reaction string (key or emoji) to a Teams internal key
pub fn map_to_key(reaction: &str) -> String {
    // A custom emote's key is already the only thing that names it.
    if custom_emote(reaction).is_some() {
        return reaction.to_string();
    }

    let reaction_lower = reaction.to_lowercase();

    // If it is already a known key, return it
    if toned(&reaction_lower).is_some() {
        return reaction_lower;
    }

    // If it is an emoji, try to find its key
    if let Some(key) = get_key_by_emoji(reaction) {
        return key.to_string();
    }

    // Fallback or return lowercased if unknown
    reaction_lower
}

/// What a reaction key should read as: the character for a built-in emoticon,
/// the tenant's own name for a custom emote, and the key itself for anything
/// this build has never heard of.
pub fn label(key: &str) -> String {
    if let Some(emote) = custom_emote(key) {
        return emote.name.to_string();
    }
    toned(key).unwrap_or_else(|| key.to_string())
}

/// Format a summary of reactions (e.g., "👍 2 ❤️"). The count is spaced off
/// the emoji: a custom emote ends in an object id, and "…bc2" hid whether the
/// 2 was part of it.
pub fn format_reactions_summary(props: &Option<crate::types::MessageProperties>) -> String {
    if let Some(properties) = props {
        if let Some(emotions) = &properties.emotions {
            let parts: Vec<String> = emotions
                .iter()
                // Taking a reaction back leaves its key behind with nobody on
                // it. Shown as written, a message would keep the reaction for
                // good.
                .filter(|e| !e.users.is_empty())
                .map(|e| {
                    let count = e.users.len();
                    let emoji = label(&e.key);
                    if count > 1 {
                        format!("{} {}", emoji, count)
                    } else {
                        emoji
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

    /// The catalogue is what a picker orders its emoji by, and what gives a key
    /// a name rather than only a character.
    #[tokio::test]
    async fn the_catalogue_carries_names_in_metadata_order() {
        init().await.unwrap();
        let all = catalogue();
        assert!(all.len() > 1000, "only {} emoticons", all.len());
        let like = all.iter().find(|e| e.key == "like").expect("no like");
        assert_eq!(like.character, "👍");
        assert!(!like.name.is_empty());
        // Every key the mapping answers for is in the catalogue, and once only.
        let mut keys: Vec<&str> = all.iter().map(|e| e.key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "a key is listed twice");
    }

    /// Teams sends a toned reaction as `<key>-tone1`, and the metadata lists
    /// only the untoned emoticon. Real reactions arrive this way.
    #[tokio::test]
    async fn a_skin_tone_suffix_draws_the_toned_character() {
        init().await.unwrap();
        assert_eq!(map_to_unicode("yes-tone1"), "👍\u{1F3FB}");
        assert_eq!(map_to_unicode("ok-tone3"), "👌\u{1F3FD}");
        assert_eq!(map_to_unicode("yes-tone5"), "👍\u{1F3FF}");
        // Out of range, and a base nobody knows, stay as they came.
        assert_eq!(map_to_unicode("yes-tone9"), "yes-tone9");
        assert_eq!(map_to_unicode("nosuch-tone1"), "nosuch-tone1");
    }

    #[test]
    fn a_semicolon_is_what_makes_a_key_a_custom_emote() {
        let emote = custom_emote("strong-acme;0-frca-d2-dddddddddddddddddddddddddddddd01")
            .expect("not read as a custom emote");
        assert_eq!(emote.name, "strong-acme");
        assert_eq!(
            emote.object_id,
            "0-frca-d2-dddddddddddddddddddddddddddddd01"
        );
        assert_eq!(custom_emote("like"), None);
        assert_eq!(custom_emote("yes-tone1"), None);
        // Half a key is not one.
        assert_eq!(custom_emote(";0-frca-d2-abc"), None);
        assert_eq!(custom_emote("heart;"), None);
    }

    /// A tenant may name a custom emote after a built-in one. Only the
    /// semicolon separates them, so the name must never be consulted first.
    #[tokio::test]
    async fn a_custom_emote_named_after_a_built_in_stays_custom() {
        init().await.unwrap();
        let key = "heart;0-frca-d9-eeeeeeeeeeeeeeeeeeeeeeeeeeeeee01";
        assert_eq!(custom_emote(key).unwrap().name, "heart");
        assert_eq!(label(key), "heart");
        assert_eq!(label("heart"), "❤️");
        // Sending it back must not be rewritten into the built-in key.
        assert_eq!(map_to_key(key), key);
    }

    #[tokio::test]
    async fn a_summary_names_a_custom_emote_and_draws_a_built_in() {
        init().await.unwrap();
        use crate::types::{Emotion, EmotionUser, MessageProperties};
        let user = |mri: &str| EmotionUser {
            mri: mri.to_string(),
            time: 1,
            value: "1".to_string(),
        };
        let props = Some(MessageProperties {
            emotions: Some(vec![
                Emotion {
                    key: "like".to_string(),
                    users: vec![user("8:orgid:u1"), user("8:orgid:u2")],
                },
                Emotion {
                    key: "hurray-acme;0-weu-d7-cccccccccccccccccccccccccccccc01".to_string(),
                    users: vec![user("8:orgid:u1")],
                },
            ]),
            ..Default::default()
        });
        assert_eq!(format_reactions_summary(&props), "👍 2 hurray-acme");
    }

    /// Teams keeps the key of a reaction everyone has taken back, with an empty
    /// user list. Nobody reacted, so nothing should be shown.
    #[tokio::test]
    async fn a_reaction_nobody_is_left_on_is_not_shown() {
        init().await.unwrap();
        use crate::types::{Emotion, MessageProperties};
        let props = Some(MessageProperties {
            emotions: Some(vec![Emotion {
                key: "like".to_string(),
                users: vec![],
            }]),
            ..Default::default()
        });
        assert_eq!(format_reactions_summary(&props), "");
    }
}
