use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use teloxide::prelude::*;
use teloxide::dispatching::UpdateFilterExt;

use super::{bot::build_bot, filter::make_chat_id_filter};

/// Run the teloxide long-polling dispatcher with:
/// - CacheMe<Throttle<Bot>> adaptor ordering (BOT-03)
/// - chat_id allow-list filtering (BOT-05)
/// - SIGTERM + SIGINT graceful shutdown (BOT-04)
/// - Arc<Mutex<Vec<tokio::process::Child>>> for in-flight subprocess tracking (D-07)
///   (Vec is empty in Phase 23; Phase 25 populates it)
///
/// Does NOT call enable_ctrlc_handler() — Phase 23 owns signal handling (anti-pattern #2).
pub async fn run_telegram(token: String, allowed_chat_ids: Vec<i64>) -> miette::Result<()> {
    let bot = build_bot(token);

    let allowed: HashSet<i64> = allowed_chat_ids.into_iter().collect();
    let filter = make_chat_id_filter(allowed);

    // No-op message schema for Phase 23.
    // Phase 25 replaces the endpoint with real dispatch logic.
    let schema = Update::filter_message()
        .filter_map(filter)
        .endpoint(|_msg: Message| async { respond(()) });

    let mut dispatcher = Dispatcher::builder(bot, schema).build();
    let shutdown_token = dispatcher.shutdown_token();

    // Shared subprocess tracking (empty in Phase 23, populated in Phase 25).
    // Arc<Mutex<Vec<Child>>> defined here so Phase 25 can extend without restructuring.
    let children: Arc<Mutex<Vec<tokio::process::Child>>> = Arc::new(Mutex::new(Vec::new()));

    // Signal handler task: handles SIGTERM and SIGINT.
    // D-08: both signals trigger the same shutdown path.
    // D-09: sequence: kill children → shutdown dispatcher → process exits.
    let children_clone = Arc::clone(&children);
    tokio::spawn(async move {
        // Register SIGTERM (process-compose sends this on `rightclaw down`)
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received — initiating graceful shutdown");
            }
            result = tokio::signal::ctrl_c() => {
                if result.is_ok() {
                    tracing::info!("SIGINT received — initiating graceful shutdown");
                }
            }
        }

        // Kill all in-flight claude -p subprocesses (Phase 23: vec is empty).
        {
            let mut locked = children_clone.lock().await;
            for child in locked.iter_mut() {
                if let Err(e) = child.kill().await {
                    tracing::error!("failed to kill subprocess: {:#}", e);
                }
            }
        }
        tracing::info!("in-flight subprocesses terminated");

        // Shut down teloxide dispatcher.
        // IdleShutdownError means dispatcher not yet started — treat as already stopped.
        match shutdown_token.shutdown() {
            Ok(fut) => {
                fut.await;
                tracing::info!("dispatcher stopped");
            }
            Err(_idle) => {
                tracing::debug!("dispatcher was idle at shutdown — already stopped");
            }
        }
    });

    tracing::info!("teloxide dispatcher starting (long-polling)");
    dispatcher.dispatch().await;
    tracing::info!("dispatcher exited cleanly");
    Ok(())
}
