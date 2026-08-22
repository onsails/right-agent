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

use dashmap::DashMap;
use frankenstein::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, WebAppInfo,
};

use crate::cc::markdown_utils::{html_escape, strip_html_tags};

use super::msg_ext;
use super::router::HandlerCtx;
use super::session::{
    activate_session, create_session, deactivate_current, effective_thread_id,
    find_sessions_by_uuid, list_sessions, truncate_label,
};
use super::tg_bot::TgError;
use super::worker::{SessionKey, WorkerContext, spawn_worker};

/// Newtype wrapper for the agent directory passed via dptree dependencies.
/// Distinct from RightHome to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct AgentDir(pub PathBuf);

/// Newtype wrapper for the right home directory passed via dptree dependencies.
/// Distinct from AgentDir to prevent TypeId collision in dptree.
#[derive(Clone)]
pub struct RightHome(pub PathBuf);

/// Telegram conversation that owns a pending setup-token request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthRequestScope {
    pub(crate) chat_id: i64,
    pub(crate) effective_thread_id: i64,
}

impl AuthRequestScope {
    pub(crate) fn new(chat_id: i64, effective_thread_id: i64) -> Self {
        Self {
            chat_id,
            effective_thread_id,
        }
    }
}

/// Result of atomically trying to reserve the single setup-token slot.
pub(crate) enum AuthRequestStart {
    Started {
        request_id: u64,
        receiver: tokio::sync::oneshot::Receiver<String>,
    },
    AlreadyPending {
        owner: AuthRequestScope,
    },
}

struct AuthCodeIntercept {
    request_id: u64,
    scope: AuthRequestScope,
    sender: Option<tokio::sync::oneshot::Sender<String>>,
}

/// Single source of truth for the one pending setup-token request per agent.
#[derive(Default)]
pub(crate) struct PendingAuthState {
    next_request_id: u64,
    pending: Option<AuthCodeIntercept>,
}

impl PendingAuthState {
    pub(crate) fn start_if_idle(&mut self, scope: AuthRequestScope) -> AuthRequestStart {
        if let Some(pending) = &self.pending {
            return AuthRequestStart::AlreadyPending {
                owner: pending.scope,
            };
        }

        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .expect("pending auth request id overflow");
        let request_id = self.next_request_id;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending = Some(AuthCodeIntercept {
            request_id,
            scope,
            sender: Some(sender),
        });
        AuthRequestStart::Started {
            request_id,
            receiver,
        }
    }

    pub(crate) fn take_sender_for_scope(
        &mut self,
        scope: AuthRequestScope,
    ) -> Option<tokio::sync::oneshot::Sender<String>> {
        self.pending
            .as_mut()
            .filter(|pending| pending.scope == scope)
            .and_then(|pending| pending.sender.take())
    }

    pub(crate) fn cleanup_if_owned(&mut self, request_id: u64) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.pending.take();
            return true;
        }
        false
    }
}

pub(crate) type PendingAuthRequests = Arc<tokio::sync::Mutex<PendingAuthState>>;

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
    pub(crate) mcp_init_health: Arc<crate::keepalive::McpInitHealth>,
    /// Process shutdown token used to cancel detached user-turn repair work.
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
    /// Shared sandbox-backend state. Read before every sandboxed turn by the
    /// pre-invocation health gate (Task 9) to fail-closed when Unavailable,
    /// and to resolve the live sandbox handle for the turn.
    pub sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
}

/// Wrap an arbitrary error as a `TgError::Other`, preserving the full source
/// chain via alternate Display.
fn other_err(e: impl std::fmt::Display) -> TgError {
    TgError::Other(format!("{e:#}"))
}

/// Send an HTML-formatted message, respecting thread_id for topic replies.
async fn send_html_reply(
    bot: &super::BotType,
    chat_id: i64,
    eff_thread_id: i64,
    text: &str,
) -> Result<frankenstein::types::Message, TgError> {
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);
    bot.send_message_opts(chat_id, text, true, thread, None, None)
        .await
}

/// Handle an incoming text message.
///
/// 1. Compute effective_thread_id (normalise General topic).
/// 2. Look up existing sender in DashMap or spawn a new worker task.
/// 3. Send the message into the worker's mpsc channel.
///
/// Serialisation guarantee (SES-05): all messages to the same (chat_id, thread_id)
/// go through the same mpsc channel -> worker processes them serially.
pub(crate) async fn handle_message(
    ctx: &HandlerCtx,
    msg: &Message,
    decision: super::filter::RoutingDecision,
) -> Result<(), TgError> {
    use super::worker::DebounceMsg;
    let settings = &ctx.settings;
    let identity = &ctx.identity;
    ctx.idle_ts.0.store(
        chrono::Utc::now().timestamp(),
        std::sync::atomic::Ordering::Relaxed,
    );

    // Extract text from message body OR caption (media messages use captions)
    let text = msg_ext::text_or_caption(msg).map(|t| t.to_string());

    // Extract attachments from all media types
    let attachments = super::attachments::extract_attachments(msg);

    // Skip messages with neither text nor attachments
    if text.is_none() && attachments.is_empty() {
        return Ok(());
    }

    // Extract author from sender
    let author = match msg.from.as_ref() {
        Some(user) => super::attachments::MessageAuthor {
            name: msg_ext::full_name(user),
            username: user.username.as_ref().map(|u| format!("@{u}")),
            user_id: Some(user.id as i64),
        },
        None => super::attachments::MessageAuthor {
            name: msg_ext::chat_title(&msg.chat)
                .unwrap_or("unknown")
                .to_owned(),
            username: msg_ext::chat_username(&msg.chat).map(|u| format!("@{u}")),
            user_id: None,
        },
    };

    // Extract forward origin
    let forward_info = msg.forward_origin.as_ref().map(|origin| {
        use frankenstein::types::MessageOrigin;
        let (from, date) = match origin.as_ref() {
            MessageOrigin::User(o) => (
                super::attachments::MessageAuthor {
                    name: msg_ext::full_name(&o.sender_user),
                    username: o.sender_user.username.as_ref().map(|u| format!("@{u}")),
                    user_id: Some(o.sender_user.id as i64),
                },
                o.date,
            ),
            MessageOrigin::HiddenUser(o) => (
                super::attachments::MessageAuthor {
                    name: o.sender_user_name.clone(),
                    username: None,
                    user_id: None,
                },
                o.date,
            ),
            MessageOrigin::Chat(o) => (
                super::attachments::MessageAuthor {
                    name: msg_ext::chat_title(&o.sender_chat)
                        .unwrap_or("unknown")
                        .to_owned(),
                    username: msg_ext::chat_username(&o.sender_chat).map(|u| format!("@{u}")),
                    user_id: None,
                },
                o.date,
            ),
            MessageOrigin::Channel(o) => (
                super::attachments::MessageAuthor {
                    name: msg_ext::chat_title(&o.chat).unwrap_or("unknown").to_owned(),
                    username: msg_ext::chat_username(&o.chat).map(|u| format!("@{u}")),
                    user_id: None,
                },
                o.date,
            ),
        };
        super::attachments::ForwardInfo {
            from,
            date: chrono::DateTime::from_timestamp(date as i64, 0).unwrap_or_default(),
        }
    });

    // Extract reply-to message ID
    let reply_to_id = msg
        .reply_to_message
        .as_ref()
        .map(|message| message.message_id);
    let quoted_text = msg_ext::quote_text(msg);

    // Resolve the conversation before checking the auth intercept: only the
    // chat/thread that received setup instructions may submit its token.
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(msg);
    if let Some(text_val) = &text {
        let scope = AuthRequestScope::new(chat_id, eff_thread_id);
        if let Some(sender) = ctx.pending_auth.lock().await.take_sender_for_scope(scope) {
            tracing::info!(
                chat_id,
                eff_thread_id,
                "handle_message: forwarding message as auth code"
            );
            sender
                .send(text_val.clone())
                .map_err(|_| other_err("setup-token receiver closed"))?;
            return Ok(());
        }
    }

    super::archive::archive_routed_dm_message(&ctx.agent_dir.0, msg, decision.address.clone());

    let key: SessionKey = (chat_id, eff_thread_id);
    let worker_exists = ctx.worker_map.contains_key(&key);
    tracing::info!(
        ?key,
        worker_exists,
        has_text = text.is_some(),
        attachment_count = attachments.len(),
        "handle_message: routing"
    );

    // Build ChatContext: DM emits nothing; Group emits id/title/topic_id.
    // General topic has thread_id = 1 in supergroups — normalise to "no topic".
    let chat_ctx = if msg_ext::is_private(&msg.chat) {
        super::attachments::ChatContext::Private { id: msg.chat.id }
    } else {
        super::attachments::ChatContext::Group {
            id: msg.chat.id,
            title: msg_ext::chat_title(&msg.chat).map(|s| s.to_string()),
            topic_id: msg.message_thread_id.map(i64::from).filter(|&n| n > 1),
        }
    };

    // Capture the replied-to message for all targets. The worker gate decides
    // whether to render full text, a locator, or a note.
    let (reply_to_body, reply_to_attachments) = match msg.reply_to_message.as_ref() {
        Some(r) => {
            let from = r.from.as_ref();
            let is_bot_target = from
                .map(|f| f.is_bot && f.id == identity.user_id)
                .unwrap_or(false);
            let author = match from {
                Some(f) => super::attachments::MessageAuthor {
                    name: msg_ext::full_name(f),
                    username: f.username.as_ref().map(|u| format!("@{u}")),
                    user_id: Some(f.id as i64),
                },
                // No `from` (channel auto-forward / anonymous admin): attribute
                // to the sending chat, mirroring the primary-message author path
                // rather than emitting an empty name.
                None => super::attachments::MessageAuthor {
                    name: msg_ext::chat_title(&r.chat).unwrap_or("unknown").to_owned(),
                    username: msg_ext::chat_username(&r.chat).map(|u| format!("@{u}")),
                    user_id: None,
                },
            };
            let body = super::attachments::RawReply {
                author,
                text: msg_ext::text_or_caption(r).map(|t| t.to_string()),
                attachments: vec![],
                is_bot_target,
            };
            let inbound = super::attachments::extract_attachments(r);
            (Some(body), inbound)
        }
        None => (None, vec![]),
    };

    // Strip `@botname` mentions from text AFTER interceptors (auth code / MCP
    // token) have seen the raw string. No-op when the pattern isn't present.
    let text = text
        .map(|t| super::mention::strip_bot_mentions(&t, &identity.username))
        .filter(|t| !t.trim().is_empty());

    let debounce_msg = DebounceMsg {
        message_id: msg.message_id,
        text,
        timestamp: chrono::Utc::now(),
        attachments,
        author,
        forward_info,
        reply_to_id,
        quoted_text,
        address: decision.address.clone(),
        response_mode: decision.response_mode,
        group_open: decision.group_open,
        chat: chat_ctx,
        reply_to_body,
        reply_to_attachments,
        media_group_id: msg.media_group_id.clone(),
    };

    // Check for existing worker or spawn a new one.
    // Pitfall 7 mitigation: if send fails, the worker task has exited -- remove + respawn.
    // Note: DashMap read guard is NOT held across .await to avoid blocking. Clone the
    // sender before awaiting.
    loop {
        let maybe_tx = ctx.worker_map.get(&key).map(|entry| entry.value().clone());
        match maybe_tx {
            Some(tx) => match tx.send(debounce_msg.clone()).await {
                Ok(_) => break,
                Err(e) => {
                    // Worker task panicked or exited -- remove stale sender and respawn
                    tracing::warn!(?key, "worker send failed, respawning: {:#}", e);
                    ctx.worker_map.remove(&key);
                    // fall through to spawn new worker below on next loop iteration
                }
            },
            None => {
                // No sender yet -- spawn a new worker task
                let agent_name = ctx
                    .agent_dir
                    .0
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let worker_ctl = &ctx.worker_ctl;
                let wctx = WorkerContext {
                    chat_id,
                    effective_thread_id: eff_thread_id,
                    agent_dir: ctx.agent_dir.0.clone(),
                    agent_name,
                    bot: ctx.bot.clone(),
                    agent_db_dir: ctx.agent_dir.0.clone(),
                    debug: Arc::clone(&settings.debug),
                    // Seeded empty on purpose: `spawn_worker`'s debounce loop
                    // re-resolves this from `sandbox_runtime` at the top of
                    // every batch, because the supervisor publishes a new
                    // handle after each recovery. Seeding a handle here would
                    // be dead on arrival and invite the next reader to trust
                    // a snapshot.
                    sandbox: None,
                    pending_auth: Arc::clone(&ctx.pending_auth),
                    show_thinking: settings.show_thinking,
                    model: settings.model.clone(),
                    stop_tokens: Arc::clone(&worker_ctl.stop_tokens),
                    session_locks: Arc::clone(&worker_ctl.session_locks),
                    bootstrap_lock: Arc::clone(&worker_ctl.bootstrap_lock),
                    compact_timers: Arc::clone(&worker_ctl.compact_timers),
                    bg_requests: Arc::clone(&worker_ctl.bg_requests),
                    bg_handoff_gates: Arc::clone(&worker_ctl.bg_handoff_gates),
                    thinking_visibility: Arc::clone(&worker_ctl.thinking_visibility),
                    idle_timestamp: Arc::clone(&ctx.idle_ts.0),
                    internal_client: Arc::clone(&ctx.internal_api.0),
                    progress_state: worker_ctl.progress.clone(),
                    hindsight: settings.hindsight.clone(),
                    prefetch_cache: settings.prefetch_cache.clone(),
                    memory_status_last: Arc::new(DashMap::new()),
                    upgrade_lock: Arc::clone(&settings.upgrade_lock),
                    stt: settings.stt.clone(),
                    learning: settings.learning.clone(),
                    mcp_init_health: Arc::clone(&settings.mcp_init_health),
                    shutdown: settings.shutdown.clone(),
                    sandbox_runtime: Arc::clone(&settings.sandbox_runtime),
                };
                let tx = spawn_worker(key, wctx, Arc::clone(&ctx.worker_map));
                ctx.worker_map.insert(key, tx.clone());
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
///
/// A `focus` deep-link payload (`/start f<chat_id>_<thread_id>`, produced by
/// `/set_focus` in a group/topic) re-emits the focus Mini App button here in the
/// DM, scoped to the originating conversation. Inline `web_app` buttons are
/// private-chat-only, so the group bounces the operator through this DM path.
pub(crate) async fn handle_start(
    ctx: &HandlerCtx,
    msg: &Message,
    payload: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let home = &ctx.home;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "start", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    // The deep-link scope is attacker-supplied, but the routing filter only lets
    // trusted senders reach a private chat (`make_routing_filter` drops untrusted
    // DM senders), and the dashboard re-validates the operator's `tma` initData
    // against the allowlist plus the focus-scope MAC on every focus read/write.
    // So minting a token for the requested scope here cannot grant an untrusted
    // user any access. Keep that filter gate if this handler ever stops being
    // DM-only.
    if let Some((scope_chat, scope_thread)) =
        super::focus_deeplink::decode_focus_start_param(payload.trim())
    {
        tracing::info!(
            scope_chat,
            scope_thread,
            "start: focus deep-link → focus button"
        );
        return send_focus_webapp_button(
            bot,
            msg.chat.id,
            home,
            agent_dir,
            scope_chat,
            scope_thread,
        )
        .await;
    }
    bot.send_text(msg.chat.id, "Agent is running. Send a message to start.")
        .await?;
    Ok(())
}

/// Build a single-button inline keyboard launching a Mini App at `url`.
fn webapp_keyboard(label: &str, url: url::Url) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text(label)
                .web_app(WebAppInfo {
                    url: url.to_string(),
                })
                .build(),
        ]])
        .build()
}

/// Build a single-button inline keyboard linking to `url`.
fn url_keyboard(label: &str, url: url::Url) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text(label)
                .url(url.to_string())
                .build(),
        ]])
        .build()
}

/// Handle the /dashboard command -- send a Telegram Mini App launch button.
pub(crate) async fn handle_dashboard(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let home = &ctx.home;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat)
        && !super::allowlist_commands::sender_is_trusted(msg, &ctx.allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id,
            user_id = msg.from.as_ref().map(|user| user.id),
            "/dashboard ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| other_err(format!("dashboard: read config.yaml: {e:#}")))?;
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            other_err(format!(
                "dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| other_err(format!("dashboard: invalid URL: {e:#}")))?;
    let keyboard = webapp_keyboard("Open dashboard", url);

    let eff_thread_id = effective_thread_id(msg);
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);
    bot.send_message_opts(
        msg.chat.id,
        "Dashboard",
        false,
        thread,
        None,
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Handle the /new command — start a new session.
pub(crate) async fn handle_new(
    ctx: &HandlerCtx,
    msg: &Message,
    name: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "new", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(msg);
    let key: SessionKey = (chat_id, eff_thread_id);

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| other_err(format!("new: open DB: {:#}", e)))?;

    let prev_uuid = deactivate_current(&conn, chat_id, eff_thread_id)
        .await
        .map_err(|e| other_err(format!("new: deactivate: {:#}", e)))?;

    // Kill worker — channel closes, CC subprocess killed via kill_on_drop
    ctx.worker_map.remove(&key);

    let name = name.trim().to_string();
    let mut reply = String::new();

    if !name.is_empty() {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let label = truncate_label(&name);
        create_session(&conn, chat_id, eff_thread_id, &new_uuid, Some(label))
            .await
            .map_err(|e| other_err(format!("new: create session: {:#}", e)))?;
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

    send_html_reply(bot, chat_id, eff_thread_id, &reply).await?;

    tracing::info!(?key, "new session");
    Ok(())
}

/// Handle the /list command — show all sessions for this chat+thread.
pub(crate) async fn handle_list(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "list", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(msg);

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| other_err(format!("list: open DB: {:#}", e)))?;

    let sessions = list_sessions(&conn, chat_id, eff_thread_id)
        .await
        .map_err(|e| other_err(format!("list: query: {:#}", e)))?;

    if sessions.is_empty() {
        bot.send_text(chat_id, "No sessions yet. Send a message to start one.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("Sessions:\n");
    for s in &sessions {
        text.push_str(&format_session_line(s));
    }

    send_html_reply(bot, chat_id, eff_thread_id, &text).await?;
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
pub(crate) async fn handle_switch(
    ctx: &HandlerCtx,
    msg: &Message,
    uuid: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "switch", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(msg);
    let key: SessionKey = (chat_id, eff_thread_id);
    let uuid = uuid.trim().to_string();

    if uuid.is_empty() {
        bot.send_text(
            chat_id,
            "Usage: /switch <uuid>\nUse /list to see available sessions.",
        )
        .await?;
        return Ok(());
    }

    let conn = right_db::open_connection(&agent_dir.0, false)
        .await
        .map_err(|e| other_err(format!("switch: open DB: {:#}", e)))?;

    let matches = find_sessions_by_uuid(&conn, chat_id, eff_thread_id, &uuid)
        .await
        .map_err(|e| other_err(format!("switch: query: {:#}", e)))?;

    match matches.len() {
        0 => {
            send_html_reply(
                bot,
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
                bot.send_text(chat_id, "Already active.").await?;
                return Ok(());
            }

            // activate_session atomically deactivates any other active session
            activate_session(&conn, target.id)
                .await
                .map_err(|e| other_err(format!("switch: activate: {:#}", e)))?;

            ctx.worker_map.remove(&key);

            let label = target.label.as_deref().unwrap_or("(unnamed)");
            send_html_reply(
                bot,
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
            send_html_reply(bot, chat_id, eff_thread_id, &text).await?;
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
pub(crate) async fn handle_mcp(
    ctx: &HandlerCtx,
    msg: &Message,
    _args: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    let home = &ctx.home;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "mcp", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!(agent_dir = %agent_dir.0.display(), "mcp: opening dashboard");
    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| other_err(format!("mcp dashboard: read config.yaml: {e:#}")))?;
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            other_err(format!(
                "mcp dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| other_err(format!("mcp dashboard: invalid URL: {e:#}")))?;
    url.set_query(Some("view=mcp"));

    let keyboard = webapp_keyboard(dashboard_mcp_button_label(), url);

    let eff_thread_id = effective_thread_id(msg);
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);
    bot.send_message_opts(msg.chat.id, "MCP", false, thread, None, Some(keyboard))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /providers command handler
// ---------------------------------------------------------------------------

/// Handle the /providers command by opening the dashboard providers view.
pub(crate) async fn handle_providers(
    ctx: &HandlerCtx,
    msg: &Message,
    _args: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    let home = &ctx.home;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(
            cmd = "providers",
            "ignoring command in group chat (DM-only)"
        );
        return Ok(());
    }
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            other_err(format!(
                "providers dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    tracing::info!(agent_dir = %agent_dir.0.display(), "providers: opening dashboard");
    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| other_err(format!("providers dashboard: read config.yaml: {e:#}")))?;
    let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| other_err(format!("providers dashboard: invalid URL: {e:#}")))?;
    url.set_query(Some("view=providers"));

    let keyboard = webapp_keyboard("Open providers dashboard", url);

    let eff_thread_id = effective_thread_id(msg);
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);
    bot.send_message_opts(
        msg.chat.id,
        "Providers",
        false,
        thread,
        None,
        Some(keyboard),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /set_focus command handler
// ---------------------------------------------------------------------------

/// Handle the /set_focus command, opening the dashboard focus view scoped to the
/// current `(chat_id, effective_thread_id)`.
///
/// In a DM the focus Mini App opens directly via an inline `web_app` button. In
/// a group or topic that button is impossible — Telegram rejects inline
/// `web_app` buttons outside private chats (`BUTTON_TYPE_INVALID`) — so we send a
/// `t.me/<bot>?start=<scope>` deep-link `url` button instead. Tapping it opens
/// the DM and delivers `/start <scope>`, where `handle_start` re-emits the
/// `web_app` button scoped to this conversation.
pub(crate) async fn handle_set_focus(
    ctx: &HandlerCtx,
    msg: &Message,
    _args: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    let home = &ctx.home;
    let identity = &ctx.identity;
    let eff_thread_id = effective_thread_id(msg);
    if msg_ext::is_private(&msg.chat) {
        tracing::info!(agent_dir = %agent_dir.0.display(), "set_focus: opening dashboard (DM)");
        return send_focus_webapp_button(
            bot,
            msg.chat.id,
            home,
            agent_dir,
            msg.chat.id,
            eff_thread_id,
        )
        .await;
    }

    tracing::info!(
        chat_id = msg.chat.id,
        thread_id = eff_thread_id,
        "set_focus: sending DM deep-link button (group/topic)"
    );
    let param = super::focus_deeplink::encode_focus_start_param(msg.chat.id, eff_thread_id);
    let link = format!("https://t.me/{}?start={}", identity.username, param);
    let url = url::Url::parse(&link)
        .map_err(|e| other_err(format!("set_focus: build deep link: {e:#}")))?;
    let keyboard = url_keyboard("Set focus", url);
    let thread = (eff_thread_id != 0).then_some(eff_thread_id as i32);
    bot.send_message_opts(
        msg.chat.id,
        "Open focus settings:",
        false,
        thread,
        None,
        Some(keyboard),
    )
    .await?;
    Ok(())
}

/// Send the focus Mini App `web_app` button into a private chat, scoped to
/// `(scope_chat, scope_thread)`. DM-only: Telegram rejects inline `web_app`
/// buttons outside private chats, so the scope of the conversation being focused
/// is carried in the URL/token rather than implied by the send target.
async fn send_focus_webapp_button(
    bot: &super::BotType,
    send_to: i64,
    home: &RightHome,
    agent_dir: &AgentDir,
    scope_chat: i64,
    scope_thread: i64,
) -> Result<(), TgError> {
    let global_config = right_config::read_global_config(&home.0)
        .map_err(|e| other_err(format!("set_focus dashboard: read config.yaml: {e:#}")))?;
    let agent_name = agent_dir
        .0
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            other_err(format!(
                "set_focus dashboard: invalid agent directory name: {}",
                agent_dir.0.display()
            ))
        })?;
    let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
        .map_err(|e| other_err(format!("set_focus dashboard: invalid URL: {e:#}")))?;
    let focus_token = super::dashboard::generate_focus_scope_token(
        bot.token(),
        agent_name,
        scope_chat,
        scope_thread,
    );
    url.query_pairs_mut()
        .append_pair("view", "focus")
        .append_pair("chat_id", &scope_chat.to_string())
        .append_pair("thread_id", &scope_thread.to_string())
        .append_pair("token", &focus_token);

    let keyboard = webapp_keyboard("Set focus", url);
    bot.send_message_opts(send_to, "Focus", false, None, None, Some(keyboard))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /cron command handler
// ---------------------------------------------------------------------------

/// Handle the /cron command — routes to list (no args) or detail (job name).
pub(crate) async fn handle_cron(
    ctx: &HandlerCtx,
    msg: &Message,
    args: String,
) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let agent_dir = &ctx.agent_dir;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "cron", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    if args.trim().is_empty() {
        handle_cron_list(bot, msg, &agent_dir.0).await
    } else {
        handle_cron_detail(bot, msg, args.trim(), &agent_dir.0).await
    }
}

/// `/cron` — list all cron jobs with human-readable schedule and last run status.
async fn handle_cron_list(
    bot: &super::BotType,
    msg: &Message,
    agent_dir: &Path,
) -> Result<(), TgError> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| other_err(format!("DB open failed: {e:#}")))?;

    let specs = right_agent::cron_spec::load_specs_from_db(&conn)
        .await
        .map_err(|e| other_err(format!("load specs failed: {e:#}")))?;

    if specs.is_empty() {
        bot.send_text(msg.chat.id, "No cron jobs configured.")
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
            .map_err(|e| other_err(format!("get runs failed: {e:#}")))?;

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
    bot: &super::BotType,
    msg: &Message,
    job_name: &str,
    agent_dir: &Path,
) -> Result<(), TgError> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| other_err(format!("DB open failed: {e:#}")))?;

    let detail = right_agent::cron_spec::get_spec_detail(&conn, job_name)
        .await
        .map_err(|e| other_err(format!("query failed: {e:#}")))?;

    let Some(detail) = detail else {
        bot.send_text(msg.chat.id, &format!("Cron job '{job_name}' not found."))
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
        .map_err(|e| other_err(format!("get runs failed: {e:#}")))?;

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
pub(crate) async fn handle_doctor(ctx: &HandlerCtx, msg: &Message) -> Result<(), TgError> {
    let bot = &ctx.bot;
    let home = &ctx.home;
    if !msg_ext::is_private(&msg.chat) {
        tracing::debug!(cmd = "doctor", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    tracing::info!("handle_doctor: running diagnostics");
    let checks = right_agent::doctor::run_doctor(&home.0).await;
    for text in format_doctor_result_messages(&checks) {
        if let Err(e) = bot
            .send_message_opts(msg.chat.id, &text, true, None, None, None)
            .await
        {
            tracing::error!("handle_doctor: Telegram rejected HTML message: {e:#}");
            bot.send_text(msg.chat.id, &strip_html_tags(&text)).await?;
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
pub(crate) async fn handle_usage(
    ctx: &HandlerCtx,
    msg: &Message,
    _arg: String,
) -> Result<(), TgError> {
    handle_dashboard(ctx, msg).await
}

// ---------------------------------------------------------------------------
// Stop button callback query handler
// ---------------------------------------------------------------------------

/// Handle the Stop button callback query from thinking messages.
///
/// Callback data format: `stop:{chat_id}:{eff_thread_id}`
/// Looks up the CancellationToken in StopTokens and cancels it.
pub(crate) async fn handle_stop_callback(
    ctx: &HandlerCtx,
    q: &CallbackQuery,
) -> Result<(), TgError> {
    let worker_ctl = &ctx.worker_ctl;
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();

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

    ctx.bot.answer_callback(&q.id, text, false).await?;
    Ok(())
}

fn apply_thinking_toggle_callback(
    thinking_visibility: &crate::telegram::ThinkingVisibility,
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
pub(crate) async fn handle_thinking_toggle_callback(
    ctx: &HandlerCtx,
    q: &CallbackQuery,
) -> Result<(), TgError> {
    let text = q
        .data
        .as_deref()
        .and_then(|data| apply_thinking_toggle_callback(&ctx.worker_ctl.thinking_visibility, data));
    ctx.bot.answer_callback(&q.id, text, false).await?;
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
pub(crate) async fn handle_bg_callback(ctx: &HandlerCtx, q: &CallbackQuery) -> Result<(), TgError> {
    let worker_ctl = &ctx.worker_ctl;
    let data = q.data.as_deref().unwrap_or("");
    let parts: Vec<&str> = data.splitn(3, ':').collect();

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

    ctx.bot.answer_callback(&q.id, text, false).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use frankenstein::types::Chat;

    use super::*;

    #[test]
    fn pending_auth_start_if_idle_does_not_overwrite_sender() {
        let mut state = PendingAuthState::default();
        let first_scope = AuthRequestScope::new(1, 2);
        let second_scope = AuthRequestScope::new(3, 4);
        let first = state.start_if_idle(first_scope);
        let first_request_id = match first {
            AuthRequestStart::Started { request_id, .. } => request_id,
            AuthRequestStart::AlreadyPending { .. } => panic!("first request must start"),
        };

        match state.start_if_idle(second_scope) {
            AuthRequestStart::AlreadyPending { owner } => assert_eq!(owner, first_scope),
            AuthRequestStart::Started { .. } => panic!("second request must not overwrite first"),
        }
        assert!(state.cleanup_if_owned(first_request_id));
    }

    #[tokio::test]
    async fn pending_auth_token_sender_matches_only_owner_scope() {
        let mut state = PendingAuthState::default();
        let owner = AuthRequestScope::new(11, 22);
        let other = AuthRequestScope::new(11, 23);
        let receiver = match state.start_if_idle(owner) {
            AuthRequestStart::Started { receiver, .. } => receiver,
            AuthRequestStart::AlreadyPending { .. } => panic!("request must start"),
        };

        assert!(state.take_sender_for_scope(other).is_none());
        match state.start_if_idle(other) {
            AuthRequestStart::AlreadyPending {
                owner: pending_owner,
            } => {
                assert_eq!(pending_owner, owner);
            }
            AuthRequestStart::Started { .. } => panic!("non-owner token attempt cleared request"),
        }
        let sender = state
            .take_sender_for_scope(owner)
            .expect("owner scope must take sender");
        sender.send("opaque-token".to_owned()).unwrap();
        assert_eq!(receiver.await.unwrap(), "opaque-token");
        match state.start_if_idle(other) {
            AuthRequestStart::AlreadyPending {
                owner: pending_owner,
            } => {
                assert_eq!(pending_owner, owner);
            }
            AuthRequestStart::Started { .. } => panic!("token claim must await owner cleanup"),
        }
    }

    #[test]
    fn pending_auth_cleanup_rejects_stale_request_id() {
        let mut state = PendingAuthState::default();
        let first_id = match state.start_if_idle(AuthRequestScope::new(1, 0)) {
            AuthRequestStart::Started { request_id, .. } => request_id,
            AuthRequestStart::AlreadyPending { .. } => panic!("first request must start"),
        };
        assert!(state.cleanup_if_owned(first_id));

        let second_scope = AuthRequestScope::new(2, 0);
        let second_id = match state.start_if_idle(second_scope) {
            AuthRequestStart::Started { request_id, .. } => request_id,
            AuthRequestStart::AlreadyPending { .. } => panic!("second request must start"),
        };
        assert!(!state.cleanup_if_owned(first_id));
        match state.start_if_idle(AuthRequestScope::new(3, 0)) {
            AuthRequestStart::AlreadyPending { owner } => assert_eq!(owner, second_scope),
            AuthRequestStart::Started { .. } => panic!("stale cleanup removed the newer request"),
        }
        assert!(state.cleanup_if_owned(second_id));
    }

    fn make_private_chat() -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "type": "private",
            "first_name": "Test"
        }))
        .unwrap()
    }

    fn make_group_chat() -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": -1,
            "type": "group",
            "title": "Group"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn msg_ext_is_private_detects_dm() {
        assert!(super::msg_ext::is_private(&make_private_chat()));
        assert!(!super::msg_ext::is_private(&make_group_chat()));
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
        let map: crate::telegram::ThinkingVisibility = Arc::new(DashMap::new());
        let key = (42_i64, 7_i64);
        map.insert(key, false);

        let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
        assert_eq!(text, Some("Showing thinking..."));

        assert!(*map.get(&key).unwrap().value());
    }

    #[tokio::test]
    async fn thinking_toggle_hide_updates_active_visibility() {
        let map: crate::telegram::ThinkingVisibility = Arc::new(DashMap::new());
        let key = (42_i64, 7_i64);
        map.insert(key, true);

        let text = apply_thinking_toggle_callback(&map, "think:42:7:hide");
        assert_eq!(text, Some("Hiding thinking..."));

        assert!(!*map.get(&key).unwrap().value());
    }

    #[tokio::test]
    async fn thinking_toggle_after_finish_reports_already_finished() {
        let map: crate::telegram::ThinkingVisibility = Arc::new(DashMap::new());

        let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
        assert_eq!(text, Some("Already finished"));
    }

    #[tokio::test]
    async fn thinking_toggle_malformed_callback_returns_none() {
        let map: crate::telegram::ThinkingVisibility = Arc::new(DashMap::new());

        assert_eq!(apply_thinking_toggle_callback(&map, "think:42:7"), None);
        assert_eq!(apply_thinking_toggle_callback(&map, "stop:42:7"), None);
    }
}
