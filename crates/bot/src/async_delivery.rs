use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use right_db::OptionalExtension as _;

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
    pub force_notify: bool,
}

/// Query the oldest undelivered async result with a non-null delivery_json.
#[cfg(test)]
pub(crate) async fn fetch_pending(
    conn: &right_db::Connection,
) -> Result<Option<PendingAsyncResult>, right_db::DbError> {
    Ok(fetch_pending_batch(conn, 1).await?.into_iter().next())
}

async fn fetch_pending_batch(
    conn: &right_db::Connection,
    limit: usize,
) -> Result<Vec<PendingAsyncResult>, right_db::DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                NULLIF(target_chat_id, 0), target_thread_id, force_notify \
         FROM async_runs \
         WHERE delivery_required = 1 \
           AND delivery_status IN ('pending', 'retryable') \
           AND status IN ('success', 'failed') \
           AND delivery_json IS NOT NULL \
         ORDER BY finished_at ASC \
         LIMIT ?1",
    )?;
    stmt.query_map(right_db::params![limit.max(1) as i64], pending_from_row)
        .await?
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
        force_notify: row.get::<_, i64>(8)? != 0,
    })
}

pub(crate) async fn fetch_next_pending(
    conn: &right_db::Connection,
    delivered_in_memory: &HashSet<String>,
) -> Result<Option<PendingAsyncResult>, right_db::DbError> {
    let limit = PENDING_FETCH_BATCH_SIZE.max(delivered_in_memory.len().saturating_add(1));
    let batch = fetch_pending_batch(conn, limit).await?;
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
        if let Err(e) = mark_delivery_outcome(conn, &id, "delivered").await {
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
async fn mark_delivery_outcome(
    conn: &right_db::Connection,
    run_id: &str,
    status: &str,
) -> Result<(), right_db::DbError> {
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE async_runs \
         SET delivery_status = ?1, delivered_at = ?2, updated_at = ?2 \
        WHERE id = ?3",
            right_db::params![status, now, run_id],
        )
        .await?;
    if rows == 0 {
        return Err(right_db::DbError::NotFound);
    }
    Ok(())
}

/// Deduplicate: for a given job, find the latest undelivered result and mark all
/// older undelivered results as delivered. Returns (latest_result, skipped_count).
pub(crate) async fn deduplicate_job(
    conn: &right_db::Connection,
    producer_ref: &str,
) -> Result<Option<(PendingAsyncResult, u32)>, right_db::DbError> {
    // The candidate's `force_notify` is the OR across all undelivered runs of
    // this job, not just the latest row's value. Otherwise a forced run would
    // lose its idle-gate bypass whenever a later non-forced scheduled run wins
    // the `finished_at DESC` tie-break — the forced verification report the user
    // explicitly requested would be silently held behind the idle gate.
    let latest = conn
        .query_row(
            "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                    NULLIF(target_chat_id, 0), target_thread_id, \
                    COALESCE(( \
                        SELECT MAX(force_notify) FROM async_runs a2 \
                        WHERE a2.kind = 'cron' \
                          AND a2.producer_ref = ?1 \
                          AND a2.delivery_required = 1 \
                          AND a2.delivery_status IN ('pending', 'retryable') \
                          AND a2.status IN ('success', 'failed') \
                          AND a2.delivery_json IS NOT NULL \
                    ), 0) \
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
        .await
        .optional()?;

    let Some(latest) = latest else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn
        .execute(
            "UPDATE async_runs \
         SET delivered_at = ?1, delivery_status = 'superseded', updated_at = ?1 \
         WHERE kind = 'cron' \
           AND producer_ref = ?2 \
           AND id != ?3 \
           AND delivery_required = 1 \
           AND delivery_status IN ('pending', 'retryable') \
           AND status IN ('success', 'failed')",
            right_db::params![now, producer_ref, &latest.id],
        )
        .await?;

    Ok(Some((latest, count as u32)))
}

pub(crate) async fn select_delivery_candidate(
    conn: &right_db::Connection,
    pending: PendingAsyncResult,
) -> Result<Option<(PendingAsyncResult, u32)>, right_db::DbError> {
    if pending.kind == "cron"
        && let Some(producer_ref) = pending.producer_ref.as_deref()
    {
        return deduplicate_job(conn, producer_ref).await;
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
The `content` field below is the FINAL user-facing message — place it VERBATIM in your reply's `content` field.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Do NOT call `mcp__right__send_message` or any other send/notify tool. The platform delivers your reply to Telegram itself — writing `content` (and optionally `attachments`) is the only way to reach the user.
Re-emit any attachments from the report in your reply's `attachments` array. `content` and an attachment `caption` are delivered as SEPARATE messages — never repeat the content text in a caption.

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
The `content` field below is the FINAL user-facing message - place it VERBATIM in your reply's `content` field.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Do NOT call `mcp__right__send_message` or any other send/notify tool. The platform delivers your reply to Telegram itself - writing `content` (and optionally `attachments`) is the only way to reach the user.
Re-emit any attachments from the report in your reply's `attachments` array. `content` and an attachment `caption` are delivered as SEPARATE messages - never repeat the content text in a caption.

Here is the YAML report of the background task:
";

const BACKGROUND_DELIVERY_INSTRUCTION_FAILURE: &str = "\
The background task below did not complete successfully. The `content` field contains
a platform-generated summary of the failure. Relay it to the user in natural prose -
you MAY rephrase lightly for flow with the recent conversation, but keep all factual
claims intact. Do not invent details. Ignore the attachments field.

Here is the YAML report of the background task:
";

/// Platform-rendered status line prepended to a delivered async result.
/// HTML (matches the delivery send path, which uses `ParseMode::Html`).
/// Deterministic from the run row — never produced by the relay model.
pub(crate) fn render_delivery_header(pending: &PendingAsyncResult) -> String {
    let glyph = if pending.status == "failed" {
        "✗"
    } else {
        "✓"
    };
    // Background runs always carry `producer_ref = "background"` (a raw slug);
    // present the friendly label instead. Cron runs use the spec name as-is,
    // falling back to "cron" only if absent.
    let label = match pending.producer_ref.as_deref() {
        _ if pending.kind == "background" => "background task",
        Some(name) => name,
        None => "cron",
    };
    let label = crate::cc::markdown_utils::html_escape(label);
    let status_word = if pending.status == "failed" {
        "failed"
    } else {
        "success"
    };
    if pending.force_notify {
        format!("{glyph} <b>{label}</b> · manual run · {status_word}")
    } else {
        format!("{glyph} <b>{label}</b> · {status_word}")
    }
}

/// Join a platform header above the relayed body. Header is already HTML;
/// body is the relay model's content (also HTML at the send site).
pub(crate) fn prepend_delivery_header(header: &str, body: &str) -> String {
    if body.trim().is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{body}")
    }
}

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
const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const DELIVERY_TIMEOUT_ERROR: &str = "delivery CC subprocess timed out after 120s";

#[derive(Debug, Clone, Copy)]
struct DeliveryShutdownControl<'a> {
    token: Option<&'a tokio_util::sync::CancellationToken>,
    deadline: Option<tokio::time::Instant>,
}

#[derive(Debug, Clone, Copy)]
struct DeliveryDeadlineControl<'a> {
    shutdown: DeliveryShutdownControl<'a>,
    deadline_error: &'static str,
}

fn delivery_subprocess_control<'a>(
    shutdown: DeliveryShutdownControl<'a>,
    delivery_deadline: tokio::time::Instant,
) -> DeliveryDeadlineControl<'a> {
    match shutdown.deadline {
        Some(caller_deadline) if caller_deadline <= delivery_deadline => DeliveryDeadlineControl {
            shutdown,
            deadline_error: DELIVERY_INTERRUPTED_BY_SHUTDOWN,
        },
        _ => DeliveryDeadlineControl {
            shutdown: DeliveryShutdownControl {
                token: shutdown.token,
                deadline: Some(delivery_deadline),
            },
            deadline_error: DELIVERY_TIMEOUT_ERROR,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMode {
    Normal,
    ShutdownFlush,
}

fn should_wait_for_idle(mode: DeliveryMode, idle_for: i64) -> bool {
    mode == DeliveryMode::Normal && idle_for < IDLE_THRESHOLD_SECS
}

/// Whether a pending result must wait before delivery. Force-notify runs are
/// never held — they bypass the idle gate so a forced verification result lands
/// promptly.
fn should_hold_delivery(force_notify: bool, mode: DeliveryMode, idle_for: i64) -> bool {
    !force_notify && should_wait_for_idle(mode, idle_for)
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
    sandbox: Option<&crate::sandbox::Sandbox>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    upgrade_lock: &Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: &Arc<std::sync::atomic::AtomicBool>,
    shutdown: DeliveryShutdownControl<'_>,
) -> bool {
    let pending = match fetch_next_pending(conn, &state.delivered_in_memory).await {
        Ok(Some(p)) => p,
        Ok(None) => return false,
        Err(e) => {
            tracing::error!("async delivery: fetch_next_pending failed: {e:#}");
            return false;
        }
    };

    let (to_deliver, skipped) = match select_delivery_candidate(conn, pending).await {
        Ok(Some((result, s))) => (result, s),
        Ok(None) => return false,
        Err(e) => {
            tracing::error!("async delivery: candidate selection failed: {e:#}");
            return false;
        }
    };

    // Idle gate runs against the row that will actually be delivered. For cron,
    // `select_delivery_candidate` may return a newer run than the one fetched
    // (dedup keeps the latest), so a force-notify newer run must override the
    // gate even when the oldest pending row is non-forced.
    let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
    let now = chrono::Utc::now().timestamp();
    let idle_for = now - last;
    if should_hold_delivery(to_deliver.force_notify, mode, idle_for) {
        let wait = IDLE_THRESHOLD_SECS - idle_for;
        tracing::info!(
            kind = %to_deliver.kind,
            producer_ref = ?to_deliver.producer_ref,
            run_id = %to_deliver.id,
            idle_secs = idle_for,
            wait_secs = wait,
            "async delivery: result pending, waiting for chat idle ({IDLE_THRESHOLD_SECS}s)"
        );
        return false;
    }

    if state.delivered_in_memory.contains(&to_deliver.id) {
        if mark_delivery_outcome(conn, &to_deliver.id, "delivered")
            .await
            .is_ok()
        {
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
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "no_target").await {
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
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "denied").await {
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
    )
    .await
    {
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
            if let Err(db_err) = mark_delivery_outcome(conn, &to_deliver.id, "failed").await {
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

    let header = render_delivery_header(&to_deliver);

    match deliver_through_session(
        &yaml,
        &header,
        agent_dir,
        agent_name,
        bot,
        target_chat_id,
        target_thread_id,
        sandbox,
        crate::snapshot_model(model),
        session_id,
        internal_client,
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
            if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "delivered").await {
                tracing::error!(run_id = %to_deliver.id, "delivery DB update failed: {e:#}");
                state.delivered_in_memory.insert(to_deliver.id.clone());
            }
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
                if let Err(mark_err) = mark_delivery_outcome(conn, &to_deliver.id, "failed").await {
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
                if let Err(e) = mark_delivery_outcome(conn, &to_deliver.id, "failed").await {
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
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: std::sync::Arc<right_mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    tracing::info!(agent = %agent_name, "async delivery loop started");

    let mut conn = match right_db::open_connection(&agent_dir, false).await {
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

        // Resolved per poll: the loop outlives any one sandbox handle, and a
        // recovery between polls retires the previous one.
        let sandbox = sandbox_runtime.current_sandbox();

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
            sandbox.as_ref(),
            &internal_client,
            &upgrade_lock,
            &session_locks,
            &debug,
            DeliveryShutdownControl {
                token: Some(&shutdown),
                deadline: None,
            },
        )
        .await;
    }
}

async fn run_or_delivery_deadline<T>(
    control: DeliveryDeadlineControl<'_>,
    future: impl Future<Output = T>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = async {
            if let Some(token) = control.shutdown.token {
                token.cancelled().await;
            }
        }, if control.shutdown.token.is_some() => Err(DELIVERY_INTERRUPTED_BY_SHUTDOWN.to_owned()),
        _ = async {
            if let Some(deadline) = control.shutdown.deadline {
                tokio::time::sleep_until(deadline).await;
            }
        }, if control.shutdown.deadline.is_some() => Err(control.deadline_error.to_owned()),
        result = future => Ok(result),
    }
}

async fn run_or_delivery_shutdown<T>(
    control: DeliveryShutdownControl<'_>,
    future: impl Future<Output = T>,
) -> Result<T, String> {
    run_or_delivery_deadline(
        DeliveryDeadlineControl {
            shutdown: control,
            deadline_error: DELIVERY_INTERRUPTED_BY_SHUTDOWN,
        },
        future,
    )
    .await
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
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut conn = match right_db::open_connection(&agent_dir, false).await {
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
        let sandbox = sandbox_runtime.current_sandbox();
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
            sandbox.as_ref(),
            &internal_client,
            &upgrade_lock,
            &session_locks,
            &debug,
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

/// Decide whether a failed attachment batch should fail the whole delivery.
///
/// Retrying a delivery re-invokes the session and re-sends any *body* content
/// that already reached Telegram, so once at least one body message has been
/// sent the delivery MUST NOT be retried — the failed attachment is dropped
/// (and logged) instead, preventing duplicate posts. An attachment-only
/// delivery (no body content sent yet) remains retryable: the user's payload
/// never reached them, so a retry must run.
///
/// The platform status header is deliberately EXCLUDED from this count: on an
/// attachments-only delivery the standalone header send must not flip an
/// attachment failure to non-fatal. Re-sending the one-line header on retry is
/// cheap; losing the attachment is not.
fn attachment_failure_is_fatal(body_messages_sent: usize) -> bool {
    body_messages_sent == 0
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
        // baseline so the relay can't self-loop or escape via harness
        // orchestration tools (Cron*, ScheduleWakeup, plan mode, etc.).
        disallowed_tools: crate::cc::invocation::disallow_channel_post(
            crate::cc::invocation::disallow_foreground_only_tools(
                crate::cc::invocation::baseline_disallowed_tools(),
            ),
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
    header: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    sandbox: Option<&crate::sandbox::Sandbox>,
    configured_model: Option<String>,
    session_id: Option<String>,
    internal_client: &right_mcp::internal_client::InternalClient,
    upgrade_lock: &tokio::sync::RwLock<()>,
    session_locks: crate::telegram::SessionLocks,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown: DeliveryShutdownControl<'_>,
) -> Result<DeliverySendReport, String> {
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

    let mcp_path = crate::sandbox::SANDBOX_MCP_JSON_PATH.to_owned();

    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    let json_schema = std::fs::read_to_string(&reply_schema_path).unwrap_or_default();

    let claude_args = build_delivery_invocation_args(
        mcp_path,
        json_schema,
        configured_model,
        session_id,
        Some(debug),
    );

    let base_prompt = right_codegen::generate_system_prompt(agent_name, "/sandbox");

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

    let sandbox = crate::cc::invocation::guard_no_sandboxed_host_exec(agent_name, sandbox)
        .map_err(|e| format!("{e:#}"))?;

    let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
        &base_prompt,
        crate::cc::prompt::PromptMode::Normal,
        "/sandbox",
        &crate::cc::prompt::sandbox_prompt_file_path("system-prompt"),
        "/sandbox",
        &claude_args,
        mcp_instructions.as_deref(),
        memory_mode.as_ref(),
        None,
        None,
        None,
    );

    let mut child =
        crate::cc::invocation::build_claude_script_command(assembly_script, agent_dir, sandbox)
            .await
            .stdin_piped()
            .stdout(crate::cc::sandbox_process::Capture::Pipe)
            .stderr(crate::cc::sandbox_process::Capture::Pipe)
            .spawn()
            .await
            .map_err(|e| format!("spawn failed: {e:#}"))?;

    let delivery =
        delivery_subprocess_control(shutdown, tokio::time::Instant::now() + DELIVERY_TIMEOUT);
    if let Some(mut stdin) = child.stdin() {
        run_or_delivery_deadline(delivery, stdin.write_all(yaml_input.as_bytes()))
            .await?
            .map_err(|e| format!("stdin write: {e:#}"))?;
        run_or_delivery_deadline(delivery, stdin.close())
            .await?
            .map_err(|e| format!("stdin close: {e:#}"))?;
    }

    // Any deadline error returns from this function and drops `child`; the
    // SandboxChild drop contract kills the still-running guest process.
    // Break on the terminal JSON envelope and kill the guest, never waiting
    // for EOF — the SDK may not report an exit after a stdin-piped resume.
    let output = run_or_delivery_deadline(delivery, child.wait_for_json_envelope())
        .await?
        .map_err(|e| format!("wait_for_json_envelope: {e:#}"))?;

    if !output.success() {
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
        return Err(format!(
            "CC delivery failed (code {}): {detail}",
            output.code
        ));
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
    // Body-content messages only — excludes the standalone platform header below.
    // Drives the attachment-failure fatality decision so the header send can't
    // turn an attachments-only failure non-fatal.
    let mut body_messages_sent = 0usize;

    if let Some(ref content) = reply.content
        && !content.trim().is_empty()
    {
        let html = crate::telegram::markdown::md_to_telegram_html(content);
        let html = prepend_delivery_header(header, &html);
        let parts = crate::telegram::markdown::split_html_message(&html);
        let thread = target_thread_id.map(|t| t as i32);
        for part in &parts {
            let send = bot.send_message_opts(target_chat_id, part, true, thread, None, None);
            if let Err(e) =
                run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, send).await?
            {
                tracing::warn!(
                    chat_id = target_chat_id,
                    "async delivery: HTML send failed, retrying plain: {e:#}"
                );
                let plain = strip_html_tags(part);
                let fallback =
                    bot.send_message_opts(target_chat_id, &plain, false, thread, None, None);
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
                body_messages_sent += 1;
            } else {
                report.text_messages_sent += 1;
                body_messages_sent += 1;
            }
        }
    }

    // Attachments-only delivery (no text content): the header would otherwise be
    // lost, so send it as a standalone text message before the attachment batch.
    if !has_content && has_attachments {
        let thread = target_thread_id.map(|t| t as i32);
        let send = bot.send_message_opts(target_chat_id, header, true, thread, None, None);
        if let Err(e) =
            run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, send).await?
        {
            // Degrade gracefully like the body path: don't let a header HTML
            // rejection sink an otherwise-deliverable attachments-only payload.
            tracing::warn!(
                chat_id = target_chat_id,
                "async delivery: header HTML send failed, retrying plain: {e:#}"
            );
            let plain = strip_html_tags(header);
            let fallback = bot.send_message_opts(target_chat_id, &plain, false, thread, None, None);
            run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, fallback)
                .await?
                .map_err(|e2| format!("telegram header send failed; html: {e:#}; plain: {e2:#}"))?;
        }
        report.text_messages_sent += 1;
    }

    if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
    {
        let send_result = run_telegram_request_with_shutdown(
            shutdown,
            report.total_sent() > 0,
            crate::telegram::attachments::send_attachments(
                atts,
                bot,
                target_chat_id,
                target_thread_id.unwrap_or(0),
                agent_dir,
                sandbox,
            ),
        )
        .await?;
        match send_result {
            Ok(()) => report.attachment_batches_sent += 1,
            Err(e) if attachment_failure_is_fatal(body_messages_sent) => {
                tracing::error!(
                    chat_id = target_chat_id,
                    "async delivery: attachment send failed: {e:#}"
                );
                return Err(format!("telegram attachment send failed: {e:#}"));
            }
            Err(e) => {
                // The text content already reached Telegram. Failing here would
                // requeue the whole delivery and re-send that text on the next
                // attempt, duplicating the post. Drop the failed attachment and
                // treat the (text-only) delivery as done.
                tracing::error!(
                    chat_id = target_chat_id,
                    "async delivery: attachment send failed after text delivered; \
                     dropping attachment to avoid duplicate re-send: {e:#}"
                );
            }
        }
    }

    ensure_delivery_send_report_non_empty(report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_failure_fatal_only_when_no_body_sent() {
        // No body content reached the user yet → retry is safe and required.
        assert!(attachment_failure_is_fatal(0));
        // Body content already delivered → retry would duplicate it, so the
        // attachment failure must be tolerated, not fatal.
        assert!(!attachment_failure_is_fatal(1));
        assert!(!attachment_failure_is_fatal(3));
    }

    #[test]
    fn attachments_only_with_header_keeps_attachment_failure_fatal() {
        // Regression: an attachments-only delivery sends the one-line platform
        // status header standalone before the attachment batch. That header send
        // must NOT count as body content — otherwise an attachment failure would
        // flip non-fatal and the user's payload would be dropped with no retry.
        // The body-content count stays 0, so the failure remains fatal (requeue).
        let body_messages_sent_after_header_only = 0usize;
        assert!(attachment_failure_is_fatal(
            body_messages_sent_after_header_only
        ));
    }

    #[tokio::test]
    async fn delivery_mode_shutdown_flush_skips_idle_gate() {
        assert!(should_wait_for_idle(DeliveryMode::Normal, 10));
        assert!(!should_wait_for_idle(DeliveryMode::ShutdownFlush, 10));
    }

    #[test]
    fn subprocess_deadline_merges_with_caller_control_and_preserves_diagnostic() {
        let token = tokio_util::sync::CancellationToken::new();
        let now = tokio::time::Instant::now();
        let internal_deadline = now + DELIVERY_TIMEOUT;
        let caller_deadline = now + std::time::Duration::from_secs(10);
        let caller_control = DeliveryShutdownControl {
            token: Some(&token),
            deadline: Some(caller_deadline),
        };

        let caller_bounded = delivery_subprocess_control(caller_control, internal_deadline);
        assert_eq!(caller_bounded.shutdown.deadline, Some(caller_deadline));
        assert!(std::ptr::eq(caller_bounded.shutdown.token.unwrap(), &token));
        assert_eq!(
            caller_bounded.deadline_error,
            DELIVERY_INTERRUPTED_BY_SHUTDOWN
        );

        let delivery_bounded = delivery_subprocess_control(
            DeliveryShutdownControl {
                token: Some(&token),
                deadline: None,
            },
            internal_deadline,
        );
        assert_eq!(delivery_bounded.shutdown.deadline, Some(internal_deadline));
        assert!(std::ptr::eq(
            delivery_bounded.shutdown.token.unwrap(),
            &token
        ));
        assert_eq!(delivery_bounded.deadline_error, DELIVERY_TIMEOUT_ERROR);

        let later_caller = delivery_subprocess_control(
            DeliveryShutdownControl {
                token: None,
                deadline: Some(internal_deadline + std::time::Duration::from_secs(1)),
            },
            internal_deadline,
        );
        assert_eq!(later_caller.shutdown.deadline, Some(internal_deadline));
        assert_eq!(later_caller.deadline_error, DELIVERY_TIMEOUT_ERROR);
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

    #[tokio::test]
    async fn delivery_shutdown_interruption_is_not_retry_failure() {
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

    async fn setup_db() -> (tempfile::TempDir, right_db::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
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

    async fn insert_async_cron_run(conn: &right_db::Connection, run: TestCronRun) {
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
        .await
        .unwrap();
    }

    async fn insert_async_background_run(
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
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fetch_pending_empty_db() {
        let (_dir, conn) = setup_db().await;
        assert!(fetch_pending(&conn).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fetch_pending_returns_oldest() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"first\"}"),
                ..Default::default()
            },
        )
        .await;
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
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.id, "a", "should return oldest first");
    }

    #[tokio::test]
    async fn fetch_next_pending_skips_in_memory_delivered_oldest() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"first\"}"),
                ..Default::default()
            },
        )
        .await;
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
        )
        .await;
        let delivered_in_memory = HashSet::from(["a".to_string()]);

        let pending = fetch_next_pending(&conn, &delivered_in_memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.id, "b");
    }

    #[tokio::test]
    async fn fetch_pending_skips_null_delivery() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("silent"),
                ..Default::default()
            },
        )
        .await;
        assert!(fetch_pending(&conn).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fetch_pending_ignores_silent_delivery_decision() {
        let (_dir, conn) = setup_db().await;
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
        )
        .await;
        assert!(fetch_pending(&conn).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fetch_pending_skips_delivered() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"done\"}"),
                delivered_at: Some("2026-01-01T00:10:00Z"),
                delivery_status: Some("delivered"),
                ..Default::default()
            },
        )
        .await;
        assert!(fetch_pending(&conn).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fetch_pending_reads_async_runs_and_skips_none_delivery() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "silent",
                delivery_json: Some("{\"kind\":\"silent\",\"reason\":\"quiet\"}"),
                delivery_required: Some(false),
                delivery_status: Some("none"),
                ..Default::default()
            },
        )
        .await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "pending",
                started_at: "2026-01-01T00:05:00Z",
                finished_at: Some("2026-01-01T00:06:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"deliver\"}"),
                ..Default::default()
            },
        )
        .await;

        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.id, "pending");
        assert_eq!(pending.kind, "cron");
        assert_eq!(pending.producer_ref.as_deref(), Some("job1"));
    }

    #[tokio::test]
    async fn deduplicate_keeps_latest_marks_older() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                ..Default::default()
            },
        )
        .await;
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
        )
        .await;
        let (latest, skipped) = deduplicate_job(&conn, "job1").await.unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        let delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM async_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(delivered.is_some());
        let not_delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM async_runs WHERE id = 'b'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert!(not_delivered.is_none());
    }

    #[tokio::test]
    async fn deduplicate_does_not_touch_other_jobs() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        )
        .await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "b",
                job_name: "job2",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"y\"}"),
                ..Default::default()
            },
        )
        .await;
        let (latest, skipped) = deduplicate_job(&conn, "job1").await.unwrap().unwrap();
        assert_eq!(latest.id, "a");
        assert_eq!(skipped, 0);
    }

    #[tokio::test]
    async fn deduplicate_sets_superseded_status() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                delivery_status: Some("pending"),
                ..Default::default()
            },
        )
        .await;
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
        )
        .await;
        let (latest, skipped) = deduplicate_job(&conn, "job1").await.unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);

        let status: Option<String> = conn
            .query_row(
                "SELECT delivery_status FROM async_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(status.as_deref(), Some("superseded"));
    }

    #[tokio::test]
    async fn format_async_yaml_basic_cron() {
        let pending = PendingAsyncResult {
            id: "abc".into(),
            kind: "cron".into(),
            producer_ref: Some("health-check".into()),
            delivery_json: r#"{"kind":"notify","content":"BTC up 2%"}"#.into(),
            run_note: "Checked 5 pairs".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
            force_notify: false,
        };
        let output = format_async_yaml(&pending, 2).unwrap();
        // Instruction prefix assertions
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("place it VERBATIM in your reply's `content` field"));
        assert!(output.contains("Do NOT call `mcp__right__send_message`"));
        assert!(output.contains("never repeat the content text in a caption"));
        assert!(output.contains("Here is the YAML report of the cron job:"));
        // YAML content assertions
        assert!(output.contains("job: \"health-check\""));
        assert!(output.contains("runs_total: 3"));
        assert!(output.contains("skipped_runs: 2"));
        assert!(output.contains("BTC up 2%"));
        assert!(output.contains("Checked 5 pairs"));
    }

    #[tokio::test]
    async fn format_async_yaml_no_skipped() {
        let pending = PendingAsyncResult {
            id: "abc".into(),
            kind: "cron".into(),
            producer_ref: Some("job1".into()),
            delivery_json: r#"{"kind":"notify","content":"hello"}"#.into(),
            run_note: "done".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
            force_notify: false,
        };
        let output = format_async_yaml(&pending, 0).unwrap();
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("runs_total: 1"));
        assert!(!output.contains("skipped_runs"));
    }

    #[tokio::test]
    async fn format_async_yaml_uses_cron_failure_instruction_when_status_failed() {
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
            force_notify: false,
        };
        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("did not complete successfully"));
        assert!(!out.contains("send it VERBATIM"));
    }

    #[tokio::test]
    async fn format_async_yaml_uses_cron_success_instruction_when_status_success() {
        let pending = PendingAsyncResult {
            id: "r2".into(),
            kind: "cron".into(),
            producer_ref: Some("watcher".into()),
            delivery_json: r#"{"kind":"notify","content":"BTC up 2%"}"#.into(),
            run_note: "ok".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
            force_notify: false,
        };
        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("VERBATIM"));
    }

    #[tokio::test]
    async fn format_async_yaml_rejects_silent_delivery_json() {
        let pending = PendingAsyncResult {
            id: "r-silent".into(),
            kind: "cron".into(),
            producer_ref: Some("watcher".into()),
            delivery_json: r#"{"kind":"silent","reason":"No changes"}"#.into(),
            run_note: "quiet".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
            force_notify: false,
        };
        let err = format_async_yaml(&pending, 0).unwrap_err();
        assert!(
            err.to_string().contains("not a notify decision"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn format_async_yaml_background_includes_background_instruction_and_content() {
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
            force_notify: false,
        };

        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.starts_with("You are delivering a background task result"));
        assert!(out.contains("background_result:"));
        assert!(out.contains("label: \"background\""));
        assert!(out.contains("Finished the answer in background"));
        assert!(!out.contains("cron_result:"));
    }

    #[tokio::test]
    async fn format_async_yaml_background_uses_failure_instruction_when_status_failed() {
        let pending = PendingAsyncResult {
            id: "bg-2".into(),
            kind: "background".into(),
            producer_ref: Some("custom-bg".into()),
            delivery_json: r#"{"kind":"notify","content":"Background work failed"}"#.into(),
            run_note: "background failed".into(),
            status: "failed".into(),
            target_chat_id: Some(-100),
            target_thread_id: None,
            force_notify: false,
        };

        let out = format_async_yaml(&pending, 0).unwrap();
        assert!(out.contains("background task below did not complete successfully"));
        assert!(out.contains("label: \"custom-bg\""));
        assert!(out.contains("Background work failed"));
    }

    #[tokio::test]
    async fn delivery_invocation_uses_configured_agent_model() {
        let args = build_delivery_invocation_args(
            "/sandbox/mcp.json".into(),
            r#"{"type":"object"}"#.into(),
            Some("claude-opus-4-8[1m]".into()),
            Some("session-1".into()),
            None,
        );

        let model_pos = args
            .iter()
            .position(|arg| arg == "--model")
            .expect("configured model must be passed to Claude");
        assert_eq!(args[model_pos + 1], "claude-opus-4-8[1m]");
        assert!(
            !args.iter().any(|arg| arg == "claude-haiku-4-5-20251001"),
            "delivery must not override the configured agent model with Haiku"
        );
    }

    #[tokio::test]
    async fn fetch_pending_resolves_target_after_spec_deletion() {
        // Reproduces the production bug: a one-shot spec auto-deletes after
        // firing, but the run row still needs to know where to deliver.
        let (_dir, conn) = setup_db().await;
        let now = chrono::Utc::now().to_rfc3339();

        // 1. Spec is created (recurring=0, one-shot).
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('one-shot', '*/5 * * * *', 'p', 1.0, 0, -4996137249, NULL, ?1, ?1)",
            [&now],
        ).await.unwrap();

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
        )
        .await;

        // 3. Spec is auto-deleted (one-shot completion).
        conn.execute("DELETE FROM cron_specs WHERE job_name = 'one-shot'", [])
            .await
            .unwrap();

        // 4. Delivery loop fetches — must still find the target.
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-4996137249));
        assert_eq!(pending.target_thread_id, None);

        // And dedup must agree.
        let (latest, _skipped) = deduplicate_job(&conn, "one-shot").await.unwrap().unwrap();
        assert_eq!(latest.target_chat_id, Some(-4996137249));
    }

    #[tokio::test]
    async fn fetch_pending_carries_target_fields() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-555),
                target_thread_id: Some(9),
                ..Default::default()
            },
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-555));
        assert_eq!(pending.target_thread_id, Some(9));
    }

    #[tokio::test]
    async fn fetch_pending_returns_none_target_when_run_has_none() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "legacy",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert!(pending.target_chat_id.is_none());
        assert!(pending.target_thread_id.is_none());
    }

    #[tokio::test]
    async fn null_target_classifies_as_no_target() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "legacy",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                ..Default::default()
            },
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[]));
        assert!(
            matches!(outcome, TargetClassification::NoTarget),
            "got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn target_chat_id_zero_fetches_as_no_target() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "zero-target",
                job_name: "targetless",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(0),
                ..Default::default()
            },
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.target_chat_id, None);

        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[0]));
        assert!(
            matches!(outcome, TargetClassification::NoTarget),
            "got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn target_not_in_allowlist_classifies_as_denied() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                job_name: "agenda",
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-777),
                ..Default::default()
            },
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[100], &[-200]));
        assert!(
            matches!(outcome, TargetClassification::Denied),
            "got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn target_in_allowlist_classifies_as_ready() {
        let (_dir, conn) = setup_db().await;
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
        )
        .await;
        let pending = fetch_pending(&conn).await.unwrap().unwrap();
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
        use right_agent::agent::allowlist::{
            AllowedGroup, AllowedUser, AllowlistState, GroupKind, ResponseMode,
        };
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
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
                kind: GroupKind::Group,
            });
        }
        state
    }

    #[tokio::test]
    async fn deduplicate_job_carries_target_fields() {
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "a",
                run_note: Some("sum1"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"x\"}"),
                target_chat_id: Some(-100),
                ..Default::default()
            },
        )
        .await;
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
        )
        .await;
        let (latest, skipped) = deduplicate_job(&conn, "job1").await.unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        assert_eq!(latest.target_chat_id, Some(-100));
    }

    #[tokio::test]
    async fn select_delivery_candidate_does_not_deduplicate_background_rows() {
        let (_dir, conn) = setup_db().await;
        insert_async_background_run(
            &conn,
            "bg-old",
            "2026-01-01T00:00:00Z",
            "success",
            "{\"kind\":\"notify\",\"content\":\"old\"}",
            -100,
        )
        .await;
        insert_async_background_run(
            &conn,
            "bg-new",
            "2026-01-01T00:05:00Z",
            "success",
            "{\"kind\":\"notify\",\"content\":\"new\"}",
            -100,
        )
        .await;

        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.id, "bg-old");
        let (selected, skipped) = select_delivery_candidate(&conn, pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(selected.id, "bg-old");
        assert_eq!(skipped, 0);

        let statuses = conn
            .prepare("SELECT id, delivery_status FROM async_runs ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .await
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

    #[tokio::test]
    async fn empty_delivery_send_report_is_rejected() {
        let report = DeliverySendReport {
            text_messages_sent: 0,
            attachment_batches_sent: 0,
        };

        let err = ensure_delivery_send_report_non_empty(report).unwrap_err();
        assert!(err.contains("empty delivery reply"));
    }

    #[test]
    fn force_notify_skips_idle_gate() {
        // Non-forced, recently active chat → held.
        assert!(should_hold_delivery(false, DeliveryMode::Normal, 10));
        // Forced → never held, even when active.
        assert!(!should_hold_delivery(true, DeliveryMode::Normal, 10));
        // Idle long enough → not held regardless.
        assert!(!should_hold_delivery(
            false,
            DeliveryMode::Normal,
            IDLE_THRESHOLD_SECS + 1
        ));
    }

    #[tokio::test]
    async fn fetch_pending_reads_force_notify() {
        let (_dir, conn) = setup_db().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
                status, started_at, finished_at, log_path, run_note, delivery_json,
                delivery_required, delivery_status, force_notify, created_at, updated_at
             ) VALUES (
                'r-fn', 'cron', 'job', 'r-fn', 5, NULL,
                'success', '2026-06-02T00:00:00Z', '2026-06-02T00:01:00Z', '/log', 'note',
                '{\"kind\":\"notify\",\"content\":\"hi\"}',
                1, 'pending', 1, '2026-06-02T00:00:00Z', '2026-06-02T00:01:00Z'
             )",
            right_db::params![],
        )
        .await
        .unwrap();

        let pending = fetch_pending(&conn).await.unwrap().unwrap();
        assert_eq!(pending.id, "r-fn");
        assert!(
            pending.force_notify,
            "force_notify must be read from the row"
        );
    }

    #[tokio::test]
    async fn dedup_surfaces_latest_force_notify() {
        // Two cron runs for the same job: older non-forced, newer forced. The
        // delivery loop fetches the oldest, but candidate selection (dedup) must
        // surface the newer forced run so the idle gate reads its force_notify.
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "older",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                finished_at: Some("2026-06-02T00:01:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                ..Default::default()
            },
        )
        .await;
        // Newer forced run. insert_async_cron_run does not set force_notify, so
        // stamp it directly after insert.
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "newer",
                job_name: "job",
                started_at: "2026-06-02T00:05:00Z",
                finished_at: Some("2026-06-02T00:06:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"new\"}"),
                ..Default::default()
            },
        )
        .await;
        conn.execute(
            "UPDATE async_runs SET force_notify = 1 WHERE id = 'newer'",
            right_db::params![],
        )
        .await
        .unwrap();

        // Fetch returns the oldest (non-forced) row.
        let delivered_in_memory = HashSet::new();
        let pending = fetch_next_pending(&conn, &delivered_in_memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.id, "older");
        assert!(!pending.force_notify);

        // Candidate selection surfaces the newer forced run.
        let (to_deliver, _skipped) = select_delivery_candidate(&conn, pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(to_deliver.id, "newer");
        assert!(
            to_deliver.force_notify,
            "candidate must carry the forced flag of the latest run"
        );
    }

    #[tokio::test]
    async fn dedup_carries_force_notify_from_older_displaced_run() {
        // Inverse of the above: the OLDER run is forced, a NEWER non-forced
        // scheduled run wins the finished_at tie-break. The candidate is the
        // newer run (freshest content), but it must still carry force_notify so
        // the user's forced verification request bypasses the idle gate.
        let (_dir, conn) = setup_db().await;
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "older-forced",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                finished_at: Some("2026-06-02T00:01:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"old\"}"),
                ..Default::default()
            },
        )
        .await;
        conn.execute(
            "UPDATE async_runs SET force_notify = 1 WHERE id = 'older-forced'",
            right_db::params![],
        )
        .await
        .unwrap();
        insert_async_cron_run(
            &conn,
            TestCronRun {
                id: "newer-scheduled",
                job_name: "job",
                started_at: "2026-06-02T00:05:00Z",
                finished_at: Some("2026-06-02T00:06:00Z"),
                delivery_json: Some("{\"kind\":\"notify\",\"content\":\"new\"}"),
                ..Default::default()
            },
        )
        .await;

        let (to_deliver, _skipped) = deduplicate_job(&conn, "job").await.unwrap().unwrap();
        assert_eq!(
            to_deliver.id, "newer-scheduled",
            "latest run wins on content"
        );
        assert!(
            to_deliver.force_notify,
            "force_notify must be OR'd across the group so the older forced run's bypass survives"
        );
    }

    fn test_pending(
        kind: &str,
        status: &str,
        job: Option<&str>,
        force_notify: bool,
    ) -> PendingAsyncResult {
        PendingAsyncResult {
            id: "x".into(),
            kind: kind.into(),
            producer_ref: job.map(|s| s.to_string()),
            delivery_json: "{}".into(),
            run_note: String::new(),
            status: status.into(),
            target_chat_id: Some(1),
            target_thread_id: None,
            force_notify,
        }
    }

    #[test]
    fn header_success_scheduled() {
        let p = test_pending("cron", "success", Some("sources-update"), false);
        assert_eq!(
            render_delivery_header(&p),
            "✓ <b>sources-update</b> · success"
        );
    }

    #[test]
    fn header_success_manual() {
        let p = test_pending("cron", "success", Some("sources-update"), true);
        assert_eq!(
            render_delivery_header(&p),
            "✓ <b>sources-update</b> · manual run · success"
        );
    }

    #[test]
    fn header_failed() {
        let p = test_pending("cron", "failed", Some("sources-update"), false);
        assert_eq!(
            render_delivery_header(&p),
            "✗ <b>sources-update</b> · failed"
        );
    }

    #[test]
    fn header_background_label_fallback() {
        let p = test_pending("background", "success", None, false);
        assert_eq!(
            render_delivery_header(&p),
            "✓ <b>background task</b> · success"
        );
    }

    #[test]
    fn header_background_slug_normalized() {
        // Real background runs carry `producer_ref = Some("background")`; the raw
        // slug must not surface — present "background task" instead.
        let p = test_pending("background", "success", Some("background"), false);
        assert_eq!(
            render_delivery_header(&p),
            "✓ <b>background task</b> · success"
        );
    }

    #[test]
    fn header_escapes_label() {
        let p = test_pending("cron", "success", Some("a<b>&c"), false);
        assert_eq!(
            render_delivery_header(&p),
            "✓ <b>a&lt;b&gt;&amp;c</b> · success"
        );
    }

    #[test]
    fn prepend_header_separates_with_blank_lines() {
        let out = prepend_delivery_header("✓ <b>job</b> · success", "body text");
        assert_eq!(out, "✓ <b>job</b> · success\n\nbody text");
    }

    #[test]
    fn prepend_header_handles_empty_body() {
        let out = prepend_delivery_header("✓ <b>job</b> · success", "");
        assert_eq!(out, "✓ <b>job</b> · success");
    }
}
