//! Teloxide endpoint handlers: message dispatch + /reset command.
//!
//! handle_message: routes incoming text to the per-session worker via DashMap.
//! handle_reset: deletes the session row for the current thread.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;
use teloxide::prelude::*;
use teloxide::types::Message;
use teloxide::RequestError;

use super::session::{delete_session, effective_thread_id};
use super::worker::{DebounceMsg, SessionKey, WorkerContext, spawn_worker};
use super::BotType;

/// Newtype wrapper for the debug flag passed via dptree dependencies.
#[derive(Clone)]
pub struct DebugFlag(pub bool);

/// Convert an arbitrary error into `RequestError::Io` so it propagates through `ResponseResult`.
fn to_request_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::Io(std::io::Error::other(e.to_string()).into())
}

/// Handle an incoming text message.
///
/// 1. Compute effective_thread_id (normalise General topic).
/// 2. Look up existing sender in DashMap or spawn a new worker task.
/// 3. Send the message into the worker's mpsc channel.
///
/// Serialisation guarantee (SES-05): all messages to the same (chat_id, thread_id)
/// go through the same mpsc channel → worker processes them serially.
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<PathBuf>,
    debug_flag: Arc<DebugFlag>,
) -> ResponseResult<()> {
    // Only process messages with text (ignore stickers, photos, etc. in Phase 25)
    let text = match msg.text() {
        Some(t) => t.to_string(),
        None => return Ok(()),
    };

    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);
    let worker_exists = worker_map.contains_key(&key);
    tracing::info!(?key, worker_exists, "handle_message: routing");

    let debounce_msg = DebounceMsg {
        message_id: msg.id.0,
        text,
        timestamp: chrono::Utc::now(),
    };

    // Check for existing worker or spawn a new one.
    // Pitfall 7 mitigation: if send fails, the worker task has exited — remove + respawn.
    // Note: DashMap read guard is NOT held across .await to avoid blocking. Clone the
    // sender before awaiting.
    loop {
        let maybe_tx = worker_map.get(&key).map(|entry| entry.value().clone());
        match maybe_tx {
            Some(tx) => match tx.send(debounce_msg.clone()).await {
                Ok(_) => break,
                Err(e) => {
                    // Worker task panicked or exited — remove stale sender and respawn
                    tracing::warn!(?key, "worker send failed, respawning: {:#}", e);
                    worker_map.remove(&key);
                    // fall through to spawn new worker below on next loop iteration
                }
            },
            None => {
                // No sender yet — spawn a new worker task
                let agent_name = agent_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let ctx = WorkerContext {
                    chat_id,
                    effective_thread_id: eff_thread_id,
                    agent_dir: (*agent_dir).clone(),
                    agent_name,
                    bot: bot.clone(),
                    db_path: (*agent_dir).clone(),
                    debug: debug_flag.0,
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
/// alongside the bot — no explicit bootstrap needed.
pub async fn handle_start(
    bot: BotType,
    msg: Message,
) -> ResponseResult<()> {
    bot.send_message(msg.chat.id, "👋 Agent is running. Send a message to start.").await?;
    Ok(())
}

/// Handle the /reset command.
///
/// Deletes the telegram_sessions row for the current (chat_id, effective_thread_id).
/// Also removes the worker sender from DashMap so the worker task exits cleanly.
/// Next message will create a fresh session with a new UUID (SES-06).
///
/// Both DB errors propagate — a failed reset is surfaced to the caller so the dispatcher
/// can log it and teloxide can handle the update appropriately.
pub async fn handle_reset(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<PathBuf>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
    let key: SessionKey = (chat_id.0, eff_thread_id);

    // Remove the worker sender — channel closes, worker task exits and removes its own entry
    worker_map.remove(&key);

    // Delete session from DB — errors propagate via `?` (CLAUDE.rust.md: fail fast)
    let conn = rightclaw::memory::open_connection(&agent_dir)
        .map_err(|e| to_request_err(format!("reset: open DB: {:#}", e)))?;
    delete_session(&conn, chat_id.0, eff_thread_id)
        .map_err(|e| to_request_err(format!("reset: delete session: {:#}", e)))?;

    tracing::info!(?key, "session reset");

    // Send confirmation reply
    let mut send =
        bot.send_message(chat_id, "Session reset. Next message starts a fresh conversation.");
    if eff_thread_id != 0 {
        send = send.message_thread_id(teloxide::types::ThreadId(
            teloxide::types::MessageId(eff_thread_id as i32),
        ));
    }
    send.await?;

    Ok(())
}
