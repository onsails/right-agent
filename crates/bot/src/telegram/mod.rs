pub(crate) mod alerts;
pub mod allowlist_commands;
pub(crate) mod archive;
pub mod attachments;
pub(crate) mod bootstrap_photo;
pub mod bot;
pub(crate) mod command;
pub(crate) mod dashboard;
pub(crate) mod debug_command;
pub mod dispatch;
pub(crate) mod error_details;
pub mod filter;
pub(crate) mod focus_deeplink;
pub mod handler;
pub(crate) mod idle;
pub mod markdown;
pub mod memory_alerts;
pub mod mention;
pub(crate) mod mode_command;
pub(crate) mod model_command;
pub mod oauth_callback;
pub(crate) mod oauth_status;
pub(crate) mod progress;
pub(crate) mod reply_context;
pub mod session;
pub mod shutdown_listener;
pub(crate) mod tg_bot;
pub mod webhook;
pub mod worker;

pub(crate) use dispatch::run_telegram;
pub use session::effective_thread_id;

/// Bot adaptor type alias used by WorkerContext and dispatch logic.
/// Ordering: CacheMe<Throttle<Bot>> per BOT-03 (Throttle inner, CacheMe outer).
pub type BotType =
    teloxide::adaptors::CacheMe<teloxide::adaptors::throttle::Throttle<teloxide::Bot>>;

/// Best-effort broadcast to a list of chat IDs. Errors are logged and swallowed
/// (alerts shouldn't fail hard if one chat is unreachable).
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

/// Process-local monotonic turn id. Allocated at the start of every `invoke_cc`
/// call so the worker can match concurrent bg-callback inserts to the *current*
/// turn (not a previous one).
static NEXT_TURN_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh per-turn id above any already-persisted session turn.
pub(crate) fn next_turn_id_after(stored_max_turn_id: Option<u64>) -> u64 {
    let floor = stored_max_turn_id.unwrap_or(0).saturating_add(1);
    loop {
        let current = NEXT_TURN_ID.load(Ordering::Relaxed);
        let allocated = current.max(floor);
        let next = allocated.saturating_add(1);
        if NEXT_TURN_ID
            .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return allocated;
        }
    }
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

/// Per-(chat_id, thread_id) idle-compaction debounce timers. Cancelling the
/// token aborts a *pending* (still-sleeping) compaction; once the 2h sleep
/// wins, the compaction runs to completion regardless. In-memory only —
/// lost on restart, re-armed on the next turn.
pub(crate) type CompactTimers = Arc<DashMap<(i64, i64), tokio_util::sync::CancellationToken>>;

/// A foreground turn request to convert the active Claude invocation into a
/// background continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BgRequest {
    pub(crate) turn_id: u64,
    pub(crate) reason: worker::BgReason,
}

/// Per-(chat, thread) flag set by the Background button callback or shutdown.
/// Presence in the map means the foreground turn should be backgrounded (not a Stop).
/// Worker only honors entries whose stored turn_id matches its own current
/// turn — stale entries from a previous turn are dropped on exit.
pub(crate) type BgRequests = Arc<DashMap<(i64, i64), BgRequest>>;

/// Per-(chat, thread) handoff gate set before a foreground turn is moved to
/// background. Workers wait while a gate is present so the next foreground turn
/// cannot mutate the main session before the background fork is confirmed.
pub(crate) type BgHandoffGates = Arc<DashMap<(i64, i64), Arc<tokio::sync::Notify>>>;

pub(crate) fn set_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    gates
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Notify::new()));
}

pub(crate) fn release_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    if let Some((_, notify)) = gates.remove(&key) {
        notify.notify_waiters();
    }
}

pub(crate) async fn wait_for_bg_handoff_gate(gates: &BgHandoffGates, key: (i64, i64)) {
    loop {
        let Some(notify) = gates.get(&key).map(|entry| Arc::clone(entry.value())) else {
            return;
        };
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if gates.get(&key).is_none() {
            return;
        }
        notified.await;
    }
}

pub(crate) fn request_shutdown_backgrounding(
    stop_tokens: &StopTokens,
    bg_requests: &BgRequests,
    gates: &BgHandoffGates,
) -> usize {
    let mut requested = 0usize;
    for entry in stop_tokens.iter() {
        let key = *entry.key();
        let (turn_id, token) = entry.value();
        set_bg_handoff_gate(gates, key);
        bg_requests.insert(
            key,
            BgRequest {
                turn_id: *turn_id,
                reason: worker::BgReason::Shutdown,
            },
        );
        token.cancel();
        requested += 1;
    }
    requested
}

pub(crate) async fn wait_for_handoff_gates_empty(
    gates: &BgHandoffGates,
    timeout: std::time::Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if gates.is_empty() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// User-requested thinking visibility action from an inline callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingToggleAction {
    Show,
    Hide,
}

impl ThinkingToggleAction {
    pub(crate) fn expanded(self) -> bool {
        matches!(self, Self::Show)
    }
}

/// Per-(chat, thread) thinking-preview visibility for active CC sessions.
///
/// Key: (chat_id, eff_thread_id). Value: `true` when the preview is expanded.
/// The worker inserts at run start and removes on run completion.
pub(crate) type ThinkingVisibility = Arc<DashMap<(i64, i64), bool>>;

/// Initial thinking visibility for a run. Direct chats honor config; groups stay quiet.
pub(crate) fn initial_thinking_visibility(show_thinking: bool, is_group: bool) -> bool {
    show_thinking && !is_group
}

/// Parse `think:{chat_id}:{eff_thread_id}:{show|hide}` callback data.
pub(crate) fn parse_thinking_toggle_callback(
    data: &str,
) -> Option<((i64, i64), ThinkingToggleAction)> {
    let mut parts = data.splitn(4, ':');
    if parts.next()? != "think" {
        return None;
    }
    let chat_id = parts.next()?.parse::<i64>().ok()?;
    let thread_id = parts.next()?.parse::<i64>().ok()?;
    let action = match parts.next()? {
        "show" => ThinkingToggleAction::Show,
        "hide" => ThinkingToggleAction::Hide,
        _ => return None,
    };
    Some(((chat_id, thread_id), action))
}

/// Update active visibility. Returns false when the run already finished.
pub(crate) fn set_thinking_visibility(
    map: &ThinkingVisibility,
    key: (i64, i64),
    expanded: bool,
) -> bool {
    let Some(mut entry) = map.get_mut(&key) else {
        return false;
    };
    *entry.value_mut() = expanded;
    true
}

/// Bundle of per-session control maps that flow into `WorkerContext` when
/// `handle_message` spawns a per-session worker. Bundled because dptree
/// 0.5.1's `Injectable` impl tops out at 12 type params, and the message
/// handler was already at the limit — we cannot inject these as separate
/// top-level deps without pushing the message handler over.
///
/// These shared control maps share bot-process lifetime and are injected together:
/// - `stop_tokens`: per-(chat, thread) cancellation tokens for in-flight CC subprocesses.
/// - `session_locks`: per-main-session async mutex map (TOCTOU on session JSONL).
/// - `bg_requests`: per-(chat, thread) Background-button request flags.
/// - `bg_handoff_gates`: per-(chat, thread) foreground gate during background fork handoff.
/// - `thinking_visibility`: per-(chat, thread) Show/Hide thinking state for active runs.
/// - `progress`: per-foreground-invocation Telegram progress targets.
/// - `compact_timers`: per-(chat, thread) idle-compaction debounce timers.
#[derive(Clone)]
pub struct WorkerControlDeps {
    pub(crate) stop_tokens: StopTokens,
    pub(crate) session_locks: SessionLocks,
    pub(crate) bg_requests: BgRequests,
    pub(crate) bg_handoff_gates: BgHandoffGates,
    pub(crate) thinking_visibility: ThinkingVisibility,
    pub(crate) progress: progress::ProgressState,
    pub(crate) compact_timers: CompactTimers,
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
            debug: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: HashMap::new(),
            secret: None,
            attachments: Default::default(),
            network_policy: Default::default(),
            show_thinking: true,
            learning: Default::default(),
            memory: None,
            stt: Default::default(),
        }
    }

    #[tokio::test]
    async fn resolve_token_from_config() {
        let mut config = minimal_config();
        config.telegram_token = Some("999:inline_token".to_string());
        let token = resolve_token(&config).unwrap();
        assert_eq!(token, "999:inline_token");
    }

    #[tokio::test]
    async fn resolve_token_returns_err_when_nothing_configured() {
        let config = minimal_config();
        assert!(resolve_token(&config).is_err());
    }

    #[tokio::test]
    async fn resolve_token_returns_err_when_empty_string() {
        let mut config = minimal_config();
        config.telegram_token = Some(String::new());
        assert!(resolve_token(&config).is_err());
    }

    #[tokio::test]
    async fn initial_thinking_visibility_respects_context() {
        for (show_thinking, is_group, expected) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
            (false, true, false),
        ] {
            assert_eq!(
                initial_thinking_visibility(show_thinking, is_group),
                expected
            );
        }
    }

    #[tokio::test]
    async fn bg_handoff_wait_blocks_until_release() {
        let gates: BgHandoffGates = Arc::new(DashMap::new());
        let key = (42, 7);
        set_bg_handoff_gate(&gates, key);

        let waiter_gates = Arc::clone(&gates);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut waiter = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            wait_for_bg_handoff_gate(&waiter_gates, key).await;
        });
        started_rx.await.unwrap();

        assert!(
            tokio::time::timeout(tokio::time::Duration::from_millis(20), &mut waiter)
                .await
                .is_err(),
            "waiter must stay blocked while the gate is present"
        );

        release_bg_handoff_gate(&gates, key);
        tokio::time::timeout(tokio::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should unblock after gate release")
            .expect("waiter task should not panic");
        assert!(
            gates.get(&key).is_none(),
            "release must remove the gate entry"
        );
    }

    #[tokio::test]
    async fn parse_thinking_toggle_callback_accepts_valid_data() {
        assert_eq!(
            parse_thinking_toggle_callback("think:12345:678:show"),
            Some(((12345, 678), ThinkingToggleAction::Show))
        );
        assert_eq!(
            parse_thinking_toggle_callback("think:-100123:0:hide"),
            Some(((-100123, 0), ThinkingToggleAction::Hide))
        );
        assert!(ThinkingToggleAction::Show.expanded());
        assert!(!ThinkingToggleAction::Hide.expanded());
    }

    #[tokio::test]
    async fn parse_thinking_toggle_callback_rejects_malformed_data() {
        for bad in [
            "",
            "think",
            "think:1",
            "think:1:2",
            "think:1:2:toggle",
            "think:not-a-chat:2:show",
            "think:1:not-a-thread:show",
            "stop:1:2",
        ] {
            assert_eq!(parse_thinking_toggle_callback(bad), None, "bad={bad}");
        }
    }

    #[tokio::test]
    async fn set_thinking_visibility_writes_state_for_active_run() {
        let map: ThinkingVisibility = Arc::new(DashMap::new());
        let key = (12345_i64, 0_i64);
        map.insert(key, false);

        assert!(set_thinking_visibility(&map, key, true));
        assert!(*map.get(&key).unwrap().value());

        assert!(set_thinking_visibility(&map, key, false));
        assert!(!*map.get(&key).unwrap().value());

        assert!(!set_thinking_visibility(&map, (999, 0), true));
    }
}

#[cfg(test)]
mod shutdown_request_tests {
    use super::*;

    #[tokio::test]
    async fn request_shutdown_backgrounding_sets_gate_and_cancels_tokens() {
        let stop_tokens: StopTokens = Arc::new(DashMap::new());
        let bg_requests: BgRequests = Arc::new(DashMap::new());
        let gates: BgHandoffGates = Arc::new(DashMap::new());
        let token = CancellationToken::new();

        stop_tokens.insert((10, 0), (7, token.clone()));

        let requested = request_shutdown_backgrounding(&stop_tokens, &bg_requests, &gates);

        assert_eq!(requested, 1);
        assert!(token.is_cancelled());
        assert!(gates.get(&(10, 0)).is_some());
        let request = bg_requests.get(&(10, 0)).unwrap();
        assert_eq!(request.turn_id, 7);
        assert_eq!(request.reason, worker::BgReason::Shutdown);
    }

    #[tokio::test]
    async fn wait_for_handoff_gates_empty_returns_after_release() {
        let gates: BgHandoffGates = Arc::new(DashMap::new());
        set_bg_handoff_gate(&gates, (10, 0));
        let release = Arc::clone(&gates);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            release_bg_handoff_gate(&release, (10, 0));
        });

        let done = wait_for_handoff_gates_empty(&gates, std::time::Duration::from_secs(1)).await;
        assert!(done);
    }
}
