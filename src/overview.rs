use anyhow::Result;

use crate::cache::{Cache, CHATS_FILE, ME_FILE, TEAMS_FILE};
use crate::types::{Chat, Message, Team};

/// The chat and channel lists saved by the previous run.
#[derive(Default)]
pub struct Overview {
    pub chats: Vec<Chat>,
    pub teams: Vec<Team>,
    pub my_user_id: Option<String>,
}

/// A chat as it should be written to disk.
///
/// Only `last_message.properties` is dropped. Teams sends a message's `files`
/// and `cards` as JSON-encoded strings, so serializing them writes real arrays
/// that cannot be read back, and one chat with an attachment would make the
/// whole file unparseable. The rest of `last_message` has to stay: a 1:1 chat
/// with an app has no title and no member names, so `imDisplayName` is the only
/// thing that names it.
fn cacheable(chat: &Chat) -> Chat {
    Chat {
        last_message: chat.last_message.clone().map(|message| Message {
            properties: None,
            ..message
        }),
        ..chat.clone()
    }
}

/// Read the cached lists, so the UI can show something before the first
/// request comes back. Anything missing or unreadable comes back empty: a
/// stale cache is a nicety, never a reason to fail.
pub fn load() -> Overview {
    let Ok(cache) = Cache::new() else {
        return Overview::default();
    };
    Overview {
        chats: cache.load(CHATS_FILE).ok().flatten().unwrap_or_default(),
        teams: cache.load(TEAMS_FILE).ok().flatten().unwrap_or_default(),
        my_user_id: cache.load(ME_FILE).ok().flatten(),
    }
}

/// Save the lists for the next run.
pub fn save(chats: &[Chat], teams: &[Team], my_user_id: Option<&str>) -> Result<()> {
    let cache = Cache::new()?;
    let chats: Vec<Chat> = chats.iter().map(cacheable).collect();
    cache.save(CHATS_FILE, &chats)?;
    cache.save(TEAMS_FILE, &teams.to_vec())?;
    if let Some(id) = my_user_id {
        cache.save(ME_FILE, &id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat as the API sends it: `files` inside `lastMessage` is a string
    /// holding JSON, not an array.
    const CHAT_JSON: &str = r#"{
        "id": "19:abc@thread.v2",
        "members": [{"mri": "8:orgid:u1", "objectId": "u1", "displayName": "Ada Fenwick"}],
        "isRead": false,
        "title": "Design",
        "lastMessage": {
            "content": "see attached",
            "from": "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/contacts/8:orgid:u1",
            "imdisplayname": "Ada Fenwick",
            "properties": {
                "files": "[{\"fileName\":\"spec.pdf\",\"fileInfo\":{}}]"
            }
        }
    }"#;

    /// A 1:1 chat with an app: no title, no member display names, named only
    /// by the last message's sender.
    const APP_CHAT_JSON: &str = r#"{
        "id": "19:me_app@unq.gbl.spaces",
        "members": [{"mri": "8:orgid:me"}, {"mri": "28:app"}],
        "isOneOnOne": true,
        "isLastMessageFromMe": false,
        "lastMessage": {
            "from": "https://teams.microsoft.com/api/chatsvc/emea/v1/users/ME/contacts/28:app",
            "imdisplayname": "Confluence"
        }
    }"#;

    #[test]
    fn api_shape_parses() {
        let chat: Chat = serde_json::from_str(CHAT_JSON).expect("API payload should parse");
        assert_eq!(chat.id, "19:abc@thread.v2");
        assert!(chat.last_message.is_some());
    }

    /// Why `cacheable` drops `properties`. If this ever starts passing, the
    /// serialized form has become symmetric and the stripping can go.
    #[test]
    fn properties_cannot_round_trip() {
        let chat: Chat = serde_json::from_str(CHAT_JSON).unwrap();
        let json = serde_json::to_string(&chat).unwrap();
        assert!(serde_json::from_str::<Chat>(&json).is_err());
    }

    #[test]
    fn cacheable_chat_round_trips() {
        let chat: Chat = serde_json::from_str(CHAT_JSON).unwrap();
        let json = serde_json::to_string(&cacheable(&chat)).unwrap();
        let back: Chat = serde_json::from_str(&json).expect("cached chat should read back");
        assert_eq!(back.id, "19:abc@thread.v2");
        assert_eq!(back.is_read, Some(false));
        assert_eq!(back.members[0].display_name.as_deref(), Some("Ada Fenwick"));
    }

    /// A 1:1 chat with an app has no title and no member names, so the cached
    /// list can only name it from the last message. Losing this showed the
    /// chat as "1:1 Chat" until the fresh data arrived.
    #[test]
    fn cache_keeps_the_only_name_an_app_chat_has() {
        let chat: Chat = serde_json::from_str(APP_CHAT_JSON).unwrap();
        assert!(chat.title.is_none());
        assert!(chat.members.iter().all(|m| m.display_name.is_none()));

        let json = serde_json::to_string(&cacheable(&chat)).unwrap();
        let back: Chat = serde_json::from_str(&json).expect("cached chat should read back");
        let last = back.last_message.expect("last message must survive");
        assert_eq!(last.im_display_name.as_deref(), Some("Confluence"));
        assert_eq!(back.is_last_message_from_me, Some(false));
    }
}
