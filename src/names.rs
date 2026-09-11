use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::api::{TeamsClient, SCOPE_GRAPH, SCOPE_IC3};
use crate::cache::{Cache, USERS_FILE};
use crate::types::Chat;

/// Parallel Graph user lookups when listing chats.
const GRAPH_CONCURRENCY: usize = 8;
/// Parallel chat message fetches when falling back to sender names.
const MESSAGES_CONCURRENCY: usize = 4;
/// How long a cached display name stays usable. People rename themselves, and
/// nothing else clears this cache.
const NAME_CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// A display name on disk, with the time it was resolved.
#[derive(Serialize, Deserialize)]
struct CachedName {
    name: String,
    at: u64,
}

/// The chat's own title, when Teams set a real one instead of a placeholder.
/// A chat with a real title needs no member name resolved.
pub fn chat_title(chat: &Chat) -> Option<&str> {
    let title = chat.title.as_deref()?;
    let placeholder = title.is_empty()
        || title == "Direct Chat"
        || title == "Group Chat"
        || title.starts_with("Group (");
    (!placeholder).then_some(title)
}

/// Object IDs of a chat's members, without the current user.
fn member_ids(chat: &Chat, my_user_id: Option<&str>) -> Vec<String> {
    chat.members
        .iter()
        .filter_map(|member| member.object_id.clone())
        .filter(|id| my_user_id != Some(id.as_str()))
        .collect()
}

/// Display names resolved by an earlier run, straight off disk. Lets a cached
/// chat list show real titles before any request completes.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub fn cached_member_names() -> HashMap<String, String> {
    read_cache(epoch_secs())
        .0
        .into_iter()
        .map(|(id, entry)| (id, entry.name))
        .collect()
}

/// Cached names that have not expired yet, plus how many entries the file held
/// before the expired ones were dropped.
fn read_cache(now: u64) -> (HashMap<String, CachedName>, usize) {
    let mut cached: HashMap<String, CachedName> = Cache::new()
        .ok()
        .and_then(|c| c.load(USERS_FILE).ok().flatten())
        .unwrap_or_default();
    let loaded = cached.len();
    cached.retain(|_, entry| now.saturating_sub(entry.at) < NAME_CACHE_TTL_SECS);
    (cached, loaded)
}

/// Resolve the display names of chat members, keyed by user object ID.
///
/// Sources are tried cheapest first: the on-disk cache, then Graph, then chat
/// messages for the users Graph cannot see (cross-tenant guests). Lookups run
/// in parallel and newly found names are cached for the next run, until they
/// reach `NAME_CACHE_TTL_SECS`.
pub async fn resolve_member_names(
    client: &TeamsClient,
    chats: &[&Chat],
    my_user_id: Option<&str>,
) -> HashMap<String, String> {
    let now = epoch_secs();
    let cache = Cache::new().ok();
    let (cached, loaded_count) = read_cache(now);

    let mut names: HashMap<String, String> = cached
        .iter()
        .map(|(id, entry)| (id.clone(), entry.name.clone()))
        .collect();

    // Chats Teams gave a real title to need no member name resolved.
    let untitled: Vec<&&Chat> = chats
        .iter()
        .filter(|chat| chat_title(chat).is_none())
        .collect();

    let mut missing: Vec<String> = Vec::new();
    for chat in &untitled {
        for id in member_ids(chat, my_user_id) {
            if !names.contains_key(&id) && !missing.contains(&id) {
                missing.push(id);
            }
        }
    }

    // Mint the token before fanning out: parallel requests would each mint
    // their own and each rewrite the token cache, which can corrupt it. Without
    // a token the lookups would all fail anyway, so give up on this source.
    if !missing.is_empty() && client.get_token(SCOPE_GRAPH).await.is_ok() {
        let found = stream::iter(missing)
            .map(|id| async move {
                let name = client
                    .get_user_by_id(&id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|user| user.display_name);
                (id, name)
            })
            .buffer_unordered(GRAPH_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for (id, name) in found {
            if let Some(name) = name {
                names.insert(id, name);
            }
        }
    }

    // Graph knows nothing about cross-tenant users, but their messages carry
    // `imdisplayname`. One fetch per chat covers all of its members. Members the
    // chat payload already names are skipped: that name is fresher than a
    // sender name taken from an old message, and it costs no request.
    let pending: Vec<(String, Vec<String>)> = untitled
        .iter()
        .filter_map(|chat| {
            let ids: Vec<String> = chat
                .members
                .iter()
                .filter(|member| {
                    member
                        .display_name
                        .as_deref()
                        .is_none_or(|name| name.is_empty())
                })
                .filter_map(|member| member.object_id.clone())
                .filter(|id| my_user_id != Some(id.as_str()))
                .filter(|id| !names.contains_key(id))
                .collect();
            if ids.is_empty() {
                return None;
            }
            Some((chat.id.clone(), ids))
        })
        .collect();

    if !pending.is_empty() && client.get_token(SCOPE_IC3).await.is_ok() {
        let found = stream::iter(pending)
            .map(|(chat_id, ids)| async move {
                client
                    .resolve_names_from_messages(&chat_id, &ids)
                    .await
                    .unwrap_or_default()
            })
            .buffer_unordered(MESSAGES_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for chat_names in found {
            for (id, name) in chat_names {
                names.entry(id).or_insert(name);
            }
        }
    }

    // Rewrite only when something moved: new names found, or expired ones dropped.
    if names.len() != cached.len() || cached.len() != loaded_count {
        if let Some(cache) = &cache {
            let entries: HashMap<&str, CachedName> = names
                .iter()
                .map(|(id, name)| {
                    let at = cached.get(id).map_or(now, |entry| entry.at);
                    (
                        id.as_str(),
                        CachedName {
                            name: name.clone(),
                            at,
                        },
                    )
                })
                .collect();
            let _ = cache.save(USERS_FILE, &entries);
        }
    }

    names
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
