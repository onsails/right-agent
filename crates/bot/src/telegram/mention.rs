//! Detect whether a group message addresses the bot, and prepare the
//! cleaned-up prompt text.

use teloxide::types::{Message, MessageEntityKind};

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
    GroupMentionText,       // `@botname` in text
    GroupMentionEntity,     // TextMention entity pointing at bot user_id
    GroupReplyToBot,        // reply_to_message is from bot
    GroupSlashCommand,      // /cmd@botname (or any cmd in a group-to-bot)
}

/// Returns `Some(AddressKind)` when the message should be treated as addressed
/// to the bot; `None` in groups where the message is unrelated.
pub fn is_bot_addressed(msg: &Message, identity: &BotIdentity) -> Option<AddressKind> {
    use teloxide::types::ChatKind;
    match &msg.chat.kind {
        ChatKind::Private(_) => Some(AddressKind::DirectMessage),
        _ => {
            // 1) reply to bot's message
            if let Some(reply) = msg.reply_to_message()
                && let Some(from) = reply.from.as_ref()
                && from.id.0 == identity.user_id
            {
                return Some(AddressKind::GroupReplyToBot);
            }

            // 2) parse entities with correct UTF-8 offsets (teloxide converts
            //    from UTF-16 code units). Use text entities first, fall back
            //    to caption entities.
            let entities = msg.parse_entities().or_else(|| msg.parse_caption_entities());
            if let Some(entities) = entities {
                for e in entities {
                    match e.kind() {
                        MessageEntityKind::TextMention { user }
                            if user.id.0 == identity.user_id =>
                        {
                            return Some(AddressKind::GroupMentionEntity);
                        }
                        MessageEntityKind::Mention => {
                            // Slice is e.g. "@botname"; compare case-insensitively.
                            if e.text()
                                .strip_prefix('@')
                                .map(|u| u.eq_ignore_ascii_case(&identity.username))
                                .unwrap_or(false)
                            {
                                return Some(AddressKind::GroupMentionText);
                            }
                        }
                        MessageEntityKind::BotCommand => {
                            let slice = e.text();
                            // Accept /cmd (no suffix — only one bot in chat or we're the default)
                            // or /cmd@botname (explicit).
                            if let Some((_, maybe_user)) = slice.split_once('@') {
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
    }
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

    #[test]
    fn strip_removes_bot_mention() {
        assert_eq!(
            strip_bot_mentions("@rightclaw_bot hello", "rightclaw_bot"),
            "hello"
        );
        assert_eq!(
            strip_bot_mentions("hey @rightclaw_bot how are you", "rightclaw_bot"),
            "hey how are you"
        );
    }

    #[test]
    fn strip_leaves_other_mentions() {
        assert_eq!(
            strip_bot_mentions("@alice says hi to @rightclaw_bot", "rightclaw_bot"),
            "@alice says hi to"
        );
    }

    #[test]
    fn strip_is_case_insensitive() {
        assert_eq!(
            strip_bot_mentions("@RightClaw_Bot hi", "rightclaw_bot"),
            "hi"
        );
    }

    #[test]
    fn strip_preserves_newlines() {
        let input = "@rightclaw_bot hello\nline two\nline three";
        assert_eq!(
            strip_bot_mentions(input, "rightclaw_bot"),
            "hello\nline two\nline three"
        );
    }

    #[test]
    fn dm_returns_direct_message() {
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "U"},
            "from": {"id": 1, "is_bot": false, "first_name": "U"},
            "text": "hi"
        }))
        .unwrap();
        let identity = BotIdentity { username: "rightclaw_bot".into(), user_id: 999 };
        assert_eq!(is_bot_addressed(&msg, &identity), Some(AddressKind::DirectMessage));
    }

    #[test]
    fn group_non_mention_returns_none() {
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": -1001, "type": "group", "title": "g"},
            "from": {"id": 1, "is_bot": false, "first_name": "U"},
            "text": "just chatting"
        }))
        .unwrap();
        let identity = BotIdentity { username: "rightclaw_bot".into(), user_id: 999 };
        assert_eq!(is_bot_addressed(&msg, &identity), None);
    }
}
