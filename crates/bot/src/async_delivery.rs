use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use right_db::OptionalExtension as _;
use teloxide::payloads::SendMessageSetters as _;

use crate::cc::markdown_utils::strip_html_tags;
use crate::telegram::handler::IdleTimestamp;

/// A pending async run result ready for delivery.
#[derive(Debug)]
pub(crate) struct PendingAsyncResult {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub delivery_json: String,
    pub run_note: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}

/// Query the oldest undelivered async result with a non-null delivery_json.
#[cfg(test)]
pub(crate) fn fetch_pending(
    conn: &right_db::Connection,
) -> Result<Option<PendingAsyncResult>, right_db::DbError> {
    Ok(fetch_pending_batch(conn, 1)?.into_iter().next())
}

fn fetch_pending_batch(
    conn: &right_db::Connection,
    limit: usize,
) -> Result<Vec<PendingAsyncResult>, right_db::DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                NULLIF(target_chat_id, 0), target_thread_id \
         FROM async_runs \
         WHERE delivery_required = 1 \
           AND delivery_status IN ('pending', 'retryable') \
           AND status IN ('success', 'failed') \
           AND delivery_json IS NOT NULL \
         ORDER BY finished_at ASC \
         LIMIT ?1",
    )?;
    stmt.query_map(right_db::params![limit.max(1) as i64], pending_from_row)?
        .collect()
}

fn pending_from_row(row: &right_db::row::Row<'_>) -> Result<PendingAsyncResult, right_db::DbError> {
    Ok(PendingAsyncResult {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        delivery_json: row.get(3)?,
        run_note: row.get(4)?,
        status: row.get(5)?,
        target_chat_id: row.get(6)?,
        target_thread_id: row.get(7)?,
    })
}

pub(crate) fn fetch_next_pending(
    conn: &right_db::Connection,
    delivered_in_memory: &HashSet<String>,
) -> Result<Option<PendingAsyncResult>, right_db::DbError> {
    let limit = PENDING_FETCH_BATCH_SIZE.max(delivered_in_memory.len().saturating_add(1));
    let batch = fetch_pending_batch(conn, limit)?;
    let mut in_memory_ids = Vec::new();
    for pending in batch {
        if delivered_in_memory.contains(&pending.id) {
            in_memory_ids.push(pending.id);
            continue;
        }
        return Ok(Some(pending));
    }

    let mut mark_error = None;
    for id in in_memory_ids {
        if let Err(e) = mark_delivery_outcome(conn, &id, "delivered") {
            tracing::warn!(run_id = %id, "async delivery: retry mark delivered failed: {e:#}");
            if mark_error.is_none() {
                mark_error = Some(e);
            }
        }
    }
    if let Some(e) = mark_error {
        return Err(e);
    }
    Ok(None)
}

/// Mark an async run delivery as complete with a given status.
///
/// Single UPDATE sets both `delivery_status` and `delivered_at` atomically.
fn mark_delivery_outcome(
    conn: &right_db::Connection,
    run_id: &str,
    status: &str,
) -> Result<(), right_db::DbError> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn.execute(
        "UPDATE async_runs \
         SET delivery_status = ?1, delivered_at = ?2, updated_at = ?2 \
         WHERE id = ?3",
        right_db::params![status, now, run_id],
    )?;
    if rows == 0 {
        return Err(right_db::DbError::NotFound);
    }
    Ok(())
}

/// Deduplicate: for a given job, find the latest undelivered result and mark all
/// older undelivered results as delivered. Returns (latest_result, skipped_count).
pub(crate) fn deduplicate_job(
    conn: &right_db::Connection,
    producer_ref: &str,
) -> Result<Option<(PendingAsyncResult, u32)>, right_db::DbError> {
    let latest = conn
        .query_row(
            "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                    NULLIF(target_chat_id, 0), target_thread_id \
             FROM async_runs \
             WHERE kind = 'cron' \
               AND producer_ref = ?1 \
               AND delivery_required = 1 \
               AND delivery_status IN ('pending', 'retryable') \
               AND status IN ('success', 'failed') \
               AND delivery_json IS NOT NULL \
             ORDER BY finished_at DESC \
             LIMIT 1",
            right_db::params![producer_ref],
            pending_from_row,
        )
        .optional()?;

    let Some(latest) = latest else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE async_runs \
         SET delivered_at = ?1, delivery_status = 'superseded', updated_at = ?1 \
         WHERE kind = 'cron' \
           AND producer_ref = ?2 \
           AND id != ?3 \
           AND delivery_required = 1 \
           AND delivery_status IN ('pending', 'retryable') \
           AND status IN ('success', 'failed')",
        right_db::params![now, producer_ref, &latest.id],
    )?;

    Ok(Some((latest, count as u32)))
}

pub(crate) fn select_delivery_candidate(
    conn: &right_db::Connection,
    pending: PendingAsyncResult,
) -> Result<Option<(PendingAsyncResult, u32)>, right_db::DbError> {
    if pending.kind == "cron"
        && let Some(producer_ref) = pending.producer_ref.as_deref()
    {
        return deduplicate_job(conn, producer_ref);
    }

    Ok(Some((pending, 0)))
}

/// Instruction prefix for the delivery CC session (success path).
///
/// This is approach A: instruction in stdin. If Haiku ignores these instructions
/// (summarizes instead of relaying verbatim), migrate to approach B: add a
/// delivery-specific block to the system prompt via `build_prompt_assembly_script()`.
/// See docs/superpowers/specs/2026-04-15-cron-delivery-verbatim-relay.md.
const CRON_DELIVERY_INSTRUCTION_SUCCESS: &str = "\
You are delivering a cron job result to the user.
The `content` field below is the FINAL user-facing message — send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Ignore the attachments field — attachments are sent separately.

Here is the YAML report of the cron job:
";

/// Delivery instruction used when a cron job's `status` is 'failed'.
///
/// The `content` field carries a platform-generated failure summary (either
/// produced by the agent's reflection pass in Task 9, or a raw exit-code
/// fallback). Haiku should relay naturally, not verbatim.
const DELIVERY_INSTRUCTION_FAILURE: &str = "\
The cron job below did not complete successfully. The `content` field contains
a platform-generated summary of the failure (produced by the agent's reflection
pass). Relay it to the user in natural prose — you MAY rephrase lightly for
flow with the recent conversation, but keep all factual claims intact. Do not
invent details. Ignore the attachments field.

Here is the YAML report of the cron job:
";

const BACKGROUND_DELIVERY_INSTRUCTION_SUCCESS: &str = "\
You are delivering a background task result to the user.
The `content` field below is the FINAL user-facing message - send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Ignore the attachments field - attachments are sent separately.

Here is the YAML report of the background task:
";

const BACKGROUND_DELIVERY_INSTRUCTION_FAILURE: &str = "\
The background task below did not complete successfully. The `content` field contains
a platform-generated summary of the failure. Relay it to the user in natural prose -
you MAY rephrase lightly for flow with the recent conversation, but keep all factual
claims intact. Do not invent details. Ignore the attachments field.

Here is the YAML report of the background task:
";

/// Format a pending async result as YAML for the main CC session.
///
/// The output begins with an instruction prefix selected by kind/status,
/// followed by the YAML payload.
pub(crate) fn format_async_yaml(
    pending: &PendingAsyncResult,
    skipped: u32,
) -> Result<String, String> {
    let total = skipped + 1;
    let instruction = match (pending.kind.as_str(), pending.status.as_str()) {
        ("background", "failed") => BACKGROUND_DELIVERY_INSTRUCTION_FAILURE,
        ("background", _) => BACKGROUND_DELIVERY_INSTRUCTION_SUCCESS,
        (_, "failed") => DELIVERY_INSTRUCTION_FAILURE,
        _ => CRON_DELIVERY_INSTRUCTION_SUCCESS,
    };
    let mut output = String::from(instruction);
    let is_background = pending.kind == "background";
    if is_background {
        let label = pending.producer_ref.as_deref().unwrap_or("background");
        output.push_str("\nbackground_result:\n");
        output.push_str(&format!(
            "  label: \"{}\"\n",
            crate::telegram::attachments::yaml_escape_string(label)
        ));
    } else {
        let job = pending.producer_ref.as_deref().unwrap_or("cron");
        output.push_str("\ncron_result:\n");
        output.push_str(&format!(
            "  job: \"{}\"\n",
            crate::telegram::attachments::yaml_escape_string(job)
        ));
        output.push_str(&format!("  runs_total: {total}\n"));
        if skipped > 0 {
            output.push_str(&format!("  skipped_runs: {skipped}\n"));
        }
    }

    let notify = crate::cron::notify_from_delivery_json(&pending.delivery_json)?;
    output.push_str("  result:\n");
    output.push_str("    notify:\n");
    output.push_str(&format!(
        "      content: \"{}\"\n",
        crate::telegram::attachments::yaml_escape_string(&notify.content)
    ));
    if let Some(ref atts) = notify.attachments
        && !atts.is_empty()
    {
        output.push_str("      attachments:\n");
        for att in atts {
            let kind_str = serde_json::to_value(att.kind)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "document".to_string());
            output.push_str(&format!(
                "        - type: \"{}\"\n",
                crate::telegram::attachments::yaml_escape_string(&kind_str)
            ));
            output.push_str(&format!(
                "          path: \"{}\"\n",
                crate::telegram::attachments::yaml_escape_string(&att.path)
            ));
            if let Some(ref caption) = att.caption {
                output.push_str(&format!(
                    "          caption: \"{}\"\n",
                    crate::telegram::attachments::yaml_escape_string(caption)
                ));
            }
        }
    }
    output.push_str(&format!(
        "    run_note: \"{}\"\n",
        crate::telegram::attachments::yaml_escape_string(&pending.run_note)
    ));

    Ok(output)
}

use right_platform_knobs::IDLE_THRESHOLD_SECS;

const POLL_INTERVAL_SECS: u64 = 30; // Check every 30s
const PENDING_FETCH_BATCH_SIZE: usize = 25;
const MAX_DELIVERY_ATTEMPTS: u32 = 3;
pub(crate) const ASYNC_DELIVERY_SHUTDOWN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(20);
const TELEGRAM_SEND_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(15);
const DELIVERY_INTERRUPTED_BY_SHUTDOWN: &str = "delivery interrupted by shutdown";
const DELIVERY_SEND_OUTCOME_UNKNOWN_AFTER_SHUTDOWN: &str =
    "delivery send outcome unknown after shutdown";

#[derive(Debug, Clone, Copy)]
struct DeliveryShutdownControl<'a> {
    token: Option<&'a tokio_util::sync::CancellationToken>,
    deadline: Option<tokio::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMode {
    Normal,
    ShutdownFlush,
}

fn should_wait_for_idle(mode: DeliveryMode, idle_for: i64) -> bool {
    mode == DeliveryMode::Normal && idle_for < IDLE_THRESHOLD_SECS
}

struct DeliveryLoopState {
    delivered_in_memory: HashSet<String>,
    attempt_counts: std::collections::HashMap<String, u32>,
}

impl DeliveryLoopState {
    fn new() -> Self {
        Self {
            delivered_in_memory: HashSet::new(),
            attempt_counts: std::collections::HashMap::new(),
        }
    }
}

/// Outcome of resolving a pending async run's delivery target against the live allowlist.
#[derive(Debug)]
pub(crate) enum TargetClassification {
    NoTarget,
    Denied,
    Ready {
        chat_id: i64,
        thread_id: Option<i64>,
    },
}

/// Classify a pending async result. Pure function; no side effects.
pub(crate) fn classify_pending_target(
    pending: &PendingAsyncResult,
    allowlist: &right_agent::agent::allowlist::AllowlistState,
) -> TargetClassification {
    match pending.target_chat_id {
        None => TargetClassification::NoTarget,
        Some(id) if !allowlist.is_chat_allowed(id) => TargetClassification::Denied,
        Some(id) => TargetClassification::Ready {
            chat_id: id,
            thread_id: pending.target_thread_id,
        },
    }
}

fn pending_label(pending: &PendingAsyncResult) -> &str {
    pending
        .producer_ref
        .as_deref()
        .unwrap_or(pending.kind.as_str())
}

#[allow(clippy::too_many_arguments)]
async fn run_delivery_once(
    conn: &mut right_db::Connection,
    state: &mut DeliveryLoopState,
    mode: DeliveryMode,
    agent_dir: &Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    bot: &crate::telegram::BotType,
    allowlist: &right_agent::agent::allowlist::AllowlistHandle,
    idle_ts: &Arc<IdleTimestamp>,
    ssh_config_path: Option<&Path>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<&str>,
    upgrade_lock: &Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: &Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent::agent::types::LearningConfig,
    learning_drain_scheduler: &Arc<crate::learning_episode::DrainScheduler>,
    shutdown: DeliveryShutdownControl<'_>,
) -> bool {
    let pending = match fetch_next_pending(conn, &state.delivered_in_memory) {
        Ok(Some(p)) => p,
        Ok(None) => return false,
        Err(e) => {
            tracing::error!("async delivery: fetch_next_pending failed: {e:#}");
            return false;
        }
    };

    let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
    let now = chrono::Utc::now().timestamp();
    let idle_for = now - last;
    if should_wait_for_idle(mode, idle_for) {
        let wait = IDLE_THRESHOLD_SECS - idle_for;
        tracing::info!(
            kind = %pending.kind,
            producer_ref = ?pending.producer_ref,
            run_id = %pending.id,
            idle_secs = idle_for,
            wait_secs = wait,
            "async delivery: result pending, waiting for chat idle ({IDLE_THRESHOLD_SECS}s)"
        );
        return false;
    }

    let (to_deliver, skipped) = match select_delivery_candidate(conn, pending) {
        Ok(Some((result, s))) => (result, s),
        Ok(None) => return false,
        Err(e) => {
            tracing::error!("async delivery: candidate selection failed: {e:#}");
            return false;
        }
    };

    if state.delivered_in_memory.contains(&to_deliver.id) {
        if mark_delivery_outcome(conn, &to_deliver.id, "delivered").is_ok() {
            state.delivered_in_memory.remove(&to_deliver.id);
        }
        tracing::debug!(run_id = %to_deliver.id, "skipping already-delivered run (in-memory dedup)");
        return true;
    }

    let allowlist_snapshot = {
        let guard = allowlist.0.read().expect("allowlist lock poisoned");
        guard.clone()
    };

    let (target_chat_id, target_thread_id) =
        match classify_pending_target(&to_deliver, &allowlist_snapshot) {
            TargetClassification::NoTarget => {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    producer_ref = ?to_deliver.producer_ref,
                    run_id = %to_deliver.id,
                    "async run has no target_chat_id; marking delivery no_target"
                );
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "no_target") {
                    tracing::error!(run_id = %to_deliver.id, "mark no_target failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                return true;
            }
            TargetClassification::Denied => {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    producer_ref = ?to_deliver.producer_ref,
                    run_id = %to_deliver.id,
                    target_chat_id = ?to_deliver.target_chat_id,
                    "async delivery target chat is not in allowlist; skipping delivery"
                );
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "denied") {
                    tracing::error!(run_id = %to_deliver.id, "mark denied failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                return true;
            }
            TargetClassification::Ready { chat_id, thread_id } => (chat_id, thread_id),
        };

    let session_id = match crate::telegram::session::get_active_session(
        conn,
        target_chat_id,
        target_thread_id.unwrap_or(0),
    ) {
        Ok(s) => s.map(|s| s.root_session_id),
        Err(e) => {
            tracing::error!("async delivery: session lookup failed: {e:#}");
            None
        }
    };

    let yaml = match format_async_yaml(&to_deliver, skipped) {
        Ok(y) => y,
        Err(e) => {
            tracing::error!(
                kind = %to_deliver.kind,
                label = %pending_label(&to_deliver),
                run_id = %to_deliver.id,
                "async delivery: delivery_json deserialization failed, marking delivery failed: {e:#}"
            );
            if let Err(db_err) = mark_delivery_outcome(conn, &to_deliver.id, "failed") {
                tracing::error!(run_id = %to_deliver.id, "mark failed for malformed delivery_json failed: {db_err:#}");
                state.delivered_in_memory.insert(to_deliver.id.clone());
            }
            return true;
        }
    };
    tracing::info!(
        kind = %to_deliver.kind,
        label = %pending_label(&to_deliver),
        run_id = %to_deliver.id,
        skipped,
        target_chat_id,
        ?target_thread_id,
        "delivering async result through main session"
    );

    match deliver_through_session(
        &yaml,
        agent_dir,
        agent_name,
        bot,
        target_chat_id,
        target_thread_id,
        ssh_config_path,
        crate::snapshot_model(model),
        session_id,
        internal_client,
        resolved_sandbox,
        upgrade_lock,
        session_locks.clone(),
        Arc::clone(debug),
        shutdown,
    )
    .await
    {
        Ok(report) => {
            if let Err(e) = ensure_delivery_send_report_non_empty(report) {
                tracing::error!(run_id = %to_deliver.id, "async delivery returned empty send report: {e}");
                return true;
            }
            // TODO(usage): delivery stream capture lives elsewhere — follow up.
            // deliver_through_session uses OutputFormat::Json (single JSON blob, not stream-json
            // NDJSON), so there is no "result" event line to feed parse_usage_full. Usage
            // tracking for delivery sessions requires either switching to stream-json output
            // or extracting cost from the non-streaming JSON response format.
            if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "delivered") {
                tracing::error!(run_id = %to_deliver.id, "delivery DB update failed: {e:#}");
                state.delivered_in_memory.insert(to_deliver.id.clone());
            }
            capture_async_delivery_seed(
                conn,
                agent_dir,
                agent_name,
                &to_deliver,
                ssh_config_path,
                resolved_sandbox,
                debug,
                learning,
                crate::snapshot_model(model),
                learning_drain_scheduler,
            );
            let outbox_subdir = match to_deliver.kind.as_str() {
                "background" => "background",
                _ => "cron",
            };
            let outbox_dir = agent_dir
                .join("outbox")
                .join(outbox_subdir)
                .join(&to_deliver.id);
            if let Err(e) = std::fs::remove_dir_all(&outbox_dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(run_id = %to_deliver.id, "outbox cleanup failed: {e:#}");
            }
            idle_ts.0.store(
                chrono::Utc::now().timestamp(),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        Err(e) => {
            if is_delivery_shutdown_interruption(&e) {
                tracing::info!(
                    kind = %to_deliver.kind,
                    label = %pending_label(&to_deliver),
                    run_id = %to_deliver.id,
                    "async delivery interrupted by shutdown; leaving row pending"
                );
                return false;
            }
            if is_delivery_terminal_shutdown_send_error(&e) {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    label = %pending_label(&to_deliver),
                    run_id = %to_deliver.id,
                    "async delivery send outcome is unknown after shutdown deadline; marking terminal failed to avoid duplicate delivery"
                );
                if let Err(mark_err) = mark_delivery_outcome(conn, &to_deliver.id, "failed") {
                    tracing::error!(run_id = %to_deliver.id, "terminal delivery-failure DB update failed: {mark_err:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                state.attempt_counts.remove(&to_deliver.id);
                return true;
            }
            let attempts = state
                .attempt_counts
                .entry(to_deliver.id.clone())
                .and_modify(|c| *c += 1)
                .or_insert(1);
            tracing::error!(
                kind = %to_deliver.kind,
                label = %pending_label(&to_deliver),
                run_id = %to_deliver.id,
                attempt = *attempts,
                max = MAX_DELIVERY_ATTEMPTS,
                "async delivery failed: {e:#}"
            );
            if *attempts >= MAX_DELIVERY_ATTEMPTS {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    label = %pending_label(&to_deliver),
                    run_id = %to_deliver.id,
                    "giving up after {MAX_DELIVERY_ATTEMPTS} attempts, marking as delivered"
                );
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "failed") {
                    tracing::error!(run_id = %to_deliver.id, "delivery-failure DB update failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                state.attempt_counts.remove(&to_deliver.id);
            }
        }
    }

    true
}

/// Main delivery loop. Runs as a tokio task.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    bot: crate::telegram::BotType,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<right_mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: right_agent::agent::types::LearningConfig,
    learning_drain_scheduler: Arc<crate::learning_episode::DrainScheduler>,
) {
    tracing::info!(agent = %agent_name, "async delivery loop started");

    let mut conn = match right_db::open_connection(&agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("async delivery: DB open failed: {e:#}");
            return;
        }
    };

    let mut state = DeliveryLoopState::new();

    loop {
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)) => {}
            () = shutdown.cancelled() => {
                tracing::info!("async delivery loop shutting down");
                return;
            }
        }

        run_delivery_once(
            &mut conn,
            &mut state,
            DeliveryMode::Normal,
            &agent_dir,
            &agent_name,
            &model,
            &bot,
            &allowlist,
            &idle_ts,
            ssh_config_path.as_deref(),
            &internal_client,
            resolved_sandbox.as_deref(),
            &upgrade_lock,
            &session_locks,
            &debug,
            &learning,
            &learning_drain_scheduler,
            DeliveryShutdownControl {
                token: Some(&shutdown),
                deadline: None,
            },
        )
        .await;
    }
}

async fn run_or_delivery_shutdown<T>(
    control: DeliveryShutdownControl<'_>,
    future: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = async {
            if let Some(token) = control.token {
                token.cancelled().await;
            }
        }, if control.token.is_some() => Err(DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()),
        _ = async {
            if let Some(deadline) = control.deadline {
                tokio::time::sleep_until(deadline).await;
            }
        }, if control.deadline.is_some() => Err(DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()),
        result = future => Ok(result),
    }
}

fn is_delivery_shutdown_interruption(error: &str) -> bool {
    error == DELIVERY_INTERRUPTED_BY_SHUTDOWN
}

async fn run_telegram_request_with_shutdown<F, T, E>(
    control: DeliveryShutdownControl<'_>,
    previous_telegram_side_effect: bool,
    request: F,
) -> Result<Result<T, E>, String>
where
    F: std::future::IntoFuture<Output = Result<T, E>>,
{
    let pre_poll_shutdown_error = if previous_telegram_side_effect {
        DELIVERY_SEND_OUTCOME_UNKNOWN_AFTER_SHUTDOWN
    } else {
        DELIVERY_INTERRUPTED_BY_SHUTDOWN
    };
    if control
        .deadline
        .is_some_and(|deadline| deadline <= tokio::time::Instant::now())
    {
        return Err(pre_poll_shutdown_error.to_owned());
    }
    if control.token.is_some_and(|token| token.is_cancelled()) {
        return Err(pre_poll_shutdown_error.to_owned());
    }

    let request_was_polled = std::sync::atomic::AtomicBool::new(false);
    let mut inner = Box::pin(request.into_future());
    let future = std::future::poll_fn(|cx| {
        request_was_polled.store(true, std::sync::atomic::Ordering::Relaxed);
        inner.as_mut().poll(cx)
    });
    tokio::pin!(future);
    let mut deadline = control.deadline;

    loop {
        if let Some(until) = deadline {
            if until <= tokio::time::Instant::now() {
                return Err(DELIVERY_SEND_OUTCOME_UNKNOWN_AFTER_SHUTDOWN.to_owned());
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(until) => {
                    return Err(DELIVERY_SEND_OUTCOME_UNKNOWN_AFTER_SHUTDOWN.to_owned());
                }
                result = &mut future => return Ok(result),
            }
        }

        if let Some(token) = control.token {
            if token.is_cancelled() {
                if request_was_polled.load(std::sync::atomic::Ordering::Relaxed) {
                    deadline = Some(tokio::time::Instant::now() + TELEGRAM_SEND_SHUTDOWN_GRACE);
                    continue;
                }
                return Err(pre_poll_shutdown_error.to_owned());
            }
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    if request_was_polled.load(std::sync::atomic::Ordering::Relaxed) {
                        deadline = Some(tokio::time::Instant::now() + TELEGRAM_SEND_SHUTDOWN_GRACE);
                    } else {
                        return Err(pre_poll_shutdown_error.to_owned());
                    }
                }
                result = &mut future => return Ok(result),
            }
        } else {
            return Ok(future.await);
        }
    }
}

fn is_delivery_terminal_shutdown_send_error(error: &str) -> bool {
    error == DELIVERY_SEND_OUTCOME_UNKNOWN_AFTER_SHUTDOWN
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn flush_ready_deliveries_for_shutdown(
    agent_dir: PathBuf,
    agent_name: String,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    bot: crate::telegram::BotType,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
    learning: right_agent::agent::types::LearningConfig,
    learning_drain_scheduler: Arc<crate::learning_episode::DrainScheduler>,
) {
    let mut conn = match right_db::open_connection(&agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("async delivery shutdown flush: DB open failed: {e:#}");
            return;
        }
    };
    let mut state = DeliveryLoopState::new();
    let deadline = tokio::time::Instant::now() + ASYNC_DELIVERY_SHUTDOWN_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("async delivery shutdown flush timed out");
            return;
        }
        let delivered = run_delivery_once(
            &mut conn,
            &mut state,
            DeliveryMode::ShutdownFlush,
            &agent_dir,
            &agent_name,
            &model,
            &bot,
            &allowlist,
            &idle_ts,
            ssh_config_path.as_deref(),
            &internal_client,
            resolved_sandbox.as_deref(),
            &upgrade_lock,
            &session_locks,
            &debug,
            &learning,
            &learning_drain_scheduler,
            DeliveryShutdownControl {
                token: None,
                deadline: Some(deadline),
            },
        )
        .await;
        if !delivered {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_async_delivery_seed(
    conn: &right_db::Connection,
    agent_dir: &Path,
    agent_name: &str,
    delivered: &PendingAsyncResult,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
    debug: &Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent::agent::types::LearningConfig,
    inherited_model: Option<String>,
    learning_drain_scheduler: &Arc<crate::learning_episode::DrainScheduler>,
) {
    if delivered.kind != "background" {
        return;
    }
    let seed_ref = format!("async:{}", delivered.id);
    let runtime = crate::learning_episode::LearningEpisodeRuntime::new(
        agent_dir.to_path_buf(),
        agent_dir.to_path_buf(),
        agent_name.to_owned(),
        inherited_model,
        ssh_config_path.map(Path::to_path_buf),
        resolved_sandbox.map(str::to_owned),
        Arc::clone(debug),
        learning.clone(),
        Some(Arc::clone(learning_drain_scheduler)),
        None,
    );
    if let Err(e) = runtime.capture_completion_seed(
        conn,
        right_agent::learning_episodes::LearningEpisodeKind::AsyncContinuation,
        right_agent::learning_episodes::EpisodeSeedTriggerKind::AsyncResult,
        &seed_ref,
        delivered.target_chat_id,
        delivered.target_thread_id,
    ) {
        tracing::warn!(
            agent = %agent_name,
            run_id = %delivered.id,
            "async delivery learning episode seed capture failed: {e:#}"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeliverySendReport {
    pub text_messages_sent: usize,
    pub attachment_batches_sent: usize,
}

impl DeliverySendReport {
    fn total_sent(self) -> usize {
        self.text_messages_sent + self.attachment_batches_sent
    }
}

pub(crate) fn ensure_delivery_send_report_non_empty(
    report: DeliverySendReport,
) -> Result<(), String> {
    if report.total_sent() == 0 {
        return Err("empty delivery reply: no text messages or attachment batches sent".into());
    }
    Ok(())
}

fn build_delivery_invocation_args(
    mcp_config_path: String,
    json_schema: String,
    configured_model: Option<String>,
    session_id: Option<String>,
    debug_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Vec<String> {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path),
        json_schema: Some(json_schema),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: configured_model,
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: session_id,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        // Delivery is a relay, but harness built-ins are still available — apply
        // baseline so the relay can't self-loop or escape via TeamCreate etc.
        disallowed_tools: crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        ),
        extra_args: vec![],
        prompt: None, // stdin-piped
        debug_flag,
    }
    .into_args()
}

/// Invoke the main CC session with async result YAML and send the reply to Telegram.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    ssh_config_path: Option<&Path>,
    configured_model: Option<String>,
    session_id: Option<String>,
    internal_client: &right_mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    upgrade_lock: &tokio::sync::RwLock<()>,
    session_locks: crate::telegram::SessionLocks,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown: DeliveryShutdownControl<'_>,
) -> Result<DeliverySendReport, String> {
    use std::process::Stdio;

    // Block while upgrade is running.
    let _upgrade_guard = run_or_delivery_shutdown(shutdown, upgrade_lock.read()).await?;

    // Acquire the per-session mutex so delivery doesn't race with an active worker
    // turn on the same CC session (same --resume chain). None when there is no
    // active session — nothing to race with.
    let _session_guard = match session_id.clone() {
        Some(id) => {
            let entry = session_locks
                .entry(id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
            Some(run_or_delivery_shutdown(shutdown, entry.lock_owned()).await?)
        }
        None => None,
    };

    let mcp_path = crate::cc::invocation::mcp_config_path(ssh_config_path, agent_dir);

    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    let json_schema = std::fs::read_to_string(&reply_schema_path).unwrap_or_default();

    let claude_args = build_delivery_invocation_args(
        mcp_path,
        json_schema,
        configured_model,
        session_id,
        Some(debug),
    );

    // Derive sandbox_mode and home_dir from ssh_config_path.
    let (sandbox_mode, home_dir) = if ssh_config_path.is_some() {
        (
            right_agent::agent::types::SandboxMode::Openshell,
            "/sandbox".to_owned(),
        )
    } else {
        (
            right_agent::agent::types::SandboxMode::None,
            agent_dir.to_string_lossy().into_owned(),
        )
    };
    let base_prompt = right_codegen::generate_system_prompt(agent_name, &sandbox_mode, &home_dir);

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> =
        match run_or_delivery_shutdown(shutdown, internal_client.mcp_instructions(agent_name))
            .await?
        {
            Ok(resp) => {
                if resp.instructions.trim().len()
                    > right_codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER
                        .trim()
                        .len()
                {
                    Some(resp.instructions)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("delivery: failed to fetch MCP instructions: {e:#}");
                None
            }
        };

    // Delivery sessions skip memory injection — same rationale as cron jobs.
    let memory_mode: Option<crate::cc::prompt::MemoryMode> = None;

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let mut assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Normal,
            "/sandbox",
            "/tmp/right-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = agent_dir.join(".claude").join("delivery-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Normal,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        let cc_bin = which::which("claude")
            .or_else(|_| which::which("claude-bun"))
            .map_err(|_| "claude binary not found in PATH".to_string())?;
        let _ = cc_bin; // Existence check only — bash -c runs the script
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child =
        right_process::ProcessGroupChild::spawn(cmd).map_err(|e| format!("spawn failed: {e:#}"))?;

    if let Some(mut stdin) = child.stdin() {
        use tokio::io::AsyncWriteExt;
        run_or_delivery_shutdown(shutdown, stdin.write_all(yaml_input.as_bytes()))
            .await?
            .map_err(|e| format!("stdin write: {e:#}"))?;
    }

    const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
    let output = run_or_delivery_shutdown(
        shutdown,
        tokio::time::timeout(DELIVERY_TIMEOUT, child.wait_with_output()),
    )
    .await?
    .map_err(|_| "delivery CC subprocess timed out after 120s".to_string())?
    .map_err(|e| format!("wait_with_output: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // CC writes errors to stdout (as JSON) when using --output-format json.
        // Log both streams so the actual error is visible.
        let detail = if !stderr.is_empty() {
            stderr.into_owned()
        } else if !stdout.is_empty() {
            // Truncate to avoid flooding logs with full JSON blobs
            stdout.chars().take(500).collect()
        } else {
            "(no output)".into()
        };
        return Err(format!("CC exited with {}: {detail}", output.status));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let (reply, _) = crate::cc::worker_reply::parse_reply_output(&raw)
        .map_err(|e| format!("reply parse: {e}"))?;

    let has_content = reply
        .content
        .as_deref()
        .is_some_and(|content| !content.trim().is_empty());
    let has_attachments = reply
        .attachments
        .as_ref()
        .is_some_and(|attachments| !attachments.is_empty());
    if !has_content && !has_attachments {
        return Err("empty delivery reply: no content or attachments".into());
    }

    let mut report = DeliverySendReport {
        text_messages_sent: 0,
        attachment_batches_sent: 0,
    };

    if let Some(ref content) = reply.content
        && !content.trim().is_empty()
    {
        use teloxide::prelude::Requester as _;
        use teloxide::types::{ChatId, MessageId, ThreadId};
        let html = crate::telegram::markdown::md_to_telegram_html(content);
        let parts = crate::telegram::markdown::split_html_message(&html);
        let chat_id = ChatId(target_chat_id);
        for part in &parts {
            let mut send = bot
                .send_message(chat_id, part)
                .parse_mode(teloxide::types::ParseMode::Html);
            if let Some(t) = target_thread_id {
                send = send.message_thread_id(ThreadId(MessageId(t as i32)));
            }
            if let Err(e) =
                run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, send).await?
            {
                tracing::warn!(
                    chat_id = target_chat_id,
                    "async delivery: HTML send failed, retrying plain: {e:#}"
                );
                let plain = strip_html_tags(part);
                let mut fallback = bot.send_message(chat_id, &plain);
                if let Some(t) = target_thread_id {
                    fallback = fallback.message_thread_id(ThreadId(MessageId(t as i32)));
                }
                if let Err(e2) =
                    run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, fallback)
                        .await?
                {
                    tracing::error!(
                        chat_id = target_chat_id,
                        "async delivery: plain text fallback also failed: {e2:#}"
                    );
                    return Err(format!(
                        "telegram text send failed; html: {e:#}; plain fallback: {e2:#}"
                    ));
                }
                report.text_messages_sent += 1;
            } else {
                report.text_messages_sent += 1;
            }
        }
    }

    if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
        && let Err(e) = run_telegram_request_with_shutdown(
            shutdown,
            report.total_sent() > 0,
            crate::telegram::attachments::send_attachments(
                atts,
                bot,
                teloxide::types::ChatId(target_chat_id),
                target_thread_id.unwrap_or(0),
                agent_dir,
                ssh_config_path,
                resolved_sandbox,
            ),
        )
        .await?
    {
        tracing::error!(
            chat_id = target_chat_id,
            "async delivery: attachment send failed: {e:#}"
        );
        return Err(format!("telegram attachment send failed: {e:#}"));
    } else if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
    {
        report.attachment_batches_sent += 1;
    }

    ensure_delivery_send_report_non_empty(report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_mode_shutdown_flush_skips_idle_gate() {
        assert!(should_wait_for_idle(DeliveryMode::Normal, 10));
        assert!(!should_wait_for_idle(DeliveryMode::ShutdownFlush, 10));
    }

    #[tokio::test]
    async fn shutdown_deadline_bounds_single_delivery_attempt() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        let result = run_or_delivery_shutdown(
            DeliveryShutdownControl {
                token: None,
                deadline: Some(deadline),
            },
            async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                true
            },
        )
        .await;

        assert_eq!(
            result.unwrap_err(),
            DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
        );
    }

    #[tokio::test]
    async fn delivery_shutdown_cancels_pending_future() {
        let shutdown = tokio_util::sync::CancellationToken::new();
        shutdown.cancel();

        let result = run_or_delivery_shutdown(
            DeliveryShutdownControl {
                token: Some(&shutdown),
                deadline: None,
            },
            async { true },
        )
        .await;

        assert_eq!(
            result.unwrap_err(),
            DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
        );
    }

    #[tokio::test]
    async fn delivery_without_shutdown_waits_for_future() {
        let result = run_or_delivery_shutdown(
            DeliveryShutdownControl {
                token: None,
                deadline: None,
            },
            async { true },
        )
        .await;

        assert_eq!(result, Ok(true));
    }

    #[test]
    fn delivery_shutdown_interruption_is_not_retry_failure() {
        assert!(is_delivery_shutdown_interruption(
            DELIVERY_INTERRUPTED_BY_SHUTDOWN
        ));
        assert!(!is_delivery_shutdown_interruption(
            "stdin write: broken pipe"
        ));
    }

    #[tokio::test]
    async fn shutdown_bounded_telegram_send_timeout_is_terminal_not_retryable() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        let result = run_telegram_request_with_shutdown(
            DeliveryShutdownControl {
                token: None,
                deadline: Some(deadline),
            },
            false,
            async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Ok::<_, String>(())
            },
        )
        .await;

        let error = result.unwrap_err();
        assert!(is_delivery_terminal_shutdown_send_error(&error));
        assert!(!is_delivery_shutdown_interruption(&error));
    }

    #[tokio::test]
    async fn expired_shutdown_deadline_does_not_start_fresh_telegram_send() {
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let was_polled = Arc::clone(&polled);
        let deadline = tokio::time::Instant::now() - std::time::Duration::from_millis(1);

        let result = run_telegram_request_with_shutdown(
            DeliveryShutdownControl {
                token: None,
                deadline: Some(deadline),
            },
            false,
            async move {
                was_polled.store(true, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<Result<(), String>>().await
            },
        )
        .await;

        assert_eq!(
            result.unwrap_err(),
            DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()
        );
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancelled_shutdown_after_prior_send_is_terminal_without_new_send_poll() {
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let was_polled = Arc::clone(&polled);
        let shutdown = tokio_util::sync::CancellationToken::new();
        shutdown.cancel();

        let result = run_telegram_request_with_shutdown(
            DeliveryShutdownControl {
                token: Some(&shutdown),
                deadline: None,
            },
            true,
            async move {
                was_polled.store(true, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<Result<(), String>>().await
            },
        )
        .await;

        assert!(is_delivery_terminal_shutdown_send_error(
            &result.unwrap_err()
        ));
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    }

    fn setup_db() -> (tempfile::TempDir, right_db::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).unwrap();
        (dir, conn)
    }

    #[derive(Clone, Copy)]
    struct TestCronRun {
        id: &'static str,
        job_name: &'static str,
        started_at: &'static str,
        finished_at: Option<&'static str>,
        status: &'static str,
        log_path: &'static str,
        run_note: Option<&'static str>,
        delivery_json: Option<&'static str>,
        delivered_at: Option<&'static str>,
        delivery_status: Option<&'static str>,
        delivery_required: Option<bool>,
        target_chat_id: Option<i64>,
        target_thread_id: Option<i64>,
    }

    impl Default for TestCronRun {
        fn default() -> Self {
            Self {
                id: "run-1",
                job_name: "job1",
                started_at: "2026-01-01T00:00:00Z",
                finished_at: Some("2026-01-01T00:01:00Z"),
                status: "success",
                log_path: "/log",
                run_note: Some("sum"),
                delivery_json: None,
                delivered_at: None,
                delivery_status: None,
                delivery_required: None,
                target_chat_id: None,
                target_thread_id: None,
            }
        }
    }

    fn insert_async_cron_run(conn: &right_db::Connection, run: TestCronRun) {
        let delivery_required = run.delivery_required.unwrap_or(run.delivery_json.is_some());
        let delivery_status =
            run.delivery_status
                .unwrap_or(if delivery_required { "pending" } else { "none" });
        let updated_at = run.finished_at.unwrap_or(run.started_at);
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
                status, started_at, finished_at, log_path, run_note, delivery_json,
                delivery_required, delivery_status, delivered_at, created_at, updated_at
             ) VALUES (
                ?1, 'cron', ?2, ?1, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?6, ?14
             )",
            right_db::params![
                run.id,
                run.job_name,
                run.target_chat_id.unwrap_or(0),
                run.target_thread_id,
                run.status,
                run.started_at,
                run.finished_at,
                run.log_path,
                run.run_note,
                run.delivery_json,
                if delivery_required { 1 } else { 0 },
                delivery_status,
                run.delivered_at,
                updated_at,
            ],
        )
        .unwrap();
    }

    fn insert_async_background_run(
        conn: &right_db::Connection,
        id: &str,
        started_at: &str,
        status: &str,
        delivery_json: &str,
        target_chat_id: i64,
    ) {
        let finished_at = (status == "success" || status == "failed").then_some(started_at);
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
                status, started_at, finished_at, log_path, run_note, delivery_json,
                delivery_required, delivery_status, delivered_at, created_at, updated_at
             ) VALUES (
                ?1, 'background', NULL, ?1, ?2, NULL,
                ?3, ?4, ?5, '/log', 'summary', ?6,
                1, 'pending', NULL, ?4, ?4
             )",
            right_db::params![
                id,
                target_chat_id,
                status,
                started_at,
                finished_at,
                delivery_json,
            ],
        )
        .unwrap();
    }

    #[test]
    fn fetch_pending_empty_db() {
        let (_dir, conn) = setup_db();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_returns_oldest() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"first\"}"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                run_note: Some("sum2"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"second\"}"),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "a", "should return oldest first");
    }

    #[test]
    fn fetch_next_pending_skips_in_memory_delivered_oldest() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"first\"}"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                run_note: Some("sum2"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"second\"}"),
                ..Default::default()
            },
        );
        let delivered_in_memory = HashSet::from(["a".to_string()]);

        let pending = fetch_next_pending(&conn, &delivered_in_memory)
            .unwrap()
            .unwrap();
        assert_eq!(pending.id, "b");
    }

    #[test]
    fn fetch_pending_skips_null_delivery() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("silent"),
                ..Default::default()
            },
        );
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_ignores_silent_delivery_decision() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "silent",
                run_note: Some("no changes"),
                delivery_json: Some("{\"kind\":\"silent\",\"reason\":\"No changes\"}"),
                delivery_required: Some(false),
                delivery_status: Some("none"),
                ..Default::default()
            },
        );
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_skips_delivered() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"done\"}"),
                delivered_at: Some("2026-01-01T00:10:00Z"),
                delivery_status: Some("delivered"),
                ..Default::default()
            },
        );
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_reads_async_runs_and_skips_none_delivery() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "silent",
                delivery_json: Some("{\"kind\":\"silent\",\"reason\":\"quiet\"}"),
                delivery_required: Some(false),
                delivery_status: Some("none"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "pending",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"deliver\"}"),
                ..Default::default()
            },
        );

        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "pending");
        assert_eq!(pending.kind, "cron");
        assert_eq!(pending.producer_ref.as_deref(), Some("job1"));
    }

    #[test]
    fn deduplicate_keeps_latest_marks_older() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                run_note: Some("sum2"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"new\"}"),
                ..Default::default()
            },
        );
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        let delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM async_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(delivered.is_some());
        let not_delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM async_runs WHERE id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(not_delivered.is_none());
    }

    #[test]
    fn deduplicate_does_not_touch_other_jobs() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                job_name: "job2",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"y\"}"),
                ..Default::default()
            },
        );
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "a");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn deduplicate_sets_superseded_status() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                delivery_status: Some("pending"),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                run_note: Some("sum2"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"new\"}"),
                delivery_status: Some("pending"),
                ..Default::default()
            },
        );
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);

        let status: Option<String> = conn
            .query_row(
                "SELECT delivery_status FROM async_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status.as_deref(), Some("superseded"));
    }

    #[test]
    fn format_async_yaml_basic_cron() {
        let pending = PendingAsyncResult {
            id: "abc".into(),
            kind: "cron".into(),
            producer_ref: Some("health-check".into()),
            delivery_json: r#"{"kind":"notify","content":"BTC up 2%"}"#.into(),
            run_note: "Checked 5 pairs".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let output = format_async_yaml(&pending, 2).unwrap();
        // Instruction prefix assertions
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("VERBATIM"));
        assert!(output.contains("attachments are sent separately"));
        assert!(output.contains("Here is the YAML report of the cron job:"));
        // YAML content assertions
        assert!(output.contains("job: \"health-check\""));
        assert!(output.contains("runs_total: 3"));
        assert!(output.contains("skipped_runs: 2"));
        assert!(output.contains("BTC up 2%"));
        assert!(output.contains("Checked 5 pairs"));
    }

    #[test]
    fn format_async_yaml_no_skipped() {
        let pending = PendingAsyncResult {
            id: "abc".into(),
            kind: "cron".into(),
            producer_ref: Some("job1".into()),
            delivery_json: r#"{"kind":"notify","content":"hello"}"#.into(),
            run_note: "done".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let output = format_async_yaml(&pending, 0).unwrap();
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("runs_total: 1"));
        assert!(!output.contains("skipped_runs"));
    }

    #[test]
    fn format_async_yaml_uses_cron_failure_instruction_when_status_failed() {
        let pending = PendingAsyncResult {
            id: "r1".into(),
            kind: "cron".into(),
            producer_ref: Some("watcher".into()),
            delivery_json: r#"{"kind":"notify","content":"Partial data fetched then hit budget"}"#
                .into(),
            run_note: "failed".into(),
            status: "failed".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("did not complete successfully"));
        assert!(!out.contains("send it VERBATIM"));
    }

    #[test]
    fn format_async_yaml_uses_cron_success_instruction_when_status_success() {
        let pending = PendingAsyncResult {
            id: "r2".into(),
            kind: "cron".into(),
            producer_ref: Some("watcher".into()),
            delivery_json: r#"{"kind":"notify","content":"BTC up 2%"}"#.into(),
            run_note: "ok".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("VERBATIM"));
    }

    #[test]
    fn format_async_yaml_rejects_silent_delivery_json() {
        let pending = PendingAsyncResult {
            id: "r-silent".into(),
            kind: "cron".into(),
            producer_ref: Some("watcher".into()),
            delivery_json: r#"{"kind":"silent","reason":"No changes"}"#.into(),
            run_note: "quiet".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let err = format_async_yaml(&pending, 0).unwrap_err();
        assert!(
            err.to_string().contains("not a notify decision"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn format_async_yaml_background_includes_background_instruction_and_content() {
        let pending = PendingAsyncResult {
            id: "bg-1".into(),
            kind: "background".into(),
            producer_ref: None,
            delivery_json: r#"{"kind":"notify","content":"Finished the answer in background"}"#
                .into(),
            run_note: "background summary".into(),
            status: "success".into(),
            target_chat_id: Some(-100),
            target_thread_id: None,
        };

        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.starts_with("You are delivering a background task result"));
        assert!(out.contains("background_result:"));
        assert!(out.contains("label: \"background\""));
        assert!(out.contains("Finished the answer in background"));
        assert!(!out.contains("cron_result:"));
    }

    #[test]
    fn format_async_yaml_background_uses_failure_instruction_when_status_failed() {
        let pending = PendingAsyncResult {
            id: "bg-2".into(),
            kind: "background".into(),
            producer_ref: Some("custom-bg".into()),
            delivery_json: r#"{"kind":"notify","content":"Background work failed"}"#.into(),
            run_note: "background failed".into(),
            status: "failed".into(),
            target_chat_id: Some(-100),
            target_thread_id: None,
        };

        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("background task below did not complete successfully"));
        assert!(out.contains("label: \"custom-bg\""));
        assert!(out.contains("Background work failed"));
    }

    #[test]
    fn delivery_invocation_uses_configured_agent_model() {
        let args = build_delivery_invocation_args(
            "/sandbox/mcp.json".into(),
            r#"{"type":"object"}"#.into(),
            Some("claude-opus-4-7[1m]".into()),
            Some("session-1".into()),
            None,
        );

        let model_pos = args
            .iter()
            .position(|arg| arg == "--model")
            .expect("configured model must be passed to Claude");
        assert_eq!(args[model_pos + 1], "claude-opus-4-7[1m]");
        assert!(
            !args.iter().any(|arg| arg == "claude-haiku-4-5-20251001"),
            "delivery must not override the configured agent model with Haiku"
        );
    }

    #[test]
    fn fetch_pending_resolves_target_after_spec_deletion() {
        // Reproduces the production bug: a one-shot spec auto-deletes after
        // firing, but the run row still needs to know where to deliver.
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();

        // 1. Spec is created (recurring=0, one-shot).
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('one-shot', '*/5 * * * *', 'p', 1.0, 0, -4996137249, NULL, ?1, ?1)",
            [&now],
        ).unwrap();

        // 2. Run row inserted with snapshot of target (what new execute_job does).
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "run-1",
                job_name: "one-shot",
                started_at: "2026-05-05T12:36:00Z",
                finished_at: Some("2026-05-05T12:41:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-4996137249),
                ..Default::default()
            },
        );

        // 3. Spec is auto-deleted (one-shot completion).
        conn.execute("DELETE FROM cron_specs WHERE job_name = 'one-shot'", [])
            .unwrap();

        // 4. Delivery loop fetches — must still find the target.
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-4996137249));
        assert_eq!(pending.target_thread_id, None);

        // And dedup must agree.
        let (latest, _skipped) = deduplicate_job(&conn, "one-shot").unwrap().unwrap();
        assert_eq!(latest.target_chat_id, Some(-4996137249));
    }

    #[test]
    fn fetch_pending_carries_target_fields() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-555),
                target_thread_id: Some(9),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-555));
        assert_eq!(pending.target_thread_id, Some(9));
    }

    #[test]
    fn fetch_pending_returns_none_target_when_run_has_none() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "legacy",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert!(pending.target_chat_id.is_none());
        assert!(pending.target_thread_id.is_none());
    }

    #[test]
    fn null_target_classifies_as_no_target() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "legacy",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[]));
        assert!(
            matches!(outcome, TargetClassification::NoTarget),
            "got: {outcome:?}"
        );
    }

    #[test]
    fn target_chat_id_zero_fetches_as_no_target() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "zero-target",
                job_name: "targetless",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(0),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, None);

        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[0]));
        assert!(
            matches!(outcome, TargetClassification::NoTarget),
            "got: {outcome:?}"
        );
    }

    #[test]
    fn target_not_in_allowlist_classifies_as_denied() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "agenda",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-777),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[100], &[-200]));
        assert!(
            matches!(outcome, TargetClassification::Denied),
            "got: {outcome:?}"
        );
    }

    #[test]
    fn target_in_allowlist_classifies_as_ready() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "agenda",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-200),
                target_thread_id: Some(5),
                ..Default::default()
            },
        );
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[-200]));
        assert!(
            matches!(
                outcome,
                TargetClassification::Ready {
                    chat_id: -200,
                    thread_id: Some(5)
                }
            ),
            "got: {outcome:?}"
        );
    }

    fn fake_allowlist(
        users: &[i64],
        groups: &[i64],
    ) -> right_agent::agent::allowlist::AllowlistState {
        use right_agent::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistState};
        let now = chrono::Utc::now();
        let mut state = AllowlistState::default();
        for &id in users {
            state.add_user(AllowedUser {
                id,
                label: None,
                added_by: None,
                added_at: now,
            });
        }
        for &id in groups {
            state.add_group(AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
            });
        }
        state
    }

    #[test]
    fn deduplicate_job_carries_target_fields() {
        let (_dir, conn) = setup_db();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-100),
                ..Default::default()
            },
        );
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                run_note: Some("sum2"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"y\"}"),
                target_chat_id: Some(-100),
                ..Default::default()
            },
        );
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        assert_eq!(latest.target_chat_id, Some(-100));
    }

    #[test]
    fn select_delivery_candidate_does_not_deduplicate_background_rows() {
        let (_dir, conn) = setup_db();
        insert_async_background_run(
            &conn,
            "bg-old",
            "2026-01-01T00:00:00Z",
            "success",
            "{\"kind\":\"notify\",\"content\":\"old\"}",
            -100,
        );
        insert_async_background_run(
            &conn,
            "bg-new",
            "2026-01-01T00:05:00Z",
            "success",
            "{\"kind\":\"notify\",\"content\":\"new\"}",
            -100,
        );

        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "bg-old");
        let (selected, skipped) = select_delivery_candidate(&conn, pending).unwrap().unwrap();
        assert_eq!(selected.id, "bg-old");
        assert_eq!(skipped, 0);

        let statuses = conn
            .prepare("SELECT id, delivery_status FROM async_runs ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            statuses,
            vec![
                ("bg-new".to_string(), "pending".to_string()),
                ("bg-old".to_string(), "pending".to_string()),
            ]
        );
    }

    #[test]
    fn empty_delivery_send_report_is_rejected() {
        let report = DeliverySendReport {
            text_messages_sent: 0,
            attachment_batches_sent: 0,
        };

        let err = ensure_delivery_send_report_non_empty(report).unwrap_err();
        assert!(err.contains("empty delivery reply"));
    }
}
