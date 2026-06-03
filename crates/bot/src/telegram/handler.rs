//! Teloxide endpoint handlers: message dispatch + /new, /list, /switch + /mcp + /cron + /doctor.
//!
//! handle_message: routes incoming text to the per-session worker via DashMap.
//! handle_new: deactivates current session, optionally creates a named one.
//! handle_list: shows all sessions for the current chat+thread.
//! handle_switch: switches to a different session by partial UUID match.
//! handle_mcp: opens the dashboard MCP view.
//! handle_doctor: runs right doctor and returns results.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dashmap::DashMap;
use right_agent::agent::allowlist::AllowlistHandle;
use teloxide::RequestError;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message};
use tokio::sync::mpsc;

use crate::cc::markdown_utils::{html_escape, strip_html_tags};

use super::BotType;
#[cfg(test)]
use super::ThinkingVisibility;
use super::oauth_callback::PendingAuthMap;
use super::session::{
    activate_session, create_session, deactivate_current, effective_thread_id,
    find_sessions_by_uuid, list_sessions, truncate_label,
};
use super::worker::{DebounceMsg, SessionKey, WorkerContext, spawn_worker};

/// Newtype wrapper for the agent directory passed via dptree dependencies.
/// Distinct from RightHome to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct AgentDir(pub PathBuf);

/// SSH config path for the agent's OpenShell sandbox.
#[derive(Clone)]
pub struct SshConfigPath(pub Option<PathBuf>);

/// Newtype wrapper for the right home directory passed via dptree dependencies.
/// Distinct from AgentDir to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct RightHome(pub PathBuf);

/// Compatibility dependency for the former Telegram MCP token prompt flow.
#[derive(Clone)]
pub struct PendingTokenSlot;

/// Compatibility dependency for the former Telegram MCP auth-choice prompt flow.
#[derive(Clone)]
pub struct PendingMcpAuthChoiceSlot;

/// Bundle of message-intercept slots to reduce dptree DI parameter count.
/// Contains the auth code intercept slot plus the auth-watcher-active flag.
#[derive(Clone)]
pub struct InterceptSlots {
    pub auth_code: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
    pub auth_watcher: Arc<AtomicBool>,
}

/// Newtype wrapper for the InternalClient used to communicate with the MCP aggregator.
#[derive(Clone)]
pub struct InternalApi(pub Arc<right_mcp::internal_client::InternalClient>);

/// Shared timestamp of last interaction (unix seconds).
/// Updated by handler on incoming messages and by worker after sending replies.
#[derive(Clone)]
pub struct IdleTimestamp(pub Arc<std::sync::atomic::AtomicI64>);

/// Bundled agent invocation settings (reduces dptree injectable arity).
#[derive(Clone)]
pub struct AgentSettings {
    pub show_thinking: bool,
    /// Claude model override (passed as --model). None = inherit CLI default.
    /// Lock-free swap cell — `/model` callback and `config_watcher` (model-only diff)
    /// store new values; CC invocations load on every call.
    pub model: std::sync::Arc<arc_swap::ArcSwap<Option<String>>>,
    /// Resolved sandbox name (None when running without sandbox).
    pub resolved_sandbox: Option<String>,
    /// Hindsight memory client (None when using file-based memory).
    pub hindsight: Option<std::sync::Arc<right_memory::ResilientHindsight>>,
    /// Prefetch cache for Hindsight recall results.
    pub prefetch_cache: Option<right_memory::prefetch::PrefetchCache>,
    /// RwLock gate — upgrade takes write (exclusive), CC invocations take read (shared).
    pub upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    /// Hot-reloadable debug flag. When true, CC subprocesses run with --verbose,
    /// stderr is logged at debug level, AND `claude` runs with --debug --debug-file=...
    /// Updated by `/debug` Telegram command and config_watcher (yaml diff). Read on
    /// every CC invocation.
    pub debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// STT context — None when stt.enabled=false or whisper model not yet cached.
    pub stt: Option<std::sync::Arc<crate::stt::SttContext>>,
    /// Learning-review configuration captured at bot startup. Changes require restart.
    pub learning: right_agent::agent::types::LearningConfig,
    /// Shared Claude health state for MCP self-heal and one-shot repair notices.
    pub(crate) claude_health: Arc<crate::keepalive::ClaudeHealth>,
    /// Process shutdown token used to cancel detached user-turn repair work.
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
    /// Shared sandbox-backend health. Read before every sandboxed turn by the
    /// pre-invocation health gate (Task 9) to fail-closed when Unavailable.
    pub sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
}

/// Convert an arbitrary error into `RequestError::Io` so it propagates through `ResponseResult`.
fn to_request_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::Io(std::io::Error::other(e.to_string()).into())
}

/// True when the chat is a private (1:1) chat. Used by DM-only command gates.
pub(crate) fn is_private_chat(kind: &teloxide::types::ChatKind) -> bool {
    matches!(kind, teloxide::types::ChatKind::Private(_))
}

/// Send an HTML-formatted message, respecting thread_id for topic replies.
async fn send_html_reply(
    bot: &BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    text: &str,
) -> Result<teloxide::types::Message, RequestError> {
    let mut send = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
            eff_thread_id as i32,
        )));
    }
    send.await
}

/// Handle an incoming text message.
///
/// 1. Compute effective_thread_id (normalise General topic).
/// 2. Look up existing sender in DashMap or spawn a new worker task.
/// 3. Send the message into the worker's mpsc channel.
///
/// Serialisation guarantee (SES-05): all messages to the same (chat_id, thread_id)
/// go through the same mpsc channel -> worker processes them serially.
#[allow(clippy::too_many_arguments)]
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    decision: super::filter::RoutingDecision,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
    ssh_config: Arc<SshConfigPath>,
    intercept_slots: Arc<InterceptSlots>,
    settings: Arc<AgentSettings>,
    idle_ts: Arc<IdleTimestamp>,
    internal_api: Arc<InternalApi>,
    identity: Arc<super::mention::BotIdentity>,
    worker_ctl: super::WorkerControlDeps,
) -> ResponseResult<()> {
    idle_ts.0.store(
        chrono::Utc::now().timestamp(),
        std::sync::atomic::Ordering::Relaxed,
    );

    // Extract text from message body OR caption (media messages use captions)
    let text = msg.text().or(msg.caption()).map(|t| t.to_string());

    // Extract attachments from all media types
    let attachments = super::attachments::extract_attachments(&msg);

    // Skip messages with neither text nor attachments
    if text.is_none() && attachments.is_empty() {
        return Ok(());
    }

    // Extract author from sender
    let author = match msg.from.as_ref() {
        Some(user) => super::attachments::MessageAuthor {
            name: user.full_name(),
            username: user.username.as_ref().map(|u| format!("@{u}")),
            user_id: Some(user.id.0 as i64),
        },
        None => super::attachments::MessageAuthor {
            name: msg.chat.title().unwrap_or("unknown").to_owned(),
            username: msg.chat.username().map(|u| format!("@{u}")),
            user_id: None,
        },
    };

    // Extract forward origin
    let forward_info = msg.forward_origin().map(|origin| {
        use teloxide::types::MessageOrigin;
        let (from, date) = match origin {
            MessageOrigin::User { sender_user, date } => (
                super::attachments::MessageAuthor {
                    name: sender_user.full_name(),
                    username: sender_user.username.as_ref().map(|u| format!("@{u}")),
                    user_id: Some(sender_user.id.0 as i64),
                },
                *date,
            ),
            MessageOrigin::HiddenUser {
                sender_user_name,
                date,
            } => (
                super::attachments::MessageAuthor {
                    name: sender_user_name.clone(),
                    username: None,
                    user_id: None,
                },
                *date,
            ),
            MessageOrigin::Chat {
                sender_chat, date, ..
            } => (
                super::attachments::MessageAuthor {
                    name: sender_chat.title().unwrap_or("unknown").to_owned(),
                    username: sender_chat.username().map(|u| format!("@{u}")),
                    user_id: None,
                },
                *date,
            ),
            MessageOrigin::Channel { chat, date, .. } => (
                super::attachments::MessageAuthor {
                    name: chat.title().unwrap_or("unknown").to_owned(),
                    username: chat.username().map(|u| format!("@{u}")),
                    user_id: None,
                },
                *date,
            ),
        };
        super::attachments::ForwardInfo { from, date }
    });

    // Extract reply-to message ID
    let reply_to_id = msg.reply_to_message().map(|m| m.id.0);
    let quoted_text = msg.quote().map(|q| q.text.clone());

    // Intercept auth code: if login flow is waiting for a code, forward this message.
    if let Some(ref text_val) = text {
        let mut slot = intercept_slots.auth_code.lock().await;
        if let Some(sender) = slot.take() {
            tracing::info!("handle_message: forwarding message as auth code");
            let _ = sender.send(text_val.clone());
            return Ok(());
        }
    }

    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    super::archive::archive_routed_dm_message(&agent_dir.0, &msg, decision.address.clone());

    let key: SessionKey = (chat_id.0, eff_thread_id);
    let worker_exists = worker_map.contains_key(&key);
    tracing::info!(
        ?key,
        worker_exists,
        has_text = text.is_some(),
        attachment_count = attachments.len(),
        "handle_message: routing"
    );

    // Build ChatContext: DM emits nothing; Group emits id/title/topic_id.
    // General topic has thread_id = 1 in supergroups — normalise to "no topic".
    let chat_ctx = match &msg.chat.kind {
        teloxide::types::ChatKind::Private(_) => {
            super::attachments::ChatContext::Private { id: msg.chat.id.0 }
        }
        _ => super::attachments::ChatContext::Group {
            id: msg.chat.id.0,
            title: msg.chat.title().map(|s| s.to_string()),
            topic_id: msg.thread_id.map(|t| i64::from(t.0.0)).filter(|&n| n > 1),
        },
    };

    // Populate reply_to_body only when the user replied to a non-bot message.
    // When they reply to our own bot message, the context is already in the CC
    // session history — emitting it again would be noisy and duplicative.
    // `reply_to_attachments` mirrors `reply_to_body`: empty when the body is
    // None, otherwise the inbound attachments of the replied-to message.
    let (reply_to_body, reply_to_attachments) = match msg.reply_to_message() {
        Some(r) => match r.from.as_ref() {
            Some(from) if !(from.is_bot && from.id.0 == identity.user_id) => {
                let body = super::attachments::ReplyToBody {
                    author: super::attachments::MessageAuthor {
                        name: from.full_name(),
                        username: from.username.as_ref().map(|u| format!("@{u}")),
                        user_id: Some(from.id.0 as i64),
                    },
                    text: r.text().or(r.caption()).map(|t| t.to_string()),
                    attachments: vec![], // populated post-debounce in worker
                    omitted: false,
                };
                let inbound = super::attachments::extract_attachments(r);
                (Some(body), inbound)
            }
            _ => (None, vec![]),
        },
        None => (None, vec![]),
    };

    // Strip `@botname` mentions from text AFTER interceptors (auth code / MCP
    // token) have seen the raw string. No-op when the pattern isn't present.
    let text = text.map(|t| super::mention::strip_bot_mentions(&t, &identity.username));

    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
        author,
        forward_info,
        reply_to_id,
        quoted_text,
        address: decision.address.clone(),
        group_open: decision.group_open,
        chat: chat_ctx,
        reply_to_body,
        reply_to_attachments,
        media_group_id: msg.media_group_id().map(|m| m.0.clone()),
    };

    // Check for existing worker or spawn a new one.
    // Pitfall 7 mitigation: if send fails, the worker task has exited -- remove + respawn.
    // Note: DashMap read guard is NOT held across .await to avoid blocking. Clone the
    // sender before awaiting.
    loop {
        let maybe_tx = worker_map.get(&key).map(|entry| entry.value().clone());
        match maybe_tx {
            Some(tx) => match tx.send(debounce_msg.clone()).await {
                Ok(_) => break,
                Err(e) => {
                    // Worker task panicked or exited -- remove stale sender and respawn
                    tracing::warn!(?key, "worker send failed, respawning: {:#}", e);
                    worker_map.remove(&key);
                    // fall through to spawn new worker below on next loop iteration
                }
            },
            None => {
                // No sender yet -- spawn a new worker task
                let agent_name = agent_dir
                    .0
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let ctx = WorkerContext {
                    chat_id,
                    effective_thread_id: eff_thread_id,
                    agent_dir: agent_dir.0.clone(),
                    agent_name,
                    bot: bot.clone(),
                    agent_db_dir: agent_dir.0.clone(),
                    debug: Arc::clone(&settings.debug),
                    ssh_config_path: ssh_config.0.clone(),
                    resolved_sandbox: settings.resolved_sandbox.clone(),
                    auth_watcher_active: Arc::clone(&intercept_slots.auth_watcher),
                    auth_code_tx: Arc::clone(&intercept_slots.auth_code),
                    show_thinking: settings.show_thinking,
                    model: settings.model.clone(),
                    stop_tokens: Arc::clone(&worker_ctl.stop_tokens),
                    session_locks: Arc::clone(&worker_ctl.session_locks),
                    compact_timers: Arc::clone(&worker_ctl.compact_timers),
                    bg_requests: Arc::clone(&worker_ctl.bg_requests),
                    bg_handoff_gates: Arc::clone(&worker_ctl.bg_handoff_gates),
                    thinking_visibility: Arc::clone(&worker_ctl.thinking_visibility),
                    idle_timestamp: Arc::clone(&idle_ts.0),
                    internal_client: Arc::clone(&internal_api.0),
                    progress_state: worker_ctl.progress.clone(),
                    hindsight: settings.hindsight.clone(),
                    prefetch_cache: settings.prefetch_cache.clone(),
                    memory_status_last: Arc::new(DashMap::new()),
                    upgrade_lock: Arc::clone(&settings.upgrade_lock),
                    stt: settings.stt.clone(),
                    learning: settings.learning.clone(),
                    claude_health: Arc::clone(&settings.claude_health),
                    shutdown: settings.shutdown.clone(),
                    sandbox_runtime: Arc::clone(&settings.sandbox_runtime),
                };
                let tx = spawn_worker(key, ctx, Arc::clone(&worker_map));
                worker_map.insert(key, tx.clone());
                // Send to the freshly spawned worker
                if let Err(e) = tx.send(debounce_msg).await {
                    tracing::error!(?key, "send to freshly spawned worker failed: {:#}", e);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Handle the /start command.
///
/// Sends a greeting without invoking CC. Cron runtime starts automatically
/// alongside the bot -- no explicit bootstrap needed.
pub async fn handle_start(bot: BotType, msg: Message) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "start", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    bot.send_message(msg.chat.id, "Agent is running. Send a message to start.")
        .await?;
    Ok(())
}

/// Handle the /dashboard command -- send a Telegram Mini App launch button.
pub async fn handle_dashboard(
    bot: BotType,
    msg: Message,
    home: Arc<RightHome>,
    agent_dir: Arc<AgentDir>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind)
        && !super::allowlist_commands::sender_is_trusted(&msg, &allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|user| user.id.0),
            "/dashboard ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| to_request_err(format!("dashboard: read config.yaml: {e:#}")))?;
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            to_request_err(format!(
                "dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| to_request_err(format!("dashboard: invalid URL: {e:#}")))?;
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::web_app(
        "Open dashboard",
        teloxide::types::WebAppInfo { url },
    )]]);

    let mut send = bot
        .send_message(msg.chat.id, "Dashboard")
        .reply_markup(keyboard);
    let eff_thread_id = effective_thread_id(&msg);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
            eff_thread_id as i32,
        )));
    }
    send.await?;
    Ok(())
}

/// Handle the /new command — start a new session.
pub async fn handle_new(
    bot: BotType,
    msg: Message,
    name: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "new", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| to_request_err(format!("new: open DB: {:#}", e)))?;

    let prev_uuid = deactivate_current(&conn, chat_id.0, eff_thread_id)
        .await
        .map_err(|e| to_request_err(format!("new: deactivate: {:#}", e)))?;

    // Kill worker — channel closes, CC subprocess killed via kill_on_drop
    worker_map.remove(&key);

    let name = name.trim().to_string();
    let mut reply = String::new();

    if !name.is_empty() {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let label = truncate_label(&name);
        create_session(&conn, chat_id.0, eff_thread_id, &new_uuid, Some(label))
            .await
            .map_err(|e| to_request_err(format!("new: create session: {:#}", e)))?;
        reply.push_str(&format!("New session: {name}\n"));
    } else {
        reply.push_str("Session cleared.\n");
    }

    if let Some(prev) = prev_uuid {
        reply.push_str(&format!(
            "Previous session:\n<pre>/switch {prev}</pre>\nTap to copy to return."
        ));
    }

    if name.is_empty() {
        reply.push_str("\nSend a message to start a new conversation.");
    }

    send_html_reply(&bot, chat_id, eff_thread_id, &reply).await?;

    tracing::info!(?key, "new session");
    Ok(())
}

/// Handle the /list command — show all sessions for this chat+thread.
pub async fn handle_list(
    bot: BotType,
    msg: Message,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "list", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| to_request_err(format!("list: open DB: {:#}", e)))?;

    let sessions = list_sessions(&conn, chat_id.0, eff_thread_id)
        .await
        .map_err(|e| to_request_err(format!("list: query: {:#}", e)))?;

    if sessions.is_empty() {
        bot.send_message(chat_id, "No sessions yet. Send a message to start one.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Sessions:\n");
    for s in &sessions {
        text.push_str(&format_session_line(s));
    }

    send_html_reply(&bot, chat_id, eff_thread_id, &text).await?;
    Ok(())
}

/// Format a session row as an HTML line for /list and /switch display.
fn format_session_line(s: &super::session::SessionRow) -> String {
    let marker = if s.is_active { "●" } else { " " };
    let label = s.label.as_deref().unwrap_or("(unnamed)");
    let ago = format_relative_time(&s.last_used_at);
    format!(
        "{marker} {label} — {ago}\n<pre>{}</pre>\n",
        s.root_session_id
    )
}

/// Map cron run status to a Unicode icon.
fn status_icon(status: &str) -> &'static str {
    match status {
        "success" => "\u{2705}",
        "failed" => "\u{274c}",
        "running" => "\u{23f3}",
        _ => "?",
    }
}

/// Format an ISO timestamp as a relative time string.
fn format_relative_time(iso_timestamp: &str) -> String {
    let Ok(then) = chrono::NaiveDateTime::parse_from_str(iso_timestamp, "%Y-%m-%dT%H:%M:%SZ")
    else {
        return iso_timestamp.to_string();
    };
    let then_utc = then.and_utc();
    let now = chrono::Utc::now();
    let delta = now - then_utc;

    if delta.num_minutes() < 1 {
        "just now".to_string()
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

/// Handle the /switch command — switch to a different session.
pub async fn handle_switch(
    bot: BotType,
    msg: Message,
    uuid: String,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "switch", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let uuid = uuid.trim().to_string();

    if uuid.is_empty() {
        bot.send_message(
            chat_id,
            "Usage: /switch <uuid>\nUse /list to see available sessions.",
        )
        .await?;
        return Ok(());
    }

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| to_request_err(format!("switch: open DB: {:#}", e)))?;

    let matches = find_sessions_by_uuid(&conn, chat_id.0, eff_thread_id, &uuid)
        .await
        .map_err(|e| to_request_err(format!("switch: query: {:#}", e)))?;

    match matches.len() {
        0 => {
            send_html_reply(
                &bot,
                chat_id,
                eff_thread_id,
                &format!(
                    "No session matching <pre>{uuid}</pre>. Use /list to see available sessions."
                ),
            )
            .await?;
        }
        1 => {
            let target = &matches[0];
            if target.is_active {
                bot.send_message(chat_id, "Already active.").await?;
                return Ok(());
            }

            // activate_session atomically deactivates any other active session
            activate_session(&conn, target.id)
                .await
                .map_err(|e| to_request_err(format!("switch: activate: {:#}", e)))?;

            worker_map.remove(&key);

            let label = target.label.as_deref().unwrap_or("(unnamed)");
            send_html_reply(
                &bot,
                chat_id,
                eff_thread_id,
                &format!(
                    "Switched to: {label}\n<pre>{}</pre>",
                    target.root_session_id
                ),
            )
            .await?;

            tracing::info!(?key, session = %target.root_session_id, "switched session");
        }
        _ => {
            let mut text = format!("Multiple sessions match <pre>{uuid}</pre>:\n\n");
            for m in &matches {
                text.push_str(&format_session_line(m));
            }
            text.push_str("\nBe more specific.");
            send_html_reply(&bot, chat_id, eff_thread_id, &text).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /mcp command handler
// ---------------------------------------------------------------------------

fn dashboard_mcp_button_label() -> &'static str {
    "Open MCP dashboard"
}

/// Handle the /mcp command by opening the dashboard MCP view.
#[allow(clippy::too_many_arguments)]
pub async fn handle_mcp(
    bot: BotType,
    msg: Message,
    _args: String,
    agent_dir: Arc<AgentDir>,
    _pending_auth: PendingAuthMap,
    home: Arc<RightHome>,
    _internal: Arc<InternalApi>,
    _pending_token_slot: Arc<PendingTokenSlot>,
    _pending_auth_choice_slot: Arc<PendingMcpAuthChoiceSlot>,
    _ssh_config: Arc<SshConfigPath>,
    _settings: Arc<AgentSettings>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "mcp", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!(agent_dir = %agent_dir.0.display(), "mcp: opening dashboard");
    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| to_request_err(format!("mcp dashboard: read config.yaml: {e:#}")))?;
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            to_request_err(format!(
                "mcp dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| to_request_err(format!("mcp dashboard: invalid URL: {e:#}")))?;
    url.set_query(Some("view=mcp"));

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::web_app(
        dashboard_mcp_button_label(),
        teloxide::types::WebAppInfo { url },
    )]]);

    let mut send = bot.send_message(msg.chat.id, "MCP").reply_markup(keyboard);
    let eff_thread_id = effective_thread_id(&msg);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
            eff_thread_id as i32,
        )));
    }
    send.await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /providers command handler
// ---------------------------------------------------------------------------

/// Handle the /providers command by opening the dashboard providers view.
#[allow(clippy::too_many_arguments)]
pub async fn handle_providers(
    bot: BotType,
    msg: Message,
    _args: String,
    agent_dir: Arc<AgentDir>,
    _pending_auth: PendingAuthMap,
    home: Arc<RightHome>,
    _internal: Arc<InternalApi>,
    _pending_token_slot: Arc<PendingTokenSlot>,
    _pending_auth_choice_slot: Arc<PendingMcpAuthChoiceSlot>,
    _ssh_config: Arc<SshConfigPath>,
    _settings: Arc<AgentSettings>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(
            cmd = "providers",
            "ignoring command in group chat (DM-only)"
        );
        return Ok(());
    }
    // Sandbox-mode guard: providers are only valid for openshell-sandboxed agents.
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            to_request_err(format!(
                "providers dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let cfg = right_agent::agent::discovery::parse_agent_config(&agent_dir.0)
        .map_err(|e| to_request_err(format!("providers dashboard: load agent.yaml: {e:#}")))?;
    let mode = cfg
        .as_ref()
        .map(|c| *c.sandbox_mode())
        .unwrap_or(right_agent_config::SandboxMode::Openshell);
    if mode != right_agent_config::SandboxMode::Openshell {
        let _ = bot
            .send_message(
                msg.chat.id,
                "Providers are only available for sandboxed agents. This agent runs in host mode.",
            )
            .await;
        return Ok(());
    }
    tracing::info!(agent_dir = %agent_dir.0.display(), "providers: opening dashboard");
    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| to_request_err(format!("providers dashboard: read config.yaml: {e:#}")))?;
    let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| to_request_err(format!("providers dashboard: invalid URL: {e:#}")))?;
    url.set_query(Some("view=providers"));

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::web_app(
        "Open providers dashboard",
        teloxide::types::WebAppInfo { url },
    )]]);

    let mut send = bot
        .send_message(msg.chat.id, "Providers")
        .reply_markup(keyboard);
    let eff_thread_id = effective_thread_id(&msg);
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
            eff_thread_id as i32,
        )));
    }
    send.await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /cron command handler
// ---------------------------------------------------------------------------

/// Handle the /cron command — routes to list (no args) or detail (job name).
pub async fn handle_cron(
    bot: BotType,
    msg: Message,
    args: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "cron", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let result = if args.trim().is_empty() {
        handle_cron_list(&bot, &msg, &agent_dir.0).await
    } else {
        handle_cron_detail(&bot, &msg, args.trim(), &agent_dir.0).await
    };
    result.map_err(|e| to_request_err(format!("{e:#}")))?;
    Ok(())
}

/// `/cron` — list all cron jobs with human-readable schedule and last run status.
async fn handle_cron_list(
    bot: &BotType,
    msg: &Message,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let specs = right_agent::cron_spec::load_specs_from_db(&conn)
        .await
        .map_err(|e| to_request_err(format!("load specs failed: {e:#}")))?;

    if specs.is_empty() {
        bot.send_message(msg.chat.id, "No cron jobs configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Cron Jobs:\n\n");
    let mut names: Vec<&String> = specs.keys().collect();
    names.sort();

    for name in names {
        let spec = &specs[name];
        let desc = match &spec.schedule_kind {
            right_agent::cron_spec::ScheduleKind::RunAt(dt) => {
                html_escape(&format!("once at {}", dt.format("%Y-%m-%d %H:%M UTC")))
            }
            _ => html_escape(&right_agent::cron_spec::describe_schedule(
                spec.schedule_kind.cron_schedule().unwrap_or(""),
            )),
        };

        let last_run = right_agent::cron_spec::get_recent_runs(&conn, name, 1)
            .await
            .map_err(|e| to_request_err(format!("get runs failed: {e:#}")))?;

        let status_str = match last_run.first() {
            Some(run) => {
                let icon = status_icon(&run.status);
                let ago = format_relative_time(&run.started_at);
                format!("last: {ago} {icon}")
            }
            None => "never run".to_string(),
        };

        text.push_str(&format!(
            "\u{2022} {name} \u{2014} {desc} \u{2014} {status_str}\n"
        ));
    }

    let eff_thread_id = effective_thread_id(msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

/// `/cron <job-name>` — show job detail + last 5 runs.
async fn handle_cron_detail(
    bot: &BotType,
    msg: &Message,
    job_name: &str,
    agent_dir: &Path,
) -> Result<(), RequestError> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let detail = right_agent::cron_spec::get_spec_detail(&conn, job_name)
        .await
        .map_err(|e| to_request_err(format!("query failed: {e:#}")))?;

    let Some(detail) = detail else {
        bot.send_message(msg.chat.id, format!("Cron job '{job_name}' not found."))
            .await?;
        return Ok(());
    };

    let desc = html_escape(&right_agent::cron_spec::describe_schedule(&detail.schedule));
    let schedule_escaped = html_escape(&detail.schedule);
    let mut text = format!(
        "<b>{}</b>\nSchedule: {} (<code>{}</code>)\nBudget: ${:.2}",
        detail.job_name, desc, schedule_escaped, detail.max_budget_usd,
    );
    if let Some(ref ttl) = detail.lock_ttl {
        let ttl_escaped = html_escape(ttl);
        text.push_str(&format!("\nLock TTL: {ttl_escaped}"));
    }
    if detail.triggered_at.is_some() {
        text.push_str("\n\u{26a1} Trigger pending");
    }

    let runs = right_agent::cron_spec::get_recent_runs(&conn, job_name, 5)
        .await
        .map_err(|e| to_request_err(format!("get runs failed: {e:#}")))?;

    if runs.is_empty() {
        text.push_str("\n\nNo runs yet.");
    } else {
        text.push_str("\n\nRecent runs:");
        for (i, run) in runs.iter().enumerate() {
            let icon = status_icon(&run.status);
            let ago = format_relative_time(&run.started_at);
            let duration = match &run.finished_at {
                Some(end) => format_duration(&run.started_at, end),
                None => String::new(),
            };
            text.push_str(&format!(
                "\n  {}. {ago} \u{2014} {icon} {}{duration}",
                i + 1,
                run.status
            ));
        }
    }

    let eff_thread_id = effective_thread_id(msg);
    send_html_reply(bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

/// Format duration between two ISO 8601 timestamps (e.g. " (12s)", " (2m 30s)").
fn format_duration(start_iso: &str, end_iso: &str) -> String {
    let Ok(start) = chrono::NaiveDateTime::parse_from_str(start_iso, "%Y-%m-%dT%H:%M:%SZ") else {
        return String::new();
    };
    let Ok(end) = chrono::NaiveDateTime::parse_from_str(end_iso, "%Y-%m-%dT%H:%M:%SZ") else {
        return String::new();
    };
    let secs = (end - start).num_seconds();
    if secs < 60 {
        format!(" ({secs}s)")
    } else {
        format!(" ({}m {}s)", secs / 60, secs % 60)
    }
}

// ---------------------------------------------------------------------------
// /doctor command handler
// ---------------------------------------------------------------------------

/// Handle the /doctor command -- run all doctor checks and return results.
pub async fn handle_doctor(bot: BotType, msg: Message, home: Arc<RightHome>) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "doctor", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!("handle_doctor: running diagnostics");
    let checks = right_agent::doctor::run_doctor(&home.0).await;
    for text in format_doctor_result_messages(&checks) {
        if let Err(e) = bot
            .send_message(msg.chat.id, &text)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await
        {
            tracing::error!("handle_doctor: Telegram rejected HTML message: {e:#}");
            bot.send_message(msg.chat.id, strip_html_tags(&text))
                .await?;
        }
    }
    Ok(())
}

fn format_doctor_result_messages(checks: &[right_agent::doctor::DoctorCheck]) -> Vec<String> {
    let body = format_doctor_result_body(checks);
    let text = format!("Doctor results:\n\n<pre>{}</pre>", html_escape(&body));
    super::markdown::split_html_message(&text)
}

fn format_doctor_result_body(checks: &[right_agent::doctor::DoctorCheck]) -> String {
    let theme = right_ui::Theme::Mono;
    let mut block = right_ui::Block::new();
    for check in checks {
        block.push(check.to_ui_line());
    }
    let pass_count = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Pass))
        .count();
    let fail_count = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Fail))
        .count();
    let warn_count = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Warn))
        .count();
    let total = checks.len();
    let summary = if warn_count == 0 && fail_count == 0 {
        format!("{pass_count}/{total} checks passed")
    } else {
        let mut parts = Vec::new();
        if warn_count > 0 {
            parts.push(format!("{warn_count} warn"));
        }
        if fail_count > 0 {
            parts.push(format!("{fail_count} fail"));
        }
        format!("{pass_count}/{total} checks passed ({})", parts.join(", "))
    };
    format!("{}\n\n{}", block.render(theme), summary)
}

// ---------------------------------------------------------------------------
// /usage command handler
// ---------------------------------------------------------------------------

/// Handle manual /usage compatibility by opening the dashboard.
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    _arg: String,
    home: Arc<RightHome>,
    agent_dir: Arc<AgentDir>,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    handle_dashboard(bot, msg, home, agent_dir, allowlist).await
}

// ---------------------------------------------------------------------------
// Stop button callback query handler
// ---------------------------------------------------------------------------

/// Handle the Stop button callback query from thinking messages.
///
/// Callback data format: `stop:{chat_id}:{eff_thread_id}`
/// Looks up the CancellationToken in StopTokens and cancels it.
pub async fn handle_stop_callback(
    bot: BotType,
    q: CallbackQuery,
    worker_ctl: super::WorkerControlDeps,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    let qid = q.id;

    let text = if parts.len() == 3
        && parts[0] == "stop"
        && let Ok(chat_id) = parts[1].parse::<i64>()
        && let Ok(thread_id) = parts[2].parse::<i64>()
    {
        let key = (chat_id, thread_id);
        if let Some(entry) = worker_ctl.stop_tokens.get(&key) {
            // Value is (turn_id, CancellationToken). turn_id is unused here —
            // Stop has the same effect regardless of which turn is running.
            entry.value().1.cancel();
            drop(entry); // release DashMap read guard before await
            Some("Stopping...")
        } else {
            Some("Already finished")
        }
    } else {
        None
    };

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

    Ok(())
}

fn apply_thinking_toggle_callback(
    thinking_visibility: &super::ThinkingVisibility,
    data: &str,
) -> Option<&'static str> {
    let (key, action) = super::parse_thinking_toggle_callback(data)?;
    if super::set_thinking_visibility(thinking_visibility, key, action.expanded()) {
        Some(match action {
            super::ThinkingToggleAction::Show => "Showing thinking...",
            super::ThinkingToggleAction::Hide => "Hiding thinking...",
        })
    } else {
        Some("Already finished")
    }
}

/// Handle Show/Hide thinking callback queries from thinking messages.
///
/// Callback data format: `think:{chat_id}:{eff_thread_id}:{show|hide}`.
pub async fn handle_thinking_toggle_callback(
    bot: BotType,
    q: CallbackQuery,
    worker_ctl: super::WorkerControlDeps,
) -> ResponseResult<()> {
    let qid = q.id;
    let text = q
        .data
        .as_deref()
        .and_then(|data| apply_thinking_toggle_callback(&worker_ctl.thinking_visibility, data));

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Background button callback query handler
// ---------------------------------------------------------------------------

/// Handle the Background button callback query from thinking messages.
///
/// Callback data format: `bg:{chat_id}:{eff_thread_id}`
/// Sets the bg flag in `BgRequests` and cancels the worker's stop token —
/// the worker reads the flag after kill+wait and emits Backgrounded.
pub async fn handle_bg_callback(
    bot: BotType,
    q: CallbackQuery,
    worker_ctl: super::WorkerControlDeps,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    let qid = q.id;

    let text = if parts.len() == 3
        && parts[0] == "bg"
        && let Ok(chat_id) = parts[1].parse::<i64>()
        && let Ok(thread_id) = parts[2].parse::<i64>()
    {
        let key = (chat_id, thread_id);
        if let Some(entry) = worker_ctl.stop_tokens.get(&key) {
            // Stamp the bg request with the *current* turn's id (read from the
            // stop_tokens entry itself). The worker matches this id on exit so
            // a click that races a stream-end completion can never cause the
            // worker to misclassify a normal-finished turn as Backgrounded.
            let (turn_id, token) = entry.value();
            super::set_bg_handoff_gate(&worker_ctl.bg_handoff_gates, key);
            worker_ctl.bg_requests.insert(
                key,
                super::BgRequest {
                    turn_id: *turn_id,
                    reason: super::worker::BgReason::UserRequested,
                },
            );
            token.cancel();
            drop(entry);
            Some("Sending to background...")
        } else {
            Some("Already finished")
        }
    } else {
        None
    };

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    fn make_private_chat_kind() -> teloxide::types::ChatKind {
        serde_json::from_value(serde_json::json!({
            "type": "private",
            "first_name": "Test"
        }))
        .unwrap()
    }

    fn make_group_chat_kind() -> teloxide::types::ChatKind {
        serde_json::from_value(serde_json::json!({
            "type": "group",
            "title": "Group"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn is_private_chat_detects_dm() {
        assert!(is_private_chat(&make_private_chat_kind()));
        assert!(!is_private_chat(&make_group_chat_kind()));
    }

    /// Regression test: AgentDir and RightHome must have distinct TypeIds.
    /// If they shared the same type (e.g., both Arc<PathBuf>), dptree would overwrite
    /// the first registration with the second, causing all handlers to receive the
    /// wrong path for one of the two parameters.
    #[tokio::test]
    async fn agent_dir_and_right_home_have_distinct_type_ids() {
        assert_ne!(
            TypeId::of::<AgentDir>(),
            TypeId::of::<RightHome>(),
            "AgentDir and RightHome must be distinct types to avoid dptree TypeId collision"
        );
    }

    #[tokio::test]
    async fn agent_dir_and_right_home_hold_independent_paths() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightHome(PathBuf::from("/home/user/.right"));

        assert_eq!(agent.0, PathBuf::from("/agents/myagent"));
        assert_eq!(home.0, PathBuf::from("/home/user/.right"));
        assert_ne!(agent.0, home.0);
    }

    #[test]
    fn dashboard_mcp_button_label_names_destination() {
        assert_eq!(dashboard_mcp_button_label(), "Open MCP dashboard");
    }

    #[test]
    fn parse_stop_callback_data_valid() {
        let data = "stop:12345:678";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "stop");
        assert_eq!(parts[1].parse::<i64>().unwrap(), 12345);
        assert_eq!(parts[2].parse::<i64>().unwrap(), 678);
    }

    #[tokio::test]
    async fn parse_stop_callback_data_zero_thread() {
        let data = "stop:12345:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[2].parse::<i64>().unwrap(), 0);
    }

    #[tokio::test]
    async fn parse_stop_callback_data_invalid() {
        let data = "stop:notanumber:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert!(parts[1].parse::<i64>().is_err());
    }

    #[test]
    fn format_doctor_result_messages_splits_long_output_for_telegram() {
        let checks: Vec<_> = (0..40)
            .map(|i| right_agent::doctor::DoctorCheck {
                name: format!("long-check-{i}"),
                status: right_agent::doctor::CheckStatus::Warn,
                detail: "x".repeat(180),
                fix: Some("y".repeat(80)),
            })
            .collect();

        let messages = format_doctor_result_messages(&checks);

        assert!(messages.len() > 1);
        for message in &messages {
            assert!(
                message.len() <= 4096,
                "doctor message too long: {} chars",
                message.len()
            );
            assert!(
                message.contains("<pre>"),
                "message missing <pre>: {message}"
            );
            assert!(
                message.contains("</pre>"),
                "message missing </pre>: {message}"
            );
        }
    }

    #[tokio::test]
    async fn stop_token_cancel_via_dashmap_lookup() {
        use dashmap::DashMap;
        use tokio_util::sync::CancellationToken;

        let map = DashMap::new();
        let token = CancellationToken::new();
        let key = (12345_i64, 0_i64);
        map.insert(key, token.clone());

        // Simulate callback handler lookup + cancel
        let entry = map.get(&key).unwrap();
        entry.value().cancel();
        drop(entry);

        assert!(token.is_cancelled());

        // After removal, lookup returns None (race: stop after finish)
        map.remove(&key);
        assert!(map.get(&key).is_none());
    }

    #[tokio::test]
    async fn agent_dir_and_right_home_clone_independently() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightHome(PathBuf::from("/home/user/.right"));

        let agent2 = agent.clone();
        let home2 = home.clone();

        assert_eq!(agent.0, agent2.0);
        assert_eq!(home.0, home2.0);
    }

    #[tokio::test]
    async fn format_relative_time_just_now() {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert_eq!(format_relative_time(&now), "just now");
    }

    #[tokio::test]
    async fn format_relative_time_minutes() {
        let then = (chrono::Utc::now() - chrono::Duration::minutes(15))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "15m ago");
    }

    #[tokio::test]
    async fn format_relative_time_hours() {
        let then = (chrono::Utc::now() - chrono::Duration::hours(3))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "3h ago");
    }

    #[tokio::test]
    async fn format_relative_time_days() {
        let then = (chrono::Utc::now() - chrono::Duration::days(5))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "5d ago");
    }

    #[tokio::test]
    async fn format_relative_time_malformed() {
        assert_eq!(format_relative_time("not-a-timestamp"), "not-a-timestamp");
    }

    #[tokio::test]
    async fn format_duration_seconds() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:00:12Z"),
            " (12s)"
        );
    }

    #[tokio::test]
    async fn format_duration_minutes() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:02:30Z"),
            " (2m 30s)"
        );
    }

    #[tokio::test]
    async fn format_duration_malformed() {
        assert_eq!(format_duration("bad", "2026-04-11T10:00:00Z"), "");
    }

    #[tokio::test]
    async fn parse_bg_callback_data_valid() {
        let data = "bg:42:7";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[0], "bg");
        assert_eq!(parts[1].parse::<i64>().unwrap(), 42);
        assert_eq!(parts[2].parse::<i64>().unwrap(), 7);
    }

    #[tokio::test]
    async fn parse_bg_callback_data_malformed() {
        for bad in ["", "bg", "bg:", "bg:abc:0", "bg:1", "stop:1:2"] {
            let parts: Vec<&str> = bad.splitn(3, ':').collect();
            let valid = parts.len() == 3
                && parts[0] == "bg"
                && parts[1].parse::<i64>().is_ok()
                && parts[2].parse::<i64>().is_ok();
            assert!(!valid, "bad={bad} unexpectedly parsed as valid");
        }
    }

    #[tokio::test]
    async fn thinking_toggle_show_updates_active_visibility() {
        let map: super::ThinkingVisibility = Arc::new(DashMap::new());
        let key = (42_i64, 7_i64);
        map.insert(key, false);

        let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
        assert_eq!(text, Some("Showing thinking..."));

        assert!(*map.get(&key).unwrap().value());
    }

    #[tokio::test]
    async fn thinking_toggle_hide_updates_active_visibility() {
        let map: super::ThinkingVisibility = Arc::new(DashMap::new());
        let key = (42_i64, 7_i64);
        map.insert(key, true);

        let text = apply_thinking_toggle_callback(&map, "think:42:7:hide");
        assert_eq!(text, Some("Hiding thinking..."));

        assert!(!*map.get(&key).unwrap().value());
    }

    #[tokio::test]
    async fn thinking_toggle_after_finish_reports_already_finished() {
        let map: super::ThinkingVisibility = Arc::new(DashMap::new());

        let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
        assert_eq!(text, Some("Already finished"));
    }

    #[tokio::test]
    async fn thinking_toggle_malformed_callback_returns_none() {
        let map: super::ThinkingVisibility = Arc::new(DashMap::new());

        assert_eq!(apply_thinking_toggle_callback(&map, "think:42:7"), None);
        assert_eq!(apply_thinking_toggle_callback(&map, "stop:42:7"), None);
    }
}
