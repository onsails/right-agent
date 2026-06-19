//! Manual `/command` parser replacing teloxide's `BotCommands` derive.
//!
//! Behavior-preserving port: `parse` replicates teloxide-0.17's default
//! command parsing exactly (single-ASCII-space separator, case-sensitive
//! command names, untrimmed payload, `@username` targeting that rejects
//! mismatched bots). `visible` builds the `setMyCommands` rows, hiding the
//! `usage` command just like the prior `visible_bot_commands()` filter.

/// One of the bot's slash commands. Variants carrying a `String` hold the raw
/// (untrimmed) payload — everything after the first space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotCommand {
    Start(String),
    New(String),
    List,
    Switch(String),
    Mcp(String),
    Providers(String),
    SetFocus(String),
    Doctor,
    Model,
    Mode,
    ModeGroup,
    Dashboard,
    Debug(String),
    Cron(String),
    Allow(String),
    Deny(String),
    Allowed,
    AllowAll,
    DenyAll,
    Usage(String),
}

/// Command name → description pairs for `setMyCommands`, mirroring the
/// `dispatch.rs` `BotCommand` enum order. `usage` is intentionally absent —
/// it is the hidden command (matches the prior `visible_bot_commands()`
/// filter dropping `"usage"`).
const VISIBLE_COMMANDS: &[(&str, &str)] = &[
    ("start", "Start interacting with this agent"),
    ("new", "Start a new conversation"),
    ("list", "List all sessions"),
    ("switch", "Switch to another session"),
    ("mcp", "Open MCP dashboard"),
    ("providers", "Open providers dashboard"),
    ("set_focus", "Set the focus for this conversation"),
    ("doctor", "Run diagnostics"),
    ("model", "Switch Claude model (menu)"),
    ("mode", "Set response mode for this topic"),
    ("mode_group", "Set response mode for this group"),
    ("dashboard", "Open dashboard"),
    ("debug", "Toggle debug mode (on/off/status)"),
    ("cron", "Cron job status (list or detail)"),
    (
        "allow",
        "Add trusted user (reply to user, or /allow <user_id>)",
    ),
    ("deny", "Remove trusted user"),
    ("allowed", "List trusted users and opened groups"),
    ("allow_all", "Open this group for all members (group only)"),
    ("deny_all", "Close this group (group only)"),
];

/// Parse a Telegram message text into a [`BotCommand`].
///
/// Replicates teloxide-0.17's default parser:
/// 1. Split on the FIRST single ASCII space into command-token + payload.
/// 2. The token must start with `/`; strip exactly one leading `/`.
/// 3. An optional `@username` suffix must match `bot_username`
///    (case-insensitive) or the message is rejected (teloxide `WrongBotName`).
/// 4. The command name matches CASE-SENSITIVELY (teloxide does not lowercase
///    input — `/START` ≠ `/start`).
/// 5. The payload is the raw, UNTRIMMED remainder after the first space.
///
/// Returns `None` for non-commands, unknown commands, and `@username`
/// mismatches (which fall through to the text handler).
pub fn parse(text: &str, bot_username: &str) -> Option<BotCommand> {
    let mut split = text.splitn(2, ' ');
    let command_token = split.next()?;
    let payload = split.next().unwrap_or("");

    let stripped = command_token.strip_prefix('/')?;

    // Split the stripped token on '@' into name + optional @bot target.
    let mut at_split = stripped.splitn(2, '@');
    let name = at_split.next()?;
    if let Some(bot_part) = at_split.next()
        && !bot_part.eq_ignore_ascii_case(bot_username)
    {
        // teloxide `WrongBotName`: a command addressed to a different bot is
        // not our command.
        return None;
    }

    let payload = payload.to_owned();
    let command = match name {
        "start" => BotCommand::Start(payload),
        "new" => BotCommand::New(payload),
        "list" => BotCommand::List,
        "switch" => BotCommand::Switch(payload),
        "mcp" => BotCommand::Mcp(payload),
        "providers" => BotCommand::Providers(payload),
        "set_focus" => BotCommand::SetFocus(payload),
        "doctor" => BotCommand::Doctor,
        "model" => BotCommand::Model,
        "mode" => BotCommand::Mode,
        "mode_group" => BotCommand::ModeGroup,
        "dashboard" => BotCommand::Dashboard,
        "debug" => BotCommand::Debug(payload),
        "cron" => BotCommand::Cron(payload),
        "allow" => BotCommand::Allow(payload),
        "deny" => BotCommand::Deny(payload),
        "allowed" => BotCommand::Allowed,
        "allow_all" => BotCommand::AllowAll,
        "deny_all" => BotCommand::DenyAll,
        "usage" => BotCommand::Usage(payload),
        _ => return None,
    };
    Some(command)
}

/// Build the `setMyCommands` rows (visible commands only; `usage` is hidden).
pub fn visible() -> Vec<frankenstein::types::BotCommand> {
    VISIBLE_COMMANDS
        .iter()
        .map(|(command, description)| {
            frankenstein::types::BotCommand::builder()
                .command(*command)
                .description(*description)
                .build()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: &str = "right_bot";

    #[test]
    fn parses_simple_command() {
        assert_eq!(parse("/list", BOT), Some(BotCommand::List));
    }

    #[test]
    fn parses_command_with_payload() {
        assert_eq!(
            parse("/usage detail", BOT),
            Some(BotCommand::Usage("detail".into()))
        );
    }

    #[test]
    fn empty_payload_for_payload_command_without_args() {
        assert_eq!(parse("/new", BOT), Some(BotCommand::New(String::new())));
    }

    #[test]
    fn parses_snake_case_renames() {
        assert_eq!(
            parse("/set_focus now", BOT),
            Some(BotCommand::SetFocus("now".into()))
        );
        assert_eq!(parse("/mode_group", BOT), Some(BotCommand::ModeGroup));
        assert_eq!(parse("/allow_all", BOT), Some(BotCommand::AllowAll));
        assert_eq!(parse("/deny_all", BOT), Some(BotCommand::DenyAll));
    }

    #[test]
    fn payload_is_untrimmed_after_first_space() {
        // teloxide assigns the whole remainder verbatim; only the first space
        // is consumed as the separator.
        assert_eq!(parse("/cron  x", BOT), Some(BotCommand::Cron(" x".into())));
    }

    #[test]
    fn non_command_text_returns_none() {
        assert_eq!(parse("hello", BOT), None);
        assert_eq!(parse("", BOT), None);
    }

    #[test]
    fn unknown_command_returns_none() {
        assert_eq!(parse("/nope", BOT), None);
    }

    #[test]
    fn at_username_matching_ours_is_accepted() {
        assert_eq!(parse("/doctor@right_bot", BOT), Some(BotCommand::Doctor));
    }

    #[test]
    fn at_username_is_case_insensitive() {
        assert_eq!(parse("/doctor@Right_Bot", BOT), Some(BotCommand::Doctor));
    }

    #[test]
    fn at_username_for_other_bot_returns_none() {
        // teloxide WrongBotName: not our command, falls through to text.
        assert_eq!(parse("/doctor@other_bot", BOT), None);
    }

    #[test]
    fn command_name_is_case_sensitive() {
        // teloxide matches the command name case-sensitively.
        assert_eq!(parse("/START", BOT), None);
        assert_eq!(parse("/Doctor", BOT), None);
    }

    #[test]
    fn visible_excludes_usage_and_has_expected_rows() {
        let rows = visible();
        assert_eq!(rows.len(), 19);
        assert!(
            rows.iter().all(|r| r.command != "usage"),
            "usage must be hidden"
        );
        let start = rows
            .iter()
            .find(|r| r.command == "start")
            .expect("start row present");
        assert_eq!(start.command, "start");
        assert_eq!(start.description, "Start interacting with this agent");
    }
}
