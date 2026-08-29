use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

async fn fetch_next_pending(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    delivered_in_memory: &HashSet<String>,
) -> Result<Option<PendingAsyncResult>, right_mcp::internal_db::InternalDbError> {
    let limit = PENDING_FETCH_BATCH_SIZE.max(delivered_in_memory.len().saturating_add(1));
    let response = client
        .delivery_fetch_pending(&right_mcp::internal_db::DeliveryFetchPendingRequest {
            agent: agent.to_owned(),
            limit: limit as u32,
        })
        .await?;
    for dto in response.pending {
        let pending = pending_from_dto(dto);
        if delivered_in_memory.contains(&pending.id) {
            mark_delivery_outcome(client, agent, &pending.id, "delivered").await?;
        } else {
            return Ok(Some(pending));
        }
    }
    Ok(None)
}

fn pending_from_dto(dto: right_mcp::internal_db::PendingAsyncResultDto) -> PendingAsyncResult {
    PendingAsyncResult {
        id: dto.id,
        kind: dto.kind,
        producer_ref: dto.producer_ref,
        delivery_json: dto.delivery_json,
        run_note: dto.run_note,
        status: dto.status,
        target_chat_id: dto.target_chat_id,
        target_thread_id: dto.target_thread_id,
        force_notify: dto.force_notify,
    }
}

/// Mark an async run delivery as complete with a given status.
///
/// Single UPDATE sets both `delivery_status` and `delivered_at` atomically.
async fn mark_delivery_outcome(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    run_id: &str,
    status: &str,
) -> Result<(), right_mcp::internal_db::InternalDbError> {
    client
        .delivery_mark_outcome(&right_mcp::internal_db::DeliveryMarkOutcomeRequest {
            agent: agent.to_owned(),
            request_id: crate::db::request_id(),
            run_id: run_id.to_owned(),
            status: status.to_owned(),
        })
        .await
        .map(drop)
}

async fn select_delivery_candidate(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    pending: PendingAsyncResult,
) -> Result<Option<(PendingAsyncResult, u32)>, right_mcp::internal_db::InternalDbError> {
    if pending.kind == "cron"
        && let Some(producer_ref) = pending.producer_ref.as_deref()
    {
        let response = client
            .delivery_deduplicate_job(&right_mcp::internal_db::DeliveryDeduplicateJobRequest {
                agent: agent.to_owned(),
                producer_ref: producer_ref.to_owned(),
            })
            .await?;
        return Ok(response
            .candidate
            .map(pending_from_dto)
            .map(|row| (row, response.superseded)));
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
pass). Place your natural-prose version in your reply's `content` field — you
MAY rephrase lightly for flow with the recent conversation, but keep all
factual claims intact. Do not invent details.
Do NOT call `mcp__right__send_message` or any other send/notify tool. The
platform delivers your reply to Telegram itself — writing `content` is the only
way to reach the user. Ignore the attachments field.

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
a platform-generated summary of the failure. Place your natural-prose version in your
reply's `content` field - you MAY rephrase lightly for flow with the recent conversation,
but keep all factual claims intact. Do not invent details.
Do NOT call `mcp__right__send_message` or any other send/notify tool. The platform
delivers your reply to Telegram itself - writing `content` is the only way to reach
the user. Ignore the attachments field.

Here is the YAML report of the background task:
";

/// Platform-rendered plain status line prepended as its own RichContent block.
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
    let label = label.replace(['\n', '\r'], " ");
    let status_word = if pending.status == "failed" {
        "failed"
    } else {
        "success"
    };
    if pending.force_notify {
        format!("{glyph} {label} · manual run · {status_word}")
    } else {
        format!("{glyph} {label} · {status_word}")
    }
}

/// Join a platform status line above normalized body text for diagnostics/tests.
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
        crate::telegram::attachments::yaml_escape_string(&notify.content.normalized_text())
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

/// Resolve the active session root for a delivery target. Owner errors
/// propagate as typed IPC errors: falling back to `None` here would fork
/// the run's result into a fresh session and lose conversation continuity.
async fn active_delivery_session_id(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, right_mcp::internal_db::InternalDbError> {
    Ok(
        crate::telegram::session::get_active_session(client, agent, chat_id, thread_id)
            .await?
            .map(|session| session.root_session_id),
    )
}

#[allow(clippy::too_many_arguments)]
async fn run_delivery_once(
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
) -> Result<bool, right_mcp::internal_db::InternalDbError> {
    // Owner read failures are typed and abort this delivery attempt: the poll
    // loop (or the shutdown-flush deadline) is the designed retry boundary.
    let pending =
        match fetch_next_pending(internal_client, agent_name, &state.delivered_in_memory).await {
            Ok(Some(pending)) => pending,
            Ok(None) => return Ok(false),
            Err(error) => return Err(error),
        };
    let (to_deliver, skipped) =
        match select_delivery_candidate(internal_client, agent_name, pending).await {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return Ok(false),
            Err(error) => return Err(error),
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
        return Ok(false);
    }

    if state.delivered_in_memory.contains(&to_deliver.id) {
        if mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "delivered")
            .await
            .is_ok()
        {
            state.delivered_in_memory.remove(&to_deliver.id);
        }
        tracing::debug!(run_id = %to_deliver.id, "skipping already-delivered run (in-memory dedup)");
        return Ok(true);
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
                if let Err(e) =
                    mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "no_target")
                        .await
                {
                    tracing::error!(run_id = %to_deliver.id, "mark no_target failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                return Ok(true);
            }
            TargetClassification::Denied => {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    producer_ref = ?to_deliver.producer_ref,
                    run_id = %to_deliver.id,
                    target_chat_id = ?to_deliver.target_chat_id,
                    "async delivery target chat is not in allowlist; skipping delivery"
                );
                if let Err(e) =
                    mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "denied")
                        .await
                {
                    tracing::error!(run_id = %to_deliver.id, "mark denied failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                return Ok(true);
            }
            TargetClassification::Ready { chat_id, thread_id } => (chat_id, thread_id),
        };

    // A session-lookup failure must abort this attempt: continuing with
    // `None` would silently fork the run's result into a fresh session and
    // lose conversation continuity. The row stays pending for the next poll.
    let session_id = active_delivery_session_id(
        internal_client,
        agent_name,
        target_chat_id,
        target_thread_id.unwrap_or(0),
    )
    .await?;

    let yaml = match format_async_yaml(&to_deliver, skipped) {
        Ok(y) => y,
        Err(e) => {
            tracing::error!(
                kind = %to_deliver.kind,
                label = %pending_label(&to_deliver),
                run_id = %to_deliver.id,
                "async delivery: delivery_json deserialization failed, marking delivery failed: {e:#}"
            );
            if let Err(db_err) =
                mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "failed").await
            {
                tracing::error!(run_id = %to_deliver.id, "mark failed for malformed delivery_json failed: {db_err:#}");
                state.delivered_in_memory.insert(to_deliver.id.clone());
            }
            return Ok(true);
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
                return Ok(true);
            }
            // TODO(usage): delivery stream capture lives elsewhere — follow up.
            // deliver_through_session uses OutputFormat::Json (single JSON blob, not stream-json
            // NDJSON), so there is no "result" event line to feed parse_usage_full. Usage
            // tracking for delivery sessions requires either switching to stream-json output
            // or extracting cost from the non-streaming JSON response format.
            if let Err(e) =
                mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "delivered")
                    .await
            {
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
                return Ok(false);
            }
            if is_delivery_terminal_shutdown_send_error(&e) {
                tracing::warn!(
                    kind = %to_deliver.kind,
                    label = %pending_label(&to_deliver),
                    run_id = %to_deliver.id,
                    "async delivery send outcome is unknown after shutdown deadline; marking terminal failed to avoid duplicate delivery"
                );
                if let Err(mark_err) =
                    mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "failed")
                        .await
                {
                    tracing::error!(run_id = %to_deliver.id, "terminal delivery-failure DB update failed: {mark_err:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                state.attempt_counts.remove(&to_deliver.id);
                return Ok(true);
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
                if let Err(e) =
                    mark_delivery_outcome(internal_client, agent_name, &to_deliver.id, "failed")
                        .await
                {
                    tracing::error!(run_id = %to_deliver.id, "delivery-failure DB update failed: {e:#}");
                    state.delivered_in_memory.insert(to_deliver.id.clone());
                }
                state.attempt_counts.remove(&to_deliver.id);
            }
        }
    }

    Ok(true)
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

        // Owner IPC failures surface here; the poll loop is the designed
        // retry boundary, so the next cycle tries again.
        if let Err(error) = run_delivery_once(
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
        .await
        {
            tracing::error!(agent = %agent_name, "async delivery poll failed: {error:#}");
        }
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
    let mut state = DeliveryLoopState::new();
    let deadline = tokio::time::Instant::now() + ASYNC_DELIVERY_SHUTDOWN_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("async delivery shutdown flush timed out");
            return;
        }
        let sandbox = sandbox_runtime.current_sandbox();
        // Owner IPC failures abort the flush: without the owner we cannot
        // read pending rows or mark outcomes, so spinning until the deadline
        // would only delay shutdown.
        let delivered = match run_delivery_once(
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
        .await
        {
            Ok(delivered) => delivered,
            Err(error) => {
                tracing::error!(agent = %agent_name, "async delivery shutdown flush failed: {error:#}");
                return;
            }
        };
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
            .map_err(|e| format!("build delivery command: {e:#}"))?
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
    let (mut reply, _) = crate::cc::worker_reply::parse_reply_output(&raw)
        .map_err(|e| format!("reply parse: {e}"))?;

    let has_content = reply.content.is_some();
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

    if let Some(content) = &mut reply.content {
        let header_plain = strip_html_tags(header);
        content.prepend_platform_paragraph(header_plain);
        let thread = target_thread_id.map(|t| t as i32);
        let send = async {
            crate::telegram::rich_content::send(bot, target_chat_id, content, thread, None, None)
                .await
        };
        // `send` is infallible at the top level (partial failures are carried
        // in the outcome), so wrap it in Ok for the Result-typed wrapper.
        let outcome =
            run_telegram_request_with_shutdown(shutdown, report.total_sent() > 0, async {
                Ok(send.await)
            })
            .await?
            .map_err(|error: std::convert::Infallible| -> String { match error {} })?;
        report.text_messages_sent += outcome.delivered.len();
        body_messages_sent += outcome.delivered.len();
        if !outcome.is_complete() {
            // Any delivered text is terminal: failing here would requeue the
            // delivery and re-send that text, duplicating the post. Log the
            // omitted fragments and treat the delivered prefix as final.
            tracing::error!(
                chat_id = target_chat_id,
                delivered_messages = outcome.delivered.len(),
                "async delivery: rich parts failed after text delivered; \
                 keeping delivered prefix to avoid duplicate re-send: {}",
                outcome.error_display()
            );
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
mod owner_error_propagation_tests {
    use super::*;

    /// An owner session-lookup failure must fail the delivery operation with
    /// the typed IPC error. Falling back to `None` here would silently fork
    /// the run's result into a fresh session and lose conversation
    /// continuity while the audit trail looks like a normal delivery.
    #[tokio::test]
    async fn active_session_lookup_failure_propagates() {
        let client = right_mcp::internal_client::InternalClient::new(std::path::PathBuf::from(
            "/nonexistent-right-test-internal.sock",
        ));
        let result = active_delivery_session_id(&client, "alpha", 42, 0).await;
        assert!(
            matches!(
                result,
                Err(right_mcp::internal_db::InternalDbError::Transport(_))
            ),
            "lookup failure must propagate as typed transport error, got {result:?}"
        );
    }
}

#[cfg(test)]
#[path = "async_delivery_tests.rs"]
mod tests;
