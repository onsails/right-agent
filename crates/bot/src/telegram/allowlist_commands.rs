//! Handlers for `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all`.
//!
//! Every handler is gated to trusted users only. Non-trusted senders'
//! commands are silently ignored (no reply, no warning — per spec §Command Routing Rules).

use chrono::Utc;
use frankenstein::types::{Message, MessageEntityType};
use right_agent::agent::allowlist::{
    self, AddOutcome, AllowedGroup, AllowedUser, AllowlistHandle, AllowlistState, RemoveOutcome,
    ResponseMode,
};

use super::msg_ext;
use super::router::HandlerCtx;
use super::tg_bot::TgError;

/// Who the sender intends to add/remove.
#[derive(Debug, Clone, PartialEq)]
pub enum UserTarget {
    NumericId(i64),
    TextMention {
        id: i64,
        name: Option<String>,
    },
    Reply {
        id: i64,
        name: Option<String>,
    },
    /// `@username` mention without entity-level user_id — unresolvable.
    UnresolvableUsername(String),
    None,
}

pub fn resolve_user_target(msg: &Message, args: &str) -> UserTarget {
    // 1) reply-to-message
    if let Some(reply) = msg.reply_to_message.as_ref()
        && let Some(from) = reply.from.as_ref()
    {
        return UserTarget::Reply {
            id: from.id as i64,
            name: Some(msg_ext::full_name(from)),
        };
    }
    // 2) TextMention entity in this message
    if let Some(entities) = msg.entities.as_ref() {
        for e in entities {
            if e.type_field == MessageEntityType::TextMention
                && let Some(user) = e.user.as_ref()
            {
                return UserTarget::TextMention {
                    id: user.id as i64,
                    name: Some(msg_ext::full_name(user)),
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

/// Persist the proposed `AllowlistState` atomically to disk, then swap it
/// into the in-memory handle on success.
///
/// Order matters: if we mutated the in-memory state first and the disk write
/// failed, the filter would honor an entry that is not on disk until the
/// watcher reload fires — a security-relevant consistency hole.
pub(crate) async fn persist_new(
    handle: &AllowlistHandle,
    agent_dir: &std::path::Path,
    new_state: AllowlistState,
) -> Result<(), String> {
    let file = new_state.to_file();
    let dir = agent_dir.to_path_buf();
    tokio::task::spawn_blocking(move || allowlist::write_file(&dir, &file))
        .await
        .map_err(|e| format!("join: {e:#}"))??;
    *handle.0.write().expect("allowlist lock poisoned") = new_state;
    Ok(())
}

/// Read, mutate, and write `allowlist.yaml` while holding the allowlist file
/// lock, then swap the persisted state into memory before releasing the lock.
pub(crate) async fn update_locked<R, F>(
    handle: &AllowlistHandle,
    agent_dir: &std::path::Path,
    f: F,
) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&mut AllowlistState) -> R + Send + 'static,
{
    let dir = agent_dir.to_path_buf();
    let handle = handle.clone();
    let result = tokio::task::spawn_blocking(move || {
        allowlist::with_lock(&dir, |d| {
            let file = allowlist::read_file(d)?.unwrap_or_default();
            let mut state = AllowlistState::from_file(file);
            let result = f(&mut state);
            allowlist::write_file_inner(d, &state.to_file())?;
            *handle.0.write().expect("allowlist lock poisoned") = state;
            Ok(result)
        })
    })
    .await
    .map_err(|e| format!("join: {e:#}"))??;
    Ok(result)
}

/// Trusted-only gate. Returns true when the sender is in the trusted-users allowlist.
pub(crate) fn sender_is_trusted(msg: &Message, allowlist: &AllowlistHandle) -> bool {
    let Some(sender) = msg.from.as_ref() else {
        return false;
    };
    allowlist
        .0
        .read()
        .expect("allowlist lock poisoned")
        .is_user_trusted(sender.id as i64)
}

async fn reply(bot: &super::BotType, msg: &Message, text: &str) -> Result<(), TgError> {
    bot.send_message_opts(
        msg.chat.id,
        text,
        false,
        msg.message_thread_id,
        Some(msg.message_id),
        None,
    )
    .await?;
    Ok(())
}

pub async fn handle_allow(ctx: &HandlerCtx, msg: &Message, args: String) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let allowlist = &ctx.allowlist;
    let agent_dir = &ctx.agent_dir;
    if !sender_is_trusted(msg, allowlist) {
        tracing::debug!("/allow ignored: non-trusted sender");
        return Ok(());
    }

    let target = resolve_user_target(msg, &args);
    let (id, label) = match target {
        UserTarget::NumericId(id) => (id, None),
        UserTarget::Reply { id, name } | UserTarget::TextMention { id, name } => (id, name),
        UserTarget::UnresolvableUsername(u) => {
            reply(bot, msg,
                &format!(
                    "\u{2717} cannot resolve @{u} — reply to their message or use numeric user_id"
                ),
            )
            .await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(bot, msg,
                "\u{2717} usage: /allow (reply to user) or /allow <user_id>",
            )
            .await?;
            return Ok(());
        }
    };

    // Reject negative IDs (groups/channels use /allow_all).
    if id < 0 {
        reply(bot, msg,
            "\u{2717} user_id cannot be negative (groups/channels use /allow_all)",
        )
        .await?;
        return Ok(());
    }

    let (outcome, new_state) = {
        let current = allowlist.0.read().expect("allowlist lock poisoned").clone();
        let mut next = current;
        let outcome = next.add_user(AllowedUser {
            id,
            label: label.clone(),
            added_by: msg.from.as_ref().map(|u| u.id as i64),
            added_at: Utc::now(),
        });
        (outcome, next)
    };

    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist_new(allowlist, &agent_dir.0, new_state).await {
                tracing::error!(error = %e, "allowlist persist failed for /allow");
                reply(bot, msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            let disp = label.unwrap_or_else(|| id.to_string());
            reply(bot, msg,
                &format!("\u{2713} allowed user {disp} (id {id})"),
            )
            .await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(bot, msg,
                &format!("\u{2713} user {id} already in allowlist"),
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn handle_deny(ctx: &HandlerCtx, msg: &Message, args: String) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let allowlist = &ctx.allowlist;
    let agent_dir = &ctx.agent_dir;
    if !sender_is_trusted(msg, allowlist) {
        tracing::debug!("/deny ignored: non-trusted sender");
        return Ok(());
    }

    let target = resolve_user_target(msg, &args);
    let id = match target {
        UserTarget::NumericId(id) => id,
        UserTarget::Reply { id, .. } | UserTarget::TextMention { id, .. } => id,
        UserTarget::UnresolvableUsername(u) => {
            reply(bot, msg,
                &format!(
                    "\u{2717} cannot resolve @{u} — reply to their message or use numeric user_id"
                ),
            )
            .await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(bot, msg,
                "\u{2717} usage: /deny (reply to user) or /deny <user_id>",
            )
            .await?;
            return Ok(());
        }
    };

    // Self-deny rejection.
    if let Some(from) = msg.from.as_ref()
        && from.id as i64 == id
    {
        reply(bot, msg,
            "\u{2717} cannot deny yourself — add another trusted user first",
        )
        .await?;
        return Ok(());
    }

    let (outcome, new_state) = {
        let current = allowlist.0.read().expect("allowlist lock poisoned").clone();
        let mut next = current;
        let outcome = next.remove_user(id);
        (outcome, next)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist_new(allowlist, &agent_dir.0, new_state).await {
                tracing::error!(error = %e, "allowlist persist failed for /deny");
                reply(bot, msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(bot, msg, &format!("\u{2713} user {id} removed")).await?;
        }
        RemoveOutcome::NotFound => {
            reply(bot, msg, &format!("\u{2717} user {id} not in allowlist")).await?;
        }
    }
    Ok(())
}

pub async fn handle_allowed(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let allowlist = &ctx.allowlist;
    if !sender_is_trusted(msg, allowlist) {
        tracing::debug!("/allowed ignored: non-trusted sender");
        return Ok(());
    }

    let file = {
        let state = allowlist.0.read().expect("allowlist lock poisoned");
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
    bot.send_message_opts(msg.chat.id, &text, true, msg.message_thread_id, None, None)
        .await?;
    Ok(())
}

pub async fn handle_allow_all(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let allowlist = &ctx.allowlist;
    let agent_dir = &ctx.agent_dir;
    if !sender_is_trusted(msg, allowlist) {
        tracing::debug!("/allow_all ignored: non-trusted sender");
        return Ok(());
    }

    if msg_ext::is_private(&msg.chat) {
        reply(bot, msg, "\u{2717} /allow_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let label = msg_ext::chat_title(&msg.chat).map(|s| s.to_string());
    let (outcome, new_state) = {
        let current = allowlist.0.read().expect("allowlist lock poisoned").clone();
        let mut next = current;
        let outcome = next.add_group(AllowedGroup {
            id: chat_id,
            label: label.clone(),
            opened_by: msg.from.as_ref().map(|u| u.id as i64),
            opened_at: Utc::now(),
            mode: ResponseMode::Addressed,
            topics: Vec::new(),
        });
        (outcome, next)
    };
    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist_new(allowlist, &agent_dir.0, new_state).await {
                tracing::error!(error = %e, "allowlist persist failed for /allow_all");
                reply(bot, msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(bot, msg, "\u{2713} group opened").await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(bot, msg, "\u{2713} group already opened").await?;
        }
    }
    Ok(())
}

pub async fn handle_deny_all(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let allowlist = &ctx.allowlist;
    let agent_dir = &ctx.agent_dir;
    if !sender_is_trusted(msg, allowlist) {
        tracing::debug!("/deny_all ignored: non-trusted sender");
        return Ok(());
    }

    if msg_ext::is_private(&msg.chat) {
        reply(bot, msg, "\u{2717} /deny_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let (outcome, new_state) = {
        let current = allowlist.0.read().expect("allowlist lock poisoned").clone();
        let mut next = current;
        let outcome = next.remove_group(chat_id);
        (outcome, next)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist_new(allowlist, &agent_dir.0, new_state).await {
                tracing::error!(error = %e, "allowlist persist failed for /deny_all");
                reply(bot, msg, &format!("\u{2717} persist failed: {e}")).await?;
                return Ok(());
            }
            reply(bot, msg, "\u{2713} group closed").await?;
        }
        RemoveOutcome::NotFound => {
            reply(bot, msg, "\u{2713} group was not opened").await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_agent::agent::allowlist::AllowlistFile;
    use frankenstein::types::Message;

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

    fn opened_state(chat_id: i64, mode: ResponseMode) -> AllowlistState {
        AllowlistState::from_file(AllowlistFile {
            groups: vec![AllowedGroup {
                id: chat_id,
                label: Some("group".to_string()),
                opened_by: Some(1),
                opened_at: Utc::now(),
                mode,
                topics: Vec::new(),
            }],
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn resolve_numeric_id() {
        let m = dm_msg(1, "42");
        assert_eq!(resolve_user_target(&m, "42"), UserTarget::NumericId(42));
    }

    #[tokio::test]
    async fn resolve_empty_args() {
        let m = dm_msg(1, "");
        assert_eq!(resolve_user_target(&m, ""), UserTarget::None);
    }

    #[tokio::test]
    async fn resolve_unresolvable_username() {
        let m = dm_msg(1, "@someone");
        match resolve_user_target(&m, "@someone") {
            UserTarget::UnresolvableUsername(u) => assert_eq!(u, "someone"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_locked_keeps_file_lock_until_memory_publish() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let chat_id = -100;
        let initial = opened_state(chat_id, ResponseMode::Addressed);
        allowlist::write_file(dir.path(), &initial.to_file()).unwrap();
        let handle = AllowlistHandle::new(initial);

        let memory_guard = handle.0.write().expect("allowlist lock poisoned");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let update_handle = handle.clone();
        let update_dir = dir.path().to_path_buf();
        let update = tokio::spawn(async move {
            update_locked(&update_handle, &update_dir, move |state| {
                entered_tx.send(()).unwrap();
                continue_rx.recv().expect("test released update");
                state.set_group_mode(chat_id, ResponseMode::All)
            })
            .await
        });

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("update entered locked mutation");

        let (lock_acquired_tx, lock_acquired_rx) = mpsc::channel();
        let probe_dir = dir.path().to_path_buf();
        let probe = std::thread::spawn(move || {
            allowlist::with_lock(&probe_dir, |_| {
                lock_acquired_tx.send(()).unwrap();
                Ok(())
            })
            .unwrap();
        });

        continue_tx.send(()).unwrap();
        assert!(
            lock_acquired_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "allowlist file lock released before in-memory publish completed"
        );

        drop(memory_guard);
        assert!(update.await.unwrap().unwrap());
        lock_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("probe acquired lock after memory publish completed");
        probe.join().unwrap();

        let state = handle.0.read().expect("allowlist lock poisoned");
        assert_eq!(
            state
                .groups()
                .iter()
                .find(|g| g.id == chat_id)
                .map(|g| g.mode),
            Some(ResponseMode::All)
        );
    }
}
