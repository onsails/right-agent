//! Idle-compaction debounce: after 2h of inactivity, run CC's native
//! `/compact` on an opus[1m] session that is >=40% full. See
//! docs/superpowers/specs/2026-05-31-idle-compaction-design.md

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
use tokio_util::sync::CancellationToken;

/// Idle window before compaction fires. A turn resets this debounce.
const IDLE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
/// Compact only when the last turn's context footprint reached this many
/// tokens (40% of the opus[1m] 1,000,000-token window).
const MIN_USED_TOKENS: u64 = 400_000;
/// Wall-clock cap on a single `/compact` call. Bounds how long a returning
/// user waits on the session lock if they arrive mid-compaction.
const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);
/// Steers CC's summary toward the active discussion. Static — CC already has
/// the full conversation at compaction time.
const RECENCY_INSTRUCTION: &str = "Prioritize the most recently discussed \
topics and any open or unresolved threads. Preserve concrete details from \
recent exchanges — names, file paths, decisions, values, and the user's \
current goal — over older, settled context.";

/// True for an Opus model running the 1M-context (`[1m]`) window. Matches the
/// suffix rather than a pinned id so an opus version bump keeps working while
/// `sonnet[1m]` and non-1M opus stay excluded.
pub(crate) fn is_opus_1m(model: Option<&str>) -> bool {
    matches!(model, Some(m) if m.starts_with("claude-opus") && m.ends_with("[1m]"))
}

/// The full gate: opus[1m] AND context footprint at/above the threshold.
pub(crate) fn should_compact(model: Option<&str>, used_tokens: u64) -> bool {
    is_opus_1m(model) && used_tokens >= MIN_USED_TOKENS
}

/// Context footprint of the most recent interactive turn for this session:
/// `input + cache_read + cache_creation` tokens. `None` when no turn exists.
/// This is the prompt size going into the last API call — i.e. how full the
/// context is, regardless of how much was cache-served.
pub(crate) async fn latest_interactive_context_tokens(
    conn: &right_db::Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<u64>, right_db::DbError> {
    use right_db::OptionalExtension as _;
    conn.query_row(
        "SELECT input_tokens + cache_read_tokens + cache_creation_tokens \
         FROM usage_events \
         WHERE chat_id = ?1 AND thread_id = ?2 AND source = 'interactive' \
         ORDER BY ts DESC LIMIT 1",
        right_db::params![chat_id, thread_id],
        |r| r.get::<_, i64>(0),
    )
    .await
    .optional()
    .map(|opt| opt.map(|v| v.max(0) as u64))
}

/// Build the specialized maintenance invocation: `claude -p --resume <id>
/// --model <opus[1m]> "/compact <recency instruction>"`, no schema, no MCP,
/// tools disabled. Deliberate exception to the standard session-bearing
/// contract (see ARCHITECTURE.md → Claude Invocation Contract).
///
/// `model` is the gate-verified opus[1m] id (the caller only reaches here after
/// `should_compact` confirmed it). Pinning it makes the compaction provably run
/// on opus[1m] instead of relying on `--resume` to inherit the model.
pub(crate) fn build_compact_invocation(
    root_session_id: &str,
    model: Option<String>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ClaudeInvocation {
    ClaudeInvocation {
        mcp_config_path: None,
        json_schema: None,
        output_format: OutputFormat::Json,
        model, // gate-verified opus[1m]; pinned so /compact runs on opus[1m]
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: Some(root_session_id.to_owned()),
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(format!("/compact {RECENCY_INSTRUCTION}")),
        debug_flag: Some(debug),
    }
}

/// Everything a fire task needs. Cloned from `WorkerContext` at the turn-end
/// hook (Task 7).
#[derive(Clone)]
pub(crate) struct IdleCompactionCtx {
    pub compact_timers: crate::telegram::CompactTimers,
    pub model: Arc<arc_swap::ArcSwap<Option<String>>>,
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub session_locks: crate::telegram::SessionLocks,
    pub debug: Arc<AtomicBool>,
    pub chat_id: i64,
    pub thread_id: i64,
}

/// Open the per-agent DB and read this session's context footprint.
/// Best-effort: logs and returns `None` on any DB error (the caller then
/// skips this cycle).
async fn open_and_read_fullness(ctx: &IdleCompactionCtx) -> Option<(right_db::Connection, u64)> {
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: open_connection failed: {e:#}");
            return None;
        }
    };
    let used = match latest_interactive_context_tokens(&conn, ctx.chat_id, ctx.thread_id).await {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: fullness query failed: {e:#}");
            return None;
        }
    };
    Some((conn, used))
}

/// Fire path. Re-checks eligibility, resolves the active session, takes the
/// per-session mutex, runs `/compact`, records usage. Best-effort: every
/// failure logs and returns (the next idle cycle retries). Never aborted
/// mid-flight (see `arm`), so the session lock is always released cleanly.
async fn run_compaction(ctx: IdleCompactionCtx) {
    let Some((conn, used)) = open_and_read_fullness(&ctx).await else {
        return;
    };

    // Fire-time re-checks: model (hot-reloadable via /model) and fullness.
    let model = crate::snapshot_model(&ctx.model);
    if !should_compact(model.as_deref(), used) {
        tracing::debug!(agent = %ctx.agent_name, "idle-compaction: no longer eligible at fire time, skipping");
        return;
    }

    let root_session_id = match crate::telegram::session::get_active_session(
        &conn,
        ctx.chat_id,
        ctx.thread_id,
    )
    .await
    {
        Ok(Some(s)) => s.root_session_id,
        Ok(None) => {
            tracing::debug!(agent = %ctx.agent_name, "idle-compaction: no active session, skipping");
            return;
        }
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: get_active_session failed: {e:#}");
            return;
        }
    };

    // Serialize against live worker/delivery turns on the same session.
    let _guard: tokio::sync::OwnedMutexGuard<()> = {
        let entry = ctx
            .session_locks
            .entry(root_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        entry.lock_owned().await
    };

    let args =
        build_compact_invocation(&root_session_id, model, Arc::clone(&ctx.debug)).into_args();
    let mut cmd = match crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: build_claude_command refused: {e:#}");
            return;
        }
    };
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(COMPACT_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!(agent = %ctx.agent_name, "idle-compaction: spawn failed: {e:#}");
            return;
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "idle-compaction: timed out after {}s",
                COMPACT_TIMEOUT.as_secs()
            );
            return;
        }
    };

    if !output.status.success() {
        // `/compact` returns an empty `result`; success is exit status only.
        tracing::warn!(
            agent = %ctx.agent_name,
            status = ?output.status,
            stderr_bytes = output.stderr.len(),
            "idle-compaction: /compact non-zero exit"
        );
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
        && let Err(e) = right_agent::usage::insert::insert_idle_compaction(
            &conn,
            &b,
            ctx.chat_id,
            ctx.thread_id,
        )
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "idle-compaction: usage insert failed: {e:#}");
    }

    tracing::info!(
        agent = %ctx.agent_name,
        chat_id = ctx.chat_id,
        thread_id = ctx.thread_id,
        used_tokens = used,
        "idle-compaction complete"
    );
}

/// Cancel and remove any pending compaction for this session. Called at turn
/// start (activity) and when a turn ends ineligible. No-op if none armed.
pub(crate) fn cancel(timers: &crate::telegram::CompactTimers, chat_id: i64, thread_id: i64) {
    if let Some((_, token)) = timers.remove(&(chat_id, thread_id)) {
        token.cancel();
    }
}

/// (Re)arm the 2h debounce. Replaces any existing timer. The spawned task
/// waits on `sleep` racing the token: a cancel during the wait returns without
/// compacting; once `sleep` wins, the token is no longer awaited, so a late
/// cancel cannot tear down the in-flight compaction (which would orphan the
/// `claude` child and drop the session lock mid-write).
fn arm(ctx: IdleCompactionCtx) {
    let key = (ctx.chat_id, ctx.thread_id);
    if let Some((_, prev)) = ctx.compact_timers.remove(&key) {
        prev.cancel();
    }
    let token = CancellationToken::new();
    ctx.compact_timers.insert(key, token.clone());

    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(IDLE_AFTER) => {}
            _ = token.cancelled() => return,
        }
        // Survived the debounce. Drop our map entry first so a concurrent
        // cancel finds nothing to cancel, then run to completion uncancelled.
        ctx.compact_timers.remove(&key);
        run_compaction(ctx).await;
    });
}

/// Turn-end hook for Normal foreground turns. Model-checks first (no DB for
/// non-opus[1m] agents); on opus[1m], reads fullness and arms or cancels.
pub(crate) async fn on_turn_end(ctx: IdleCompactionCtx) {
    let model = crate::snapshot_model(&ctx.model);
    if !is_opus_1m(model.as_deref()) {
        cancel(&ctx.compact_timers, ctx.chat_id, ctx.thread_id);
        return;
    }
    let Some((_conn, used)) = open_and_read_fullness(&ctx).await else {
        return;
    };
    if should_compact(model.as_deref(), used) {
        arm(ctx);
    } else {
        cancel(&ctx.compact_timers, ctx.chat_id, ctx.thread_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_1m_variants_match() {
        assert!(is_opus_1m(Some("claude-opus-4-8[1m]")));
        assert!(is_opus_1m(Some("claude-opus-4-9[1m]"))); // future bump
    }

    #[test]
    fn non_opus_1m_rejected() {
        assert!(!is_opus_1m(Some("claude-sonnet-4-6[1m]")));
        assert!(!is_opus_1m(Some("claude-opus-4-8"))); // not 1m
        assert!(!is_opus_1m(Some("claude-haiku-4-5")));
        assert!(!is_opus_1m(None));
    }

    #[test]
    fn should_compact_boundary() {
        assert!(should_compact(Some("claude-opus-4-8[1m]"), 400_000));
        assert!(!should_compact(Some("claude-opus-4-8[1m]"), 399_999));
        assert!(!should_compact(Some("claude-sonnet-4-6[1m]"), 1_000_000));
        assert!(!should_compact(None, 1_000_000));
    }

    #[test]
    fn compact_invocation_argv_is_maintenance_shaped() {
        let debug = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let args =
            build_compact_invocation("root-uuid", Some("claude-opus-4-8[1m]".to_string()), debug)
                .into_args();
        let joined = args.join(" ");
        // resumes the real session
        let pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[pos + 1], "root-uuid");
        // pins the gate-verified opus[1m] model rather than inheriting implicitly
        let mpos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[mpos + 1], "claude-opus-4-8[1m]");
        // prompt is the /compact command with the recency instruction
        let dash = args.iter().position(|a| a == "--").unwrap();
        assert!(args[dash + 1].starts_with("/compact "));
        assert!(args[dash + 1].contains("most recently discussed"));
        // maintenance contract: no schema, no MCP
        assert!(!joined.contains("--json-schema"));
        assert!(!joined.contains("--mcp-config"));
    }

    #[tokio::test]
    async fn fullness_reads_latest_interactive_sum() {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        let mk =
            |input: u64, cache_read: u64, cache_create: u64| right_agent::usage::UsageBreakdown {
                session_uuid: "s".into(),
                total_cost_usd: 0.0,
                num_turns: 1,
                input_tokens: input,
                output_tokens: 0,
                cache_creation_tokens: cache_create,
                cache_read_tokens: cache_read,
                web_search_requests: 0,
                web_fetch_requests: 0,
                model_usage_json: "{}".into(),
                api_key_source: "none".into(),
                wall_elapsed_ms: None,
            };

        // Older smaller turn, then a newer larger turn, for the same (chat, thread).
        right_agent::usage::insert::insert_interactive(&conn, &mk(1, 1, 1), 42, 0)
            .await
            .unwrap();
        right_agent::usage::insert::insert_interactive(&conn, &mk(100, 200, 50), 42, 0)
            .await
            .unwrap();
        // A different source must be ignored.
        right_agent::usage::insert::insert_learning_prefilter(&conn, &mk(9_999, 0, 0), 42, 0)
            .await
            .unwrap();

        let used = latest_interactive_context_tokens(&conn, 42, 0)
            .await
            .unwrap();
        assert_eq!(used, Some(350)); // 100 + 200 + 50

        let absent = latest_interactive_context_tokens(&conn, 999, 0)
            .await
            .unwrap();
        assert_eq!(absent, None);
    }

    fn dummy_ctx(
        timers: crate::telegram::CompactTimers,
        chat: i64,
        thread: i64,
    ) -> IdleCompactionCtx {
        IdleCompactionCtx {
            compact_timers: timers,
            model: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(Some(
                "claude-opus-4-8[1m]".to_string(),
            ))),
            agent_dir: std::path::PathBuf::from("/nonexistent"),
            agent_db_dir: std::path::PathBuf::from("/nonexistent"),
            agent_name: "test".into(),
            ssh_config_path: None,
            resolved_sandbox: None,
            session_locks: std::sync::Arc::new(dashmap::DashMap::new()),
            debug: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            chat_id: chat,
            thread_id: thread,
        }
    }

    #[tokio::test]
    async fn arm_then_cancel_removes_and_cancels() {
        let timers: crate::telegram::CompactTimers = std::sync::Arc::new(dashmap::DashMap::new());
        arm(dummy_ctx(timers.clone(), 1, 0));
        let token = timers.get(&(1, 0)).map(|e| e.value().clone());
        assert!(token.is_some(), "arm must register a timer");
        cancel(&timers, 1, 0);
        assert!(
            timers.get(&(1, 0)).is_none(),
            "cancel must remove the entry"
        );
        assert!(
            token.unwrap().is_cancelled(),
            "cancel must cancel the token"
        );
        // The spawned task takes the cancelled branch and never runs run_compaction
        // (the /nonexistent paths would otherwise error), so a short yield is safe.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn arm_twice_replaces_previous_timer() {
        let timers: crate::telegram::CompactTimers = std::sync::Arc::new(dashmap::DashMap::new());
        arm(dummy_ctx(timers.clone(), 2, 0));
        let first = timers.get(&(2, 0)).unwrap().value().clone();
        arm(dummy_ctx(timers.clone(), 2, 0));
        assert!(
            first.is_cancelled(),
            "re-arming must cancel the prior timer"
        );
        assert!(
            timers.get(&(2, 0)).is_some(),
            "a fresh timer must be present"
        );
        cancel(&timers, 2, 0);
        tokio::task::yield_now().await;
    }
}
