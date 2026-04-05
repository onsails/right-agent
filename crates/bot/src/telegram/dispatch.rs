//! Teloxide long-polling dispatcher with:
//! - DashMap per-session worker map (SES-05, D-11)
//! - BotCommand schema for /reset (SES-06)
//! - ChatId allow-list filter (BOT-05, via filter.rs)
//! - SIGTERM + SIGINT graceful shutdown (BOT-04)
//! - BOT-04 subprocess cleanup via kill_on_drop(true) in each worker (no children registry)
//!
//! GOTCHA: queued messages in a worker channel are lost on worker task panic.
//! When the worker is respawned (Pitfall 7), the in-progress batch is discarded.
//! This is an accepted trade-off -- retrying arbitrary messages is not safe.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use teloxide::dispatching::UpdateFilterExt;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use tokio::sync::mpsc;

use super::bot::build_bot;
use super::filter::make_chat_id_filter;
use super::handler::{handle_doctor, handle_mcp, handle_message, handle_reset, handle_start, AgentDir, DebugFlag, RightclawHome};
use super::worker::{DebounceMsg, SessionKey};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    #[command(description = "Start interacting with this agent")]
    Start,
    #[command(description = "Reset conversation session for this thread")]
    Reset,
    #[command(description = "MCP server management (list/add/remove)")]
    Mcp(String),
    #[command(description = "Run diagnostics")]
    Doctor,
}

/// Run the teloxide long-polling dispatcher.
///
/// - Accepts agent_dir for session DB access and CC subprocess invocation.
/// - Creates a DashMap<SessionKey, Sender<DebounceMsg>> for per-session workers.
/// - Schema: filter by chat_id -> branch /reset command -> dispatch text messages.
/// - SIGTERM/SIGINT: kill in-flight subprocesses, shutdown dispatcher.
///
/// BOT-04 subprocess cleanup strategy: use kill_on_drop(true) on each Child in invoke_cc.
/// When a worker task exits (channel closed, panic, or /reset), the Child is dropped, which
/// kills the subprocess. No explicit children registry is needed or maintained.
/// Rationale: Arc<Mutex<Vec<Child>>> was rejected because invoke_cc never added children
/// to the registry, making the kill loop dead code. kill_on_drop is sufficient.
pub async fn run_telegram(
    token: String,
    allowed_chat_ids: Vec<i64>,
    agent_dir: PathBuf,
    debug: bool,
    home: PathBuf,
) -> miette::Result<()> {
    let bot = build_bot(token);

    let allowed: HashSet<i64> = allowed_chat_ids.into_iter().collect();
    tracing::info!(allowed_chat_ids = ?allowed, "chat ID allow-list loaded");
    let filter = make_chat_id_filter(allowed);

    // Shared state
    let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> =
        Arc::new(DashMap::new());
    let agent_dir_arc: Arc<AgentDir> = Arc::new(AgentDir(agent_dir));
    let debug_arc: Arc<DebugFlag> = Arc::new(DebugFlag(debug));
    let home_arc: Arc<RightclawHome> = Arc::new(RightclawHome(home));

    // Dispatch schema (RESEARCH.md Pattern 1)
    let command_handler = dptree::entry()
        .filter_command::<BotCommand>()
        .branch(
            dptree::case![BotCommand::Start].endpoint(handle_start),
        )
        .branch(
            dptree::case![BotCommand::Reset].endpoint(handle_reset),
        )
        .branch(
            dptree::case![BotCommand::Mcp(args)].endpoint(handle_mcp),
        )
        .branch(
            dptree::case![BotCommand::Doctor].endpoint(handle_doctor),
        );

    let message_handler = Update::filter_message()
        .inspect(|msg: Message| {
            tracing::info!(chat_id = msg.chat.id.0, "message update received by dispatcher");
        })
        .filter_map(filter)
        .branch(command_handler)
        .endpoint(handle_message);

    let schema = dptree::entry().branch(message_handler);

    let mut dispatcher = Dispatcher::builder(bot.clone(), schema)
        .dependencies(dptree::deps![
            Arc::clone(&worker_map),
            Arc::clone(&agent_dir_arc),
            Arc::clone(&debug_arc),
            Arc::clone(&home_arc)
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

    // Register /reset command with Telegram Bot API.
    // First delete all existing commands to clear any leftover commands from other clients
    // (e.g., CC Telegram plugin sets /status /help /start -- these must be cleared).
    match bot.delete_my_commands().await {
        Ok(_) => tracing::info!("delete_my_commands succeeded"),
        Err(e) => tracing::warn!("delete_my_commands failed (non-fatal): {e:#}"),
    }
    match bot.set_my_commands(BotCommand::bot_commands()).await {
        Ok(_) => tracing::info!("set_my_commands succeeded -- commands registered"),
        Err(e) => tracing::warn!("set_my_commands failed (non-fatal): {e:#}"),
    }

    tracing::info!("teloxide dispatcher starting (long-polling)");
    dispatcher.dispatch().await;
    tracing::info!("dispatcher exited cleanly");
    Ok(())
}
