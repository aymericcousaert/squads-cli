use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::api::{TeamsClient, SCOPE_GRAPH, SCOPE_IC3};
use crate::cache::{Cache, UNKNOWN_USERS_FILE, USERS_FILE};
use crate::types::Chat;

/// Parallel Graph user lookups when listing chats.
const GRAPH_CONCURRENCY: usize = 8;
/// Parallel chat message fetches when falling back to sender names.
const MESSAGES_CONCURRENCY: usize = 4;
/// How long a cached display name stays usable. People rename themselves, and
/// nothing else clears this cache.
const NAME_CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// How long a member no source could name is left alone. Shorter than a name
/// that was found: this is a gap that a new account or a repaired directory
/// entry can close, and the cost of being wrong is one lookup.
const UNKNOWN_CACHE_TTL_SECS: u64 = 24 * 60 * 60;

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

/// Members an earlier run could not name, and gave up on until the entry ages
/// out. Without this every run pays the message fallback again for the same
/// handful of users, which is the slowest part of listing chats.
fn read_unknown(now: u64) -> HashMap<String, u64> {
    let mut unknown: HashMap<String, u64> = Cache::new()
        .ok()
        .and_then(|c| c.load(UNKNOWN_USERS_FILE).ok().flatten())
        .unwrap_or_default();
    unknown.retain(|_, at| now.saturating_sub(*at) < UNKNOWN_CACHE_TTL_SECS);
    unknown
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
    let unknown = read_unknown(now);

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
            if !names.contains_key(&id) && !unknown.contains_key(&id) && !missing.contains(&id) {
                missing.push(id);
            }
        }
    }
    // Everyone asked after on this run. Those still nameless at the end are
    // written down as such, so the next run does not repeat the search.
    let asked = missing.clone();

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
                .filter(|id| !names.contains_key(id) && !unknown.contains_key(id))
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

    let still_unknown = give_up_on(&asked, &names, &unknown, now);
    if still_unknown.len() != unknown.len()
        || still_unknown.keys().any(|id| !unknown.contains_key(*id))
    {
        if let Some(cache) = &cache {
            let _ = cache.save(UNKNOWN_USERS_FILE, &still_unknown);
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

/// Who to stop looking for: everyone asked after on this run who still has no
/// name, and everyone an earlier run already gave up on. A name found since
/// drops out, so a user who joins the directory later is not left out for good.
///
/// An entry keeps the time it was first written rather than taking `now`, or
/// giving up would renew itself on every run and never expire.
fn give_up_on<'a>(
    asked: &'a [String],
    names: &HashMap<String, String>,
    unknown: &'a HashMap<String, u64>,
    now: u64,
) -> HashMap<&'a str, u64> {
    asked
        .iter()
        .map(|id| (id.as_str(), now))
        .chain(unknown.iter().map(|(id, at)| (id.as_str(), *at)))
        .filter(|(id, _)| !names.contains_key(*id))
        .collect()
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_789_377_029;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The whole point: a member nobody can name is written down, so the next
    /// run skips the message fallback that cost more than a second.
    #[test]
    fn a_member_no_source_can_name_is_given_up_on() {
        let asked = ids(&["u1"]);
        let unknown = HashMap::new();
        let gave_up = give_up_on(&asked, &HashMap::new(), &unknown, NOW);
        assert_eq!(gave_up.get("u1"), Some(&NOW));
    }

    #[test]
    fn a_member_who_was_named_is_not_given_up_on() {
        let asked = ids(&["u1"]);
        let names = HashMap::from([("u1".to_string(), "Ada Fenwick".to_string())]);
        assert!(give_up_on(&asked, &names, &HashMap::new(), NOW).is_empty());
    }

    /// A name found since clears the entry, so someone who joins the directory
    /// later is not left nameless until the file ages out.
    #[test]
    fn finding_a_name_clears_an_earlier_giving_up() {
        let unknown = HashMap::from([("u1".to_string(), NOW - 60)]);
        let names = HashMap::from([("u1".to_string(), "Ada Fenwick".to_string())]);
        assert!(give_up_on(&[], &names, &unknown, NOW).is_empty());
    }

    /// Taking `now` here would push the expiry out on every run, and the entry
    /// would never age out at all.
    #[test]
    fn an_older_entry_keeps_the_time_it_was_written() {
        let asked = ids(&["u1"]);
        let unknown = HashMap::from([("u1".to_string(), NOW - 3600)]);
        let gave_up = give_up_on(&asked, &HashMap::new(), &unknown, NOW);
        assert_eq!(gave_up.get("u1"), Some(&(NOW - 3600)));
    }
}
