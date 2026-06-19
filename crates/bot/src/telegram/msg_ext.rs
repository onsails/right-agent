//! frankenstein `Message`/`User`/`Chat` accessor helpers.
//!
//! frankenstein exposes only plain fields — no helper methods (unlike teloxide,
//! which had `msg.text()`, `user.full_name()`, `chat.title()`, `chat.kind`,
//! `msg.parse_entities()` with UTF-16→UTF-8 offset conversion, etc.). These
//! free functions reproduce the teloxide conveniences the telegram modules
//! relied on, with identical semantics.

use frankenstein::types::{Chat, ChatType, Message, MessageEntity, User};

/// A bot-addressing-relevant entity slice extracted from a message, with its
/// text already sliced out using correct UTF-16 offset handling.
///
/// Replaces teloxide's `msg.parse_entities()` + `entity.text()` pair, which
/// converted Bot-API UTF-16 offsets to UTF-8 internally. frankenstein keeps the
/// raw UTF-16 `offset`/`length`, so we slice via `encode_utf16`.
pub(crate) struct ParsedEntity<'a> {
    pub(crate) entity: &'a MessageEntity,
    pub(crate) text: String,
}

/// Slice the substring an entity covers from `source`, honoring Telegram's
/// UTF-16 code-unit `offset`/`length`. Returns `None` if the range is out of
/// bounds or lands on a surrogate boundary (malformed payload).
fn slice_entity_text(source: &str, entity: &MessageEntity) -> Option<String> {
    let units: Vec<u16> = source.encode_utf16().collect();
    let start = entity.offset as usize;
    let end = start.checked_add(entity.length as usize)?;
    let slice = units.get(start..end)?;
    String::from_utf16(slice).ok()
}

/// Parse a message's text entities against its text, then fall back to caption
/// entities against its caption — mirroring teloxide's
/// `parse_entities().or_else(parse_caption_entities())`.
pub(crate) fn parse_entities(msg: &Message) -> Option<Vec<ParsedEntity<'_>>> {
    if let (Some(text), Some(entities)) = (msg.text.as_deref(), msg.entities.as_ref()) {
        return Some(collect_entities(text, entities));
    }
    if let (Some(caption), Some(entities)) = (msg.caption.as_deref(), msg.caption_entities.as_ref())
    {
        return Some(collect_entities(caption, entities));
    }
    None
}

fn collect_entities<'a>(source: &str, entities: &'a [MessageEntity]) -> Vec<ParsedEntity<'a>> {
    entities
        .iter()
        .map(|entity| ParsedEntity {
            entity,
            text: slice_entity_text(source, entity).unwrap_or_default(),
        })
        .collect()
}

/// Message text, falling back to caption (media messages carry a caption).
pub(crate) fn text_or_caption(msg: &Message) -> Option<&str> {
    msg.text.as_deref().or(msg.caption.as_deref())
}

/// `true` when the chat is a 1:1 private chat.
pub(crate) fn is_private(chat: &Chat) -> bool {
    chat.type_field == ChatType::Private
}

/// Stable label for a chat kind, used in structured logs.
pub(crate) fn chat_type_label(chat: &Chat) -> &'static str {
    match chat.type_field {
        ChatType::Private => "private",
        ChatType::Group => "group",
        ChatType::Supergroup => "supergroup",
        ChatType::Channel => "channel",
    }
}

/// A user's display name: "First" or "First Last". Mirrors teloxide
/// `User::full_name`.
pub(crate) fn full_name(user: &User) -> String {
    match &user.last_name {
        Some(last) => format!("{} {}", user.first_name, last),
        None => user.first_name.clone(),
    }
}

/// A chat's title (groups/channels) — `None` for private chats.
pub(crate) fn chat_title(chat: &Chat) -> Option<&str> {
    chat.title.as_deref()
}

/// A chat's public @username (without the `@`), if any.
pub(crate) fn chat_username(chat: &Chat) -> Option<&str> {
    chat.username.as_deref()
}

/// Normalise a message's `message_thread_id` for session keying and reply
/// routing: the General-topic id (1) and absent thread both map to 0.
pub(crate) fn effective_thread_id(msg: &Message) -> i64 {
    match msg.message_thread_id {
        Some(1) | None => 0,
        Some(n) => i64::from(n),
    }
}

/// The quoted-fragment text of a reply-with-quote, if present.
pub(crate) fn quote_text(msg: &Message) -> Option<String> {
    msg.quote.as_ref().map(|q| q.text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(json: serde_json::Value) -> Message {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn text_falls_back_to_caption() {
        let m = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"},
            "caption": "hello"
        }));
        assert_eq!(text_or_caption(&m), Some("hello"));
    }

    #[test]
    fn private_and_group_labels() {
        let p = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"}
        }));
        assert!(is_private(&p.chat));
        assert_eq!(chat_type_label(&p.chat), "private");

        let g = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": -1, "type": "supergroup", "title": "g"}
        }));
        assert!(!is_private(&g.chat));
        assert_eq!(chat_type_label(&g.chat), "supergroup");
        assert_eq!(chat_title(&g.chat), Some("g"));
    }

    #[test]
    fn full_name_joins_first_and_last() {
        let m = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"},
            "from": {"id": 5, "is_bot": false, "first_name": "Ada", "last_name": "L"}
        }));
        let user = m.from.as_ref().unwrap();
        assert_eq!(full_name(user), "Ada L");
    }

    #[test]
    fn effective_thread_id_normalises_general_and_absent() {
        let general = msg(serde_json::json!({
            "message_id": 1, "date": 0, "message_thread_id": 1,
            "chat": {"id": -1, "type": "supergroup", "title": "g"}
        }));
        assert_eq!(effective_thread_id(&general), 0);

        let topic = msg(serde_json::json!({
            "message_id": 1, "date": 0, "message_thread_id": 5,
            "chat": {"id": -1, "type": "supergroup", "title": "g"}
        }));
        assert_eq!(effective_thread_id(&topic), 5);

        let none = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"}
        }));
        assert_eq!(effective_thread_id(&none), 0);
    }

    #[test]
    fn parse_entities_slices_mention_with_utf16_offsets() {
        // "héllo @bot" — the 'é' is one UTF-16 unit; @bot starts at unit 6.
        let m = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": -1, "type": "supergroup", "title": "g"},
            "text": "héllo @bot",
            "entities": [{"type": "mention", "offset": 6, "length": 4}]
        }));
        let parsed = parse_entities(&m).expect("entities present");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "@bot");
    }

    #[test]
    fn parse_entities_falls_back_to_caption() {
        let m = msg(serde_json::json!({
            "message_id": 1, "date": 0,
            "chat": {"id": -1, "type": "supergroup", "title": "g"},
            "caption": "@bot hi",
            "caption_entities": [{"type": "mention", "offset": 0, "length": 4}]
        }));
        let parsed = parse_entities(&m).expect("caption entities present");
        assert_eq!(parsed[0].text, "@bot");
    }
}
