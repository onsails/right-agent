//! `/debug` command — toggle hot-reloadable debug flag.
//!
//! UI: text-only command (`/debug`, `/debug on`, `/debug off`). No inline
//! keyboard — the option set is binary, no point in a 2-button menu.
//!
//! Persistence: writes `agent.yaml::debug` via
//! `right_agent::agent::types::write_agent_yaml_debug`.
//! In-memory: stores into `AgentSettings.debug: Arc<AtomicBool>`.
//! Group chats are gated by the trusted-users allowlist.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugAction {
    Status,
    On,
    Off,
}

/// Parse the optional argument after `/debug`. Trims whitespace and
/// is case-insensitive. Empty / missing → Status.
pub(crate) fn parse_debug_action(args: &str) -> Result<DebugAction, String> {
    let s = args.trim().to_ascii_lowercase();
    match s.as_str() {
        "" => Ok(DebugAction::Status),
        "on" | "true" | "1" => Ok(DebugAction::On),
        "off" | "false" | "0" => Ok(DebugAction::Off),
        other => Err(format!(
            "Unknown argument: {other}. Use `/debug on`, `/debug off`, or `/debug` (status)."
        )),
    }
}

/// Format the status reply when no action was given. Includes a hint about
/// the per-session debug log file when present.
pub(crate) fn render_status(debug_on: bool, current_log_size: Option<u64>) -> String {
    if debug_on {
        let log_part = match current_log_size {
            Some(size) => format!("\n\nCurrent session log: {size} bytes."),
            None => "\n\nNo log written yet for the current session.".to_string(),
        };
        format!(
            "🐛 Debug mode is ON.\n\n\
             Future `claude -p` invocations will write API/transport logs to \
             `/sandbox/.claude/logs/<session>.log`. Use `/debug off` to disable.\
             {log_part}"
        )
    } else {
        "🐛 Debug mode is OFF.\n\n\
         Use `/debug on` to enable per-session API/transport logs at \
         `/sandbox/.claude/logs/<session>.log`. Existing CC project history at \
         `/sandbox/.claude/projects/-sandbox/*.jsonl` is always written; debug \
         mode adds deeper API-layer detail."
            .to_string()
    }
}

/// Format the reply after a successful toggle.
pub(crate) fn render_toggle(new_value: bool) -> String {
    if new_value {
        "🐛 Debug mode ON. Future turns will write API/transport logs to \
         `/sandbox/.claude/logs/<session>.log`. Past turns are unchanged."
            .to_string()
    } else {
        "🐛 Debug mode OFF. Existing logs remain.".to_string()
    }
}

/// Apply a `DebugAction`: persist to yaml, flip the AtomicBool. Returns
/// the message to show the user. Persists BEFORE swapping in-memory so that
/// a disk failure leaves runtime untouched.
pub(crate) fn apply_action(
    action: DebugAction,
    flag: &Arc<AtomicBool>,
    agent_yaml_path: &std::path::Path,
    current_log_size: Option<u64>,
) -> Result<String, String> {
    match action {
        DebugAction::Status => Ok(render_status(
            flag.load(Ordering::Relaxed),
            current_log_size,
        )),
        DebugAction::On | DebugAction::Off => {
            let new_value = action == DebugAction::On;
            right_agent::agent::types::write_agent_yaml_debug(agent_yaml_path, Some(new_value))
                .map_err(|e| format!("Failed to save debug flag: {e:#}"))?;
            flag.store(new_value, Ordering::Release);
            Ok(render_toggle(new_value))
        }
    }
}

/// teloxide handler — registered in dispatch.rs. The `args` String comes from
/// `BotCommand::Debug(args)` (whitespace-trimmed by teloxide).
pub(crate) async fn handle_debug(
    bot: super::BotType,
    msg: teloxide::types::Message,
    args: String,
    settings: std::sync::Arc<super::handler::AgentSettings>,
    agent_dir: std::sync::Arc<super::handler::AgentDir>,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
) -> teloxide::prelude::ResponseResult<()> {
    if !super::handler::is_private_chat(&msg.chat.kind)
        && !super::allowlist_commands::sender_is_trusted(&msg, &allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/debug ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let action = match parse_debug_action(&args) {
        Ok(a) => a,
        Err(e) => {
            send_reply(&bot, &msg, &e).await?;
            return Ok(());
        }
    };

    // For the status response we want the size of the current session's log.
    // Worker tracks the active CC session_id per chat; reading it here would
    // require plumbing. As a simpler proxy, read the most-recent file size
    // in /sandbox/.claude/logs/ (sandbox-side via SSH would be needed —
    // not worth the complexity for the status hint). Pass None.
    let current_log_size: Option<u64> = None;

    let agent_yaml_path = agent_dir.0.join("agent.yaml");
    let reply = match apply_action(action, &settings.debug, &agent_yaml_path, current_log_size) {
        Ok(s) => s,
        Err(e) => e,
    };

    send_reply(&bot, &msg, &reply).await?;
    Ok(())
}

async fn send_reply(
    bot: &super::BotType,
    msg: &teloxide::types::Message,
    text: &str,
) -> teloxide::prelude::ResponseResult<()> {
    use teloxide::prelude::*;
    let mut send = bot
        .send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_no_arg_is_status() {
        assert_eq!(parse_debug_action("").unwrap(), DebugAction::Status);
        assert_eq!(parse_debug_action("   ").unwrap(), DebugAction::Status);
    }

    #[tokio::test]
    async fn parse_on_synonyms() {
        assert_eq!(parse_debug_action("on").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("ON").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action(" on ").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("true").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("1").unwrap(), DebugAction::On);
    }

    #[tokio::test]
    async fn parse_off_synonyms() {
        assert_eq!(parse_debug_action("off").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("OFF").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("false").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("0").unwrap(), DebugAction::Off);
    }

    #[tokio::test]
    async fn parse_unknown_is_error() {
        let err = parse_debug_action("toggle").unwrap_err();
        assert!(err.contains("Unknown argument"));
        assert!(err.contains("toggle"));
    }

    #[tokio::test]
    async fn status_off_explains_what_on_would_do() {
        let s = render_status(false, None);
        assert!(s.contains("OFF"));
        assert!(s.contains("/debug on"));
        assert!(s.contains("/sandbox/.claude/logs/"));
    }

    #[tokio::test]
    async fn status_on_with_log_size_reports_bytes() {
        let s = render_status(true, Some(2048));
        assert!(s.contains("ON"));
        assert!(s.contains("2048 bytes"));
    }

    #[tokio::test]
    async fn status_on_without_log_says_so() {
        let s = render_status(true, None);
        assert!(s.contains("ON"));
        assert!(s.contains("No log written"));
    }

    #[tokio::test]
    async fn toggle_on_message_mentions_future_turns() {
        let s = render_toggle(true);
        assert!(s.contains("ON"));
        assert!(s.contains("Future turns"));
    }

    #[tokio::test]
    async fn toggle_off_message_mentions_existing_logs() {
        let s = render_toggle(false);
        assert!(s.contains("OFF"));
        assert!(s.contains("Existing logs remain"));
    }
}
