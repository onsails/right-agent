//! Teloxide long-polling dispatcher with:
//! - DashMap per-session worker map (SES-05, D-11)
//! - BotCommand schema for /new, /list, /switch (multi-session)
//! - ChatId allow-list filter (BOT-05, via filter.rs)
//! - SIGTERM + SIGINT graceful shutdown (BOT-04)
//! - BOT-04 subprocess cleanup via kill_on_drop(true) in each worker (no children registry)
//!
//! GOTCHA: queued messages in a worker channel are lost on worker task panic.
//! When the worker is respawned (Pitfall 7), the in-progress batch is discarded.
//! This is an accepted trade-off -- retrying arbitrary messages is not safe.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use teloxide::RequestError;
use teloxide::dispatching::{DefaultKey, UpdateFilterExt};
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::BotType;
use super::bot::build_bot;
use super::filter::make_routing_filter;
use super::handler::{
    AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, PendingTokenSlot,
    RightHome, SshConfigPath, handle_bg_callback, handle_cron, handle_doctor, handle_list,
    handle_mcp, handle_message, handle_new, handle_start, handle_stop_callback, handle_switch,
    handle_usage,
};
use super::mention::BotIdentity;
use super::oauth_callback::PendingAuthMap;
use super::worker::{DebounceMsg, SessionKey};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    #[command(description = "Start interacting with this agent")]
    Start,
    #[command(description = "Start a new conversation")]
    New(String),
    #[command(description = "List all sessions")]
    List,
    #[command(description = "Switch to another session")]
    Switch(String),
    #[command(description = "MCP server management (list/add/remove)")]
    Mcp(String),
    #[command(description = "Run diagnostics")]
    Doctor,
    #[command(description = "Cron job status (list or detail)")]
    Cron(String),
    #[command(description = "Add trusted user (reply to user, or /allow <user_id>)")]
    Allow(String),
    #[command(description = "Remove trusted user")]
    Deny(String),
    #[command(description = "List trusted users and opened groups")]
    Allowed,
    #[command(
        description = "Open this group for all members (group only)",
        rename = "allow_all"
    )]
    AllowAll,
    #[command(description = "Close this group (group only)", rename = "deny_all")]
    DenyAll,
    #[command(description = "Show usage summary (add 'detail' for raw tokens)")]
    Usage(String),
}

/// Run the teloxide long-polling dispatcher.
///
/// - Accepts agent_dir for session DB access and CC subprocess invocation.
/// - Creates a DashMap<SessionKey, Sender<DebounceMsg>> for per-session workers.
/// - Schema: filter by chat_id -> branch /new, /list, /switch commands -> dispatch text messages.
/// - SIGTERM/SIGINT: kill in-flight subprocesses, shutdown dispatcher.
///
/// BOT-04 subprocess cleanup strategy: use kill_on_drop(true) on each Child in invoke_cc.
/// When a worker task exits (channel closed, panic, or /new), the Child is dropped, which
/// kills the subprocess. No explicit children registry is needed or maintained.
/// Rationale: Arc<Mutex<Vec<Child>>> was rejected because invoke_cc never added children
/// to the registry, making the kill loop dead code. kill_on_drop is sufficient.
#[allow(clippy::too_many_arguments)]
pub async fn run_telegram<L>(
    token: String,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    show_thinking: bool,
    model: Option<String>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<right_agent::mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    hindsight_wrapper: Option<std::sync::Arc<right_agent::memory::ResilientHindsight>>,
    prefetch_cache: Option<right_agent::memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    stt: Option<std::sync::Arc<crate::stt::SttContext>>,
    session_locks: super::SessionLocks,
    bg_requests: super::BgRequests,
    update_listener: L,
) -> miette::Result<()>
where
    L: teloxide::update_listeners::UpdateListener<Err = std::convert::Infallible>
        + Send
        + 'static,
{
    let bot = build_bot(token);

    // Resolve bot identity (username + user_id) via getMe — required for group mention detection.
    let me = bot
        .get_me()
        .await
        .map_err(|e| miette::miette!("bot.get_me() failed: {e:#}"))?;
    let username = me.user.username.clone().ok_or_else(|| {
        miette::miette!("bot has no username; cannot set up group-mention detection")
    })?;
    let identity = BotIdentity {
        username: username.clone(),
        user_id: me.user.id.0,
    };
    tracing::info!(%username, user_id = identity.user_id, "bot identity resolved");
    let identity_arc = Arc::new(identity);

    // Shared state
    let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> = Arc::new(DashMap::new());
    let agent_dir_arc: Arc<AgentDir> = Arc::new(AgentDir(agent_dir));
    let ssh_config_arc: Arc<SshConfigPath> = Arc::new(SshConfigPath(ssh_config_path));
    let pending_auth_arc: PendingAuthMap = pending_auth;
    let home_arc: Arc<RightHome> = Arc::new(RightHome(home));
    let auth_watcher_arc: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let auth_code_arc = Arc::new(tokio::sync::Mutex::new(None));
    let pending_token_arc = Arc::new(tokio::sync::Mutex::new(None));
    let intercept_slots_arc: Arc<InterceptSlots> = Arc::new(InterceptSlots {
        auth_code: Arc::clone(&auth_code_arc),
        pending_token: Arc::clone(&pending_token_arc),
        auth_watcher: Arc::clone(&auth_watcher_arc),
    });
    let pending_token_slot_arc: Arc<PendingTokenSlot> =
        Arc::new(PendingTokenSlot(pending_token_arc));
    let internal_api_arc: Arc<InternalApi> = Arc::new(InternalApi(internal_client));
    let settings_arc: Arc<AgentSettings> = Arc::new(AgentSettings {
        show_thinking,
        model,
        resolved_sandbox,
        hindsight: hindsight_wrapper,
        prefetch_cache,
        upgrade_lock,
        debug,
        stt,
    });
    let stop_tokens: super::StopTokens = Arc::new(DashMap::new());

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

    let mut dispatcher = build_dispatcher(
        bot.clone(),
        allowlist.clone(),
        Arc::clone(&identity_arc),
        Arc::clone(&worker_map),
        Arc::clone(&agent_dir_arc),
        pending_auth_arc,
        Arc::clone(&home_arc),
        Arc::clone(&ssh_config_arc),
        Arc::clone(&intercept_slots_arc),
        Arc::clone(&pending_token_slot_arc),
        Arc::clone(&internal_api_arc),
        Arc::clone(&settings_arc),
        Arc::clone(&stop_tokens),
        Arc::clone(&idle_ts),
        Arc::clone(&session_locks),
        Arc::clone(&bg_requests),
    );

    let shutdown_token = dispatcher.shutdown_token();

    // SIGTERM/SIGINT listener -- runs in a dedicated std thread because signal-hook's
    // SignalsInfo<WithOrigin> iterator is blocking. The thread reads siginfo_t origin
    // (PID + UID of the sender), looks up the sender's command line via `ps`, logs it,
    // then cancels `signal_cancel`. The tokio task below observes the cancellation and
    // drives the actual dispatcher shutdown on the runtime.
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

    // Shutdown driver task: converts any shutdown trigger (SIGTERM/SIGINT or
    // config change) into a single cancellation on `signal_cancel`, then
    // drives dispatcher shutdown. The wrapped listener observes the same
    // token (see `ShutdownAware` below) and ends its update stream so
    // `dispatch_with_listener` returns even when no inbound webhook arrives
    // to close the underlying mpsc channel. Worker tasks drain their mpsc
    // channels and exit; in-flight CC subprocesses are killed by
    // kill_on_drop(true) when workers are dropped.
    let signal_cancel_task = signal_cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = signal_cancel_task.cancelled() => {
                // Signal listener thread already logged the PID/cmd. Nothing else to log here.
            }
            _ = shutdown.cancelled() => {
                tracing::info!("config change detected -- initiating graceful shutdown");
                signal_cancel_task.cancel();
            }
        }

        match shutdown_token.shutdown() {
            Ok(fut) => {
                fut.await;
                tracing::info!("dispatcher stopped");
            }
            Err(_idle) => {
                tracing::debug!("dispatcher was idle at shutdown -- already stopped");
            }
        }
    });

    // Pre-delete any language-scoped command lists from prior deployments.
    // Telegram's resolution order is: scope+language wins over scope-only, so
    // stale language-scoped entries shadow our fresh per-scope set. Best-effort,
    // errors ignored (e.g. the slot was never populated).
    for scope in [
        teloxide::types::BotCommandScope::Default,
        teloxide::types::BotCommandScope::AllPrivateChats,
        teloxide::types::BotCommandScope::AllGroupChats,
    ] {
        for lang in ["en", "ru"] {
            let _ = bot
                .delete_my_commands()
                .scope(scope.clone())
                .language_code(lang.to_string())
                .await;
        }
    }

    // Register commands in three overlapping scopes so autocomplete works in both DMs and
    // groups. Setting only Default is not enough when another tool sharing this token has
    // previously written a narrower scope (e.g. AllPrivateChats) — that narrower scope wins
    // per Telegram's resolution order and shadows Default.
    let commands = BotCommand::bot_commands();
    for scope in [
        teloxide::types::BotCommandScope::Default,
        teloxide::types::BotCommandScope::AllPrivateChats,
        teloxide::types::BotCommandScope::AllGroupChats,
    ] {
        if let Err(e) = bot
            .set_my_commands(commands.clone())
            .scope(scope.clone())
            .await
        {
            tracing::warn!(?scope, "set_my_commands failed: {e:#}");
        }
    }
    // Clean any stale commands in the admins-only scope we do not populate.
    if let Err(e) = bot
        .delete_my_commands()
        .scope(teloxide::types::BotCommandScope::AllChatAdministrators)
        .await
    {
        tracing::warn!("delete_my_commands (all_chat_administrators): {e:#}");
    }

    // Wrap the listener so its update stream ends as soon as `signal_cancel`
    // is fired. Without this wrapper, teloxide 0.17's `axum_no_setup`
    // listener feeds updates from an `UnboundedReceiverStream` whose sender
    // is only closed by an incoming HTTP request observing a stop flag —
    // during shutdown no such request arrives, so the dispatcher would hang
    // in `stream.next()` until process-compose's `timeout_seconds` SIGKILL.
    let update_listener =
        super::shutdown_listener::ShutdownAware::new(update_listener, signal_cancel.clone());

    tracing::info!("teloxide dispatcher starting (webhook)");
    dispatcher
        .dispatch_with_listener(
            update_listener,
            teloxide::error_handlers::LoggingErrorHandler::new(),
        )
        .await;
    tracing::info!("dispatcher exited cleanly");
    Ok(())
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

/// Build the teloxide `Dispatcher` with the full handler schema and dependency map.
///
/// Extracted from `run_telegram` so that dptree dependency-injection type checking
/// (which runs inside `DispatcherBuilder::build()`) can be smoke-tested without
/// going through the full bot startup path. See `dispatcher_builds_without_panic`.
#[allow(clippy::too_many_arguments)]
fn build_dispatcher(
    bot: BotType,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    identity_arc: Arc<BotIdentity>,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir_arc: Arc<AgentDir>,
    pending_auth_arc: PendingAuthMap,
    home_arc: Arc<RightHome>,
    ssh_config_arc: Arc<SshConfigPath>,
    intercept_slots_arc: Arc<InterceptSlots>,
    pending_token_slot_arc: Arc<PendingTokenSlot>,
    internal_api_arc: Arc<InternalApi>,
    settings_arc: Arc<AgentSettings>,
    stop_tokens: super::StopTokens,
    idle_ts: Arc<IdleTimestamp>,
    session_locks: super::SessionLocks,
    bg_requests: super::BgRequests,
) -> teloxide::dispatching::Dispatcher<BotType, RequestError, DefaultKey> {
    let worker_ctl = super::WorkerControlDeps {
        stop_tokens,
        session_locks,
        bg_requests,
    };
    let filter = make_routing_filter(allowlist.clone(), (*identity_arc).clone());

    // Dispatch schema (RESEARCH.md Pattern 1)
    let command_handler = dptree::entry()
        .filter_command::<BotCommand>()
        .branch(dptree::case![BotCommand::Start].endpoint(handle_start))
        .branch(dptree::case![BotCommand::New(name)].endpoint(handle_new))
        .branch(dptree::case![BotCommand::List].endpoint(handle_list))
        .branch(dptree::case![BotCommand::Switch(uuid)].endpoint(handle_switch))
        .branch(dptree::case![BotCommand::Mcp(args)].endpoint(handle_mcp))
        .branch(dptree::case![BotCommand::Doctor].endpoint(handle_doctor))
        .branch(dptree::case![BotCommand::Cron(args)].endpoint(handle_cron))
        .branch(dptree::case![BotCommand::Usage(arg)].endpoint(handle_usage))
        .branch(
            dptree::case![BotCommand::Allow(args)]
                .endpoint(super::allowlist_commands::handle_allow),
        )
        .branch(
            dptree::case![BotCommand::Deny(args)].endpoint(super::allowlist_commands::handle_deny),
        )
        .branch(
            dptree::case![BotCommand::Allowed].endpoint(super::allowlist_commands::handle_allowed),
        )
        .branch(
            dptree::case![BotCommand::AllowAll]
                .endpoint(super::allowlist_commands::handle_allow_all),
        )
        .branch(
            dptree::case![BotCommand::DenyAll].endpoint(super::allowlist_commands::handle_deny_all),
        );

    let message_handler = Update::filter_message()
        .inspect(|msg: Message| {
            let text_preview = msg.text().or(msg.caption()).map(|t| {
                let trimmed: String = t.chars().take(80).collect();
                trimmed
            });
            let entities = msg.entities().map(|e| e.len()).unwrap_or(0);
            tracing::info!(
                chat_id = msg.chat.id.0,
                ?text_preview,
                entities,
                "message update received by dispatcher"
            );
        })
        .filter_map(filter)
        .inspect(|msg: Message| {
            // Log after allow-list filter, before command parsing.
            // If this appears but no command/handle_message log follows,
            // the message was swallowed by filter_command (e.g. /command with
            // formatting entities that prevent parsing).
            let starts_with_slash = msg.text().is_some_and(|t| t.starts_with('/'));
            if starts_with_slash {
                tracing::info!(
                    chat_id = msg.chat.id.0,
                    text = msg.text().unwrap_or(""),
                    "pre-command: message starts with /, attempting command parse"
                );
            }
        })
        .branch(command_handler)
        .endpoint(handle_message);

    let callback_handler = Update::filter_callback_query()
        .branch(
            dptree::filter(|q: CallbackQuery| {
                q.data.as_deref().is_some_and(|d| d.starts_with("bg:"))
            })
            .endpoint(handle_bg_callback),
        )
        .endpoint(handle_stop_callback);

    let schema = dptree::entry()
        .branch(message_handler)
        .branch(callback_handler);

    Dispatcher::builder(bot, schema)
        .dependencies(dptree::deps![
            worker_map,
            agent_dir_arc,
            pending_auth_arc,
            home_arc,
            ssh_config_arc,
            intercept_slots_arc,
            pending_token_slot_arc,
            internal_api_arc,
            settings_arc,
            idle_ts,
            identity_arc,
            allowlist,
            worker_ctl
        ])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicI64};

    use right_agent::agent::allowlist::{AllowlistHandle, AllowlistState};
    use right_agent::mcp::internal_client::InternalClient;
    use right_agent::memory::prefetch::PrefetchCache;
    use tokio::sync::{Mutex, RwLock};

    /// Smoke test: construct the real dispatcher with dummy deps. If a handler
    /// in the tree declares a DI parameter type that is not supplied by either
    /// `.dependencies(...)` or an upstream combinator (filter_map etc.), dptree's
    /// runtime `type_check` panics inside `DispatcherBuilder::build()`. We exercise
    /// exactly that path so the regression is caught at `cargo test` time.
    ///
    /// History: commit 34b7a84 wired `make_routing_filter` returning
    /// `Option<(Message, RoutingDecision)>` into `filter_map`. dptree 0.5.1 does
    /// not unpack tuples — the bag received `(Message, RoutingDecision)` as a
    /// single type, leaving `handle_message`'s `decision: RoutingDecision`
    /// parameter unsatisfied and aborting every bot on startup.
    #[tokio::test]
    async fn dispatcher_builds_without_panic() {
        let bot = build_bot("0:fake_token_for_smoke_test".to_string());

        let allowlist =
            AllowlistHandle(Arc::new(std::sync::RwLock::new(AllowlistState::default())));
        let identity = Arc::new(BotIdentity {
            username: "smoke_bot".to_string(),
            user_id: 1,
        });
        let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> =
            Arc::new(DashMap::new());
        let agent_dir = Arc::new(AgentDir(PathBuf::from("/tmp/smoke")));
        let pending_auth: PendingAuthMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let home = Arc::new(RightHome(PathBuf::from("/tmp/smoke")));
        let ssh_config = Arc::new(SshConfigPath(None));
        let intercept_slots = Arc::new(InterceptSlots {
            auth_code: Arc::new(Mutex::new(None)),
            pending_token: Arc::new(Mutex::new(None)),
            auth_watcher: Arc::new(AtomicBool::new(false)),
        });
        let pending_token_slot = Arc::new(PendingTokenSlot(Arc::new(Mutex::new(None))));
        let internal_api = Arc::new(InternalApi(Arc::new(InternalClient::new(
            "/tmp/smoke.sock",
        ))));
        let settings = Arc::new(AgentSettings {
            show_thinking: false,
            model: None,
            resolved_sandbox: None,
            hindsight: None,
            prefetch_cache: Some(PrefetchCache::new()),
            upgrade_lock: Arc::new(RwLock::new(())),
            debug: false,
            stt: None,
        });
        let stop_tokens: super::super::StopTokens = Arc::new(DashMap::new());
        let session_locks: super::super::SessionLocks = Arc::new(DashMap::new());
        let bg_requests: super::super::BgRequests = Arc::new(DashMap::new());
        let idle_ts = Arc::new(IdleTimestamp(Arc::new(AtomicI64::new(0))));

        // The call under test. If dptree type_check fails, this aborts the
        // test process.
        let _dispatcher = build_dispatcher(
            bot,
            allowlist,
            identity,
            worker_map,
            agent_dir,
            pending_auth,
            home,
            ssh_config,
            intercept_slots,
            pending_token_slot,
            internal_api,
            settings,
            stop_tokens,
            idle_ts,
            session_locks,
            bg_requests,
        );
    }
}
