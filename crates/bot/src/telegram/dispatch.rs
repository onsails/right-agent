//! Telegram bot setup + lifecycle (webhook transport).
//!
//! - DashMap per-session worker map (SES-05, D-11)
//! - `BotCommand` parse for /new, /list, /switch (multi-session) — see `command.rs`
//! - ChatId allow-list filter (BOT-05, via filter.rs)
//! - SIGTERM + SIGINT graceful shutdown (BOT-04)
//! - BOT-04 subprocess cleanup via kill_on_drop(true) in each worker (no children registry)
//!
//! Update routing is in `router.rs` (replaces the former teloxide
//! `Dispatcher`/`dptree`); the webhook HTTP handler is in `webhook.rs`.
//!
//! GOTCHA: queued messages in a worker channel are lost on worker task panic.
//! When the worker is respawned (Pitfall 7), the in-progress batch is discarded.
//! This is an accepted trade-off -- retrying arbitrary messages is not safe.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use frankenstein::types::{BotCommandScope, BotCommandScopeChat, Message};
use right_agent::agent::allowlist::AllowlistHandle;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::BotType;
use super::handler::{
    AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, RightHome, SshConfigPath,
};
use super::mention::BotIdentity;
use super::router::HandlerCtx;
use super::tg_bot::RightBot;
use super::worker::{DebounceMsg, SessionKey};

/// Visible-command rows for `setMyCommands` (`usage` hidden) — see `command.rs`.
fn visible_bot_commands() -> Vec<frankenstein::types::BotCommand> {
    super::command::visible()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreFilterLogMeta {
    pub(crate) chat_id: i64,
    pub(crate) chat_kind: &'static str,
    pub(crate) has_text: bool,
    pub(crate) has_caption: bool,
    pub(crate) attachment_count: usize,
    pub(crate) entity_count: usize,
}

pub(crate) fn pre_filter_log_meta(msg: &Message) -> PreFilterLogMeta {
    PreFilterLogMeta {
        chat_id: msg.chat.id,
        chat_kind: super::msg_ext::chat_type_label(&msg.chat),
        has_text: msg.text.is_some(),
        has_caption: msg.caption.is_some(),
        attachment_count: super::attachments::extract_attachments(msg).len(),
        entity_count: message_entity_count(msg),
    }
}

fn message_entity_count(msg: &Message) -> usize {
    let text_entities = msg.entities.as_ref().map_or(0, |entities| entities.len());
    let caption_entities = msg
        .caption_entities
        .as_ref()
        .map_or(0, |entities| entities.len());

    text_entities + caption_entities
}

/// Set up the Telegram bot and return the webhook router plus a lifecycle future.
///
/// - Connects the bot (`getMe` once) and resolves identity for group-mention
///   detection.
/// - Builds the shared [`HandlerCtx`].
/// - Registers commands across the Default / AllPrivateChats / AllGroupChats
///   scopes and cleans up stale per-chat / per-language / admin scopes.
/// - Spawns the SIGTERM/SIGINT signal listener and the shutdown task.
///
/// The returned `axum::Router` is nested into the bot's UDS app by `lib.rs`
/// (so cloudflared can POST updates); the returned future resolves when the
/// bot shuts down and drains in-flight background handoffs.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn setup_telegram(
    token: String,
    allowlist: AllowlistHandle,
    legacy_chat_scope_ids: Vec<i64>,
    agent_dir: PathBuf,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    show_thinking: bool,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    hindsight_wrapper: Option<std::sync::Arc<right_memory::ResilientHindsight>>,
    prefetch_cache: Option<right_memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    stt: Option<std::sync::Arc<crate::stt::SttContext>>,
    claude_health: Arc<crate::keepalive::ClaudeHealth>,
    sandbox_runtime: std::sync::Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    session_locks: super::SessionLocks,
    bg_requests: super::BgRequests,
    stop_tokens: super::StopTokens,
    progress_state: super::progress::ProgressState,
    compact_timers: super::CompactTimers,
    webhook_secret: String,
) -> miette::Result<(axum::Router, impl Future<Output = ()> + Send)> {
    let bot = RightBot::connect(token)
        .await
        .map_err(|e| miette::miette!("bot connect (getMe) failed: {e:#}"))?;

    // Resolve bot identity (username + user_id) from the cached getMe — required
    // for group mention detection.
    let me = bot.me();
    let username = me.username.clone().ok_or_else(|| {
        miette::miette!("bot has no username; cannot set up group-mention detection")
    })?;
    let identity = BotIdentity {
        username: username.clone(),
        user_id: me.id,
    };
    tracing::info!(%username, user_id = identity.user_id, "bot identity resolved");
    let identity_arc = Arc::new(identity);

    // Shared state
    let learning = right_agent::agent::discovery::parse_agent_config(&agent_dir)?
        .map(|config| config.learning)
        .unwrap_or_default();
    let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> = Arc::new(DashMap::new());
    let agent_dir_arc: Arc<AgentDir> = Arc::new(AgentDir(agent_dir));
    let ssh_config_arc: Arc<SshConfigPath> = Arc::new(SshConfigPath(ssh_config_path));
    let home_arc: Arc<RightHome> = Arc::new(RightHome(home));
    let auth_watcher_arc: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let auth_code_arc = Arc::new(tokio::sync::Mutex::new(None));
    let intercept_slots_arc: Arc<InterceptSlots> = Arc::new(InterceptSlots {
        auth_code: Arc::clone(&auth_code_arc),
        auth_watcher: Arc::clone(&auth_watcher_arc),
    });
    let internal_api_arc: Arc<InternalApi> = Arc::new(InternalApi(internal_client));
    let worker_shutdown = CancellationToken::new();
    let settings_arc: Arc<AgentSettings> = Arc::new(AgentSettings {
        show_thinking,
        model,
        resolved_sandbox,
        hindsight: hindsight_wrapper,
        prefetch_cache,
        upgrade_lock,
        debug,
        stt,
        learning,
        claude_health,
        shutdown: worker_shutdown.clone(),
        sandbox_runtime,
    });
    let thinking_visibility: super::ThinkingVisibility = Arc::new(DashMap::new());
    let bg_handoff_gates: super::BgHandoffGates = Arc::new(DashMap::new());

    // Spawn memory-alerts watcher (AuthFailed + client-flood) — only when Hindsight is configured.
    // Pass the live allowlist handle; recipients are resolved at broadcast time so
    // /allow / /deny / allowlist.yaml hot-reload changes after startup are honored.
    if let Some(ref w) = settings_arc.hindsight {
        super::memory_alerts::spawn_watcher(
            bot.clone(),
            w.clone(),
            agent_dir_arc.0.clone(),
            allowlist.clone(),
        );
    }

    let worker_ctl = super::WorkerControlDeps {
        stop_tokens: Arc::clone(&stop_tokens),
        session_locks,
        bg_requests: Arc::clone(&bg_requests),
        bg_handoff_gates: Arc::clone(&bg_handoff_gates),
        thinking_visibility,
        progress: progress_state,
        compact_timers,
    };

    let ctx = Arc::new(HandlerCtx {
        bot: bot.clone(),
        allowlist: allowlist.clone(),
        identity: Arc::clone(&identity_arc),
        worker_map,
        agent_dir: Arc::clone(&agent_dir_arc),
        home: home_arc,
        ssh_config: ssh_config_arc,
        intercept_slots: intercept_slots_arc,
        internal_api: internal_api_arc,
        settings: settings_arc,
        idle_ts,
        worker_ctl,
    });

    // SIGTERM/SIGINT listener -- runs in a dedicated std thread because signal-hook's
    // SignalsInfo<WithOrigin> iterator is blocking. The thread reads siginfo_t origin
    // (PID + UID of the sender), looks up the sender's command line via `ps`, logs it,
    // then cancels `signal_cancel`. The tokio task below observes the cancellation and
    // drives the actual shutdown on the runtime.
    let signal_cancel = CancellationToken::new();
    let signal_cancel_thread = signal_cancel.clone();
    std::thread::Builder::new()
        .name("right-signal-listener".to_string())
        .spawn(move || {
            use signal_hook::consts::signal::{SIGINT, SIGTERM};
            use signal_hook::iterator::SignalsInfo;
            use signal_hook::iterator::exfiltrator::WithOrigin;

            let mut signals = SignalsInfo::<WithOrigin>::new([SIGTERM, SIGINT])
                .expect("failed to register SIGTERM/SIGINT handlers via signal-hook");

            if let Some(origin) = (&mut signals).into_iter().next() {
                let sig_name = match origin.signal {
                    SIGTERM => "SIGTERM",
                    SIGINT => "SIGINT",
                    other => {
                        tracing::warn!(
                            signal = other,
                            "signal listener received unexpected signal"
                        );
                        "UNKNOWN"
                    }
                };
                let (pid, cmd) = match origin.process {
                    Some(proc) => {
                        let pid: i32 = proc.pid;
                        (pid, lookup_sender_cmd(pid))
                    }
                    None => (0_i32, String::new()),
                };
                tracing::info!(
                    signal = sig_name,
                    sender_pid = pid,
                    sender_cmd = %cmd,
                    "{sig_name} received from pid={pid} ({cmd}) -- initiating graceful shutdown"
                );
                signal_cancel_thread.cancel();
            }
        })
        .expect("failed to spawn signal listener thread");

    let signal_cancel_task = signal_cancel.clone();
    let stop_tokens_for_shutdown = Arc::clone(&stop_tokens);
    let bg_requests_for_shutdown = Arc::clone(&bg_requests);
    let bg_handoff_gates_for_shutdown = Arc::clone(&bg_handoff_gates);
    let worker_shutdown_for_signal = worker_shutdown.clone();
    let shutdown_for_task = shutdown.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = signal_cancel_task.cancelled() => {
                // Signal listener thread already logged the PID/cmd. Nothing else to log here.
            }
            _ = shutdown_for_task.cancelled() => {
                tracing::info!("config change detected -- initiating graceful shutdown");
                signal_cancel_task.cancel();
            }
        }

        worker_shutdown_for_signal.cancel();
        let requested = super::request_shutdown_backgrounding(
            &stop_tokens_for_shutdown,
            &bg_requests_for_shutdown,
            &bg_handoff_gates_for_shutdown,
        );
        tracing::info!(
            active_foreground = requested,
            "shutdown: requested foreground background handoff"
        );
    });

    // Pre-delete any language-scoped command lists from prior deployments.
    // Telegram's resolution order is: scope+language wins over scope-only, so
    // stale language-scoped entries shadow our fresh per-scope set. Best-effort,
    // errors ignored (e.g. the slot was never populated).
    for scope in [
        BotCommandScope::Default,
        BotCommandScope::AllPrivateChats,
        BotCommandScope::AllGroupChats,
    ] {
        for lang in ["en", "ru"] {
            let _ = bot
                .delete_my_commands(Some(scope.clone()), Some(lang.to_string()))
                .await;
        }
    }
    delete_stale_chat_command_scopes(&bot, &allowlist, &legacy_chat_scope_ids).await;

    // Register commands in three overlapping scopes so autocomplete works in both DMs and
    // groups. Setting only Default is not enough when another tool sharing this token has
    // previously written a narrower scope (e.g. AllPrivateChats) — that narrower scope wins
    // per Telegram's resolution order and shadows Default.
    let commands = visible_bot_commands();
    for scope in [
        BotCommandScope::Default,
        BotCommandScope::AllPrivateChats,
        BotCommandScope::AllGroupChats,
    ] {
        if let Err(e) = bot
            .set_my_commands(commands.clone(), Some(scope.clone()), None)
            .await
        {
            tracing::warn!(?scope, "set_my_commands failed: {e:#}");
        }
    }
    // Clean any stale commands in the admins-only scope we do not populate.
    if let Err(e) = bot
        .delete_my_commands(Some(BotCommandScope::AllChatAdministrators), None)
        .await
    {
        tracing::warn!("delete_my_commands (all_chat_administrators): {e:#}");
    }

    let router = super::webhook::build_webhook_router(webhook_secret, Arc::clone(&ctx));

    // Lifecycle future: returns when shutdown fires, then drains foreground
    // handoff gates and finally cancels the worker-shutdown token.
    let lifecycle = async move {
        signal_cancel.cancelled().await;
        tracing::info!("telegram shutdown signalled -- draining handoff gates");
        let handoffs_done = super::wait_for_handoff_gates_empty(
            &bg_handoff_gates,
            std::time::Duration::from_secs(30),
        )
        .await;
        if handoffs_done {
            tracing::info!("shutdown: foreground handoff gates drained");
        } else {
            tracing::warn!("shutdown: timed out waiting for foreground handoff gates");
        }
        worker_shutdown.cancel();
    };

    Ok((router, lifecycle))
}

fn chat_scope_cleanup_ids(allowlist: &AllowlistHandle, legacy_chat_ids: &[i64]) -> Vec<i64> {
    let mut ids = std::collections::BTreeSet::new();
    ids.extend(legacy_chat_ids.iter().copied().filter(|id| *id != 0));

    let state = allowlist.0.read().expect("allowlist lock poisoned");
    ids.extend(state.users().iter().map(|u| u.id));
    ids.extend(state.groups().iter().map(|g| g.id));

    ids.into_iter().collect()
}

async fn delete_stale_chat_command_scopes(
    bot: &BotType,
    allowlist: &AllowlistHandle,
    legacy_chat_ids: &[i64],
) {
    for chat_id in chat_scope_cleanup_ids(allowlist, legacy_chat_ids) {
        let scope = BotCommandScope::Chat(BotCommandScopeChat {
            chat_id: frankenstein::types::ChatId::Integer(chat_id),
        });
        if let Err(e) = bot.delete_my_commands(Some(scope), None).await {
            tracing::warn!(chat_id, "delete_my_commands(chat) failed: {e:#}");
        }
    }
}

/// Look up the command line of a process by PID, used to attribute SIGTERM/SIGINT senders.
///
/// Runs `ps -p <pid> -o command=` (no header) and returns its trimmed stdout. Returns
/// an empty string on any failure -- the caller logs the PID even when the command is
/// missing, which is enough to identify the sender. We intentionally do not propagate
/// errors here: this is diagnostic metadata, not part of the shutdown contract.
fn lookup_sender_cmd(pid: i32) -> String {
    let output = match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(pid, error = %e, "ps lookup for signal sender failed");
            return String::new();
        }
    };
    if !output.status.success() {
        tracing::debug!(
            pid,
            exit = ?output.status,
            "ps returned non-zero for signal sender pid"
        );
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_scope_cleanup_ids_include_legacy_and_current_allowlist() {
        use right_agent::agent::allowlist::{
            AllowedGroup, AllowedUser, AllowlistFile, AllowlistHandle, AllowlistState, ResponseMode,
        };
        let now: chrono::DateTime<chrono::Utc> = "2026-05-19T12:00:00Z".parse().unwrap();
        let allowlist = AllowlistHandle::new(AllowlistState::from_file(AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 42,
                label: None,
                added_by: None,
                added_at: now,
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: None,
                opened_by: None,
                opened_at: now,
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
            }],
        }));

        assert_eq!(
            chat_scope_cleanup_ids(&allowlist, &[42, 7, 0, -1001]),
            vec![-1001, 7, 42]
        );
    }

    #[tokio::test]
    async fn visible_commands_hide_usage_but_keep_dashboard() {
        let commands = visible_bot_commands();
        let names = commands
            .iter()
            .map(|command| command.command.trim_start_matches('/'))
            .collect::<Vec<_>>();

        assert!(names.contains(&"dashboard"));
        assert!(!names.contains(&"usage"));
        assert!(matches!(
            super::super::command::parse("/usage detail", "right_bot").unwrap(),
            super::super::command::BotCommand::Usage(arg) if arg == "detail"
        ));
    }

    #[tokio::test]
    async fn set_focus_command_uses_snake_case_and_is_visible() {
        let commands = visible_bot_commands();
        let names = commands
            .iter()
            .map(|command| command.command.trim_start_matches('/'))
            .collect::<Vec<_>>();

        assert!(names.contains(&"set_focus"));
        assert!(!names.contains(&"setfocus"));
        assert!(matches!(
            super::super::command::parse("/set_focus now", "right_bot").unwrap(),
            super::super::command::BotCommand::SetFocus(arg) if arg == "now"
        ));
    }

    #[tokio::test]
    async fn pre_filter_log_meta_omits_private_text_and_caption_content() {
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 10,
            "date": 0,
            "chat": {"id": 42, "type": "private", "first_name": "Spammer"},
            "from": {"id": 42, "is_bot": false, "first_name": "Spammer"},
            "caption": "SPAM-CAPTION-SHOULD-NOT-LOG",
            "caption_entities": [{
                "type": "bold",
                "offset": 0,
                "length": 4
            }],
            "document": {
                "file_id": "BAAD-private",
                "file_unique_id": "private-doc",
                "file_name": "spam.pdf",
                "mime_type": "application/pdf",
                "file_size": 1024
            }
        }))
        .unwrap();

        let meta = pre_filter_log_meta(&msg);

        assert_eq!(meta.chat_id, 42);
        assert_eq!(meta.chat_kind, "private");
        assert!(!meta.has_text);
        assert!(meta.has_caption);
        assert_eq!(meta.attachment_count, 1);
        assert_eq!(meta.entity_count, 1);

        let rendered = format!("{meta:?}");
        assert!(!rendered.contains("SPAM-CAPTION-SHOULD-NOT-LOG"));
        assert!(!rendered.contains("spam.pdf"));
    }
}
