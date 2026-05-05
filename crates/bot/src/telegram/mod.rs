pub mod allowlist_commands;
pub mod attachments;
pub(crate) mod bootstrap_photo;
pub mod bot;
pub mod dispatch;
pub mod filter;
pub mod handler;
pub(crate) mod invocation;
pub mod markdown;
pub mod memory_alerts;
pub mod mention;
pub mod oauth_callback;
pub(crate) mod prompt;
pub mod session;
pub mod shutdown_listener;
pub mod stream;
pub mod webhook;
pub mod worker;

pub use dispatch::run_telegram;
pub use session::effective_thread_id;

/// Bot adaptor type alias used by WorkerContext and dispatch logic.
/// Ordering: CacheMe<Throttle<Bot>> per BOT-03 (Throttle inner, CacheMe outer).
pub type BotType =
    teloxide::adaptors::CacheMe<teloxide::adaptors::throttle::Throttle<teloxide::Bot>>;

/// Best-effort broadcast to a list of chat IDs. Errors are logged and swallowed
/// (alerts and OAuth notifications shouldn't fail hard if one chat is unreachable).
pub(crate) async fn broadcast_to_chats<R>(bot: &R, chat_ids: &[i64], text: &str)
where
    R: teloxide::prelude::Requester + Send + Sync,
    R::Err: std::fmt::Display,
{
    for &chat_id in chat_ids {
        if let Err(e) = bot
            .send_message(teloxide::types::ChatId(chat_id), text)
            .await
        {
            tracing::warn!(chat_id, "broadcast_to_chats send failed: {e}");
        }
    }
}

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

/// Process-local monotonic turn id. Allocated by `next_turn_id()` at the start
/// of every `invoke_cc` call so the worker can match concurrent bg-callback
/// inserts to the *current* turn (not a previous one).
static NEXT_TURN_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh per-turn id. Monotonic across the bot process.
pub(crate) fn next_turn_id() -> u64 {
    NEXT_TURN_ID.fetch_add(1, Ordering::Relaxed)
}

/// Shared map of active CC sessions that can be stopped via inline button.
/// Key: (chat_id, eff_thread_id). Value: (turn_id, CancellationToken).
///
/// `turn_id` stamps each invocation so concurrent callbacks (Background, Stop)
/// can be tied to the *current* turn instead of a stale one — see
/// `BgRequests` for the matching half of the protocol.
pub(crate) type StopTokens = Arc<DashMap<(i64, i64), (u64, CancellationToken)>>;

/// Per-main-session async mutex map. Worker acquires before `claude -p --resume <main>`;
/// delivery acquires before its own `--resume`. Closes the TOCTOU race on session JSONL.
/// Key: root_session_id UUID string. Value: shared mutex.
pub(crate) type SessionLocks = Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>;

/// Per-(chat, thread) flag set by the Background button callback.
/// Presence in the map means the user requested backgrounding (not a Stop).
/// Value: turn_id of the turn the bg request was issued against. Worker
/// only honors entries whose stored turn_id matches its own current turn —
/// stale entries from a previous turn are dropped on exit.
pub(crate) type BgRequests = Arc<DashMap<(i64, i64), u64>>;

/// Bundle of per-session control maps that flow into `WorkerContext` when
/// `handle_message` spawns a per-session worker. Bundled because dptree
/// 0.5.1's `Injectable` impl tops out at 12 type params, and the message
/// handler was already at the limit — we cannot inject these as separate
/// top-level deps without pushing the message handler over.
///
/// All three maps share lifetime and are constructed once in `lib.rs`:
/// - `stop_tokens`: per-(chat, thread) cancellation tokens for in-flight CC subprocesses.
/// - `session_locks`: per-main-session async mutex map (TOCTOU on session JSONL).
/// - `bg_requests`: per-(chat, thread) Background-button request flags.
#[derive(Clone)]
pub struct WorkerControlDeps {
    pub(crate) stop_tokens: StopTokens,
    pub(crate) session_locks: SessionLocks,
    pub(crate) bg_requests: BgRequests,
}

use right_agent::agent::types::AgentConfig;

/// Resolve Telegram token from agent.yaml config.
///
/// Returns Err if `telegram_token` is absent or empty.
pub fn resolve_token(config: &AgentConfig) -> miette::Result<String> {
    if let Some(token) = &config.telegram_token
        && !token.is_empty()
    {
        return Ok(token.clone());
    }
    Err(miette::miette!(
        help = "Add telegram_token to agent.yaml",
        "No Telegram token found for this agent"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_agent::agent::types::AgentConfig;
    use std::collections::HashMap;

    fn minimal_config() -> AgentConfig {
        AgentConfig {
            restart: Default::default(),
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: HashMap::new(),
            secret: None,
            attachments: Default::default(),
            network_policy: Default::default(),
            show_thinking: true,
            memory: None,
            stt: Default::default(),
        }
    }

    #[test]
    fn resolve_token_from_config() {
        let mut config = minimal_config();
        config.telegram_token = Some("999:inline_token".to_string());
        let token = resolve_token(&config).unwrap();
        assert_eq!(token, "999:inline_token");
    }

    #[test]
    fn resolve_token_returns_err_when_nothing_configured() {
        let config = minimal_config();
        assert!(resolve_token(&config).is_err());
    }

    #[test]
    fn resolve_token_returns_err_when_empty_string() {
        let mut config = minimal_config();
        config.telegram_token = Some(String::new());
        assert!(resolve_token(&config).is_err());
    }
}
