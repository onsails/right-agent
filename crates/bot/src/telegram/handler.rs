//! Teloxide endpoint handlers: message dispatch + /new, /list, /switch + /mcp + /cron + /doctor.
//!
//! handle_message: routes incoming text to the per-session worker via DashMap.
//! handle_new: deactivates current session, optionally creates a named one.
//! handle_list: shows all sessions for the current chat+thread.
//! handle_switch: switches to a different session by partial UUID match.
//! handle_mcp: MCP server management (list/auth/add/remove).
//! handle_doctor: runs right doctor and returns results.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dashmap::DashMap;
use teloxide::RequestError;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, Message};
use tokio::sync::mpsc;

use super::BotType;
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

/// Shared slot for pending MCP token requests. When /mcp add needs a token,
/// a oneshot::Sender is placed here. Message handler checks before routing to worker.
#[derive(Clone)]
pub struct PendingTokenSlot(pub Arc<tokio::sync::Mutex<Option<PendingTokenRequest>>>);

/// Pending token request from /mcp add flow.
pub struct PendingTokenRequest {
    /// Process-monotonic id, allocated from `NEXT_TOKEN_REQ_ID`. Used by the
    /// task spawned in `request_token_and_register` to detect supersession:
    /// on timeout/error, the task only `take()`s the slot if its id still
    /// matches, so a later `/mcp ...` invocation that already parked a fresh
    /// `PendingTokenRequest` is not silently discarded.
    pub id: u64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub sender: tokio::sync::oneshot::Sender<String>,
}

/// Process-local monotonic id allocator for `PendingTokenRequest`. Wraps after
/// 2^64-1 increments — well beyond any plausible bot lifetime, so wraparound
/// is not a correctness concern in practice.
static NEXT_TOKEN_REQ_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Bundle of message-intercept slots to reduce dptree DI parameter count.
/// Contains both auth code and MCP token intercept slots, plus the
/// auth-watcher-active flag (true while a token request task is running).
#[derive(Clone)]
pub struct InterceptSlots {
    pub auth_code: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
    pub pending_token: Arc<tokio::sync::Mutex<Option<PendingTokenRequest>>>,
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
    /// When true, CC subprocesses run with --verbose and stderr is logged at debug level.
    pub debug: bool,
    /// STT context — None when stt.enabled=false or whisper model not yet cached.
    pub stt: Option<std::sync::Arc<crate::stt::SttContext>>,
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

    // Intercept pending MCP token: if /mcp add is waiting for a token, forward this message.
    if let Some(ref text_val) = text {
        let mut slot = intercept_slots.pending_token.lock().await;
        if let Some(ref pending) = *slot
            && pending.chat_id == chat_id.0
            && pending.thread_id == eff_thread_id
            && let Some(pending) = slot.take()
        {
            tracing::info!("handle_message: forwarding message as MCP token");
            let _ = pending.sender.send(text_val.clone());
            return Ok(());
        }
    }
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
                    debug: settings.debug,
                    ssh_config_path: ssh_config.0.clone(),
                    resolved_sandbox: settings.resolved_sandbox.clone(),
                    auth_watcher_active: Arc::clone(&intercept_slots.auth_watcher),
                    auth_code_tx: Arc::clone(&intercept_slots.auth_code),
                    show_thinking: settings.show_thinking,
                    model: settings.model.clone(),
                    stop_tokens: Arc::clone(&worker_ctl.stop_tokens),
                    session_locks: Arc::clone(&worker_ctl.session_locks),
                    bg_requests: Arc::clone(&worker_ctl.bg_requests),
                    idle_timestamp: Arc::clone(&idle_ts.0),
                    internal_client: Arc::clone(&internal_api.0),
                    hindsight: settings.hindsight.clone(),
                    prefetch_cache: settings.prefetch_cache.clone(),
                    upgrade_lock: Arc::clone(&settings.upgrade_lock),
                    stt: settings.stt.clone(),
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
        .map_err(|e| to_request_err(format!("new: open DB: {:#}", e)))?;

    let prev_uuid = deactivate_current(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("new: deactivate: {:#}", e)))?;

    // Kill worker — channel closes, CC subprocess killed via kill_on_drop
    worker_map.remove(&key);

    let name = name.trim().to_string();
    let mut reply = String::new();

    if !name.is_empty() {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let label = truncate_label(&name);
        create_session(&conn, chat_id.0, eff_thread_id, &new_uuid, Some(label))
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
        .map_err(|e| to_request_err(format!("list: open DB: {:#}", e)))?;

    let sessions = list_sessions(&conn, chat_id.0, eff_thread_id)
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
        .map_err(|e| to_request_err(format!("switch: open DB: {:#}", e)))?;

    let matches = find_sessions_by_uuid(&conn, chat_id.0, eff_thread_id, &uuid)
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

/// Handle the /mcp command -- routes to subcommands: list, auth, add, remove.
///
/// Teloxide captures everything after `/mcp` as a single String (RESEARCH.md Pitfall 9).
/// We split manually and dispatch.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
pub async fn handle_mcp(
    bot: BotType,
    msg: Message,
    args: String,
    agent_dir: Arc<AgentDir>,
    pending_auth: PendingAuthMap,
    home: Arc<RightHome>,
    internal: Arc<InternalApi>,
    pending_token_slot: Arc<PendingTokenSlot>,
    ssh_config: Arc<SshConfigPath>,
    settings: Arc<AgentSettings>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "mcp", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!(agent_dir = %agent_dir.0.display(), "mcp: dispatching");
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let parts: Vec<&str> = args.split_whitespace().collect();
    let result = match parts.first().copied() {
        None | Some("list") => handle_mcp_list(&bot, &msg, agent_name, &internal.0).await,
        Some("auth") => {
            let server = match parts.get(1) {
                Some(s) => *s,
                None => {
                    bot.send_message(msg.chat.id, "Usage: /mcp auth <server>")
                        .await?;
                    return Ok(());
                }
            };
            handle_mcp_auth(
                &bot,
                &msg,
                server,
                &agent_dir.0,
                pending_auth,
                &home.0,
                &internal.0,
                &pending_token_slot,
                ssh_config.0.as_deref(),
                settings.resolved_sandbox.as_deref(),
            )
            .await
        }
        Some("add") => {
            let rest = parts[1..].join(" ");
            handle_mcp_add(
                &bot,
                &msg,
                &rest,
                &agent_dir.0,
                &internal.0,
                &pending_token_slot,
                ssh_config.0.as_deref(),
                settings.resolved_sandbox.as_deref(),
            )
            .await
        }
        Some("remove") => {
            let server = match parts.get(1) {
                Some(s) => *s,
                None => {
                    bot.send_message(msg.chat.id, "Usage: /mcp remove <server>")
                        .await?;
                    return Ok(());
                }
            };
            handle_mcp_remove(&bot, &msg, server, &agent_dir.0, &internal.0).await
        }
        Some(unknown) => bot
            .send_message(
                msg.chat.id,
                format!("Unknown /mcp subcommand: {unknown}\nUsage: /mcp [list|add|remove]"),
            )
            .await
            .map(|_| ()),
    };
    result.map_err(|e| to_request_err(format!("{e:#}")))?;
    Ok(())
}

/// `/mcp list` -- show all MCP servers via the internal aggregator API.
async fn handle_mcp_list(
    bot: &BotType,
    msg: &Message,
    agent_name: &str,
    internal: &right_mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    tracing::info!(agent = %agent_name, "mcp list");

    let result = match internal.mcp_list(agent_name).await {
        Ok(r) => r,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error listing MCP servers: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    if result.servers.is_empty() {
        bot.send_message(msg.chat.id, "No MCP servers configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("MCP Servers:\n\n");
    for s in &result.servers {
        let url_part = s
            .url
            .as_deref()
            .map(|u| format!(" [{u}]"))
            .unwrap_or_default();
        let auth_part = s
            .auth_type
            .as_deref()
            .map(|a| format!(" [{a}]"))
            .unwrap_or_default();
        text.push_str(&format!(
            "  {} -- {} ({} tools){}{}\n",
            s.name, s.status, s.tool_count, auth_part, url_part
        ));
    }
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

/// `/mcp auth <server>` -- initiate OAuth flow: discovery, PKCE, send auth URL.
///
/// If Dynamic Client Registration fails (some servers advertise OAuth metadata
/// but never implement DCR — e.g. browser-use's `/oauth/register` returns 404),
/// fall back to API-key auth: run Haiku auth-type detection (when a sandbox is
/// available) and ask the user for a token via `PendingTokenSlot`.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn handle_mcp_auth(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    agent_dir: &Path,
    pending_auth: PendingAuthMap,
    home: &Path,
    internal: &right_mcp::internal_client::InternalClient,
    pending_token_slot: &PendingTokenSlot,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), server = %server_name, "mcp auth");

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // 1. Look up server URL from aggregator (not mcp.json — external servers live in SQLite)
    let server_url = match internal.mcp_list(agent_name).await {
        Ok(resp) => {
            match resp.servers.iter().find(|s| s.name == server_name) {
                Some(s) => match &s.url {
                    Some(url) => url.clone(),
                    None => {
                        bot.send_message(
                            msg.chat.id,
                            format!("Server '{server_name}' has no URL configured"),
                        )
                        .await?;
                        return Ok(());
                    }
                },
                None => {
                    bot.send_message(
                        msg.chat.id,
                        format!("Server '{server_name}' not found. Run /mcp list to see registered servers."),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Cannot query MCP servers: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    // 2. Read tunnel config
    let global_config = match right_core::config::read_global_config(home) {
        Ok(c) => c,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Cannot read config.yaml: {e:#}"))
                .await?;
            return Ok(());
        }
    };
    let tunnel = global_config.tunnel.clone();

    // 3. Check cloudflared binary
    if which::which("cloudflared").is_err() {
        bot.send_message(
            msg.chat.id,
            "Error: cloudflared binary not found in PATH. Install cloudflared first.",
        )
        .await?;
        return Ok(());
    }

    // 4. AS discovery
    let http_client = reqwest::Client::new();
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await
        .ok();
    bot.send_message(
        msg.chat.id,
        format!("Discovering OAuth endpoints for {server_name}..."),
    )
    .await?;

    let metadata = match right_mcp::oauth::discover_as(&http_client, &server_url).await {
        Ok(m) => m,
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("AS discovery failed for {server_name}: {e:#}"),
            )
            .await?;
            return Ok(());
        }
    };

    // 5. DCR or static clientId
    let agent_name = agent_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if tunnel.hostname.is_empty() {
        bot.send_message(
            msg.chat.id,
            "Tunnel hostname not configured -- run `right init --tunnel-hostname HOSTNAME`",
        )
        .await?;
        return Ok(());
    }
    let redirect_uri = format!("https://{}/oauth/{agent_name}/callback", tunnel.hostname);
    let (client_id, client_secret) = match right_mcp::oauth::register_client_or_fallback(
        &http_client,
        &metadata,
        None, // no static clientId from .claude.json -- DCR only
        &redirect_uri,
    )
    .await
    {
        Ok(pair) => pair,
        Err(right_mcp::oauth::OAuthError::DcrFailed(detail)) => {
            // Some servers advertise OAuth metadata (RFC 8414) but never implement
            // Dynamic Client Registration — `registration_endpoint` 404s. Falling
            // back to API-key auth: detect header name via Haiku, then ask the
            // user for a token. The server stays registered; mcp_add overwrites
            // auth_type from "oauth" to whatever the user provides.
            tracing::warn!(server = server_name, %detail, "mcp auth: DCR failed, falling back to API-key auth");
            return dcr_failure_fallback(
                bot,
                msg,
                server_name,
                &server_url,
                agent_dir,
                &agent_name,
                detail,
                pending_token_slot,
                ssh_config_path,
                resolved_sandbox,
                internal,
            )
            .await;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Client registration failed: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    // 6. Generate PKCE + state
    let (code_verifier, code_challenge) = right_mcp::oauth::generate_pkce();
    let state = right_mcp::oauth::generate_state();

    // 7. Tunnel healthcheck -- hit tunnel root to verify cloudflared is running
    let healthcheck_url = format!("https://{}/", tunnel.hostname);
    match http_client
        .get(&healthcheck_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_server_error() => {
            bot.send_message(
                msg.chat.id,
                format!(
                    "Tunnel healthcheck returned {} -- cloudflared may be misconfigured",
                    resp.status()
                ),
            )
            .await?;
            return Ok(());
        }
        Ok(_) => {} // 2xx/3xx/4xx = tunnel is reachable
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("Tunnel healthcheck failed: {e:#}\nIs cloudflared running?"),
            )
            .await?;
            return Ok(());
        }
    }

    // 8. Store PendingAuth
    let pending = right_mcp::oauth::PendingAuth {
        server_name: server_name.to_string(),
        server_url: server_url.clone(),
        code_verifier,
        state: state.clone(),
        token_endpoint: metadata.token_endpoint.clone(),
        client_id: client_id.clone(),
        client_secret,
        redirect_uri: redirect_uri.clone(),
        created_at: std::time::Instant::now(),
    };
    pending_auth.lock().await.insert(state.clone(), pending);

    // 9. Build and send auth URL
    let auth_url = right_mcp::oauth::build_auth_url(
        &metadata,
        &client_id,
        &redirect_uri,
        &state,
        &code_challenge,
        None,
    );
    bot.send_message(
        msg.chat.id,
        format!("Authenticate {server_name}:\n\n{auth_url}"),
    )
    .await?;
    Ok(())
}

/// Recovery path when an OAuth-advertising MCP server fails Dynamic Client
/// Registration. Tells the user, runs Haiku auth-type detection (when a
/// sandbox is configured), and prompts for an API-key token via
/// `request_token_and_register`. The token-prompt task overwrites the
/// existing `auth_type=oauth` row with the resolved bearer/header values.
#[allow(clippy::too_many_arguments)]
async fn dcr_failure_fallback(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    server_url: &str,
    agent_dir: &Path,
    agent_name: &str,
    dcr_detail: String,
    pending_token_slot: &PendingTokenSlot,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
    internal: &right_mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    let escaped_detail = super::markdown::html_escape(&dcr_detail);
    send_html_reply(
        bot,
        msg.chat.id,
        effective_thread_id(msg),
        &format!(
            "OAuth metadata advertised a registration endpoint, but DCR failed:\n<code>{escaped_detail}</code>\n\nFalling back to API-key authentication."
        ),
    )
    .await?;

    // The aggregator stores the bare URL at registration time (see
    // handle_mcp_add's OAuth branch), so server_url here should already be
    // bare. Log anomalies instead of silently rewriting them — a query
    // string or parse failure here signals a registration-time integrity
    // bug worth surfacing.
    let bare_url = match reqwest::Url::parse(server_url) {
        Ok(u) if u.query().is_some() => {
            tracing::warn!(
                server = server_name,
                %server_url,
                "registered server URL unexpectedly has a query string"
            );
            let mut clean = u;
            clean.set_query(None);
            clean.to_string()
        }
        Ok(_) => server_url.to_string(),
        Err(e) => {
            tracing::warn!(
                server = server_name,
                %server_url,
                err = %e,
                "registered server URL failed to parse — using as-is"
            );
            server_url.to_string()
        }
    };

    // Detect auth type via Haiku when a sandbox is available; otherwise default
    // to bearer. The user can always override with `HeaderName: token` syntax.
    let (auth_type, auth_header): (String, Option<String>) =
        if ssh_config_path.is_some() && right_mcp::credentials::is_public_url(&bare_url) {
            bot.send_message(msg.chat.id, "Detecting authentication method...")
                .await?;
            let (t, h) = detect_auth_with_typing_indicator(
                bot,
                msg.chat.id,
                &bare_url,
                agent_dir,
                ssh_config_path,
                resolved_sandbox,
            )
            .await;
            // query_string is impossible after stripping the URL query;
            // downgrade so the user can still supply a token.
            if t == "bearer" || t == "header" {
                (t, h)
            } else {
                ("bearer".into(), None)
            }
        } else {
            ("bearer".into(), None)
        };

    request_token_and_register(
        bot.clone(),
        msg.chat.id,
        effective_thread_id(msg),
        right_mcp::internal_client::InternalClient::new(internal.socket_path()),
        agent_name.to_string(),
        server_name.to_string(),
        bare_url,
        auth_type,
        auth_header,
        pending_token_slot.clone(),
    )
    .await
}

/// `/mcp add <name> <url>` -- add an MCP server via the internal aggregator API.
///
/// Flow:
/// 1. Strip query string, try OAuth AS discovery on bare URL
/// 2. If OAuth found: register without auth, tell user to `/mcp auth`
/// 3. Otherwise determine auth type (query_string / haiku detection / bearer default)
/// 4. If bearer/header: ask user for token via `PendingTokenSlot`
/// 5. Register server with auth fields (connection verified server-side)
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn handle_mcp_add(
    bot: &BotType,
    msg: &Message,
    config_str: &str,
    agent_dir: &Path,
    internal: &right_mcp::internal_client::InternalClient,
    pending_token_slot: &PendingTokenSlot,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), "mcp add");
    let parts: Vec<&str> = config_str.split_whitespace().collect();
    if parts.len() < 2 {
        bot.send_message(msg.chat.id, "Usage: /mcp add <name> <url>")
            .await?;
        return Ok(());
    }
    let name = parts[0];
    let original_url = parts[1];

    // Parse URL early — reject garbage before any network calls
    let parsed = match reqwest::Url::parse(original_url) {
        Ok(u) => u,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Invalid URL: {e}"))
                .await?;
            return Ok(());
        }
    };

    let has_query = parsed.query().is_some();
    let bare_url = {
        let mut clean = parsed.clone();
        clean.set_query(None);
        clean.to_string()
    };

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let eff_thread_id = effective_thread_id(msg);

    // Step 1: Try OAuth AS discovery
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await
        .ok();
    tracing::info!(url = %bare_url, "mcp add: starting OAuth AS discovery");
    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let oauth_result = right_mcp::oauth::discover_as(&http_client, &bare_url).await;
    let oauth_discovered = oauth_result.is_ok();
    tracing::info!(url = %bare_url, oauth_discovered, err = ?oauth_result.err(), "mcp add: OAuth AS discovery complete");

    if oauth_discovered {
        // OAuth server — register without auth, tell user to run /mcp auth
        bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
            .await
            .ok();
        tracing::info!(agent = agent_name, server = name, url = %bare_url, "mcp add: registering OAuth server via internal API");
        match internal
            .mcp_add(agent_name, name, &bare_url, Some("oauth"), None, None)
            .await
        {
            Ok(resp) => {
                let escaped = super::markdown::html_escape(name);
                let mut reply = format!("Added MCP server <b>{escaped}</b> (OAuth detected).");
                if let Some(ref w) = resp.warning {
                    reply.push_str(&format!("\n{}", super::markdown::html_escape(w)));
                }
                reply.push_str(&format!(
                    "\nRun <code>/mcp auth {name}</code> to authenticate."
                ));
                send_html_reply(bot, msg.chat.id, eff_thread_id, &reply).await?;
            }
            Err(e) => {
                tracing::warn!(server = name, err = %format!("{e:#}"), "mcp add: internal API registration failed");
                let escaped_err = super::markdown::html_escape(&format!("{e:#}"));
                send_html_reply(
                    bot,
                    msg.chat.id,
                    eff_thread_id,
                    &format!("Failed: {escaped_err}"),
                )
                .await?;
            }
        }
        return Ok(());
    }

    // Step 2: Determine auth type for non-OAuth servers
    tracing::info!(url = %bare_url, "mcp add: determining auth type (non-OAuth path)");
    let is_public = right_mcp::credentials::is_public_url(&bare_url);

    let (auth_type, auth_header): (String, Option<String>) = if has_query {
        ("query_string".into(), None)
    } else if is_public {
        bot.send_message(msg.chat.id, "Detecting authentication method...")
            .await?;
        detect_auth_with_typing_indicator(
            bot,
            msg.chat.id,
            &bare_url,
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
        )
        .await
    } else {
        // Private/local — assume bearer
        ("bearer".into(), None)
    };

    // Step 3: If no token needed (query_string), register immediately.
    // If token needed, spawn a background task so the dispatcher stays unblocked
    // and can deliver the user's next message to the intercept slot.
    if auth_type != "bearer" && auth_type != "header" {
        // query_string — register immediately, no token needed
        tracing::info!(url = %bare_url, %auth_type, "mcp add: registering server (no token)");
        match internal
            .mcp_add(agent_name, name, original_url, Some(&auth_type), None, None)
            .await
        {
            Ok(resp) => {
                let escaped = super::markdown::html_escape(name);
                let mut reply = format!("Added MCP server <b>{escaped}</b>.");
                if resp.tools_count > 0 {
                    reply.push_str(&format!(" {} tools available.", resp.tools_count));
                }
                if let Some(ref w) = resp.warning {
                    reply.push_str(&format!("\n{}", super::markdown::html_escape(w)));
                }
                send_html_reply(bot, msg.chat.id, eff_thread_id, &reply).await?;
            }
            Err(e) => {
                send_html_reply(bot, msg.chat.id, eff_thread_id, &format!("Failed: {e:#}")).await?;
            }
        }
        return Ok(());
    }

    // Token needed — prompt user and spawn background task to wait + register.
    // Must return from handler so the dispatcher can deliver the token message.
    request_token_and_register(
        bot.clone(),
        msg.chat.id,
        eff_thread_id,
        right_mcp::internal_client::InternalClient::new(internal.socket_path()),
        agent_name.to_string(),
        name.to_string(),
        bare_url.to_string(),
        auth_type,
        auth_header,
        pending_token_slot.clone(),
    )
    .await
}

/// Prompt the user for an auth token, then spawn a background task that waits
/// for it (via `PendingTokenSlot`), parses optional `HeaderName: token` syntax,
/// and registers/updates the MCP server with the resolved auth fields.
///
/// Used by both `/mcp add` (for non-OAuth servers) and `/mcp auth` (as a
/// DCR-failure fallback when an OAuth-advertising server doesn't actually
/// implement Dynamic Client Registration).
///
/// Returns immediately after parking the oneshot sender so the dispatcher
/// remains free to deliver the token message into the slot.
#[allow(clippy::too_many_arguments)]
async fn request_token_and_register(
    bot: BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    internal: right_mcp::internal_client::InternalClient,
    agent_name: String,
    server_name: String,
    bare_url: String,
    initial_auth_type: String,
    initial_auth_header: Option<String>,
    pending_token_slot: PendingTokenSlot,
) -> Result<(), RequestError> {
    let header_hint = initial_auth_header
        .as_deref()
        .map(|h| format!("the {h} token"))
        .unwrap_or_else(|| "the token".into());
    bot.send_message(
        chat_id,
        format!(
            "Send {header_hint} for {server_name}, or HeaderName: token to specify a custom header:"
        ),
    )
    .await?;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let req_id =
        NEXT_TOKEN_REQ_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let prev = {
        let mut slot = pending_token_slot.0.lock().await;
        let prev = slot.take();
        *slot = Some(PendingTokenRequest {
            id: req_id,
            chat_id: chat_id.0,
            thread_id: eff_thread_id,
            sender: tx,
        });
        prev
    };
    // Drop the prior request OUTSIDE the lock. Dropping it closes the oneshot
    // synchronously, so the prior waiter wakes immediately with RecvError.
    if let Some(prev) = prev {
        let prev_chat_id = teloxide::types::ChatId(prev.chat_id);
        let prev_thread_id = prev.thread_id;
        let mut send = bot.send_message(
            prev_chat_id,
            "Previous MCP token request superseded by a new /mcp command.",
        );
        if prev_thread_id != 0 {
            send = send.message_thread_id(teloxide::types::ThreadId(
                teloxide::types::MessageId(prev_thread_id as i32),
            ));
        }
        send.await.ok();
        drop(prev); // explicit — closes the oneshot
    }

    tokio::spawn(async move {
        let raw_input = match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(input)) => input,
            _ => {
                // Only take the slot if it still belongs to *this* request.
                // Otherwise a newer /mcp invocation has already parked its own
                // PendingTokenRequest in the slot — clearing it here would
                // silently drop the user's next token message.
                let mut slot = pending_token_slot.0.lock().await;
                if slot.as_ref().map(|s| s.id) == Some(req_id) {
                    slot.take();
                }
                drop(slot);
                bot.send_message(
                    chat_id,
                    "Timed out waiting for token. MCP authentication cancelled.",
                )
                .await
                .ok();
                return;
            }
        };

        // Parse "HeaderName: token_value" format or treat as raw token
        let (token, auth_type, auth_header) =
            if let Some((header, value)) = raw_input.split_once(": ") {
                let header = header.trim();
                let value = value.trim();
                if !header.is_empty()
                    && !header.contains(' ')
                    && header
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    tracing::info!(%header, "user specified custom auth header");
                    (
                        value.to_string(),
                        "header".to_string(),
                        Some(header.to_string()),
                    )
                } else {
                    (raw_input, initial_auth_type, initial_auth_header)
                }
            } else {
                (raw_input, initial_auth_type, initial_auth_header)
            };

        tracing::info!(url = %bare_url, %auth_type, "mcp: registering server with token");
        match internal
            .mcp_add(
                &agent_name,
                &server_name,
                &bare_url,
                Some(&auth_type),
                auth_header.as_deref(),
                Some(&token),
            )
            .await
        {
            Ok(resp) => {
                let escaped = super::markdown::html_escape(&server_name);
                let mut reply = format!("Added MCP server <b>{escaped}</b>.");
                if resp.tools_count > 0 {
                    reply.push_str(&format!(" {} tools available.", resp.tools_count));
                }
                if let Some(ref w) = resp.warning {
                    reply.push_str(&format!("\n{}", super::markdown::html_escape(w)));
                }
                send_html_reply(&bot, chat_id, eff_thread_id, &reply)
                    .await
                    .ok();
            }
            Err(e) => {
                send_html_reply(&bot, chat_id, eff_thread_id, &format!("Failed: {e:#}"))
                    .await
                    .ok();
            }
        }
    });

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct AuthDetectionResult {
    auth_type: String,
    #[serde(default)]
    header_name: Option<String>,
}

/// Run Haiku auth-type detection wrapped in a Telegram typing-indicator that
/// pulses every 5s. Returns `(auth_type, auth_header)` defaulting to bearer
/// when detection fails or no sandbox is configured.
///
/// Used by `/mcp add` (after stripping query string) and the `/mcp auth`
/// DCR-failure fallback.
///
/// PRECONDITION: caller MUST gate this on
/// `right_mcp::credentials::is_public_url(bare_url) && ssh_config_path.is_some()`.
/// Calling this for a private URL burns a Haiku invocation for no benefit; calling it
/// without a sandbox falls through to the bearer default.
async fn detect_auth_with_typing_indicator(
    bot: &BotType,
    chat_id: teloxide::types::ChatId,
    bare_url: &str,
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> (String, Option<String>) {
    let typing_bot = bot.clone();
    let typing_cancel = tokio_util::sync::CancellationToken::new();
    let typing_token = typing_cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = typing_bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing) => {}
                _ = typing_token.cancelled() => break,
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                _ = typing_token.cancelled() => break,
            }
        }
    });

    let result =
        detect_auth_type_via_haiku(bare_url, agent_dir, ssh_config_path, resolved_sandbox).await;
    typing_cancel.cancel();

    match result {
        Ok(r) => {
            tracing::info!(auth_type = %r.auth_type, header = ?r.header_name, "haiku detected auth type");
            (r.auth_type, r.header_name)
        }
        Err(e) => {
            tracing::warn!("haiku auth detection failed: {e}, falling back to bearer");
            ("bearer".into(), None)
        }
    }
}

/// Run haiku in sandbox to detect MCP server auth type.
/// Returns Err if no sandbox is configured (caller should fall back to bearer).
async fn detect_auth_type_via_haiku(
    bare_url: &str,
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> Result<AuthDetectionResult, String> {
    if ssh_config_path.is_none() {
        return Err("no sandbox configured, skipping haiku detection".into());
    }

    let prompt = format!(
        "Find what authentication method the MCP server at {bare_url} uses.\n\
         Steps:\n\
         1. WebSearch for the API documentation of this service\n\
         2. WebFetch the most relevant documentation page to find auth details\n\
         3. Return the result as JSON\n\n\
         One of:\n\
         {{\"auth_type\": \"bearer\"}} — if it uses Authorization: Bearer header\n\
         {{\"auth_type\": \"header\", \"header_name\": \"X-Custom-Header\"}} — if it uses a custom header (include the exact header name)\n\
         {{\"auth_type\": \"query_string\"}} — if the API key goes in the URL query string\n\
         If you cannot determine, default to: {{\"auth_type\": \"bearer\"}}"
    );

    const AUTH_DETECTION_SCHEMA: &str = r#"{"type":"object","properties":{"auth_type":{"type":"string","enum":["bearer","header","query_string"]},"header_name":{"type":"string"}},"required":["auth_type"]}"#;

    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(AUTH_DETECTION_SCHEMA.into()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: Some("haiku".into()),
        max_budget_usd: Some(0.20),
        max_turns: Some(10),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec!["WebSearch".into(), "WebFetch".into()],
        disallowed_tools: vec![],
        extra_args: vec![],
        prompt: Some(prompt),
    };
    let claude_args = invocation.into_args();

    let mut cmd = crate::cc::invocation::build_claude_command(
        &claude_args,
        agent_dir,
        ssh_config_path,
        resolved_sandbox,
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = right_core::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn haiku failed: {e:#}"))?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "haiku timed out after 120s".to_string())?
    .map_err(|e| format!("haiku failed: {e:#}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::info!(
        exit_code = ?output.status.code(),
        stdout_preview = %stdout.chars().take(500).collect::<String>(),
        stderr_preview = %stderr.chars().take(500).collect::<String>(),
        "haiku auth detection raw output"
    );
    if !output.status.success() {
        return Err(format!(
            "haiku exited with {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            stdout.chars().take(300).collect::<String>(),
            stderr.chars().take(300).collect::<String>(),
        ));
    }

    // CC --output-format json + --json-schema puts schema-validated JSON in
    // `structured_output`, while `result` gets the text reply. Fall back to `result`
    // for older CC versions that don't have `structured_output`.
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("failed to parse CC output envelope: {e:#}"))?;

    let result_val = envelope
        .get("structured_output")
        .filter(|v| !v.is_null())
        .or_else(|| envelope.get("result"))
        .ok_or("CC output missing both 'structured_output' and 'result' fields")?;

    // structured_output is already a JSON object; result may be a string that needs parsing
    if result_val.is_object() {
        serde_json::from_value::<AuthDetectionResult>(result_val.clone())
            .map_err(|e| format!("failed to parse auth detection result: {e:#}"))
    } else if let Some(s) = result_val.as_str() {
        serde_json::from_str::<AuthDetectionResult>(s)
            .map_err(|e| format!("failed to parse haiku response: {e:#}\nRaw: {s}"))
    } else {
        Err(format!("unexpected result type: {result_val}"))
    }
}

/// `/mcp remove <server>` -- remove an MCP server via the internal aggregator API.
async fn handle_mcp_remove(
    bot: &BotType,
    msg: &Message,
    server_name: &str,
    agent_dir: &Path,
    internal: &right_mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), server = %server_name, "mcp remove");

    if server_name == right_mcp::PROTECTED_MCP_SERVER {
        bot.send_message(
            msg.chat.id,
            format!("Cannot remove '{server_name}' — required for core functionality."),
        )
        .await?;
        return Ok(());
    }

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let eff_thread_id = effective_thread_id(msg);
    let escaped_name = super::markdown::html_escape(server_name);

    match internal.mcp_remove(agent_name, server_name).await {
        Ok(_) => {
            send_html_reply(
                bot,
                msg.chat.id,
                eff_thread_id,
                &format!("Removed MCP server <b>{escaped_name}</b>."),
            )
            .await?;
        }
        Err(e) => {
            send_html_reply(bot, msg.chat.id, eff_thread_id, &format!("Failed: {e:#}")).await?;
        }
    }
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
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let specs = right_agent::cron_spec::load_specs_from_db(&conn)
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
            right_agent::cron_spec::ScheduleKind::RunAt(dt) => super::markdown::html_escape(
                &format!("once at {}", dt.format("%Y-%m-%d %H:%M UTC")),
            ),
            _ => super::markdown::html_escape(&right_agent::cron_spec::describe_schedule(
                spec.schedule_kind.cron_schedule().unwrap_or(""),
            )),
        };

        let last_run = right_agent::cron_spec::get_recent_runs(&conn, name, 1)
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
        .map_err(|e| to_request_err(format!("DB open failed: {e:#}")))?;

    let detail = right_agent::cron_spec::get_spec_detail(&conn, job_name)
        .map_err(|e| to_request_err(format!("query failed: {e:#}")))?;

    let Some(detail) = detail else {
        bot.send_message(msg.chat.id, format!("Cron job '{job_name}' not found."))
            .await?;
        return Ok(());
    };

    let desc =
        super::markdown::html_escape(&right_agent::cron_spec::describe_schedule(&detail.schedule));
    let schedule_escaped = super::markdown::html_escape(&detail.schedule);
    let mut text = format!(
        "<b>{}</b>\nSchedule: {} (<code>{}</code>)\nBudget: ${:.2}",
        detail.job_name, desc, schedule_escaped, detail.max_budget_usd,
    );
    if let Some(ref ttl) = detail.lock_ttl {
        let ttl_escaped = super::markdown::html_escape(ttl);
        text.push_str(&format!("\nLock TTL: {ttl_escaped}"));
    }
    if detail.triggered_at.is_some() {
        text.push_str("\n\u{26a1} Trigger pending");
    }

    let runs = right_agent::cron_spec::get_recent_runs(&conn, job_name, 5)
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
pub async fn handle_doctor(
    bot: BotType,
    msg: Message,
    home: Arc<RightHome>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "doctor", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!("handle_doctor: running diagnostics");
    let checks = right_agent::doctor::run_doctor(&home.0);
    let theme = right_core::ui::Theme::Mono;
    let mut block = right_core::ui::Block::new();
    for check in &checks {
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
        if warn_count > 0 { parts.push(format!("{warn_count} warn")); }
        if fail_count > 0 { parts.push(format!("{fail_count} fail")); }
        format!("{pass_count}/{total} checks passed ({})", parts.join(", "))
    };
    let body = format!("{}\n\n{}", block.render(theme), summary);
    // HTML-escape body before wrapping in <pre> -- doctor output may contain <, >, &
    let escaped = body
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let text = format!("Doctor results:\n\n<pre>{}</pre>", escaped);
    if let Err(e) = bot
        .send_message(msg.chat.id, &text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await
    {
        tracing::error!("handle_doctor: Telegram rejected HTML message: {e:#}");
        // Fallback: send as plain text without <pre> wrapper
        bot.send_message(msg.chat.id, format!("Doctor results:\n\n{body}"))
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /usage command handler
// ---------------------------------------------------------------------------

/// Handle the /usage command — aggregate and render a usage summary.
/// `arg` is the trailing text after `/usage`; accepts `"detail"` or `"d"` to
/// include raw-tokens lines, anything else → default (no detail).
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    arg: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "usage", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let detail = matches!(arg.trim().to_ascii_lowercase().as_str(), "detail" | "d");
    let text = match build_usage_summary(&agent_dir.0, detail).await {
        Ok(t) => t,
        Err(e) => format!("Failed to read usage: {e:#}"),
    };
    let eff_thread_id = effective_thread_id(&msg);
    send_html_reply(&bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

async fn build_usage_summary(agent_dir: &Path, detail: bool) -> Result<String, miette::Report> {
    use chrono::{Duration, Utc};
    use right_agent::usage::aggregate::aggregate;
    use right_agent::usage::format::{AllWindows, format_summary_message};

    let conn = right_db::open_connection(agent_dir, false)
        .map_err(|e| miette::miette!("open_connection: {e:#}"))?;

    let now = Utc::now();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let week_start = now - Duration::days(7);
    let month_start = now - Duration::days(30);

    let windows = AllWindows {
        today_interactive: aggregate(&conn, Some(today_start), "interactive")
            .map_err(|e| miette::miette!("aggregate today/interactive: {e:#}"))?,
        today_cron: aggregate(&conn, Some(today_start), "cron")
            .map_err(|e| miette::miette!("aggregate today/cron: {e:#}"))?,
        today_reflection: aggregate(&conn, Some(today_start), "reflection")
            .map_err(|e| miette::miette!("aggregate today/reflection: {e:#}"))?,
        week_interactive: aggregate(&conn, Some(week_start), "interactive")
            .map_err(|e| miette::miette!("aggregate week/interactive: {e:#}"))?,
        week_cron: aggregate(&conn, Some(week_start), "cron")
            .map_err(|e| miette::miette!("aggregate week/cron: {e:#}"))?,
        week_reflection: aggregate(&conn, Some(week_start), "reflection")
            .map_err(|e| miette::miette!("aggregate week/reflection: {e:#}"))?,
        month_interactive: aggregate(&conn, Some(month_start), "interactive")
            .map_err(|e| miette::miette!("aggregate month/interactive: {e:#}"))?,
        month_cron: aggregate(&conn, Some(month_start), "cron")
            .map_err(|e| miette::miette!("aggregate month/cron: {e:#}"))?,
        month_reflection: aggregate(&conn, Some(month_start), "reflection")
            .map_err(|e| miette::miette!("aggregate month/reflection: {e:#}"))?,
        all_interactive: aggregate(&conn, None, "interactive")
            .map_err(|e| miette::miette!("aggregate all/interactive: {e:#}"))?,
        all_cron: aggregate(&conn, None, "cron")
            .map_err(|e| miette::miette!("aggregate all/cron: {e:#}"))?,
        all_reflection: aggregate(&conn, None, "reflection")
            .map_err(|e| miette::miette!("aggregate all/reflection: {e:#}"))?,
    };

    Ok(format_summary_message(&windows, detail))
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
            worker_ctl.bg_requests.insert(key, *turn_id);
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

    #[test]
    fn is_private_chat_detects_dm() {
        assert!(is_private_chat(&make_private_chat_kind()));
        assert!(!is_private_chat(&make_group_chat_kind()));
    }

    /// Regression test: AgentDir and RightHome must have distinct TypeIds.
    /// If they shared the same type (e.g., both Arc<PathBuf>), dptree would overwrite
    /// the first registration with the second, causing all handlers to receive the
    /// wrong path for one of the two parameters.
    #[test]
    fn agent_dir_and_right_home_have_distinct_type_ids() {
        assert_ne!(
            TypeId::of::<AgentDir>(),
            TypeId::of::<RightHome>(),
            "AgentDir and RightHome must be distinct types to avoid dptree TypeId collision"

        );
    }

    #[test]
    fn agent_dir_and_right_home_hold_independent_paths() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightHome(PathBuf::from("/home/user/.right"));

        assert_eq!(agent.0, PathBuf::from("/agents/myagent"));
        assert_eq!(home.0, PathBuf::from("/home/user/.right"));
        assert_ne!(agent.0, home.0);
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

    #[test]
    fn parse_stop_callback_data_zero_thread() {
        let data = "stop:12345:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[2].parse::<i64>().unwrap(), 0);
    }

    #[test]
    fn parse_stop_callback_data_invalid() {
        let data = "stop:notanumber:0";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert!(parts[1].parse::<i64>().is_err());
    }

    #[test]
    fn stop_token_cancel_via_dashmap_lookup() {
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

    #[test]
    fn agent_dir_and_right_home_clone_independently() {
        let agent = AgentDir(PathBuf::from("/agents/myagent"));
        let home = RightHome(PathBuf::from("/home/user/.right"));

        let agent2 = agent.clone();
        let home2 = home.clone();

        assert_eq!(agent.0, agent2.0);
        assert_eq!(home.0, home2.0);
    }

    #[test]
    fn format_relative_time_just_now() {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert_eq!(format_relative_time(&now), "just now");
    }

    #[test]
    fn format_relative_time_minutes() {
        let then = (chrono::Utc::now() - chrono::Duration::minutes(15))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "15m ago");
    }

    #[test]
    fn format_relative_time_hours() {
        let then = (chrono::Utc::now() - chrono::Duration::hours(3))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "3h ago");
    }

    #[test]
    fn format_relative_time_days() {
        let then = (chrono::Utc::now() - chrono::Duration::days(5))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(format_relative_time(&then), "5d ago");
    }

    #[test]
    fn format_relative_time_malformed() {
        assert_eq!(format_relative_time("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:00:12Z"),
            " (12s)"
        );
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(
            format_duration("2026-04-11T10:00:00Z", "2026-04-11T10:02:30Z"),
            " (2m 30s)"
        );
    }

    #[test]
    fn format_duration_malformed() {
        assert_eq!(format_duration("bad", "2026-04-11T10:00:00Z"), "");
    }

    #[test]
    fn parse_bg_callback_data_valid() {
        let data = "bg:42:7";
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(parts[0], "bg");
        assert_eq!(parts[1].parse::<i64>().unwrap(), 42);
        assert_eq!(parts[2].parse::<i64>().unwrap(), 7);
    }

    #[test]
    fn parse_bg_callback_data_malformed() {
        for bad in ["", "bg", "bg:", "bg:abc:0", "bg:1", "stop:1:2"] {
            let parts: Vec<&str> = bad.splitn(3, ':').collect();
            let valid = parts.len() == 3
                && parts[0] == "bg"
                && parts[1].parse::<i64>().is_ok()
                && parts[2].parse::<i64>().is_ok();
            assert!(!valid, "bad={bad} unexpectedly parsed as valid");
        }
    }

    /// Regression: a stale supersession-cleanup task must NOT clear a freshly
    /// parked PendingTokenRequest. See
    /// `request_token_and_register` — without the id-stamp guard, task A's
    /// timeout/error arm would `take()` whatever currently sits in the slot,
    /// silently dropping request B that the user just parked via a second
    /// `/mcp auth` command.
    #[tokio::test]
    async fn supersession_cleanup_does_not_clobber_newer_request() {
        // Park request A.
        let slot = PendingTokenSlot(Arc::new(tokio::sync::Mutex::new(None)));
        let (tx_a, _rx_a) = tokio::sync::oneshot::channel::<String>();
        let req_a_id = NEXT_TOKEN_REQ_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut s = slot.0.lock().await;
            *s = Some(PendingTokenRequest {
                id: req_a_id,
                chat_id: 100,
                thread_id: 0,
                sender: tx_a,
            });
        }

        // Supersede with request B.
        let (tx_b, _rx_b) = tokio::sync::oneshot::channel::<String>();
        let req_b_id = NEXT_TOKEN_REQ_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(req_b_id > req_a_id);
        {
            let mut s = slot.0.lock().await;
            // Drop the prior sender (would wake task A's `rx_a` with RecvError).
            let _prev = s.take();
            *s = Some(PendingTokenRequest {
                id: req_b_id,
                chat_id: 200,
                thread_id: 0,
                sender: tx_b,
            });
        }

        // Now exercise the guarded cleanup that task A performs in its
        // timeout/error arm. With the id guard it must NOT clear the slot.
        {
            let mut s = slot.0.lock().await;
            if s.as_ref().map(|p| p.id) == Some(req_a_id) {
                s.take();
            }
        }

        // Slot should still contain request B.
        let s = slot.0.lock().await;
        let pending = s.as_ref().expect("slot must still contain request B");
        assert_eq!(pending.id, req_b_id);
        assert_eq!(pending.chat_id, 200);
    }

    /// Sanity: when the slot still holds the same request that scheduled the
    /// cleanup, the guarded `take()` clears it (the non-superseded path).
    #[tokio::test]
    async fn supersession_cleanup_clears_when_id_matches() {
        let slot = PendingTokenSlot(Arc::new(tokio::sync::Mutex::new(None)));
        let (tx, _rx) = tokio::sync::oneshot::channel::<String>();
        let req_id = NEXT_TOKEN_REQ_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut s = slot.0.lock().await;
            *s = Some(PendingTokenRequest {
                id: req_id,
                chat_id: 300,
                thread_id: 0,
                sender: tx,
            });
        }

        {
            let mut s = slot.0.lock().await;
            if s.as_ref().map(|p| p.id) == Some(req_id) {
                s.take();
            }
        }

        let s = slot.0.lock().await;
        assert!(s.is_none(), "slot must be empty after matching cleanup");
    }
}
