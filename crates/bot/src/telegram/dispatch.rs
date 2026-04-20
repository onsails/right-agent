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
    RightclawHome, SshConfigPath, handle_cron, handle_doctor, handle_list, handle_mcp,
    handle_message, handle_new, handle_start, handle_stop_callback, handle_switch, handle_usage,
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
pub async fn run_telegram(
    token: String,
    allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    show_thinking: bool,
    model: Option<String>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    hindsight_wrapper: Option<std::sync::Arc<rightclaw::memory::ResilientHindsight>>,
    prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) -> miette::Result<()> {
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
    let home_arc: Arc<RightclawHome> = Arc::new(RightclawHome(home));
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
    );

    let shutdown_token = dispatcher.shutdown_token();

    // Signal handler task
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received -- initiating graceful shutdown");
            }
            result = tokio::signal::ctrl_c() => {
                if result.is_ok() {
                    tracing::info!("SIGINT received -- initiating graceful shutdown");
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!("config change detected -- initiating graceful shutdown");
            }
        }

        // Shutdown dispatcher -- worker tasks drain their mpsc channels and exit.
        // In-flight CC subprocesses are killed by kill_on_drop(true) when workers are dropped.
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

    tracing::info!("teloxide dispatcher starting (long-polling)");
    dispatcher.dispatch().await;
    tracing::info!("dispatcher exited cleanly");
    Ok(())
}

/// Build the teloxide `Dispatcher` with the full handler schema and dependency map.
///
/// Extracted from `run_telegram` so that dptree dependency-injection type checking
/// (which runs inside `DispatcherBuilder::build()`) can be smoke-tested without
/// going through the full bot startup path. See `dispatcher_builds_without_panic`.
#[allow(clippy::too_many_arguments)]
fn build_dispatcher(
    bot: BotType,
    allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    identity_arc: Arc<BotIdentity>,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir_arc: Arc<AgentDir>,
    pending_auth_arc: PendingAuthMap,
    home_arc: Arc<RightclawHome>,
    ssh_config_arc: Arc<SshConfigPath>,
    intercept_slots_arc: Arc<InterceptSlots>,
    pending_token_slot_arc: Arc<PendingTokenSlot>,
    internal_api_arc: Arc<InternalApi>,
    settings_arc: Arc<AgentSettings>,
    stop_tokens: super::StopTokens,
    idle_ts: Arc<IdleTimestamp>,
) -> teloxide::dispatching::Dispatcher<BotType, RequestError, DefaultKey> {
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

    let callback_handler = Update::filter_callback_query().endpoint(handle_stop_callback);

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
            stop_tokens,
            idle_ts,
            identity_arc,
            allowlist
        ])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicI64};

    use rightclaw::agent::allowlist::{AllowlistHandle, AllowlistState};
    use rightclaw::mcp::internal_client::InternalClient;
    use rightclaw::memory::prefetch::PrefetchCache;
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
        let home = Arc::new(RightclawHome(PathBuf::from("/tmp/smoke")));
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
        });
        let stop_tokens: super::super::StopTokens = Arc::new(DashMap::new());
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
        );
    }
}
