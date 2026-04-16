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
            let text_opt = msg.text().or(msg.caption()).unwrap_or("");
            let entities_opt = msg.entities().or(msg.caption_entities());

            // 1) reply to bot's message
            if let Some(reply) = msg.reply_to_message()
                && let Some(from) = reply.from.as_ref()
                && from.id.0 == identity.user_id
            {
                return Some(AddressKind::GroupReplyToBot);
            }

            if let Some(entities) = entities_opt {
                for e in entities {
                    match &e.kind {
                        MessageEntityKind::TextMention { user } if user.id.0 == identity.user_id => {
                            return Some(AddressKind::GroupMentionEntity);
                        }
                        MessageEntityKind::Mention => {
                            let start = e.offset;
                            let slice: String = text_opt.chars().skip(start).take(e.length).collect();
                            // Slice is e.g. "@botname"; compare case-insensitively.
                            if slice
                                .strip_prefix('@')
                                .map(|u| u.eq_ignore_ascii_case(&identity.username))
                                .unwrap_or(false)
                            {
                                return Some(AddressKind::GroupMentionText);
                            }
                        }
                        MessageEntityKind::BotCommand => {
                            let slice: String = text_opt.chars().skip(e.offset).take(e.length).collect();
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
pub fn strip_bot_mentions(text: &str, username: &str) -> String {
    let lower_user = username.to_ascii_lowercase();
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
            if candidate.eq_ignore_ascii_case(&lower_user) && !candidate.is_empty() {
                for _ in 0..candidate.chars().count() {
                    it.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim().to_string()
}

/// Parse a command string `/cmd[@botname] [args...]`.
/// Returns `(cmd, args_rest, addressed)` — `addressed` is `false` only when the
/// `@who` suffix names a *different* bot.
pub fn parse_bot_command<'a>(text: &'a str, username: &str) -> Option<(&'a str, &'a str, bool)> {
    let stripped = text.strip_prefix('/')?;
    let (head, rest) = stripped.split_once(char::is_whitespace).unwrap_or((stripped, ""));
    let (cmd, addressed) = match head.split_once('@') {
        Some((cmd, who)) => (cmd, who.eq_ignore_ascii_case(username)),
        None => (head, true),
    };
    Some((cmd, rest.trim_start(), addressed))
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
    fn parse_command_no_suffix() {
        let (cmd, args, addressed) = parse_bot_command("/allow 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert_eq!(args, "42");
        assert!(addressed);
    }

    #[test]
    fn parse_command_addressed_suffix() {
        let (cmd, args, addressed) = parse_bot_command("/allow@rightclaw_bot 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert_eq!(args, "42");
        assert!(addressed);
    }

    #[test]
    fn parse_command_different_bot() {
        let (cmd, _args, addressed) = parse_bot_command("/allow@otherbot 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert!(!addressed);
    }

    #[test]
    fn parse_command_bare() {
        let (cmd, args, _) = parse_bot_command("/allowed", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allowed");
        assert_eq!(args, "");
    }
}
