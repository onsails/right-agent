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
use teloxide::dispatching::UpdateFilterExt;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::bot::build_bot;
use super::filter::make_routing_filter;
use super::mention::BotIdentity;
use super::handler::{handle_cron, handle_doctor, handle_list, handle_mcp, handle_message, handle_new, handle_start, handle_stop_callback, handle_switch, AgentDir, AgentSettings, DebugFlag, IdleTimestamp, InterceptSlots, InternalApi, PendingTokenSlot, RightclawHome, SshConfigPath};
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
    hindsight_client: Option<std::sync::Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) -> miette::Result<()> {
    let bot = build_bot(token);

    // Resolve bot identity (username + user_id) via getMe — required for group mention detection.
    let me = bot.get_me().await
        .map_err(|e| miette::miette!("bot.get_me() failed: {e:#}"))?;
    let username = me.user.username.clone()
        .ok_or_else(|| miette::miette!("bot has no username; cannot set up group-mention detection"))?;
    let identity = BotIdentity { username: username.clone(), user_id: me.user.id.0 };
    tracing::info!(%username, user_id = identity.user_id, "bot identity resolved");
    let filter = make_routing_filter(allowlist.clone(), identity.clone());
    let identity_arc = Arc::new(identity);

    // Shared state
    let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> =
        Arc::new(DashMap::new());
    let agent_dir_arc: Arc<AgentDir> = Arc::new(AgentDir(agent_dir));
    let debug_arc: Arc<DebugFlag> = Arc::new(DebugFlag(debug));
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
    let pending_token_slot_arc: Arc<PendingTokenSlot> = Arc::new(PendingTokenSlot(
        pending_token_arc,
    ));
    let internal_api_arc: Arc<InternalApi> = Arc::new(InternalApi(internal_client));
    let settings_arc: Arc<AgentSettings> = Arc::new(AgentSettings {
        show_thinking,
        model,
        resolved_sandbox,
        hindsight: hindsight_client,
        prefetch_cache,
        upgrade_lock,
    });
    let stop_tokens: super::StopTokens = Arc::new(DashMap::new());

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
        .branch(
            dptree::case![BotCommand::Allow(args)]
                .endpoint(super::allowlist_commands::handle_allow),
        )
        .branch(
            dptree::case![BotCommand::Deny(args)]
                .endpoint(super::allowlist_commands::handle_deny),
        )
        .branch(
            dptree::case![BotCommand::Allowed].endpoint(super::allowlist_commands::handle_allowed),
        )
        .branch(
            dptree::case![BotCommand::AllowAll]
                .endpoint(super::allowlist_commands::handle_allow_all),
        )
        .branch(
            dptree::case![BotCommand::DenyAll]
                .endpoint(super::allowlist_commands::handle_deny_all),
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
        .endpoint(handle_stop_callback);

    let schema = dptree::entry()
        .branch(message_handler)
        .branch(callback_handler);

    let mut dispatcher = Dispatcher::builder(bot.clone(), schema)
        .dependencies(dptree::deps![
            Arc::clone(&worker_map),
            Arc::clone(&agent_dir_arc),
            Arc::clone(&debug_arc),
            pending_auth_arc,
            Arc::clone(&home_arc),
            Arc::clone(&ssh_config_arc),
            Arc::clone(&intercept_slots_arc),
            Arc::clone(&pending_token_slot_arc),
            Arc::clone(&internal_api_arc),
            Arc::clone(&settings_arc),
            Arc::clone(&stop_tokens),
            Arc::clone(&idle_ts),
            Arc::clone(&identity_arc),
            allowlist.clone()
        ])
        .build();

    let shutdown_token = dispatcher.shutdown_token();

    // Signal handler task
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
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

    // Register commands at default (global) scope. Per-chat scope is no longer
    // required now that routing is gated by allowlist.yaml instead of a static
    // allow-list of chat IDs.
    let commands = BotCommand::bot_commands();
    if let Err(e) = bot.delete_my_commands().await {
        tracing::warn!("delete_my_commands (default scope): {e:#}");
    }
    if let Err(e) = bot.set_my_commands(commands).await {
        tracing::warn!("set_my_commands (default scope): {e:#}");
    }

    tracing::info!("teloxide dispatcher starting (long-polling)");
    dispatcher.dispatch().await;
    tracing::info!("dispatcher exited cleanly");
    Ok(())
}
