//! Detect whether a group message addresses the bot, and prepare the
//! cleaned-up prompt text.

use frankenstein::types::{Message, MessageEntityType};

use super::msg_ext;

/// Bot identity: username (without '@') and user_id. Cached at bot startup.
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub username: String,
    pub user_id: u64,
}

/// How a routed message refers to the bot, in group context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressKind {
    DirectMessage,
    GroupMentionText,   // `@botname` in text
    GroupMentionEntity, // TextMention entity pointing at bot user_id
    GroupReplyToBot,    // reply_to_message is from bot
    GroupSlashCommand,  // /cmd@botname (or any cmd in a group-to-bot)
}

/// Returns `Some(AddressKind)` when the message should be treated as addressed
/// to the bot; `None` in groups where the message is unrelated.
pub fn is_bot_addressed(msg: &Message, identity: &BotIdentity) -> Option<AddressKind> {
    if msg_ext::is_private(&msg.chat) {
        return Some(AddressKind::DirectMessage);
    }
    // 1) reply to bot's message -- but NOT the forum-topic-root service
    //    message. Telegram threads topic membership via a reply to the
    //    `forum_topic_created` message, whose author is the topic
    //    creator; when the bot created the topic this would otherwise
    //    make every message look like a reply to the bot.
    if let Some(reply) = msg.reply_to_message.as_ref()
        && reply.forum_topic_created.is_none()
        && let Some(from) = reply.from.as_ref()
        && from.id == identity.user_id
    {
        return Some(AddressKind::GroupReplyToBot);
    }

    // 2) parse entities, slicing with correct UTF-16 offsets (frankenstein
    //    keeps the Bot-API UTF-16 code units; `msg_ext` converts). Text
    //    entities first, then caption entities.
    if let Some(entities) = msg_ext::parse_entities(msg) {
        for parsed in entities {
            match parsed.entity.type_field {
                MessageEntityType::TextMention
                    if parsed
                        .entity
                        .user
                        .as_ref()
                        .is_some_and(|user| user.id == identity.user_id) =>
                {
                    return Some(AddressKind::GroupMentionEntity);
                }
                MessageEntityType::Mention => {
                    // Slice is e.g. "@botname"; compare case-insensitively.
                    if parsed
                        .text
                        .strip_prefix('@')
                        .map(|u| u.eq_ignore_ascii_case(&identity.username))
                        .unwrap_or(false)
                    {
                        return Some(AddressKind::GroupMentionText);
                    }
                }
                MessageEntityType::BotCommand => {
                    // Accept /cmd (no suffix — only one bot in chat or we're the default)
                    // or /cmd@botname (explicit).
                    if let Some((_, maybe_user)) = parsed.text.split_once('@') {
                        if maybe_user.eq_ignore_ascii_case(&identity.username) {
                            return Some(AddressKind::GroupSlashCommand);
                        }
                    } else {
                        return Some(AddressKind::GroupSlashCommand);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Strip `@botname` mentions from `text` for prompt cleanup.
///
/// Preserves newlines and internal whitespace. Only collapses horizontal
/// whitespace immediately adjacent to the stripped mention (to avoid
/// double-spaces), and trims leading/trailing whitespace from the result.
pub fn strip_bot_mentions(text: &str, username: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '@' {
            let rest = &text[i + 1..];
            let end = rest
                .char_indices()
                .find(|(_, ch)| !(ch.is_ascii_alphanumeric() || *ch == '_'))
                .map(|(idx, _)| idx)
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if !candidate.is_empty() && candidate.eq_ignore_ascii_case(username) {
                // Advance iterator past the username chars.
                for _ in 0..candidate.chars().count() {
                    it.next();
                }
                // If the char before the mention (last in `out`) is a horizontal
                // whitespace (space or tab) AND the next char is also horizontal
                // whitespace, drop one trailing horizontal whitespace to avoid
                // a double gap. Never touch newlines.
                let prev_is_hspace = out
                    .chars()
                    .next_back()
                    .map(|c| c == ' ' || c == '\t')
                    .unwrap_or(true); // treat start-of-string as "space-like"
                if prev_is_hspace
                    && let Some(&(_, next_c)) = it.peek()
                    && (next_c == ' ' || next_c == '\t')
                {
                    it.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn strip_removes_bot_mention() {
        assert_eq!(strip_bot_mentions("@right_bot hello", "right_bot"), "hello");
        assert_eq!(
            strip_bot_mentions("hey @right_bot how are you", "right_bot"),
            "hey how are you"
        );
    }

    #[tokio::test]
    async fn strip_leaves_other_mentions() {
        assert_eq!(
            strip_bot_mentions("@alice says hi to @right_bot", "right_bot"),
            "@alice says hi to"
        );
    }

    #[tokio::test]
    async fn strip_is_case_insensitive() {
        assert_eq!(strip_bot_mentions("@Right_Bot hi", "right_bot"), "hi");
    }

    #[tokio::test]
    async fn strip_preserves_newlines() {
        let input = "@right_bot hello\nline two\nline three";
        assert_eq!(
            strip_bot_mentions(input, "right_bot"),
            "hello\nline two\nline three"
        );
    }

    #[tokio::test]
    async fn dm_returns_direct_message() {
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"},
            "from": {"id": 1, "is_bot": false, "first_name": "U"},
            "text": "hi"
        }))
        .unwrap();
        let identity = BotIdentity {
            username: "right_bot".into(),
            user_id: 999,
        };
        assert_eq!(
            is_bot_addressed(&msg, &identity),
            Some(AddressKind::DirectMessage)
        );
    }

    #[tokio::test]
    async fn group_non_mention_returns_none() {
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": -1001, "type": "group", "title": "g"},
            "from": {"id": 1, "is_bot": false, "first_name": "U"},
            "text": "just chatting"
        }))
        .unwrap();
        let identity = BotIdentity {
            username: "right_bot".into(),
            user_id: 999,
        };
        assert_eq!(is_bot_addressed(&msg, &identity), None);
    }

    #[tokio::test]
    async fn topic_root_reply_is_not_addressing() {
        // A plain message in a bot-created topic: reply_to_message is the
        // forum_topic_created service message whose `from` is the bot.
        // It must NOT count as addressing. (teloxide-core fixture shape.)
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 5, "date": 0,
            "chat": {"id": -1001, "is_forum": true, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "is_topic_message": true,
            "message_thread_id": 4,
            "text": "привет",
            "reply_to_message": {
                "message_id": 4, "date": 0,
                "chat": {"id": -1001, "is_forum": true, "type": "supergroup", "title": "g"},
                "from": {"id": 999, "is_bot": true, "first_name": "Bot"},
                "is_topic_message": true,
                "message_thread_id": 4,
                "forum_topic_created": {"name": "Socials", "icon_color": 9367192}
            }
        }))
        .unwrap();
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        assert_eq!(is_bot_addressed(&msg, &identity), None);
    }

    #[tokio::test]
    async fn real_reply_to_bot_is_addressing() {
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 6, "date": 0,
            "chat": {"id": -1001, "type": "supergroup", "title": "g"},
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "text": "thanks",
            "reply_to_message": {
                "message_id": 4, "date": 0,
                "chat": {"id": -1001, "type": "supergroup", "title": "g"},
                "from": {"id": 999, "is_bot": true, "first_name": "Bot"},
                "text": "here you go"
            }
        }))
        .unwrap();
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        assert_eq!(
            is_bot_addressed(&msg, &identity),
            Some(AddressKind::GroupReplyToBot)
        );
    }
}
