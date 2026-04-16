//! Handlers for `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all`.
//!
//! Every handler is gated to trusted users only. Non-trusted senders'
//! commands are silently ignored (no reply, no warning — per spec §Command Routing Rules).

use std::sync::Arc;

use chrono::Utc;
use rightclaw::agent::allowlist::{
    self, AddOutcome, AllowedGroup, AllowedUser, AllowlistHandle, RemoveOutcome,
};
use teloxide::RequestError;
use teloxide::prelude::*;
use teloxide::types::{ChatKind, Message, MessageEntityKind};

use super::BotType;
use super::handler::AgentDir;

/// Who the sender intends to add/remove.
#[derive(Debug, Clone, PartialEq)]
pub enum UserTarget {
    NumericId(i64),
    TextMention { id: i64, name: Option<String> },
    Reply { id: i64, name: Option<String> },
    /// `@username` mention without entity-level user_id — unresolvable.
    UnresolvableUsername(String),
    None,
}

pub fn resolve_user_target(msg: &Message, args: &str) -> UserTarget {
    // 1) reply-to-message
    if let Some(reply) = msg.reply_to_message()
        && let Some(from) = reply.from.as_ref()
    {
        return UserTarget::Reply {
            id: from.id.0 as i64,
            name: Some(from.full_name()),
        };
    }
    // 2) TextMention entity in this message
    if let Some(entities) = msg.entities() {
        for e in entities {
            if let MessageEntityKind::TextMention { user } = &e.kind {
                return UserTarget::TextMention {
                    id: user.id.0 as i64,
                    name: Some(user.full_name()),
                };
            }
        }
    }
    // 3) numeric arg
    let trimmed = args.trim();
    if let Ok(id) = trimmed.parse::<i64>() {
        return UserTarget::NumericId(id);
    }
    // 4) @username literal, no entity-level id — unresolvable
    if let Some(u) = trimmed.strip_prefix('@').filter(|s| !s.is_empty()) {
        return UserTarget::UnresolvableUsername(u.to_string());
    }
    UserTarget::None
}

/// Persist the current `AllowlistState` atomically to disk (under the lock).
async fn persist(handle: &AllowlistHandle, agent_dir: &std::path::Path) -> Result<(), String> {
    let file = {
        let state = handle.0.read().await;
        state.to_file()
    };
    let dir = agent_dir.to_path_buf();
    tokio::task::spawn_blocking(move || allowlist::write_file(&dir, &file))
        .await
        .map_err(|e| format!("join: {e:#}"))?
}

/// Trusted-only gate. Returns true when the sender is in the trusted-users allowlist.
async fn sender_is_trusted(msg: &Message, allowlist: &AllowlistHandle) -> bool {
    let Some(sender) = msg.from.as_ref() else {
        return false;
    };
    let state = allowlist.0.read().await;
    state.is_user_trusted(sender.id.0 as i64)
}

async fn reply(bot: &BotType, msg: &Message, text: &str) -> Result<(), RequestError> {
    bot.send_message(msg.chat.id, text)
        .reply_parameters(teloxide::types::ReplyParameters {
            message_id: msg.id,
            ..Default::default()
        })
        .await?;
    Ok(())
}

pub async fn handle_allow(
    bot: BotType,
    msg: Message,
    args: String,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !sender_is_trusted(&msg, &allowlist).await {
        tracing::debug!("/allow ignored: non-trusted sender");
        return Ok(());
    }

    let target = resolve_user_target(&msg, &args);
    let (id, label) = match target {
        UserTarget::NumericId(id) => (id, None),
        UserTarget::Reply { id, name } | UserTarget::TextMention { id, name } => (id, name),
        UserTarget::UnresolvableUsername(u) => {
            reply(
                &bot,
                &msg,
                &format!(
                    "\u{2717} cannot resolve @{u} — reply to their message or use numeric user_id"
                ),
            )
            .await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(
                &bot,
                &msg,
                "\u{2717} usage: /allow (reply to user) or /allow <user_id>",
            )
            .await?;
            return Ok(());
        }
    };

    // Reject negative IDs (groups/channels use /allow_all).
    if id < 0 {
        reply(
            &bot,
            &msg,
            "\u{2717} user_id cannot be negative (groups/channels use /allow_all)",
        )
        .await?;
        return Ok(());
    }

    let outcome = {
        let mut w = allowlist.0.write().await;
        w.add_user(AllowedUser {
            id,
            label: label.clone(),
            added_by: msg.from.as_ref().map(|u| u.id.0 as i64),
            added_at: Utc::now(),
        })
    };

    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            let disp = label.unwrap_or_else(|| id.to_string());
            reply(&bot, &msg, &format!("\u{2713} allowed user {disp} (id {id})")).await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(&bot, &msg, &format!("\u{2713} user {id} already in allowlist")).await?;
        }
    }
    Ok(())
}

pub async fn handle_deny(
    bot: BotType,
    msg: Message,
    args: String,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !sender_is_trusted(&msg, &allowlist).await {
        tracing::debug!("/deny ignored: non-trusted sender");
        return Ok(());
    }

    let target = resolve_user_target(&msg, &args);
    let id = match target {
        UserTarget::NumericId(id) => id,
        UserTarget::Reply { id, .. } | UserTarget::TextMention { id, .. } => id,
        UserTarget::UnresolvableUsername(u) => {
            reply(
                &bot,
                &msg,
                &format!(
                    "\u{2717} cannot resolve @{u} — reply to their message or use numeric user_id"
                ),
            )
            .await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(
                &bot,
                &msg,
                "\u{2717} usage: /deny (reply to user) or /deny <user_id>",
            )
            .await?;
            return Ok(());
        }
    };

    // Self-deny rejection.
    if let Some(from) = msg.from.as_ref()
        && from.id.0 as i64 == id
    {
        reply(
            &bot,
            &msg,
            "\u{2717} cannot deny yourself — add another trusted user first",
        )
        .await?;
        return Ok(());
    }

    let outcome = {
        let mut w = allowlist.0.write().await;
        w.remove_user(id)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, &format!("\u{2713} user {id} removed")).await?;
        }
        RemoveOutcome::NotFound => {
            reply(&bot, &msg, &format!("\u{2717} user {id} not in allowlist")).await?;
        }
    }
    Ok(())
}

pub async fn handle_allowed(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if !sender_is_trusted(&msg, &allowlist).await {
        tracing::debug!("/allowed ignored: non-trusted sender");
        return Ok(());
    }

    let file = {
        let state = allowlist.0.read().await;
        state.to_file()
    };
    let mut text = String::from("<b>Trusted users:</b>\n");
    if file.users.is_empty() {
        text.push_str("  (none)\n");
    } else {
        for u in &file.users {
            let label = u.label.as_deref().unwrap_or("");
            text.push_str(&format!("  • {} {}\n", u.id, label));
        }
    }
    text.push_str("\n<b>Opened groups:</b>\n");
    if file.groups.is_empty() {
        text.push_str("  (none)\n");
    } else {
        for g in &file.groups {
            let label = g.label.as_deref().unwrap_or("");
            text.push_str(&format!("  • {} {}\n", g.id, label));
        }
    }
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
    Ok(())
}

pub async fn handle_allow_all(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !sender_is_trusted(&msg, &allowlist).await {
        tracing::debug!("/allow_all ignored: non-trusted sender");
        return Ok(());
    }

    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        reply(&bot, &msg, "\u{2717} /allow_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let label = msg.chat.title().map(|s| s.to_string());
    let outcome = {
        let mut w = allowlist.0.write().await;
        w.add_group(AllowedGroup {
            id: chat_id,
            label: label.clone(),
            opened_by: msg.from.as_ref().map(|u| u.id.0 as i64),
            opened_at: Utc::now(),
        })
    };
    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, "\u{2713} group opened").await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(&bot, &msg, "\u{2713} group already opened").await?;
        }
    }
    Ok(())
}

pub async fn handle_deny_all(
    bot: BotType,
    msg: Message,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !sender_is_trusted(&msg, &allowlist).await {
        tracing::debug!("/deny_all ignored: non-trusted sender");
        return Ok(());
    }

    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        reply(&bot, &msg, "\u{2717} /deny_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let outcome = {
        let mut w = allowlist.0.write().await;
        w.remove_group(chat_id)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, "\u{2713} group closed").await?;
        }
        RemoveOutcome::NotFound => {
            reply(&bot, &msg, "\u{2713} group was not opened").await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use teloxide::types::Message;

    fn dm_msg(from_id: u64, text: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": from_id as i64, "type": "private", "first_name": "U"},
            "from": {"id": from_id, "is_bot": false, "first_name": "U"},
            "text": text
        }))
        .unwrap()
    }

    #[test]
    fn resolve_numeric_id() {
        let m = dm_msg(1, "42");
        assert_eq!(resolve_user_target(&m, "42"), UserTarget::NumericId(42));
    }

    #[test]
    fn resolve_empty_args() {
        let m = dm_msg(1, "");
        assert_eq!(resolve_user_target(&m, ""), UserTarget::None);
    }

    #[test]
    fn resolve_unresolvable_username() {
        let m = dm_msg(1, "@someone");
        match resolve_user_target(&m, "@someone") {
            UserTarget::UnresolvableUsername(u) => assert_eq!(u, "someone"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
