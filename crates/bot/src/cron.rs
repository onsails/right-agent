use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt as _;
use tokio::io::AsyncReadExt as _;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::cc::attachments_dto::OutboundAttachment;
use right_agent::cron_spec::CronSpec;

/// Lock file JSON: {"heartbeat": "2026-...Z"}
#[derive(serde::Deserialize, serde::Serialize)]
struct LockFile {
    heartbeat: chrono::DateTime<chrono::Utc>,
}

/// Errors produced by the cron engine.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CronError {
    #[error("invalid lock_ttl format '{0}' — expected e.g. '30m' or '1h'")]
    InvalidLockTtl(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("db error: {0:#}")]
    Db(#[from] right_memory::MemoryError),
}

/// Structured output from a cron CC invocation.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CronReplyOutput {
    pub delivery: CronDeliveryDecision,
    pub run_note: String,
}

/// User-facing notification from a cron job.
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub(crate) struct CronNotify {
    pub content: right_rich_content::RichContent,
    pub attachments: Option<Vec<OutboundAttachment>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CronDeliveryDecision {
    Notify {
        content: right_rich_content::RichContent,
        attachments: Option<Vec<OutboundAttachment>>,
    },
    Silent {
        reason: String,
    },
}

impl CronDeliveryDecision {
    pub(crate) fn as_notify(&self) -> Option<CronNotify> {
        match self {
            Self::Notify {
                content,
                attachments,
            } => Some(CronNotify {
                content: content.clone(),
                attachments: attachments.clone(),
            }),
            Self::Silent { .. } => None,
        }
    }

    pub(crate) fn silent_reason(&self) -> Option<&str> {
        match self {
            Self::Silent { reason } => Some(reason),
            Self::Notify { .. } => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Notify { content, .. } => content.validate().map_err(|e| e.to_string()),
            Self::Silent { reason } if reason.trim().is_empty() => {
                Err("empty silent reason".to_string())
            }
            Self::Silent { .. } => Ok(()),
        }
    }
}

pub(crate) fn notify_delivery_json(
    content: &right_rich_content::RichContent,
    attachments: Option<&[OutboundAttachment]>,
) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum DeliveryRef<'a> {
        Notify {
            content: &'a right_rich_content::RichContent,
            attachments: Option<&'a [OutboundAttachment]>,
        },
    }
    serde_json::to_string(&DeliveryRef::Notify {
        content,
        attachments,
    })
}

pub(crate) fn notify_from_delivery_json(raw: &str) -> Result<CronNotify, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("parse delivery_json: {e}"))?;
    let value = upgrade_legacy_delivery_content(value)?;
    let decision: CronDeliveryDecision =
        serde_json::from_value(value).map_err(|e| format!("parse delivery_json: {e}"))?;
    decision.validate()?;
    decision
        .as_notify()
        .ok_or_else(|| "delivery_json is not a notify decision".to_string())
}

fn upgrade_legacy_delivery_content(
    mut value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if let Some(content) = value.get_mut("content")
        && let Some(text) = content.as_str()
    {
        // Legacy queued strings predate the rich-content schema and carry no
        // authoring cap, so they upgrade as platform-owned text and may fan
        // out over several blocks at delivery time. Whitespace-only content
        // has no visible body to deliver; the row keeps failing loudly instead
        // of being upgraded into an object Telegram would reject as empty.
        if text.trim().is_empty() {
            return Err(format!(
                "legacy delivery content: {}",
                right_rich_content::ValidationError::EmptyContent
            ));
        }
        let rich = right_rich_content::RichContent::platform_text(text.to_owned())
            .map_err(|e| format!("legacy delivery content: {e}"))?;
        *content = serde_json::to_value(rich).map_err(|e| e.to_string())?;
    }
    Ok(value)
}

/// Extract the filename component from a sandbox attachment path.
pub(crate) fn attachment_filename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Convert a 5-field user expression to the 7-field format required by the cron crate.
///
/// The cron crate requires: `<sec> <min> <hour> <dom> <mon> <dow> <year>`
/// Users write standard 5-field expressions: `<min> <hour> <dom> <mon> <dow>`
///
/// Transformation: prepend "0 " (seconds=0) and append " *" (year=any).
pub(crate) fn to_7field(expr: &str) -> String {
    format!("0 {} *", expr.trim())
}

/// Parse a lock_ttl string ("30m", "1h") into a `chrono::Duration`.
pub(crate) fn parse_lock_ttl(s: &str) -> Result<chrono::Duration, CronError> {
    if let Some(mins) = s.strip_suffix('m') {
        let n: i64 = mins
            .trim()
            .parse()
            .map_err(|_| CronError::InvalidLockTtl(s.to_string()))?;
        return Ok(chrono::Duration::minutes(n));
    }
    if let Some(hrs) = s.strip_suffix('h') {
        let n: i64 = hrs
            .trim()
            .parse()
            .map_err(|_| CronError::InvalidLockTtl(s.to_string()))?;
        return Ok(chrono::Duration::hours(n));
    }
    Err(CronError::InvalidLockTtl(s.to_string()))
}

/// Check if a lock file exists and its heartbeat is within the TTL.
///
/// Returns `true` if the previous run is still considered active (skip this run).
/// Returns `false` if no lock file, lock is unparseable, or heartbeat is stale.
pub(crate) fn is_lock_fresh(
    agent_dir: &std::path::Path,
    job_name: &str,
    lock_ttl_str: &str,
) -> bool {
    let lock_path = agent_dir
        .join("crons")
        .join(".locks")
        .join(format!("{job_name}.json"));
    let Ok(raw) = std::fs::read_to_string(&lock_path) else {
        return false;
    };
    let Ok(lock) = serde_json::from_str::<LockFile>(&raw) else {
        return false;
    };
    let ttl = parse_lock_ttl(lock_ttl_str).unwrap_or(chrono::Duration::minutes(30));
    chrono::Utc::now() - lock.heartbeat < ttl
}

/// Wrap the prompt-assembly script in the cron log pipeline.
///
/// POSIX-sh only: the guest `/bin/sh` is dash (node:22-slim), which aborts on
/// bash-only `set -o pipefail` before `claude` ever runs — the Aug 18–21 cron
/// outage. No pipefail: the outcome classifier reads the terminal `result`
/// NDJSON line, never the pipeline exit code.
fn cron_wrapper_script(assembly_script: &str, log_dir: &str, log_filename: &str) -> String {
    format!("mkdir -p {log_dir}\n{assembly_script} | tee {log_dir}/{log_filename}")
}

/// Per-invocation settings override for cron execution.
///
/// `--settings` outranks project and local settings. A shell export would not:
/// `.claude/settings.local.json` and the generated agent environment may replace
/// inherited variables when Claude Code starts.
fn cron_settings_override() -> String {
    serde_json::json!({
        "env": { "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1" }
    })
    .to_string()
}

fn cron_extra_args() -> Vec<String> {
    vec!["--settings".to_owned(), cron_settings_override()]
}
/// Age of a job's lock heartbeat, for the skip log line. `None` when the lock
/// file is missing or unparseable (the caller logs `-1` — lock fresh yet
/// unreadable is itself a signal).
fn lock_age(agent_dir: &std::path::Path, job_name: &str) -> Option<chrono::Duration> {
    let lock_path = agent_dir
        .join("crons")
        .join(".locks")
        .join(format!("{job_name}.json"));
    let raw = std::fs::read_to_string(&lock_path).ok()?;
    let lock = serde_json::from_str::<LockFile>(&raw).ok()?;
    Some(chrono::Utc::now() - lock.heartbeat)
}

fn effective_lock_ttl(spec: &CronSpec) -> &str {
    if let Some(lock_ttl) = spec.lock_ttl.as_deref() {
        return lock_ttl;
    }
    if matches!(
        &spec.schedule_kind,
        right_agent::cron_spec::ScheduleKind::Immediate
    ) {
        right_agent::cron_spec::IMMEDIATE_DEFAULT_LOCK_TTL
    } else {
        "30m"
    }
}

/// Compose a triggered run's prompt: force-notify notice, then this-run-only
/// extra instruction, then the stored prompt, then linked-skills directive.
/// Each layer is optional.
fn compose_run_prompt(
    prompt: &str,
    force_notify: bool,
    extra_instruction: Option<&str>,
    notice_token: &str,
    linked_skills: &[String],
) -> String {
    let mut out = String::new();
    if force_notify {
        out.push_str(&crate::cc::system_notice::wrap_system_notice(
            notice_token,
            "Manual verification trigger: always emit delivery.kind=\"notify\" \
             with a complete report of what you found; do not go silent.",
        ));
        out.push_str("\n\n");
    }
    if let Some(extra) = extra_instruction.filter(|s| !s.trim().is_empty()) {
        out.push_str(&crate::cc::system_notice::wrap_system_notice(
            notice_token,
            &format!("Extra instruction for this run only: {extra}"),
        ));
        out.push_str("\n\n");
    }
    out.push_str(prompt);
    if !linked_skills.is_empty() {
        out.push_str("\n\n## Linked skills\nLinked skills for this job — use them via the Skill tool as appropriate: ");
        out.push_str(&linked_skills.join(", "));
        out.push('\n');
    }
    out
}

/// What a `then` continuation should deliver, if it fires.
pub(crate) struct ThenAction {
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub prompt: String,
}

/// Decide whether/where a `then` continuation fires for a finished triggered run.
/// Target precedence: explicit `then.target_chat_id` > resolved origin >
/// the job's standing `target_chat_id`. Returns `None` when there is no `then`,
/// the `run_on` does not match, or no deliverable chat is known.
///
/// NOTE: the background continuation schema forces `delivery.kind=notify`, so a
/// `then` continuation always delivers. `then.notify` only adds a prompt-emphasis
/// directive below; forcing idle-gate skip via the row's `force_notify` column is
/// a documented follow-up.
pub(crate) fn resolve_then_action(spec: &CronSpec, success: bool) -> Option<ThenAction> {
    let then = spec.then.as_ref()?;
    if !then.run_on.fires_on(success) {
        return None;
    }
    let target_chat_id = then
        .target_chat_id
        .or(spec.trigger_origin_chat_id)
        .or(spec.target_chat_id)?;
    let target_thread_id = if then.target_chat_id.is_some() {
        then.target_thread_id
    } else if spec.trigger_origin_chat_id.is_some() {
        spec.trigger_origin_thread_id
    } else {
        spec.target_thread_id
    };
    // Telegram thread 0 == "no topic": never propagate Some(0) into a send.
    let target_thread_id = target_thread_id.filter(|&t| t != 0);
    let prompt = if then.notify {
        format!(
            "⟨⟨SYSTEM_NOTICE⟩⟩ Scheduled follow-up of the job you just ran. Always emit \
             delivery.kind=\"notify\" with a complete report. ⟨⟨/SYSTEM_NOTICE⟩⟩\n\n{}",
            then.instruction
        )
    } else {
        then.instruction.clone()
    };
    Some(ThenAction {
        target_chat_id,
        target_thread_id,
        prompt,
    })
}

/// `producer_ref` stamped on every `then`-continuation `async_runs` row.
pub(crate) const THEN_PRODUCER_REF: &str = "cron_then";

fn cron_spec_from_dto(
    dto: right_mcp::internal_db::CronSpecDto,
) -> Result<(String, CronSpec), String> {
    let schedule_kind = right_agent::cron_spec::ScheduleKind::from_db_row(
        &dto.schedule,
        dto.run_at.as_deref(),
        i64::from(dto.recurring),
    )?;
    let then = dto
        .trigger_then_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("invalid trigger_then_json: {error}"))?;
    Ok((
        dto.job_name,
        CronSpec {
            schedule_kind,
            prompt: dto.prompt,
            lock_ttl: dto.lock_ttl,
            max_budget_usd: dto.max_budget_usd,
            triggered_at: dto.triggered_at,
            trigger_force_notify: dto.trigger_force_notify,
            target_chat_id: dto.target_chat_id,
            target_thread_id: dto.target_thread_id,
            model: dto.model,
            trigger_extra_instruction: dto.trigger_extra_instruction,
            then,
            trigger_origin_chat_id: dto.trigger_origin_chat_id,
            trigger_origin_thread_id: dto.trigger_origin_thread_id,
        },
    ))
}

async fn load_specs_ipc(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
) -> Result<HashMap<String, CronSpec>, right_mcp::internal_db::InternalDbError> {
    let response = client
        .cron_specs_list(&right_mcp::internal_db::CronSpecsListRequest {
            agent: agent.to_owned(),
        })
        .await?;
    response
        .specs
        .into_iter()
        .map(|dto| {
            cron_spec_from_dto(dto).map_err(|message| {
                right_mcp::internal_db::InternalDbError::Server {
                    category: right_mcp::internal_db::DbErrorCategory::Invalid,
                    status: 422,
                    message,
                }
            })
        })
        .collect()
}

/// Insert the queued `kind='background'` row for a `then` continuation, returning
/// the new run id. The row's `source_session_id` is the triggered run's session
/// (the one the continuation forks), and `run_session_id` is the new run's own
/// session. Observable boundary for tests — see
/// `triggered_run_with_then_success_spawns_continuation`.
async fn insert_then_continuation_row(
    agent_dir: &std::path::Path,
    action: &ThenAction,
    source_session_id: &str,
) -> anyhow::Result<String> {
    let new_run_id = uuid::Uuid::new_v4().to_string();
    let (client, agent) = crate::db::client_for_agent_dir(agent_dir)?;
    client
        .enqueue_background_run(&right_mcp::internal_db::EnqueueBackgroundRunRequest {
            agent,
            request_id: crate::db::request_id(),
            run_id: new_run_id.clone(),
            producer_ref: Some(THEN_PRODUCER_REF.to_owned()),
            source_session_id: source_session_id.to_owned(),
            run_session_id: new_run_id.clone(),
            target_chat_id: action.target_chat_id,
            target_thread_id: action.target_thread_id,
            created_at: chrono::Utc::now().to_rfc3339(),
        })
        .await?;
    Ok(new_run_id)
}

/// Insert the queued `kind='background'` row for a `then` continuation, acquire
/// the per-session mutex on the SOURCE session (the triggered run we fork), and
/// spawn the continuation. A failed row insert is logged and returns early — the
/// continuation never runs without its tracking row, and the helper never panics.
#[allow(clippy::too_many_arguments)]
async fn spawn_then_continuation(
    action: ThenAction,
    source_session_id: String, // the triggered run's run_id == its session id
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    sandbox: Option<&crate::sandbox::Sandbox>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: &crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
) {
    // Insert the queued background row so delivery + recovery treat it normally.
    let new_run_id =
        match insert_then_continuation_row(agent_dir, &action, &source_session_id).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("then: insert background row failed: {e:#}");
                return;
            }
        };
    // Acquire the per-session mutex on the SOURCE session (we --resume/fork it).
    let session_guard = {
        let entry = session_locks
            .entry(source_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        entry.lock_owned().await
    };
    let status = crate::background::spawn_background_continuation(
        crate::background::BackgroundRunRequest {
            run_id: new_run_id,
            source_session_id,
            target_chat_id: action.target_chat_id,
            target_thread_id: action.target_thread_id,
            prompt: action.prompt,
        },
        agent_dir.to_path_buf(),
        agent_name.to_string(),
        model.map(|s| s.to_owned()),
        sandbox.map(Arc::clone),
        Arc::clone(internal_client),
        upgrade_lock,
        session_guard,
        debug,
    )
    .await;
    tracing::info!(?status, "cron then continuation handoff");
}

/// Delete old cron log files for a job, keeping the most recent `keep` files.
///
/// Logs live inside the guest, so this is a guest command; a degraded backend
/// simply skips the sweep (the next successful run retries it).
async fn cleanup_old_logs(
    job_name: &str,
    log_dir: &str,
    keep: usize,
    sandbox: Option<&crate::sandbox::Sandbox>,
) {
    // Defense-in-depth: job names should be alphanumeric + hyphens only (validated at creation).
    if !job_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        tracing::error!(job = %job_name, "job name contains unsafe characters, skipping log cleanup");
        return;
    }
    let Some(sandbox) = sandbox else {
        tracing::debug!(job = %job_name, "skipping cron log cleanup: sandbox unavailable");
        return;
    };
    // List matching files sorted newest-first, skip `keep`, delete the rest.
    // Using find+stat avoids ls parsing pitfalls with special characters in filenames.
    let cleanup_cmd = format!(
        "find {log_dir} -maxdepth 1 -name '{job_name}-*.ndjson' -printf '%T@ %p\\n' 2>/dev/null | sort -rn | tail -n +{} | cut -d' ' -f2- | xargs -r rm -f",
        keep + 1
    );
    match crate::sandbox::exec_argv(sandbox, &["sh", "-c", &cleanup_cmd]).await {
        Ok((_, 0)) => {}
        Ok((output, code)) => {
            tracing::warn!(job = %job_name, code, "cron log cleanup failed: {}", output.trim());
        }
        Err(e) => {
            tracing::warn!(job = %job_name, "cron log cleanup exec failed: {e:#}");
        }
    }
}

/// Classify the FailureKind of a cron job based on its exit code, its last
/// `result` stream event (if any), and the spec's configured limits.
fn classify_cron_failure(
    exit_code: Option<i32>,
    raw_detail: &str,
    max_budget_usd: f64,
    max_turns: Option<u32>,
) -> crate::reflection::FailureKind {
    let lower = raw_detail.to_ascii_lowercase();
    if lower.contains("max budget") || lower.contains("budget exceeded") {
        return crate::reflection::FailureKind::BudgetExceeded {
            limit_usd: max_budget_usd,
        };
    }
    if lower.contains("max turns") || lower.contains("turn limit") {
        return crate::reflection::FailureKind::MaxTurns {
            limit: max_turns.unwrap_or(0),
        };
    }
    crate::reflection::FailureKind::NonZeroExit {
        code: exit_code.unwrap_or(-1),
    }
}

/// Insert a freshly-started cron run with `status='running'`, snapshotting the
/// spec's delivery target onto the row so one-shot delivery survives spec
/// auto-deletion.
#[cfg(test)]
async fn insert_running_run(
    conn: &right_db::Connection,
    run_id: &str,
    job_name: &str,
    started_at: &str,
    log_path: &str,
    spec: &right_agent::cron_spec::CronSpec,
) -> Result<(), right_db::DbError> {
    // `async_runs.target_chat_id` is NOT NULL. Targetless cron runs are kept
    // explicit with sentinel 0; delivery reads convert it back with NULLIF.
    let target_chat_id = spec.target_chat_id.unwrap_or(0);

    right_agent::async_runs::insert_running_cron_run(
        conn,
        right_agent::async_runs::NewCronRun {
            id: run_id,
            job_name,
            started_at,
            log_path,
            target_chat_id: Some(target_chat_id),
            target_thread_id: spec.target_thread_id,
            force_notify: spec.trigger_force_notify,
        },
    )
    .await
}

fn cron_shutdown_failure_payload(
    run_id: &str,
    job_name: &str,
    reason: &str,
) -> Result<(String, String, String), right_db::DbError> {
    let content = format!(
        "Cron job `{job_name}` was interrupted because the bot is shutting down. Run `{run_id}` did not finish."
    );
    let content = right_rich_content::RichContent::paragraph(content)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let delivery_json = notify_delivery_json(&content, None)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let run_note = format!("Cron job `{job_name}` interrupted by shutdown");
    let error_json = serde_json::json!({
        "kind": "cron_shutdown_interrupted",
        "run_id": run_id,
        "job_name": job_name,
        "reason": reason,
    })
    .to_string();
    Ok((run_note, delivery_json, error_json))
}

#[cfg(test)]
async fn mark_cron_interrupted_by_shutdown(
    conn: &right_db::Connection,
    job_name: &str,
    reason: &str,
) -> Result<usize, right_db::DbError> {
    let tx = conn.transaction().await?;
    let rows = {
        let mut stmt = tx.prepare(
            "SELECT id, target_chat_id
             FROM async_runs
             WHERE kind = 'cron'
               AND producer_ref = ?1
               AND status = 'running'",
        )?;
        stmt.query_map([job_name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .await?
        .collect::<Result<Vec<_>, _>>()?
    };
    let mut updated = 0usize;
    let now = chrono::Utc::now().to_rfc3339();
    for (run_id, target_chat_id) in rows {
        let delivery_required = target_chat_id != 0;
        let (run_note, delivery_json, error_json) =
            cron_shutdown_failure_payload(&run_id, job_name, reason)?;
        // Inline UPDATE keeps the `status = 'running'` guard so a natural
        // finish racing with the shutdown sweep does not get clobbered.
        // `exit_code` is intentionally not in the SET list: a row at
        // `status='running'` has no exit code yet, and skipping the column
        // means a future race that wrote one would not be silently erased.
        let changed = tx
            .execute(
                "UPDATE async_runs
             SET run_note = ?2,
                 delivery_json = ?3,
                 error_json = ?4,
                 delivery_required = ?5,
                 delivery_status = ?6,
                 finished_at = ?7,
                 status = 'failed',
                 updated_at = ?7
             WHERE id = ?1
               AND kind = 'cron'
               AND status = 'running'",
                right_db::params![
                    run_id,
                    run_note,
                    delivery_required.then_some(delivery_json),
                    error_json,
                    delivery_required,
                    if delivery_required { "pending" } else { "none" },
                    &now,
                ],
            )
            .await?;
        updated += changed;
    }
    tx.commit().await?;
    Ok(updated)
}

async fn update_failed_run_record(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    run_id: &str,
    exit_code: Option<i32>,
) {
    update_run_record(client, agent, run_id, exit_code, "failed").await;
}
#[cfg(test)]
async fn persist_successful_cron_output(
    conn: &right_db::Connection,
    run_id: &str,
    cron_output: &CronReplyOutput,
    delivery_json: &str,
    force_notify: bool,
) -> Result<&'static str, right_db::DbError> {
    let (delivery_required, delivery_status) = match cron_output.delivery {
        CronDeliveryDecision::Notify { .. } => (true, "pending"),
        CronDeliveryDecision::Silent { .. } if force_notify => (true, "pending"),
        CronDeliveryDecision::Silent { .. } => (false, "none"),
    };
    right_agent::async_runs::persist_run_output(
        conn,
        run_id,
        right_agent::async_runs::RunOutput {
            run_note: Some(&cron_output.run_note),
            delivery_json: Some(delivery_json),
            error_json: None,
            delivery_required,
        },
    )
    .await?;
    Ok(delivery_status)
}

/// Eligible for the immediate-fire reconcile path: kinds that must run on
/// the next reconcile tick with no `cron_schedule()` (no `run_job_loop`
/// handle is spawned for these).
fn is_reconcile_tick_kind(kind: &right_agent::cron_spec::ScheduleKind) -> bool {
    matches!(kind, right_agent::cron_spec::ScheduleKind::Immediate)
}

/// Bypassed by the recurring-handle spawn loop: these kinds are either
/// fired immediately (`Immediate`) or fired by the absolute-time path (`RunAt`).
fn is_run_job_loop_skip_kind(kind: &right_agent::cron_spec::ScheduleKind) -> bool {
    matches!(
        kind,
        right_agent::cron_spec::ScheduleKind::RunAt(_)
            | right_agent::cron_spec::ScheduleKind::Immediate
    )
}

/// Resolve the model for a cron firing: the spec's own model wins; otherwise
/// fall back to the agent's current global `/model` snapshot; otherwise `None`
/// (CC default). Snapshotting at fire time keeps `/model` hot-reload working.
fn resolve_cron_model(
    spec: &CronSpec,
    global: &arc_swap::ArcSwap<Option<String>>,
) -> Option<String> {
    spec.model.clone().or_else(|| crate::snapshot_model(global))
}

/// Fetch the skills linked to a cron job from the owner. The linked-skill
/// set shapes the job prompt (which skills the agent is told it may use),
/// so a lookup failure must fail the run — silently substituting an empty
/// list is indistinguishable from a job with no linked skills.
async fn fetch_linked_skills_for_run(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    job_name: &str,
) -> Result<Vec<String>, right_mcp::internal_db::InternalDbError> {
    let response = client
        .cron_spec_detail(&right_mcp::internal_db::CronSpecDetailRequest {
            agent: agent.to_owned(),
            job_name: job_name.to_owned(),
        })
        .await?;
    Ok(response
        .detail
        .map(|detail| detail.linked_skills)
        .unwrap_or_default())
}

/// Execute one cron job: lock check → DB insert → subprocess → log write → DB update → lock delete.
///
/// Per D-02: subprocess failures log `tracing::error` only, do not propagate.
/// Results are persisted to the `async_runs` table (`run_note` + `delivery_json`).
/// A separate Telegram delivery loop reads pending rows and sends notifications.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    sandbox: Option<&crate::sandbox::Sandbox>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent_config::LearningConfig,
    session_locks: &crate::telegram::SessionLocks,
    progress_state: &crate::telegram::progress::ProgressState,
) {
    // Lock check (CRON-04). The lock age distinguishes a healthy in-flight
    // run from a wedged one at a glance.
    let lock_ttl = effective_lock_ttl(spec);
    if is_lock_fresh(agent_dir, job_name, lock_ttl) {
        let lock_age_secs = lock_age(agent_dir, job_name)
            .map(|d| d.num_seconds())
            .unwrap_or(-1);
        tracing::info!(job = %job_name, lock_age_secs, "skipping — previous run still active (lock fresh)");
        return;
    }

    // Block while upgrade is running (upgrade holds write lock).
    let _upgrade_guard = upgrade_lock.read().await;

    // Write lock file
    let lock_dir = agent_dir.join("crons").join(".locks");
    let lock_path = lock_dir.join(format!("{job_name}.json"));
    if let Err(e) = std::fs::create_dir_all(&lock_dir) {
        tracing::error!(job = %job_name, "failed to create lock dir: {e:#}");
        return;
    }
    let lock_json = serde_json::json!({"heartbeat": chrono::Utc::now().to_rfc3339()});
    if let Err(e) = std::fs::write(&lock_path, lock_json.to_string()) {
        tracing::error!(job = %job_name, "failed to write lock file: {e:#}");
        return;
    }

    // Prepare run record
    let run_id = uuid::Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now().to_rfc3339();

    // Guest-relative log path (agents read this via the Read tool).
    let log_filename = format!("{job_name}-{run_id}.ndjson");
    let sandbox_log_dir = "/sandbox/crons/logs";
    let log_path_str = format!("{sandbox_log_dir}/{log_filename}");

    // DB insert: status='running' (D-04)
    let (db_client, db_agent) = match crate::db::client_for_agent_dir(agent_dir) {
        Ok(pair) => pair,
        Err(error) => {
            tracing::error!(job = %job_name, "owner client resolution failed: {error:#}");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    if let Err(error) = db_client
        .cron_insert_running_run(&right_mcp::internal_db::CronInsertRunningRunRequest {
            agent: db_agent.clone(),
            request_id: crate::db::request_id(),
            run_id: run_id.clone(),
            job_name: job_name.to_owned(),
            started_at: started_at.clone(),
            log_path: log_path_str.clone(),
            target_chat_id: spec.target_chat_id,
            target_thread_id: spec.target_thread_id,
            force_notify: false,
        })
        .await
    {
        tracing::error!(job = %job_name, "owner run insert failed: {error:#}");
        std::fs::remove_file(&lock_path).ok();
        return;
    }
    // Every cron invocation needs a per-invocation MCP config and a bot-local
    // UDS target. Skill learning only changes which tools CC may invoke.
    // Registration is mandatory: without it channel_post is rejected as
    // unavailable and the run would record a misleading result, so a failure
    // marks the run failed instead of degrading.
    let learning_inline_enabled = learning.prefilter_enabled;
    let registered_cron = match crate::cc::invocation::register_non_foreground_invocation(
        crate::cc::invocation::NonForegroundInvocationRegistration {
            agent_name: agent_name.to_owned(),
            agent_dir: agent_dir.to_path_buf(),
            sandbox: match crate::cc::invocation::guard_no_sandboxed_host_exec(agent_name, sandbox)
            {
                Ok(sandbox) => Arc::clone(sandbox),
                Err(e) => {
                    tracing::error!(job = %job_name, "{e:#}");
                    update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
                    std::fs::remove_file(&lock_path).ok();
                    return;
                }
            },
            internal_client: Arc::clone(internal_client),
            kind: right_mcp::internal_client::ProgressInvocationKindDto::Cron,
            chat_id: spec.target_chat_id,
            thread_id: spec.target_thread_id,
            progress_state: Some(progress_state.clone()),
        },
    )
    .await
    {
        Ok(active) => active,
        Err(e) => {
            tracing::error!(job = %job_name, "failed to register cron invocation: {e:#}");
            update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    let cron_invocation_id: Option<String> = Some(registered_cron.invocation_id().to_owned());

    let disallowed_tools = if learning_inline_enabled {
        crate::cc::invocation::disallow_foreground_only_tools_keep_learning(
            crate::cc::invocation::baseline_disallowed_tools(),
        )
    } else {
        crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        )
    };
    let mcp_path = registered_cron.mcp_config_path().to_owned();

    // Per-agent notice token for the trusted `## Platform Notice Token` prompt
    // section and for stamping the force-notify / extra-instruction SYSTEM_NOTICE
    // markers. Reuse the run's conn.
    let notice_token = {
        use secrecy::ExposeSecret as _;
        match db_client
            .notice_token_get_or_create(&right_mcp::internal_db::NoticeTokenGetOrCreateRequest {
                agent: db_agent.clone(),
                request_id: crate::db::request_id(),
            })
            .await
        {
            Ok(response) => response.token.expose_secret().to_owned(),
            Err(error) => {
                tracing::error!(job = %job_name, "notice token owner fetch failed: {error:#}");
                registered_cron.cleanup().await;
                update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
                std::fs::remove_file(&lock_path).ok();
                return;
            }
        }
    };
    let linked_skills = match fetch_linked_skills_for_run(&db_client, &db_agent, job_name).await {
        Ok(linked_skills) => linked_skills,
        Err(error) => {
            tracing::error!(job = %job_name, "owner linked-skill lookup failed: {error:#}");
            registered_cron.cleanup().await;
            update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    let prompt_for_cc = compose_run_prompt(
        &spec.prompt,
        spec.trigger_force_notify,
        spec.trigger_extra_instruction.as_deref(),
        &notice_token,
        &linked_skills,
    );

    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(mcp_path),
        json_schema: Some(right_codegen::CRON_SCHEMA_JSON.into()),
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model: model.map(|s| s.to_owned()),
        max_budget_usd: Some(spec.max_budget_usd),
        max_turns: None,
        resume_session_id: None,
        new_session_id: Some(run_id.clone()),
        fork_session: false,
        extra_args: cron_extra_args(),
        allowed_tools: vec![],
        disallowed_tools,
        prompt: Some(prompt_for_cc),
        debug_flag: Some(std::sync::Arc::clone(&debug)),
    };

    let claude_args = invocation.into_args();

    let base_prompt = right_codegen::generate_system_prompt(agent_name, "/sandbox");

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> = match internal_client.mcp_instructions(agent_name).await
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
            tracing::warn!(job = %job_name, "failed to fetch MCP instructions: {e:#}");
            None
        }
    };

    // Cron jobs skip memory injection — cron prompts are static instructions,
    // not user queries. Agents can still call memory_recall/memory_retain MCP
    // tools explicitly from within cron prompts.
    let memory_mode: Option<crate::cc::prompt::MemoryMode> = None;

    // The guard already ran (its handle is what registered the invocation), so
    // this is the same live sandbox, re-borrowed for the turn itself.
    let sandbox = match crate::cc::invocation::guard_no_sandboxed_host_exec(agent_name, sandbox) {
        Ok(sandbox) => sandbox,
        Err(e) => {
            tracing::error!(job = %job_name, "{e:#}");
            registered_cron.cleanup().await;
            update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };

    let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
        &base_prompt,
        crate::cc::prompt::PromptMode::Cron,
        "/sandbox",
        &crate::cc::prompt::sandbox_prompt_file_path("system-prompt"),
        "/sandbox",
        &claude_args,
        mcp_instructions.as_deref(),
        memory_mode.as_ref(),
        None,
        None,
        Some(&notice_token),
    );
    let assembly_script = cron_wrapper_script(&assembly_script, sandbox_log_dir, &log_filename);

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let run_started = tokio::time::Instant::now();
    let mut child = match crate::cc::invocation::build_claude_script_command(
        assembly_script,
        agent_dir,
        sandbox,
    )
    .await
    {
        Ok(command) => match command
            .stdout(crate::cc::sandbox_process::Capture::Pipe)
            .stderr(crate::cc::sandbox_process::Capture::Pipe)
            .spawn()
            .await
        {
            Ok(child) => child,
            Err(e) => {
                tracing::error!(job = %job_name, "spawn failed: {e:#}");
                registered_cron.cleanup().await;
                update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
                std::fs::remove_file(&lock_path).ok();
                return;
            }
        },
        Err(e) => {
            tracing::error!(job = %job_name, "command build failed: {e:#}");
            registered_cron.cleanup().await;
            update_failed_run_record(&db_client, &db_agent, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };

    // Stream stdout; break on the terminal result event (do not wait for EOF —
    // the guest stdout pipe can linger open after CC exits). A wall-clock cap
    // bounds the whole drive: a guest that dies or wedges without a terminal
    // `result` line must not park this task until bot restart — the cap fires,
    // the child is killed (also via `SandboxChild::Drop` on return), and the
    // run flows through the normal failure path instead of stranding the lock.
    let run_elapsed = run_started.elapsed();
    let outcome = match tokio::time::timeout(
        CRON_RUN_TIMEOUT.saturating_sub(run_elapsed),
        consume_cron_stream(&mut child),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_elapsed) => {
            tracing::error!(
                job = %job_name,
                run_id = %run_id,
                child_pid = child.pid(),
                elapsed_secs = run_started.elapsed().as_secs(),
                "cron run exceeded the wall-clock cap — killing the guest and failing the run"
            );
            child.kill().await;
            CronStreamOutcome::Failed {
                collected_lines: Vec::new(),
            }
        }
    };
    let collected_lines: Vec<String> = match &outcome {
        CronStreamOutcome::Success { collected_lines }
        | CronStreamOutcome::Failed { collected_lines } => collected_lines.clone(),
    };

    // Inline-authoring cleanup: the CC child has emitted its terminal result, so
    // any skill_learning_start/finish calls have already executed and their DB
    // rows are durable. Unregister the learning invocation and remove its
    // per-invocation MCP config now. This is the single cleanup point on the path
    // every outcome (Success AND Failed) flows through after the child finishes —
    // there are no early returns between `consume_cron_stream` and here, and all
    // outcome-specific processing (the `match outcome` below, reflection, usage
    // insert) happens after it. The finish rows persist past cleanup, so the
    // downstream probe-skip check still works.
    registered_cron.cleanup().await;

    // Post-stream-loop cleanup. `SandboxChild::Drop` kills the guest process on
    // function return, so a hang here can never outlive `execute_job`. Inside
    // the function we still bound each blocking syscall (the same wedged-pipe
    // defense the worker uses).
    let child_pid = child.pid();

    let wait_started = tokio::time::Instant::now();
    // Best-effort only: the outcome (Task 1) decides success/failure, not the
    // exit code. A wedged transport leaves `exit_code` None; we still deliver
    // a Success outcome.
    let exit_code: Option<i32> = match tokio::time::timeout(
        std::time::Duration::from_secs(POST_BREAK_WAIT_TIMEOUT_SECS),
        child.wait(),
    )
    .await
    {
        Ok(Ok(code)) => Some(code),
        Ok(Err(e)) => {
            tracing::error!(job = %job_name, child_pid, "wait failed: {e:#}");
            None
        }
        Err(_) => {
            tracing::error!(
                job = %job_name,
                child_pid,
                elapsed_ms = wait_started.elapsed().as_millis() as u64,
                "child.wait timed out — guest process wedged; SandboxChild::Drop will kill it on return",
            );
            None
        }
    };
    tracing::debug!(
        job = %job_name,
        child_pid,
        exit_code = ?exit_code,
        wait_ms = wait_started.elapsed().as_millis() as u64,
        "post-break: child wait completed (best-effort)",
    );

    // stderr is still owned by child — bounded read so a wedged pipe doesn't
    // stall the cron worker.
    let stderr_bytes = if let Some(mut stderr) = child.stderr() {
        let mut buf = Vec::new();
        let read_started = tokio::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(POST_BREAK_STDERR_TIMEOUT_SECS),
            stderr.read_to_end(&mut buf),
        )
        .await
        {
            Ok(Ok(n)) => tracing::debug!(
                job = %job_name,
                child_pid,
                bytes = n,
                read_ms = read_started.elapsed().as_millis() as u64,
                "post-break: stderr drained",
            ),
            Ok(Err(e)) => {
                tracing::warn!(job = %job_name, child_pid, "failed to read stderr: {e:#}")
            }
            Err(_) => tracing::debug!(
                job = %job_name,
                child_pid,
                bytes_so_far = buf.len(),
                elapsed_ms = read_started.elapsed().as_millis() as u64,
                "post-break: stderr drain timed out (transport keeps the pipe open after the terminal result; benign)",
            ),
        }
        buf
    } else {
        Vec::new()
    };
    let stderr_str = String::from_utf8_lossy(&stderr_bytes);

    // Determine status (D-02)
    let status = match &outcome {
        CronStreamOutcome::Success { .. } => "success",
        CronStreamOutcome::Failed { .. } => "failed",
    };
    if matches!(outcome, CronStreamOutcome::Failed { .. }) {
        // The stderr tail is the primary observability fix: a guest that dies
        // before `claude` runs (shell errors, missing binaries) writes the
        // reason here, and nothing else in the stream carries it.
        let stderr_tail: String = stderr_str
            .chars()
            .rev()
            .take(STDERR_TAIL_CHARS)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        // Distinguish a captured error result from a genuinely absent one — the
        // old blanket "no terminal result" wording masked budget/turn-limit
        // failures that DID emit a result.
        if let Some(line) = find_last_result_line(&collected_lines) {
            let detail = terminal_failure_detail(line);
            tracing::error!(
                job = %job_name,
                exit_code = ?exit_code,
                detail = detail.as_deref().unwrap_or("error result"),
                stderr_tail = %stderr_tail,
                "cron job ended with an error result"
            );
        } else {
            tracing::error!(
                job = %job_name,
                run_id = %run_id,
                exit_code = ?exit_code,

                stderr_tail = %stderr_tail,
                "cron job produced no terminal result"
            );
        }
    }

    // The terminal `status='success'` transition is deferred to the success branch
    // below so it stays atomic with the output persist. The failure branch performs
    // its own `update_run_record(failed)` before reflection. This prevents a state
    // where `status='success'` is committed but output preservation fails, leaving
    // the row at `delivery_status='none'` and silently dropping user-visible output.

    // Delete lock on completion (CRON-04)
    std::fs::remove_file(&lock_path).ok();

    // Retention: keep last 10 log files per job (fire-and-forget to keep the
    // guest round-trip off the hot path).
    let job_name_owned = job_name.to_owned();
    let log_dir_owned = sandbox_log_dir.to_owned();
    let sandbox_owned = Arc::clone(sandbox);
    tokio::spawn(async move {
        cleanup_old_logs(&job_name_owned, &log_dir_owned, 10, Some(&sandbox_owned)).await;
    });

    tracing::info!(job = %job_name, run_id = %run_id, %status, "cron job completed");

    // Parse cron output and persist to DB
    match outcome {
        CronStreamOutcome::Success { collected_lines } => {
            match parse_cron_output(&collected_lines) {
                Ok(cron_output) => {
                    // Download attachments from sandbox to host outbox
                    let delivery_json = match &cron_output.delivery {
                        CronDeliveryDecision::Notify {
                            content,
                            attachments: Some(atts),
                        } => {
                            let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                            if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
                                tracing::error!(job = %job_name, "failed to create cron outbox dir: {e:#}");
                            } else {
                                for att in atts {
                                    let dest = outbox_dir.join(attachment_filename(&att.path));
                                    if let Err(e) = sandbox.fs_copy_to_host(&att.path, &dest).await
                                    {
                                        tracing::error!(
                                            job = %job_name,
                                            path = %att.path,
                                            "failed to download cron attachment: {e:#}"
                                        );
                                    }
                                }
                            }

                            let host_attachments: Vec<OutboundAttachment> = atts
                                .iter()
                                .map(|att| OutboundAttachment {
                                    kind: att.kind,
                                    path: outbox_dir
                                        .join(attachment_filename(&att.path))
                                        .to_string_lossy()
                                        .into_owned(),
                                    filename: att.filename.clone(),
                                    caption: att.caption.clone(),
                                    media_group_id: att.media_group_id.clone(),
                                })
                                .collect();
                            notify_delivery_json(content, Some(&host_attachments))
                            .map_err(|e| {
                                tracing::error!(job = %job_name, "failed to serialize delivery_json: {e:#}");
                            })
                            .ok()
                        }
                        // Forced silent run: deliver the silent reason as notify
                        // content so there is always something to report. Otherwise
                        // serialize the decision as-is. Both yield the same Result
                        // type, so one map_err/ok covers both.
                        other => {
                            if spec.trigger_force_notify
                                && let Some(reason) = other.silent_reason()
                            {
                                let content = match right_rich_content::RichContent::literal(
                                    format!("Verification run — nothing to report. {reason}"),
                                ) {
                                    Ok(content) => content,
                                    Err(error) => {
                                        tracing::error!(job = %job_name, "verification notice was empty: {error}");
                                        return;
                                    }
                                };
                                notify_delivery_json(&content, None)
                            } else {
                                serde_json::to_string(other)
                            }
                            .map_err(|e| {
                                tracing::error!(job = %job_name, "failed to serialize delivery_json: {e:#}");
                            })
                            .ok()
                        }
                    };

                    // Persist output and flip status='success' atomically. If either
                    // write fails, roll back and mark the row 'failed' so the operator
                    // sees the run as broken instead of stuck at 'success' with no
                    // delivery payload.
                    if let Some(delivery_json) = delivery_json {
                        let delivery_required =
                            matches!(cron_output.delivery, CronDeliveryDecision::Notify { .. })
                                || spec.trigger_force_notify;
                        let tx_result = db_client
                            .persist_run_output(&right_mcp::internal_db::PersistRunOutputRequest {
                                agent: db_agent.clone(),
                                request_id: crate::db::request_id(),
                                run_id: run_id.clone(),
                                run_note: Some(cron_output.run_note.clone()),
                                delivery_json: Some(delivery_json),
                                error_json: None,
                                delivery_required,
                                exit_code,
                                status: "success".to_owned(),
                            })
                            .await
                            .map(|response| response.delivery_status);

                        match tx_result {
                            Ok(delivery_status) => {
                                tracing::info!(
                                    job = %job_name,
                                    has_notify = matches!(cron_output.delivery, CronDeliveryDecision::Notify { .. }),
                                    delivery_status,
                                    silent_reason = cron_output.delivery.silent_reason().unwrap_or("-"),
                                    "cron output persisted to DB"
                                );
                                // `then` continuation: fork this run's session for a
                                // runtime-guaranteed follow-up. Awaited inline; returns
                                // after the continuation's system/init, like the worker
                                // hand-off. Takes the SOURCE-session guard for the
                                // hand-off window; this and the learning-probe fork
                                // below both use `fork_session`, so the source
                                // transcript is read-only and concurrent forks are safe.
                                if let Some(action) = resolve_then_action(spec, true) {
                                    spawn_then_continuation(
                                        action,
                                        run_id.clone(),
                                        agent_dir,
                                        agent_name,
                                        model,
                                        Some(sandbox),
                                        internal_client,
                                        Arc::clone(&upgrade_lock),
                                        session_locks,
                                        Arc::clone(&debug),
                                    )
                                    .await;
                                }
                                // Skill learning: recurring cron runs feed the
                                // shared pipeline (prefilter → probe-writer fork
                                // of this run's session). Fire-and-forget; never
                                // affects delivery or the run record.
                                let learning_eligible = learning.prefilter_enabled
                                    && schedule_kind_feeds_learning(&spec.schedule_kind);
                                // Inline auto-link: independent of result-line parseability. Inline-authored
                                // skills are recorded under cron_invocation_id during the turn; link them even
                                // if the terminal result line is missing/unparseable. Disjoint from the async
                                // probe-writer seam (different invocation_id) so no double-linking.
                                if learning_eligible
                                    && let Some(invocation_id) = cron_invocation_id.as_deref()
                                    && let Err(error) = internal_client
                                        .learning_link_cron_authored(&right_mcp::internal_db::LearningLinkCronAuthoredRequest {
                                            agent: agent_name.to_owned(),
                                            job_name: job_name.to_owned(),
                                            invocation_id: invocation_id.to_owned(),
                                        })
                                        .await
                                {
                                    tracing::warn!(job = %job_name, "cron inline owner auto-link failed: {error:#}");
                                }
                                if learning_eligible
                                    && let Some((reply_text, num_turns, cost_usd)) =
                                        parse_result_stats(&collected_lines)
                                {
                                    let anchor = crate::telegram::worker::ProbeAnchor {
                                        user_msg_text: spec.prompt.clone(),
                                        assistant_reply_text: reply_text,
                                        main_session_uuid: run_id.clone(),
                                        captured_at: chrono::Utc::now(),
                                        // `0` sentinel for untargeted crons: chat/thread are
                                        // attribution-only on usage/skip rows here — baselines
                                        // are agent-wide and the prefilter does no chat-scoped
                                        // search, so the sentinel cannot mis-scope anything.
                                        chat_id: spec.target_chat_id.unwrap_or(0),
                                        thread_id: spec.target_thread_id.unwrap_or(0),
                                        num_turns,
                                        total_cost_usd: cost_usd,
                                        wall_elapsed_ms: run_started.elapsed().as_millis() as u64,
                                        // Empty by design: the cron output schema carries no
                                        // per-skill receipts (unlike the foreground reply schema),
                                        // so there is nothing to extract from `collected_lines`.
                                        // The prefilter reads the full rightx-* skill index
                                        // itself, so `PatchExisting` stays reachable; the only
                                        // effect of empty receipts is CreateNew-leaning framing,
                                        // acceptable for v1.
                                        used_skill_receipts: Vec::new(),
                                        learning_invocation_id: cron_invocation_id.clone(),
                                        origin_cron_job: Some(job_name.to_owned()),
                                    };
                                    let learn_ctx = crate::learning_pipeline::PostTurnLearningCtx {
                                        agent_dir: agent_dir.to_path_buf(),
                                        agent_db_dir: agent_dir.to_path_buf(),
                                        agent_name: agent_name.to_owned(),
                                        sandbox: Some(Arc::clone(sandbox)),
                                        internal_client: Arc::clone(internal_client),
                                        session_locks: session_locks.clone(),
                                        debug_flag: Arc::clone(&debug),
                                        prefilter_model: learning
                                            .prefilter_model
                                            .clone()
                                            .unwrap_or_else(|| {
                                                crate::learning_pipeline::DEFAULT_PREFILTER_MODEL
                                                    .to_owned()
                                            }),
                                        probe_writer_enabled: learning.probe_writer_enabled,
                                        probe_writer_model_override: learning
                                            .probe_writer_model
                                            .clone(),
                                        probe_writer_model_fallback: model.map(|s| s.to_owned()),
                                        daily_budget: learning.max_daily_budget_usd,
                                        baseline_window_days: learning.baseline_window_days,
                                        baseline_min_sample: learning.baseline_min_sample,
                                    };
                                    let learning_agent = agent_name.to_owned();
                                    tokio::spawn(async move {
                                        if let Err(error) = crate::learning_pipeline::run_post_turn(
                                            learn_ctx, anchor,
                                        )
                                        .await
                                        {
                                            tracing::error!(agent = %learning_agent, "post-turn learning pipeline failed: {error:#}");
                                        }
                                    });
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    job = %job_name,
                                    "failed to persist cron output atomically; marking run failed: {e:#}"
                                );
                                update_failed_run_record(&db_client, &db_agent, &run_id, exit_code)
                                    .await;
                            }
                        }
                    } else {
                        tracing::error!(
                            job = %job_name,
                            "failed to produce delivery_json; marking run failed"
                        );
                        update_failed_run_record(&db_client, &db_agent, &run_id, exit_code).await;
                    }
                }
                Err(reason) => {
                    tracing::warn!(job = %job_name, reason, "failed to parse cron output");
                    let error_json_str = serde_json::json!({
                        "kind": "cron_parse_failed",
                        "reason": reason,
                    })
                    .to_string();
                    let tx_result = db_client
                        .persist_run_output(&right_mcp::internal_db::PersistRunOutputRequest {
                            agent: db_agent.clone(),
                            request_id: crate::db::request_id(),
                            run_id: run_id.clone(),
                            run_note: None,
                            delivery_json: None,
                            error_json: Some(error_json_str),
                            delivery_required: false,
                            exit_code,
                            status: "failed".to_owned(),
                        })
                        .await;

                    if let Err(e) = tx_result {
                        tracing::error!(
                            job = %job_name,
                            "failed to persist cron parse error atomically: {e:#}"
                        );
                        update_failed_run_record(&db_client, &db_agent, &run_id, exit_code).await;
                    }

                    // Parse failed but CC produced a terminal result, so the run's
                    // session exists — fire a run_on=failure/always `then` (same as
                    // the CronStreamOutcome::Failed arm). Pre-CC-start infra failures
                    // (spawn/binary/sandbox) have no session and are intentionally not
                    // covered — there is nothing to fork.
                    if let Some(action) = resolve_then_action(spec, false) {
                        spawn_then_continuation(
                            action,
                            run_id.clone(),
                            agent_dir,
                            agent_name,
                            model,
                            Some(sandbox),
                            internal_client,
                            Arc::clone(&upgrade_lock),
                            session_locks,
                            Arc::clone(&debug),
                        )
                        .await;
                    }
                }
            }
        }
        CronStreamOutcome::Failed { collected_lines } => {
            // Failure path: commit terminal status='failed' before reflection runs.
            // Reflection then writes its own failure notify via persist_run_output,
            // which is consistent with status='failed'.
            update_run_record(&db_client, &db_agent, &run_id, exit_code, "failed").await;
            let exit_str = exit_code.map_or("unknown".to_string(), |c| c.to_string());
            // CC error results (budget/turn limits) carry no `result` text — read
            // the reason from the result `subtype` so it survives to the notice,
            // the classifier, and `error_json` instead of degrading to bare exit
            // code. Falls back to stderr for non-result failures.
            let raw_detail = find_last_result_line(&collected_lines)
                .and_then(terminal_failure_detail)
                .unwrap_or_else(|| stderr_str.to_string());
            let raw_content =
                format!("Cron job `{job_name}` failed (exit code {exit_str}):\n{raw_detail}");

            let failure_kind =
                classify_cron_failure(exit_code, &raw_detail, spec.max_budget_usd, None);

            // Machine-readable failure record, persisted regardless of whether
            // reflection runs or succeeds — so the reason is never lost (the
            // `error_json IS NULL` gap that surfaced only "exit code 1").
            let error_json_str = serde_json::json!({
                "kind": "cron_failed",
                "exit_code": exit_code,
                "failure": format!("{failure_kind:?}"),
                "detail": raw_detail.as_str(),
            })
            .to_string();
            let run_note_detail: String = raw_detail.chars().take(200).collect();

            // Reflection `--resume`s the run's session — a billable CC turn.
            // Deterministic failures make it futile: a classified error result
            // (529/429/5xx overload, rate limit, turn limit) fails the same
            // way on resume, and a budget-exhausted session immediately
            // re-hits its cumulative `--max-budget-usd` cap. Skip both and
            // report the classified user message (or raw detail for the
            // budget cap); reflect everything else.
            let skip = skip_reflection_decision(&collected_lines, &failure_kind);
            let reflected_content = if let Some(classified) = skip {
                match classified {
                    Some(c) => {
                        tracing::info!(
                            job = %job_name,
                            detail = %c.detail,
                            "cron failure classified; skipping futile reflection"
                        );
                        // user_message embeds unbounded result text; the
                        // platform constructor splits at the rich limit
                        // instead of failing (or panicking) on it.
                        right_rich_content::RichContent::platform_text(c.user_message)
                            .expect("classified failure message is non-empty")
                    }
                    None => {
                        tracing::info!(
                            job = %job_name,
                            detail = %raw_detail,
                            "cron hit budget cap; skipping futile reflection"
                        );
                        // raw_content embeds unbounded raw_detail (terminal
                        // result text or raw stderr): never length-validated.
                        right_rich_content::RichContent::platform_text(raw_content.clone())
                            .expect("cron failure message is non-empty")
                    }
                }
            } else {
                // Best-effort ring buffer: parse last ~5 stream-json lines from
                // collected_lines, keeping only displayable events. Chronological
                // order (oldest → newest) to match worker's EventRingBuffer.
                let mut tail_newest_first: Vec<_> = collected_lines
                    .iter()
                    .rev()
                    .take(10)
                    .map(|line| crate::cc::stream::parse_stream_event(line))
                    .filter(|e| {
                        matches!(
                            e,
                            crate::cc::stream::StreamEvent::Text(_)
                                | crate::cc::stream::StreamEvent::Thinking
                                | crate::cc::stream::StreamEvent::ToolUse { .. }
                        )
                    })
                    .take(5)
                    .collect();
                tail_newest_first.reverse();
                let ring_tail: std::collections::VecDeque<_> = tail_newest_first.into();

                let refl_ctx = crate::reflection::ReflectionContext {
                    session_uuid: run_id.clone(),
                    limits: crate::reflection::ReflectionLimits::CRON,
                    agent_name: agent_name.to_string(),
                    agent_dir: agent_dir.to_path_buf(),
                    sandbox: Some(Arc::clone(sandbox)),
                    parent_source: crate::reflection::ParentSource::Cron {
                        job_name: job_name.to_string(),
                    },
                    model: model.map(String::from),
                    debug: Some(std::sync::Arc::clone(&debug)),
                };

                match crate::reflection::reflect_on_failure(refl_ctx, failure_kind, ring_tail).await
                {
                    Ok(text) => {
                        tracing::info!(job = %job_name, "cron reflection reply produced");
                        text
                    }
                    Err(e) => {
                        tracing::warn!(job = %job_name, "cron reflection failed: {e:#}; using raw content");
                        right_rich_content::RichContent::platform_text(raw_content.clone())
                            .expect("cron failure message is non-empty")
                    }
                }
            };

            match notify_delivery_json(&reflected_content, None) {
                Ok(json) => {
                    if let Err(error) = db_client
                        .persist_run_output(&right_mcp::internal_db::PersistRunOutputRequest {
                            agent: db_agent.clone(),
                            request_id: crate::db::request_id(),
                            run_id: run_id.clone(),
                            run_note: Some(run_note_detail.clone()),
                            delivery_json: Some(json),
                            error_json: Some(error_json_str.clone()),
                            delivery_required: true,
                            exit_code,
                            status: "failed".to_owned(),
                        })
                        .await
                    {
                        tracing::error!(job = %job_name, "failed to persist failure notify through owner: {error:#}");
                    }
                }
                Err(e) => {
                    tracing::error!(job = %job_name, "failed to serialize failure notify: {e:#}");
                }
            }

            // `then` continuation on failure. Sequenced AFTER reflection: reflection
            // resumed A's session WITHOUT the per-session guard, so the fork here
            // must not overlap it. `spawn_then_continuation` takes the guard and
            // forks A's session, leaving the reflected session intact.
            if let Some(action) = resolve_then_action(spec, false) {
                spawn_then_continuation(
                    action,
                    run_id.clone(),
                    agent_dir,
                    agent_name,
                    model,
                    Some(sandbox),
                    internal_client,
                    Arc::clone(&upgrade_lock),
                    session_locks,
                    Arc::clone(&debug),
                )
                .await;
            }
        }
    }

    if let Some(result_line) = find_last_result_line(&collected_lines) {
        match crate::cc::stream::parse_usage_full(result_line) {
            Some(mut breakdown) => {
                // Scan all lines for the init event (first line that matches wins).
                breakdown.api_key_source = collected_lines
                    .iter()
                    .find_map(|l| crate::cc::stream::parse_api_key_source(l))
                    .unwrap_or_else(|| "none".into());
                if let Err(error) = db_client
                    .usage_insert_event(&right_mcp::internal_db::UsageInsertEventRequest {
                        agent: db_agent.clone(),
                        request_id: crate::db::request_id(),
                        source: right_mcp::internal_db::UsageSourceDto::Cron {
                            job_name: job_name.to_owned(),
                        },
                        event: crate::db::usage_dto(&breakdown),
                    })
                    .await
                {
                    tracing::warn!(job = %job_name, "owner usage insert failed: {error:#}");
                }
            }
            None => {
                tracing::warn!(job = %job_name, "result event missing required usage fields");
            }
        }
    }
}
/// Decide whether a failed cron run may skip the reflection `--resume` turn.
///
/// Reflection resumes the run's session, so it is a billable CC turn. Two
/// failure classes are deterministic — the resumed turn fails the same way:
/// a classified error result (529/429/5xx overload, rate limit, turn limit)
/// and the budget cap (whose `--max-budget-usd` is cumulative, so the resume
/// immediately re-hits it).
///
/// Returns `Some(classified)` to skip reflection and report
/// [`FailureClassification::user_message`] directly; `Some(None)` to skip
/// reflection with only the raw detail (budget cap); `None` to reflect.
fn skip_reflection_decision(
    collected_lines: &[String],
    failure_kind: &crate::reflection::FailureKind,
) -> Option<Option<FailureClassification>> {
    if let Some(classified) = classify_failed_result(collected_lines) {
        return Some(Some(classified));
    }
    if matches!(
        failure_kind,
        crate::reflection::FailureKind::BudgetExceeded { .. }
    ) {
        return Some(None);
    }
    None
}

/// Return `true` iff the parsed NDJSON value is a CC `{"type":"result"}` event.
///
/// The single owner of the "what counts as a result event" test. Shared by
/// [`find_last_result_line`], [`parse_cron_output`], and
/// [`terminal_result_is_error`]; each of those adds its own extra guard
/// (`parent_tool_use_id`, scan direction) on top of this core check, so a future
/// change to the result-event shape has one place to update here.
fn is_result_line(v: &serde_json::Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("result")
}

/// Return the last NDJSON line whose `type` field equals `"result"`.
fn find_last_result_line(lines: &[String]) -> Option<&str> {
    lines.iter().rev().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        is_result_line(&v).then_some(line.as_str())
    })
}

/// Extract `(result_text, num_turns, total_cost_usd)` from the terminal
/// `{"type":"result"}` line. `None` if there is no result line. Missing
/// `num_turns`/`total_cost_usd` default to 0 — anchor capture must never panic
/// on a partial line.
fn parse_result_stats(lines: &[String]) -> Option<(String, u32, f64)> {
    let line = find_last_result_line(lines)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let text = v
        .get("result")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_owned();
    let turns = v
        .get("num_turns")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let cost = v
        .get("total_cost_usd")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    Some((text, turns, cost))
}

/// Recurring crons are the only kind whose runs feed skill learning — one-shot
/// (`OneShotCron`/`RunAt`/`Immediate`) runs never repeat, so a learned skill
/// cannot amortize.
fn schedule_kind_feeds_learning(kind: &right_agent::cron_spec::ScheduleKind) -> bool {
    matches!(kind, right_agent::cron_spec::ScheduleKind::Recurring(_))
}

/// A user-facing summary of why a CC run failed, derived deterministically from
/// the terminal `result` line — no extra CC call. A 529/overload means another
/// invocation would just fail the same way, so reflection is the wrong tool here.
///
/// Sibling of [`terminal_failure_detail`], which extracts a raw detail to *feed*
/// reflection (a [`crate::reflection::FailureKind`]) on the cron path. This one
/// instead produces the finished user message directly and is HTTP-status-aware
/// (`api_error_status`), because the background path has no reflection stage. If
/// you extend one with a new error shape, check whether the other needs it too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailureClassification {
    /// Message relayed to the user (carries the facts; the agent naturalizes it).
    pub user_message: String,
    /// Internal detail for `error_json`/logs — never the sole user-facing text.
    pub detail: String,
}

/// Inspect CC's terminal `result` line and, when it reports `is_error: true`,
/// build a [`FailureClassification`] explaining what happened in user terms.
///
/// Returns `None` when there is no terminal result line or the run did not
/// error (the caller then proceeds with normal parsing / generic handling).
/// Classification uses CC's own observable signals (`api_error_status`,
/// `subtype`, `result`) rather than inferring from side effects.
pub(crate) fn classify_failed_result(lines: &[String]) -> Option<FailureClassification> {
    let line = find_last_result_line(lines)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("is_error").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }

    let api_status = v
        .get("api_error_status")
        .and_then(serde_json::Value::as_i64);
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
    let result_text = v
        .get("result")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .trim();

    let user_message = match api_status {
        Some(529) | Some(503) => "The AI backend was temporarily overloaded, so the background task \
             couldn't run — no work was done and nothing was lost. This is a \
             transient issue on Anthropic's side; ask again in a moment and it \
             should go through."
            .to_string(),
        Some(429) => "The AI backend hit a rate limit before the background task could run, so it \
             produced no result and nothing was lost. Wait a moment, then try again."
            .to_string(),
        Some(status) if (500..600).contains(&status) => format!(
            "The AI backend returned a server error (HTTP {status}) before the background task \
             could finish, so it produced no result. This is usually transient — try again shortly."
        ),
        Some(status) => format!(
            "The AI backend rejected the background task (HTTP {status}), so it produced no result."
        ),
        None if subtype == "error_max_turns" => {
            "The background task hit its turn limit before finishing, so it didn't produce a result."
                .to_string()
        }
        None if !result_text.is_empty() => format!(
            "The background task ended with an error before producing a result: {result_text}"
        ),
        None => "The background task ended with an error before it could produce a result."
            .to_string(),
    };

    let detail = match (api_status, result_text.is_empty()) {
        (Some(status), false) => format!("api_error_status={status}; result={result_text}"),
        (Some(status), true) => format!("api_error_status={status}; subtype={subtype}"),
        (None, false) => format!("subtype={subtype}; result={result_text}"),
        (None, true) => format!("subtype={subtype}"),
    };

    Some(FailureClassification {
        user_message,
        detail,
    })
}

/// Outcome of consuming the cron CC subprocess stdout stream.
///
/// `collected_lines` carries every NDJSON line read from stdout so the caller
/// can run [`parse_cron_output`].
#[derive(Debug)]
pub(crate) enum CronStreamOutcome {
    /// A terminal top-level `{"type":"result"}` event with `is_error` false (or
    /// absent) was observed — the loop broke on it. The success path parses and
    /// delivers the real notification.
    Success { collected_lines: Vec<String> },
    /// Either no terminal top-level result was seen (EOF / read error), or the
    /// terminal result carried `is_error: true` (auth failure, budget/turn
    /// limit). The failure path runs reflection and delivers a failure notice.
    Failed { collected_lines: Vec<String> },
}

/// Classify a line as the terminal top-level CC result event.
///
/// Returns `Some(is_error)` when `line` is the terminal top-level
/// `{"type":"result"}` summary CC emits exactly once at the end of a turn (the
/// bool is the event's `is_error` flag); returns `None` for any other line.
///
/// Sub-agent (Task tool) results arrive as nested `assistant`/`user` messages
/// carrying `parent_tool_use_id`, so [`is_result_line`] is already terminal; the
/// `parent_tool_use_id` absent/null check is defense-in-depth.
///
/// The `is_error` bool drives outcome routing in [`consume_cron_stream`]: a
/// terminal result with `is_error: true` (auth failure, budget/turn limit) still
/// breaks the read loop — avoiding the wedged-stdout hang — but yields
/// [`CronStreamOutcome::Failed`] so the failure path (reflection + user notice)
/// runs, matching the pre-result-driven `exit_status`-gated behavior. Without
/// the `is_error` check, an error result would be mis-routed to the success
/// path, fail to parse, and silently drop the user-facing failure notification.
///
/// Shares the core `type == "result"` test ([`is_result_line`]) with
/// [`find_last_result_line`] and [`parse_cron_output`].
fn terminal_result_is_error(line: &str) -> Option<bool> {
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let top_level = v.get("parent_tool_use_id").is_none_or(|p| p.is_null());
    if !is_result_line(&v) || !top_level {
        return None;
    }
    Some(v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false))
}

/// Extract a human-readable, classifiable failure detail from a terminal CC
/// `{"type":"result"}` line.
///
/// CC error results — `error_max_budget_usd`, `error_max_turns`,
/// `error_during_execution` — carry NO `result` text; their only signal is the
/// `subtype` (+ `total_cost_usd`). Reading just the `result` field therefore
/// drops the reason, mis-routes [`classify_cron_failure`] to `NonZeroExit`, and
/// leaves the user notice empty. This synthesizes a detail that both reads
/// cleanly and contains the keywords [`classify_cron_failure`] matches. Returns
/// `None` for non-result lines and for non-error results without `result` text.
fn terminal_failure_detail(line: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if !is_result_line(&v) {
        return None;
    }
    // Prefer explicit assistant result text when CC provides it.
    if let Some(text) = v
        .get("result")
        .and_then(|r| r.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(text.to_owned());
    }
    if !v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
        return None;
    }
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("error");
    let cost = match v.get("total_cost_usd").and_then(|c| c.as_f64()) {
        Some(c) => format!(" (spent ${c:.2})"),
        None => String::new(),
    };
    Some(match subtype {
        "error_max_budget_usd" => format!("max budget exceeded{cost}"),
        "error_max_turns" => "max turns reached".to_owned(),
        other => format!("{other}{cost}"),
    })
}

/// Consume the cron CC subprocess stdout line-by-line and classify the outcome.
///
/// Breaks immediately on the terminal top-level `result` event (does NOT wait
/// for EOF — the SSH stdout pipe can linger open after CC exits). The break
/// happens for an error result too, so a wedged transport can never hang the
/// task; only the [`CronStreamOutcome`] classification depends on `is_error`: a
/// non-error terminal result is `Success`, an `is_error: true` result is
/// `Failed` (so reflection runs), and EOF / read error without any terminal
/// result is `Failed`. There is no wall-clock bound here; a turn that never
/// emits a result is bounded by the shutdown-drain (`SHUTDOWN_JOB_TIMEOUT`).
pub(crate) async fn consume_cron_stream(
    child: &mut crate::cc::sandbox_process::SandboxChild,
) -> CronStreamOutcome {
    consume_cron_lines(child.stdout().expect("stdout piped")).await
}

/// Classification core, generic over the reader so it is exercisable without a
/// live sandbox. See [`consume_cron_stream`] for the semantics.
pub(crate) async fn consume_cron_lines<R: tokio::io::AsyncRead + Unpin>(
    stdout: R,
) -> CronStreamOutcome {
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut collected_lines: Vec<String> = Vec::new();
    // `Some(is_error)` once the terminal top-level result is seen.
    let mut terminal_is_error: Option<bool> = None;
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if terminal_is_error.is_none() {
                    terminal_is_error = terminal_result_is_error(&line);
                }
                collected_lines.push(line);
                if terminal_is_error.is_some() {
                    break; // terminal result seen — do not wait for EOF
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                // Surface the read error (FAIL FAST: never swallow silently),
                // then stop — no terminal result means the failure path runs.
                tracing::warn!("cron stdout read error: {e}; ending stream read");
                break;
            }
        }
    }

    match terminal_is_error {
        Some(false) => CronStreamOutcome::Success { collected_lines },
        // Error result, or no terminal result at all → failure path.
        Some(true) | None => CronStreamOutcome::Failed { collected_lines },
    }
}

/// Parse CC stream-json output (NDJSON lines) into `CronReplyOutput`.
///
/// Finds the last line with `"type": "result"`, then extracts the payload from
/// `structured_output` (preferred) or `result` field.
/// Returns `Err` if no result line found or JSON is invalid.
pub(crate) fn parse_cron_output(lines: &[String]) -> Result<CronReplyOutput, String> {
    let envelope = lines
        .iter()
        .rev()
        .find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            if is_result_line(&v) { Some(v) } else { None }
        })
        .ok_or_else(|| "no result line found in stream-json output".to_string())?;

    let payload = if let Some(so) = envelope.get("structured_output") {
        if !so.is_null() {
            so
        } else {
            envelope.get("result").unwrap_or(so)
        }
    } else if let Some(r) = envelope.get("result") {
        r
    } else {
        return Err("result line has neither 'structured_output' nor 'result' field".into());
    };

    let output: CronReplyOutput = serde_json::from_value(payload.clone())
        .map_err(|e| format!("failed to parse CronReplyOutput: {e}"))?;
    output.delivery.validate()?;
    Ok(output)
}

async fn update_run_record(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) {
    if let Err(error) = client
        .finish_run(&right_mcp::internal_db::FinishRunRequest {
            agent: agent.to_owned(),
            request_id: crate::db::request_id(),
            run_id: run_id.to_owned(),
            exit_code,
            status: status.to_owned(),
        })
        .await
    {
        tracing::error!(run_id, "owner run update failed: {error:#}");
    }
}

/// Timeout for waiting on in-flight execute_job tasks during shutdown.
///
pub(crate) const SHUTDOWN_JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Wall-clock cap on one cron run's guest drive, measured from `run_started`
/// (before the spawn). Bounds the otherwise-unbounded stream consume: a guest
/// that dies or wedges without a terminal `result` line cannot park the task
/// until bot restart or strand its lock. Generous vs. real turns — the largest
/// observed legitimate run is ~20 min.
const CRON_RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Guest stderr characters kept in the failure log line (tail). Shell-level
/// failures put the reason in the last few lines; the cap keeps the log line
/// bounded when a run floods stderr.
const STDERR_TAIL_CHARS: usize = 4 * 1024;

/// Bound on `child.wait()` after the cron stream loop exits — see the
/// matching constant in `telegram::worker` for the rationale.
const POST_BREAK_WAIT_TIMEOUT_SECS: u64 = 5;

/// Bound on draining cron stderr after exit — see the matching constant in
/// `telegram::worker`.
const POST_BREAK_STDERR_TIMEOUT_SECS: u64 = 2;

struct PendingExecuteHandle {
    job_name: String,
    handle: JoinHandle<()>,
}

impl PendingExecuteHandle {
    fn job_name(&self) -> &str {
        &self.job_name
    }
}

type ExecuteHandles = Arc<std::sync::Mutex<Vec<PendingExecuteHandle>>>;

fn register_execute_handle(
    execute_handles: &ExecuteHandles,
    job_name: String,
    handle: JoinHandle<()>,
) -> Result<(), JoinHandle<()>> {
    match execute_handles.lock() {
        Ok(mut guard) => {
            guard.retain(|h| !h.handle.is_finished());
            guard.push(PendingExecuteHandle { job_name, handle });
            Ok(())
        }
        Err(_) => Err(handle),
    }
}

/// Main reconciler loop. Polls `crons/*.yaml` every 60s, spawning per-job loops.
///
/// Cron results are persisted to DB. A separate delivery loop reads pending rows
/// and sends Telegram notifications.
///
/// Signature expected by lib.rs spawn site (CRON-01, CRON-02, CRON-06).
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: right_agent_config::LearningConfig,
    session_locks: crate::telegram::SessionLocks,
    progress_state: crate::telegram::progress::ProgressState,
) {
    tracing::info!(agent = %agent_name, "cron task started");

    let execute_handles: ExecuteHandles = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut triggered_handles: Vec<JoinHandle<()>> = Vec::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(
        &mut handles,
        &mut triggered_handles,
        &internal_client,
        &agent_dir,
        &agent_name,
        &model,
        &sandbox_runtime,
        &internal_client,
        &execute_handles,
        &upgrade_lock,
        &debug,
        &learning,
        &session_locks,
        &progress_state,
    )
    .await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &mut triggered_handles, &internal_client, &agent_dir, &agent_name, &model, &sandbox_runtime, &internal_client, &execute_handles, &upgrade_lock, &debug, &learning, &session_locks, &progress_state).await;
            }
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "cron shutdown: stopping reconciler");
                break;
            }
        }
    }

    // This does NOT kill in-flight execute_job tasks — they are separate spawns.
    let scheduler_count = handles.len();
    for (name, (_, handle)) in handles {
        handle.abort();
        tracing::info!(job = %name, "cron shutdown: aborted job scheduler");
    }
    for handle in triggered_handles {
        // Triggered handles are one-shot execute_job spawns, not loops.
        // Don't abort — they'll be collected from execute_handles below.
        handle.abort();
    }
    tracing::info!(agent = %agent_name, aborted = scheduler_count, "cron shutdown: all job schedulers aborted");

    // Phase 2: Wait for in-flight execute_job tasks with timeout.
    // Clean up finished handles first.
    let pending: Vec<PendingExecuteHandle> = {
        let mut guard = execute_handles
            .lock()
            .expect("execute_handles mutex poisoned");
        guard
            .drain(..)
            .filter(|h| !h.handle.is_finished())
            .collect()
    };

    if pending.is_empty() {
        tracing::info!(agent = %agent_name, "cron shutdown: no running jobs");
    } else {
        let names: Vec<&str> = pending.iter().map(PendingExecuteHandle::job_name).collect();
        tracing::info!(
            agent = %agent_name,
            count = pending.len(),
            jobs = ?names,
            "cron shutdown: waiting for running job(s) (timeout {}s)",
            SHUTDOWN_JOB_TIMEOUT.as_secs()
        );

        for mut pending_handle in pending {
            let name = pending_handle.job_name().to_owned();
            match tokio::time::timeout(SHUTDOWN_JOB_TIMEOUT, &mut pending_handle.handle).await {
                Ok(Ok(())) => {
                    tracing::info!(job = %name, "cron shutdown: job finished cleanly");
                }
                Ok(Err(e)) => {
                    tracing::warn!(job = %name, "cron shutdown: job panicked: {e}");
                }
                Err(_) => {
                    tracing::warn!(
                        job = %name,
                        timeout_secs = SHUTDOWN_JOB_TIMEOUT.as_secs(),
                        "cron shutdown: job timed out, aborting and marking interrupted"
                    );
                    pending_handle.handle.abort();
                    if let Err(error) = internal_client
                        .cron_mark_interrupted_by_shutdown(
                            &right_mcp::internal_db::CronMarkInterruptedByShutdownRequest {
                                agent: agent_name.clone(),
                                job_name: name.clone(),
                                reason: "shutdown timeout".to_owned(),
                            },
                        )
                        .await
                    {
                        tracing::error!(job = %name, "cron shutdown owner interruption failed: {error:#}");
                    }
                }
            }
        }
    }

    tracing::info!(agent = %agent_name, "cron shutdown complete");
}

/// Drop guard that deletes the one-shot spec when the spawned execute task
/// exits — including via cancellation (`JoinHandle::abort` on shutdown). A
/// plain post-await call would be skipped if the future is dropped mid-await.
struct OneShotSpecDeleter {
    agent_dir: std::path::PathBuf,
    job_name: String,
}

impl Drop for OneShotSpecDeleter {
    fn drop(&mut self) {
        // `tokio::spawn` panics if no runtime is current — guard so a drop
        // after the runtime has shut down (e.g. test teardown) doesn't abort.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let agent_dir = self.agent_dir.clone();
        let job_name = self.job_name.clone();
        handle.spawn(async move {
            delete_one_shot_spec(&agent_dir, &job_name).await;
        });
    }
}

/// Delete a one-shot spec after it has fired through the owner.
async fn delete_one_shot_spec(agent_dir: &std::path::Path, job_name: &str) {
    let result = async {
        let (client, agent) = crate::db::client_for_agent_dir(agent_dir)?;
        client
            .cron_delete_spec(&right_mcp::internal_db::CronDeleteSpecRequest {
                agent,
                job_name: job_name.to_owned(),
            })
            .await
            .map(drop)
            .map_err(anyhow::Error::from)
    }
    .await;
    match result {
        Ok(()) => tracing::info!(job = %job_name, "one-shot spec auto-deleted after fire"),
        Err(error) => {
            tracing::error!(job = %job_name, "failed to delete one-shot spec through owner: {error:#}")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fire_one_shot_specs(
    specs: Vec<(String, CronSpec)>,
    kind_label: &'static str,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    sandbox_runtime: &Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent_config::LearningConfig,
    session_locks: &crate::telegram::SessionLocks,
    progress_state: &crate::telegram::progress::ProgressState,
) {
    for (name, spec) in specs {
        let lock_ttl = effective_lock_ttl(&spec);
        if is_lock_fresh(agent_dir, &name, lock_ttl) {
            tracing::info!(job = %name, kind = kind_label, "one-shot job locked — skipping until next tick");
            continue;
        }

        tracing::info!(job = %name, kind = kind_label, "firing one-shot job");
        let jn = name.clone();
        let sp = spec.clone();
        let ad = agent_dir.to_path_buf();
        let an = agent_name.to_string();
        // Snapshot the model at fire time, not at loop-spawn time, so /model
        // changes take effect on the next cron firing rather than next restart.
        // Per-cron model wins over the global agent model.
        let md: Option<String> = resolve_cron_model(&spec, model);
        // Resolved at fire time, not at reconciler start: recovery publishes a
        // new handle and the reconciler outlives many of them.
        let sbx = sandbox_runtime.current_sandbox();
        let ic = Arc::clone(internal_client);
        let ul = Arc::clone(upgrade_lock);
        let dbg = debug.clone();
        let learn = learning.clone();
        let slk = session_locks.clone();
        let progress = progress_state.clone();

        let one_shot_deleter = OneShotSpecDeleter {
            agent_dir: ad.clone(),
            job_name: jn.clone(),
        };
        let handle = tokio::spawn(async move {
            let _one_shot_deleter = one_shot_deleter;
            execute_job(
                &jn,
                &sp,
                &ad,
                &an,
                md.as_deref(),
                sbx.as_ref(),
                &ic,
                ul,
                dbg,
                &learn,
                &slk,
                &progress,
            )
            .await;
        });
        if let Err(handle) = register_execute_handle(execute_handles, name, handle) {
            triggered_handles.push(handle);
        }
    }
}

// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    client: &right_mcp::internal_client::InternalClient,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    sandbox_runtime: &Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: &right_agent_config::LearningConfig,
    session_locks: &crate::telegram::SessionLocks,
    progress_state: &crate::telegram::progress::ProgressState,
) {
    // Clean up finished triggered handles
    triggered_handles.retain(|h| !h.is_finished());
    let new_specs = match load_specs_ipc(client, agent_name).await {
        Ok(specs) => specs,
        Err(error) => {
            tracing::error!("failed to load cron specs from owner: {error:#}");
            return;
        }
    };

    // Fire overdue run_at specs (one-shot absolute time jobs)
    let now = chrono::Utc::now();
    let overdue_run_at: Vec<(String, CronSpec)> = new_specs
        .iter()
        .filter(|(_, spec)| matches!(&spec.schedule_kind, right_agent::cron_spec::ScheduleKind::RunAt(dt) if *dt <= now))
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();

    fire_one_shot_specs(
        overdue_run_at,
        "run_at",
        triggered_handles,
        agent_dir,
        agent_name,
        model,
        sandbox_runtime,
        internal_client,
        execute_handles,
        upgrade_lock,
        debug,
        learning,
        session_locks,
        progress_state,
    );

    // Fire Immediate specs (every tick — they are one-shot)
    let immediate: Vec<(String, CronSpec)> = new_specs
        .iter()
        .filter(|(_, spec)| is_reconcile_tick_kind(&spec.schedule_kind))
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();

    fire_one_shot_specs(
        immediate,
        "immediate",
        triggered_handles,
        agent_dir,
        agent_name,
        model,
        sandbox_runtime,
        internal_client,
        execute_handles,
        upgrade_lock,
        debug,
        learning,
        session_locks,
        progress_state,
    );

    // Abort handles for removed or changed jobs (CRON-06)
    let to_remove: Vec<String> = handles
        .iter()
        .filter(|(name, (old_spec, _))| new_specs.get(*name) != Some(old_spec))
        .map(|(name, _)| name.clone())
        .collect();

    for name in &to_remove {
        if let Some((_, handle)) = handles.remove(name) {
            handle.abort();
            tracing::info!(job = %name, "cron job handle aborted (spec removed or changed)");
        }
    }

    // Spawn new handles for new or changed jobs
    for (name, spec) in &new_specs {
        // Skip RunAt and Immediate specs — they are handled above, not run_job_loop.
        if is_run_job_loop_skip_kind(&spec.schedule_kind) {
            continue;
        }
        if handles.contains_key(name) {
            continue; // unchanged, already running
        }
        let job_name = name.clone();
        let job_spec = spec.clone();
        let job_agent_dir = agent_dir.to_path_buf();
        let job_agent_name = agent_name.to_string();
        let job_model = Arc::clone(model);
        let job_sandbox_runtime = Arc::clone(sandbox_runtime);
        let job_execute_handles = Arc::clone(execute_handles);
        let job_internal_client = Arc::clone(internal_client);
        let job_upgrade_lock = Arc::clone(upgrade_lock);
        let job_debug = debug.clone();
        let job_learning = learning.clone();
        let job_session_locks = session_locks.clone();
        let job_progress_state = progress_state.clone();

        let handle = tokio::spawn(async move {
            run_job_loop(
                job_name,
                job_spec,
                job_agent_dir,
                job_agent_name,
                job_model,
                job_sandbox_runtime,
                job_internal_client,
                job_execute_handles,
                job_upgrade_lock,
                job_debug,
                job_learning,
                job_session_locks,
                job_progress_state,
            )
            .await;
        });
        handles.insert(name.clone(), (spec.clone(), handle));
        tracing::info!(job = %name, schedule = %spec.schedule_kind, "cron job scheduled");
    }

    // Check for triggered jobs (manual trigger via cron_trigger MCP tool)
    for (name, spec) in &new_specs {
        if spec.triggered_at.is_some() {
            // Clear trigger immediately to prevent re-firing on next tick
            if let Err(error) = client
                .cron_clear_triggered(&right_mcp::internal_db::CronJobRequest {
                    agent: agent_name.to_owned(),
                    job_name: name.clone(),
                })
                .await
            {
                tracing::error!(job = %name, "failed to clear triggered state through owner: {error:#}");
                continue;
            }
            // Check lock — if locked, skip (trigger lost, same as schedule miss while locked)
            let lock_ttl = effective_lock_ttl(spec);
            if is_lock_fresh(agent_dir, name, lock_ttl) {
                tracing::info!(job = %name, "triggered but locked — skipping");
                continue;
            }

            let jn = name.clone();
            let sp = spec.clone();
            let ad = agent_dir.to_path_buf();
            let an = agent_name.to_string();
            let md: Option<String> = resolve_cron_model(spec, model);
            let sbx = sandbox_runtime.current_sandbox();
            let ic = Arc::clone(internal_client);
            let ul = Arc::clone(upgrade_lock);
            let dbg = debug.clone();
            let learn = learning.clone();
            let slk = session_locks.clone();
            let progress = progress_state.clone();

            tracing::info!(job = %name, "executing triggered job");
            let trigger_name = name.clone();
            let handle = tokio::spawn(async move {
                execute_job(
                    &jn,
                    &sp,
                    &ad,
                    &an,
                    md.as_deref(),
                    sbx.as_ref(),
                    &ic,
                    ul,
                    dbg,
                    &learn,
                    &slk,
                    &progress,
                )
                .await;
            });
            // Register for shutdown tracking
            if let Err(handle) = register_execute_handle(execute_handles, trigger_name, handle) {
                triggered_handles.push(handle);
            }
        }
    }
}

/// Per-job loop: sleep until next scheduled time, then execute. (CRON-03, D-03)
///
/// Execute handles are pushed to `execute_handles` so shutdown can await them.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Arc<arc_swap::ArcSwap<Option<String>>>,
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: ExecuteHandles,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    learning: right_agent_config::LearningConfig,
    session_locks: crate::telegram::SessionLocks,
    progress_state: crate::telegram::progress::ProgressState,
) {
    use cron::Schedule;
    use std::str::FromStr;

    let cron_expr = match spec.schedule_kind.cron_schedule() {
        Some(s) => s,
        None => {
            tracing::error!(job = %job_name, "run_job_loop called for RunAt spec — should not happen");
            return;
        }
    };
    let seven_field = to_7field(cron_expr);
    let schedule = match Schedule::from_str(&seven_field) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(job = %job_name, "invalid cron schedule '{cron_expr}': {e:#}");
            return;
        }
    };

    loop {
        let now = chrono::Utc::now();
        let Some(fire_at) = schedule.after(&now).next() else {
            tracing::warn!(job = %job_name, "schedule has no future fires — stopping job loop");
            break;
        };

        let delay = (fire_at - now)
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);

        tokio::time::sleep(delay).await;

        if let Ok(Some(warning)) = right_agent::cron_spec::validate_schedule(cron_expr) {
            tracing::warn!(job = %job_name, "{warning}");
        }

        // Spawn execution so the loop continues counting ticks while the job runs.
        // The lock in execute_job prevents concurrent executions of the same job.
        let jn = job_name.clone();
        let sp = spec.clone();
        let ad = agent_dir.clone();
        let an = agent_name.clone();
        let md: Option<String> = resolve_cron_model(&spec, &model);
        // Resolved per firing: the per-job loop outlives individual handles.
        let sbx = sandbox_runtime.current_sandbox();
        let ic = Arc::clone(&internal_client);
        let ul = Arc::clone(&upgrade_lock);
        let dbg = debug.clone();
        let learn = learning.clone();
        let slk = session_locks.clone();
        let progress = progress_state.clone();

        let one_shot_deleter = spec
            .schedule_kind
            .is_one_shot()
            .then(|| OneShotSpecDeleter {
                agent_dir: ad.clone(),
                job_name: jn.clone(),
            });
        let handle = tokio::spawn(async move {
            let _one_shot_deleter = one_shot_deleter;
            execute_job(
                &jn,
                &sp,
                &ad,
                &an,
                md.as_deref(),
                sbx.as_ref(),
                &ic,
                ul,
                dbg,
                &learn,
                &slk,
                &progress,
            )
            .await;
        });
        if spec.schedule_kind.is_one_shot() {
            if let Err(handle) = register_execute_handle(&execute_handles, job_name.clone(), handle)
            {
                tracing::warn!(
                    job = %job_name,
                    "failed to track one-shot cron execution for shutdown"
                );
                if let Err(e) = handle.await {
                    tracing::error!(job = %job_name, "one-shot job panicked: {e}");
                }
            }
            break;
        }
        // Register for shutdown tracking (only for recurring jobs that continue the loop)
        if let Err(handle) = register_execute_handle(&execute_handles, job_name.clone(), handle) {
            tracing::warn!(job = %job_name, "failed to track cron execution for shutdown");
            drop(handle);
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use crate::reflection::FailureKind;

    #[tokio::test]
    async fn register_execute_handle_tracks_job_name_for_shutdown_marking() {
        let execute_handles: ExecuteHandles = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = tokio::spawn(async {});

        register_execute_handle(&execute_handles, "job-a".to_owned(), handle)
            .expect("execute handle should register");

        let guard = execute_handles.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].job_name(), "job-a");
    }

    #[test]
    fn classify_budget_exceeded_from_text() {
        let kind = classify_cron_failure(Some(1), "the max budget was exceeded", 2.0, Some(30));
        assert!(matches!(kind, FailureKind::BudgetExceeded { .. }));
    }

    #[test]
    fn classify_max_turns_from_text() {
        let kind = classify_cron_failure(
            Some(1),
            "reached the max turns for this session",
            2.0,
            Some(30),
        );
        assert!(matches!(kind, FailureKind::MaxTurns { .. }));
    }

    #[test]
    fn classify_other_to_non_zero_exit() {
        let kind = classify_cron_failure(Some(137), "OOM killed", 2.0, None);
        assert!(matches!(kind, FailureKind::NonZeroExit { code: 137 }));
    }

    #[test]
    fn classify_unknown_exit_defaults_to_minus_one() {
        let kind = classify_cron_failure(None, "weird failure", 2.0, None);
        if let FailureKind::NonZeroExit { code } = kind {
            assert_eq!(code, -1);
        } else {
            panic!("expected NonZeroExit");
        }
    }

    #[test]
    fn terminal_failure_detail_synthesizes_budget_reason_from_subtype() {
        // error_max_budget_usd results carry NO `result` text — the reason lives
        // in `subtype` + `total_cost_usd`. The detail must survive regardless.
        let line = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"num_turns":1,"total_cost_usd":2.08122}"#;
        let detail = terminal_failure_detail(line).expect("error result yields a detail");
        assert!(detail.contains("max budget"), "got: {detail}");
        assert!(detail.contains("2.08"), "must report spend, got: {detail}");
        // And it must classify correctly (the prior bug mis-routed to NonZeroExit).
        let kind = classify_cron_failure(Some(1), &detail, 1.5, None);
        assert!(
            matches!(kind, FailureKind::BudgetExceeded { .. }),
            "got: {kind:?}"
        );
    }

    #[test]
    fn terminal_failure_detail_synthesizes_max_turns_reason() {
        let line = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":5}"#;
        let detail = terminal_failure_detail(line).expect("error result yields a detail");
        assert!(detail.contains("max turns"), "got: {detail}");
        let kind = classify_cron_failure(Some(1), &detail, 1.5, Some(5));
        assert!(
            matches!(kind, FailureKind::MaxTurns { .. }),
            "got: {kind:?}"
        );
    }

    #[test]
    fn terminal_failure_detail_prefers_explicit_result_text() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"all good"}"#;
        assert_eq!(terminal_failure_detail(line).as_deref(), Some("all good"));
    }

    /// The cron wrapper runs under the guest's POSIX `/bin/sh` (dash on
    /// node:22-slim). `set -o pipefail` is a bashism dash aborts on before
    /// `claude` ever runs — the Aug 18–21 cron outage. Guards the real
    /// wrapper builder against bash-only constructs.
    #[test]
    fn cron_wrapper_script_is_posix_sh_safe() {
        let script = cron_wrapper_script("printf 'body'", "/sandbox/crons/logs", "job-run.ndjson");
        assert!(script.contains("tee /sandbox/crons/logs/job-run.ndjson"));
        assert!(
            !script.contains("pipefail"),
            "cron wrapper must not use bash-only pipefail: {script}"
        );
    }

    #[test]
    fn cron_settings_override_disables_background_tasks() {
        let settings: serde_json::Value =
            serde_json::from_str(&cron_settings_override()).expect("valid inline settings JSON");
        assert_eq!(
            settings.pointer("/env/CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"),
            Some(&serde_json::Value::String("1".to_owned()))
        );
    }

    #[test]
    fn cron_extra_args_pass_settings_on_command_line() {
        let args = cron_extra_args();
        assert_eq!(args.first().map(String::as_str), Some("--settings"));
        let settings: serde_json::Value =
            serde_json::from_str(&args[1]).expect("valid inline settings JSON");
        assert_eq!(
            settings.pointer("/env/CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"),
            Some(&serde_json::Value::String("1".to_owned()))
        );
    }

    #[test]
    fn terminal_failure_detail_none_for_non_result_or_textless_non_error() {
        assert_eq!(
            terminal_failure_detail(r#"{"type":"assistant","message":{}}"#),
            None
        );
        // A non-error result with no text yields no failure detail.
        assert_eq!(
            terminal_failure_detail(r#"{"type":"result","subtype":"success","is_error":false}"#),
            None
        );
    }
    #[test]
    fn skip_reflection_for_deterministic_classified_failures() {
        // A 529/429/5xx/turn-limit error result is deterministic: a reflection
        // `--resume` turn would fail the same way and cost money. The skip
        // decision must fire BEFORE the reflection call, using the
        // classification alone.
        let overloaded = vec![
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":529,"result":"API Error: 529 Overloaded."}"#.to_string(),
        ];
        let decision = skip_reflection_decision(&overloaded, &FailureKind::NonZeroExit { code: 1 });
        let classified = decision.expect("529 classifies → skip reflection").unwrap();
        assert!(
            classified.user_message.contains("overloaded"),
            "{}",
            classified.user_message
        );

        // Budget cap: classification is None, but the kind alone is
        // deterministic — still skip, with no classified message.
        let budget = skip_reflection_decision(&[], &FailureKind::BudgetExceeded { limit_usd: 1.5 });
        assert!(budget.is_some() && budget.unwrap().is_none());
    }

    #[test]
    fn reflection_runs_for_unclassifiable_failures() {
        // No result line at all (transport wedge, kill -9) and a generic
        // failure kind: nothing deterministic is known, so reflection must run.
        assert_eq!(
            skip_reflection_decision(&[], &FailureKind::NonZeroExit { code: -1 }),
            None
        );

        // A result line that is NOT an error result (is_error absent/false)
        // must not trip the classifier either.
        let not_error = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#.to_string(),
        ];
        assert_eq!(
            skip_reflection_decision(&not_error, &FailureKind::NonZeroExit { code: 1 }),
            None
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A linked-skills lookup failure must fail the cron run: silently
    /// substituting an empty list would run the job with the wrong tool set
    /// while the audit record looks identical to a job with no linked skills.
    #[tokio::test]
    async fn linked_skills_lookup_failure_propagates() {
        let client = right_mcp::internal_client::InternalClient::new(std::path::PathBuf::from(
            "/nonexistent-right-test-internal.sock",
        ));
        let result = fetch_linked_skills_for_run(&client, "alpha", "job").await;
        assert!(
            matches!(
                result,
                Err(right_mcp::internal_db::InternalDbError::Transport(_))
            ),
            "linked-skills lookup failure must propagate, got {result:?}"
        );
    }

    #[test]
    fn manual_trigger_notice_carries_token() {
        let n = crate::cc::system_notice::wrap_system_notice(
            "tok123",
            "Manual verification trigger: x",
        );
        assert!(n.contains("SYSTEM_NOTICE:tok123"));
        assert!(n.contains("Manual verification trigger"));
    }

    /// ArcSwap cell used by run_cron_task must reflect the current value, not
    /// the value at task-spawn time.  This test verifies the `snapshot_model`
    /// helper that every call site in this module uses.
    #[test]
    fn cron_reads_current_model_from_arcswap() {
        let cell: Arc<arc_swap::ArcSwap<Option<String>>> =
            Arc::new(arc_swap::ArcSwap::from_pointee(None));
        // Simulate a /model update arriving after boot.
        cell.store(Arc::new(Some("claude-haiku-4-5".to_owned())));
        let snapshot: Option<String> = crate::snapshot_model(&cell);
        assert_eq!(snapshot.as_deref(), Some("claude-haiku-4-5"));
        // Simulate a second /model update.
        cell.store(Arc::new(Some("claude-opus-4-5".to_owned())));
        let snapshot2: Option<String> = crate::snapshot_model(&cell);
        assert_eq!(snapshot2.as_deref(), Some("claude-opus-4-5"));
    }

    #[test]
    fn cron_keeps_learning_tools_when_learning_enabled() {
        let with = crate::cc::invocation::disallow_foreground_only_tools_keep_learning(
            crate::cc::invocation::baseline_disallowed_tools(),
        );
        let without = crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        );
        let start = right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL;
        assert!(
            !with.iter().any(|t| t == start),
            "learning-enabled cron must allow skill_learning_start"
        );
        assert!(
            without.iter().any(|t| t == start),
            "learning-disabled cron must disallow it"
        );
        let channel_post = right_mcp::internal_client::CHANNEL_POST_MCP_TOOL;
        assert!(
            !with.iter().any(|tool| tool == channel_post),
            "learning-enabled cron must allow channel_post"
        );
        assert!(
            !without.iter().any(|tool| tool == channel_post),
            "learning-disabled cron must allow channel_post"
        );
    }
    #[tokio::test]
    async fn cron_without_a_sandbox_refuses_before_registering_anything() {
        let agent_dir = tempdir().expect("agent dir");
        std::fs::write(
            agent_dir.path().join("mcp.json"),
            r#"{"mcpServers":{"right":{"headers":{}}}}"#,
        )
        .expect("write MCP config");
        right_db::open_connection(agent_dir.path(), true)
            .await
            .expect("create agent database");

        // Bound to a socket nothing ever accepts on: any progress_register
        // attempt would hang rather than succeed, so reaching the aggregator at
        // all is observable as a timeout instead of a fast refusal.
        let socket_dir = tempdir().expect("socket dir");
        let socket_path = socket_dir.path().join("internal.sock");
        let _listener = tokio::net::UnixListener::bind(&socket_path).expect("bind internal socket");

        let learning = right_agent_config::LearningConfig {
            prefilter_enabled: false,
            ..Default::default()
        };
        let internal_client = Arc::new(right_mcp::internal_client::InternalClient::new(
            &socket_path,
        ));
        let spec = sample_cron_spec();
        let session_locks = Arc::new(dashmap::DashMap::new());
        let progress_state = crate::telegram::progress::ProgressState::default();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute_job(
                "default-learning-job",
                &spec,
                agent_dir.path(),
                "agent-1",
                None,
                // Fail-closed: no sandbox, no cron turn, and no aggregator
                // registration to unwind.
                None,
                &internal_client,
                Arc::new(tokio::sync::RwLock::new(())),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                &learning,
                &session_locks,
                &progress_state,
            ),
        )
        .await
        .expect("a sandboxless cron must refuse promptly, not reach the aggregator");

        assert!(
            !agent_dir
                .path()
                .join("crons")
                .join(".locks")
                .join("default-learning-job.json")
                .exists(),
            "a refused run must release its lock"
        );
    }

    #[test]
    fn parse_result_stats_reads_text_turns_cost() {
        let lines = vec![
            r#"{"type":"assistant","message":{}}"#.to_string(),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done: 3 PRs","num_turns":7,"total_cost_usd":0.34}"#.to_string(),
        ];
        let (text, turns, cost) = super::parse_result_stats(&lines).expect("stats");
        assert_eq!(text, "done: 3 PRs");
        assert_eq!(turns, 7);
        assert!((cost - 0.34).abs() < 1e-9);
    }

    #[test]
    fn parse_result_stats_none_without_result_line() {
        let lines = vec![r#"{"type":"assistant","message":{}}"#.to_string()];
        assert!(super::parse_result_stats(&lines).is_none());
    }

    #[test]
    fn only_recurring_feeds_learning() {
        use right_agent::cron_spec::ScheduleKind;
        assert!(super::schedule_kind_feeds_learning(
            &ScheduleKind::Recurring("*/5 * * * *".into())
        ));
        assert!(!super::schedule_kind_feeds_learning(
            &ScheduleKind::OneShotCron("*/5 * * * *".into())
        ));
        assert!(!super::schedule_kind_feeds_learning(
            &ScheduleKind::Immediate
        ));
        assert!(!super::schedule_kind_feeds_learning(&ScheduleKind::RunAt(
            chrono::Utc::now()
        )));
    }

    /// Minimal `CronSpec` with all transient/target fields cleared, for pure
    /// decision-function tests.
    fn sample_cron_spec() -> CronSpec {
        use right_agent::cron_spec::ScheduleKind;
        CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: None,
            target_thread_id: None,
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        }
    }

    #[test]
    fn then_action_respects_run_on_and_target_precedence() {
        use right_agent::cron_spec::{RunOn, ThenSpec};

        let mk = |run_on, then_target: Option<i64>, origin: Option<i64>, standing: Option<i64>| {
            let mut s = sample_cron_spec();
            s.target_chat_id = standing;
            s.trigger_origin_chat_id = origin;
            s.then = Some(ThenSpec {
                instruction: "go".into(),
                run_on,
                notify: false,
                target_chat_id: then_target,
                target_thread_id: None,
            });
            s
        };

        // run_on=success fires only on success
        assert!(resolve_then_action(&mk(RunOn::Success, None, Some(1), Some(2)), true).is_some());
        assert!(resolve_then_action(&mk(RunOn::Success, None, Some(1), Some(2)), false).is_none());
        // run_on=failure fires only on failure
        assert!(resolve_then_action(&mk(RunOn::Failure, None, Some(1), Some(2)), false).is_some());
        // run_on=always fires both
        assert!(resolve_then_action(&mk(RunOn::Always, None, Some(1), Some(2)), true).is_some());

        // target precedence: then.target_chat_id > origin > standing
        assert_eq!(
            resolve_then_action(&mk(RunOn::Always, Some(9), Some(1), Some(2)), true)
                .unwrap()
                .target_chat_id,
            9
        );
        assert_eq!(
            resolve_then_action(&mk(RunOn::Always, None, Some(1), Some(2)), true)
                .unwrap()
                .target_chat_id,
            1
        );
        assert_eq!(
            resolve_then_action(&mk(RunOn::Always, None, None, Some(2)), true)
                .unwrap()
                .target_chat_id,
            2
        );
        // no target anywhere -> None (cannot deliver)
        assert!(resolve_then_action(&mk(RunOn::Always, None, None, None), true).is_none());

        // thread-id precedence follows the chosen chat source.
        // When then.target_chat_id wins, the action carries then.target_thread_id.
        let mut then_thread = sample_cron_spec();
        then_thread.trigger_origin_chat_id = Some(1);
        then_thread.trigger_origin_thread_id = Some(11);
        then_thread.target_thread_id = Some(22);
        then_thread.then = Some(ThenSpec {
            instruction: "go".into(),
            run_on: RunOn::Always,
            notify: false,
            target_chat_id: Some(9),
            target_thread_id: Some(99),
        });
        let act = resolve_then_action(&then_thread, true).unwrap();
        assert_eq!(act.target_chat_id, 9);
        assert_eq!(act.target_thread_id, Some(99));

        // When origin wins (no then.target_chat_id), the action carries the
        // origin thread id, ignoring then.target_thread_id and standing thread.
        let mut origin_thread = sample_cron_spec();
        origin_thread.trigger_origin_chat_id = Some(1);
        origin_thread.trigger_origin_thread_id = Some(11);
        origin_thread.target_thread_id = Some(22);
        origin_thread.then = Some(ThenSpec {
            instruction: "go".into(),
            run_on: RunOn::Always,
            notify: false,
            target_chat_id: None,
            target_thread_id: Some(99),
        });
        let act = resolve_then_action(&origin_thread, true).unwrap();
        assert_eq!(act.target_chat_id, 1);
        assert_eq!(act.target_thread_id, Some(11));

        // Telegram thread 0 ("no topic") normalizes to None even when the
        // winning source carries Some(0).
        let mut thread_zero = sample_cron_spec();
        thread_zero.then = Some(ThenSpec {
            instruction: "go".into(),
            run_on: RunOn::Always,
            notify: false,
            target_chat_id: Some(9),
            target_thread_id: Some(0),
        });
        let act = resolve_then_action(&thread_zero, true).unwrap();
        assert_eq!(act.target_chat_id, 9);
        assert_eq!(act.target_thread_id, None);
    }

    /// End-to-end at the observable boundary: a successful triggered run with
    /// `then{run_on:"success"}` and an origin chat resolves an action and inserts
    /// a `kind='background'`, `producer_ref='cron_then'` row whose
    /// `source_session_id` is the triggered run's session and `target_chat_id` is
    /// the origin. The `run_on:"failure"` variant resolves to `None` on success,
    /// so no row is inserted.
    ///
    /// We assert at the row-insert boundary (`insert_then_continuation_row`)
    /// because the full `spawn_then_continuation` spawns a real CC subprocess,
    /// which the cron unit harness cannot stand up.
    #[cfg(any())]
    #[tokio::test]
    async fn triggered_run_with_then_success_spawns_continuation() {
        use right_agent::cron_spec::{RunOn, ThenSpec};

        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        let triggered_run_id = "triggered-session-abc";
        let origin_chat: i64 = 4242;

        let mut success_spec = sample_cron_spec();
        success_spec.trigger_origin_chat_id = Some(origin_chat);
        success_spec.then = Some(ThenSpec {
            instruction: "follow up".into(),
            run_on: RunOn::Success,
            notify: false,
            target_chat_id: None,
            target_thread_id: None,
        });

        // run_on=success fires on a successful run → row inserted.
        let action = resolve_then_action(&success_spec, true).expect("then fires on success");
        let new_run_id = insert_then_continuation_row(dir.path(), &action, triggered_run_id)
            .await
            .expect("row insert");

        let row: (String, String, String, i64) = conn
            .query_row(
                "SELECT kind, producer_ref, source_session_id, target_chat_id \
                 FROM async_runs WHERE producer_ref = 'cron_then'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "background");
        assert_eq!(row.1, THEN_PRODUCER_REF);
        assert_eq!(row.2, triggered_run_id);
        assert_eq!(row.3, origin_chat);

        // run_session_id is the new run's own session, distinct from the source.
        let run_session_id: String = conn
            .query_row(
                "SELECT run_session_id FROM async_runs WHERE id = ?1",
                [new_run_id.as_str()],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(run_session_id, new_run_id);

        // run_on=failure does NOT fire on a successful run → no decision, no row.
        let mut failure_spec = sample_cron_spec();
        failure_spec.trigger_origin_chat_id = Some(origin_chat);
        failure_spec.then = Some(ThenSpec {
            instruction: "only on failure".into(),
            run_on: RunOn::Failure,
            notify: false,
            target_chat_id: None,
            target_thread_id: None,
        });
        assert!(
            resolve_then_action(&failure_spec, true).is_none(),
            "run_on=failure must not fire on a successful run"
        );

        // Exactly one cron_then row exists (the success one).
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM async_runs WHERE producer_ref = 'cron_then'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn compose_run_prompt_orders_force_notify_then_extra_then_prompt() {
        let p = compose_run_prompt("BODY", true, Some("focus on X"), "tok123", &[]);
        let fn_idx = p.find("Manual verification trigger").unwrap();
        let extra_idx = p.find("focus on X").unwrap();
        let body_idx = p.find("BODY").unwrap();
        assert!(fn_idx < extra_idx && extra_idx < body_idx);
        // Both notices carry the token.
        assert!(p.contains("SYSTEM_NOTICE:tok123"));

        // No force-notify, no extra -> body unchanged.
        assert_eq!(
            compose_run_prompt("BODY", false, None, "tok123", &[]),
            "BODY"
        );

        // Extra only.
        let e = compose_run_prompt("BODY", false, Some("X"), "tok123", &[]);
        assert!(e.contains("X") && e.ends_with("BODY"));
        assert!(e.contains("SYSTEM_NOTICE:tok123"));
    }

    #[test]
    fn compose_run_prompt_appends_linked_skills_when_present() {
        let skills = vec!["rightx-a".to_string(), "rightx-b".to_string()];
        let with = compose_run_prompt("BODY", false, None, "tok123", &skills);
        assert!(with.contains("## Linked skills"));
        assert!(with.contains("rightx-a, rightx-b"));

        let without = compose_run_prompt("BODY", false, None, "tok123", &[]);
        assert!(!without.contains("## Linked skills"));
    }

    #[test]
    fn test_to_7field_step() {
        assert_eq!(to_7field("*/5 * * * *"), "0 */5 * * * * *");
    }

    #[test]
    fn test_to_7field_specific() {
        assert_eq!(to_7field("0 9 * * 1-5"), "0 0 9 * * 1-5 *");
    }

    #[test]
    fn test_parse_lock_ttl_minutes() {
        let d = parse_lock_ttl("30m").unwrap();
        assert_eq!(d, chrono::Duration::minutes(30));
    }

    #[test]
    fn test_parse_lock_ttl_hours() {
        let d = parse_lock_ttl("1h").unwrap();
        assert_eq!(d, chrono::Duration::hours(1));
    }

    #[test]
    fn test_parse_lock_ttl_invalid() {
        assert!(parse_lock_ttl("bad").is_err());
    }

    #[test]
    fn test_is_lock_fresh_no_lock_file() {
        let dir = tempdir().unwrap();
        // No lock file exists — should return false
        assert!(!is_lock_fresh(dir.path(), "my-job", "30m"));
    }

    #[test]
    fn test_is_lock_fresh_fresh_lock() {
        let dir = tempdir().unwrap();
        // Create lock file with heartbeat = now
        let lock_dir = dir.path().join("crons").join(".locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("my-job.json");
        let lock = LockFile {
            heartbeat: chrono::Utc::now(),
        };
        std::fs::write(&lock_path, serde_json::to_string(&lock).unwrap()).unwrap();
        assert!(is_lock_fresh(dir.path(), "my-job", "30m"));
    }

    #[test]
    fn test_is_lock_fresh_stale_lock() {
        let dir = tempdir().unwrap();
        // Create lock file with heartbeat = 3 hours ago, ttl = 30m
        let lock_dir = dir.path().join("crons").join(".locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("my-job.json");
        let stale_time = chrono::Utc::now() - chrono::Duration::hours(3);
        let lock = LockFile {
            heartbeat: stale_time,
        };
        std::fs::write(&lock_path, serde_json::to_string(&lock).unwrap()).unwrap();
        assert!(!is_lock_fresh(dir.path(), "my-job", "30m"));
    }

    #[test]
    fn default_lock_ttl_uses_six_hours_for_immediate_only() {
        use right_agent::cron_spec::{CronSpec, ScheduleKind};

        let immediate = CronSpec {
            schedule_kind: ScheduleKind::Immediate,
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: None,
            target_thread_id: None,
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        let recurring = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            ..immediate.clone()
        };

        let dir = tempdir().unwrap();
        let lock_dir = dir.path().join("crons").join(".locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("my-job.json");
        let three_hours_ago = chrono::Utc::now() - chrono::Duration::hours(3);
        let lock = LockFile {
            heartbeat: three_hours_ago,
        };
        std::fs::write(&lock_path, serde_json::to_string(&lock).unwrap()).unwrap();

        assert!(is_lock_fresh(
            dir.path(),
            "my-job",
            effective_lock_ttl(&immediate)
        ));
        assert!(!is_lock_fresh(
            dir.path(),
            "my-job",
            effective_lock_ttl(&recurring)
        ));
    }

    // -- CronReplyOutput parser tests (stream-json NDJSON format) --

    #[test]
    fn parse_cron_output_notify_delivery() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":{"text":"BTC broke 100k"},"attachments":null},"run_note":"Checked 5 pairs"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.run_note, "Checked 5 pairs");
        let notify = out.delivery.as_notify().unwrap();
        assert_eq!(notify.content.normalized_text(), "BTC broke 100k");
        assert!(notify.attachments.is_none());
    }

    #[test]
    fn parse_cron_output_silent_delivery() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"silent","reason":"No changes since last run"},"run_note":"Checked feed"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert!(matches!(out.delivery, CronDeliveryDecision::Silent { .. }));
        assert_eq!(
            out.delivery.silent_reason(),
            Some("No changes since last run")
        );
    }

    #[test]
    fn parse_cron_output_structured_output_preferred() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","result":{"delivery":{"kind":"notify","content":"ignored"},"run_note":"ignored"},"structured_output":{"delivery":{"kind":"silent","reason":"from structured"},"run_note":"structured note"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.run_note, "structured note");
        assert_eq!(out.delivery.silent_reason(), Some("from structured"));
    }

    #[test]
    fn parse_cron_output_falls_back_to_result() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","result":{"delivery":{"kind":"silent","reason":"from result field"},"run_note":"result note"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.run_note, "result note");
        assert_eq!(out.delivery.silent_reason(), Some("from result field"));
    }

    #[test]
    fn parse_cron_output_missing_delivery_is_invalid() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"run_note":"note only"}}"#
                .to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("delivery"));
    }

    #[test]
    fn parse_cron_output_empty_stream_is_invalid() {
        let err = parse_cron_output(&[]).unwrap_err();
        assert!(err.contains("no result line"));
    }

    #[test]
    fn parse_cron_output_without_result_line_is_invalid() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#
                .to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("no result line"));
    }

    #[test]
    fn parse_cron_output_empty_notify_content_is_invalid() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":{"text":"   "}},"run_note":"bad"}}"#.to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("visible text"));
    }

    #[test]
    fn notify_from_delivery_json_rejects_empty_content() {
        let err = notify_from_delivery_json(r#"{"kind":"notify","content":"   "}"#).unwrap_err();
        assert!(err.contains("visible text"));
    }

    #[test]
    fn notify_from_delivery_json_upgrades_legacy_string_content() {
        let notify =
            notify_from_delivery_json(r#"{"kind":"notify","content":"legacy *literal*"}"#).unwrap();
        assert_eq!(notify.content.normalized_text(), "legacy *literal*");
    }

    #[test]
    fn notify_from_delivery_json_upgrades_oversized_legacy_string() {
        // A legacy row queued before the rich-content cap must still upgrade
        // and deliver: 40,001 chars is over `MAX_RICH_MESSAGE_UTF16` but the
        // platform-owned upgrade fans it out at delivery time.
        let body = "x".repeat(40_001);
        let raw = format!(r#"{{"kind":"notify","content":"{body}"}}"#);
        let notify = notify_from_delivery_json(&raw).unwrap();
        notify.content.validate().unwrap();
        let parts = notify.content.delivery_parts();
        assert!(parts.len() > 1);
        assert_eq!(
            parts
                .iter()
                .map(right_rich_content::RichContent::normalized_text)
                .collect::<String>(),
            body
        );
    }

    #[test]
    fn notify_from_delivery_json_rejects_whitespace_only_legacy_string() {
        let err = notify_from_delivery_json(r#"{"kind":"notify","content":" \n\t "}"#).unwrap_err();
        assert!(
            err.contains("legacy delivery content") && err.contains("visible text"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_cron_output_empty_silent_reason_is_invalid() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"silent","reason":" "},"run_note":"bad"}}"#.to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("empty silent reason"));
    }

    // -- failure classification (deterministic, no extra CC call) --

    #[test]
    fn classify_failed_result_529_overload_is_transient_user_message() {
        // The real-world 529 result line: subtype "success" but is_error true.
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":529,"result":"API Error: 529 Overloaded."}"#.to_string(),
        ];
        let c = classify_failed_result(&lines).expect("529 is an error result");
        assert!(
            c.user_message.contains("overloaded"),
            "message should name the overload: {}",
            c.user_message
        );
        assert!(
            c.user_message.contains("nothing was lost"),
            "message should reassure no work lost: {}",
            c.user_message
        );
        assert!(
            c.detail.contains("529"),
            "detail keeps status: {}",
            c.detail
        );
    }

    #[test]
    fn classify_failed_result_429_rate_limit() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"rate_limit"}"#.to_string(),
        ];
        let c = classify_failed_result(&lines).unwrap();
        assert!(c.user_message.contains("rate limit"), "{}", c.user_message);
    }

    #[test]
    fn classify_failed_result_max_turns_without_api_status() {
        let lines = vec![
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#
                .to_string(),
        ];
        let c = classify_failed_result(&lines).unwrap();
        assert!(c.user_message.contains("turn limit"), "{}", c.user_message);
    }

    #[test]
    fn classify_failed_result_unknown_error_uses_result_text() {
        let lines = vec![
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom"}"#.to_string(),
        ];
        let c = classify_failed_result(&lines).unwrap();
        assert!(c.user_message.contains("boom"), "{}", c.user_message);
    }

    #[test]
    fn classify_failed_result_returns_none_for_successful_result() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"delivery":{"kind":"notify","content":"ok"},"run_note":"n"}}"#.to_string(),
        ];
        assert!(classify_failed_result(&lines).is_none());
    }

    #[test]
    fn classify_failed_result_returns_none_without_result_line() {
        let lines = vec![r#"{"type":"assistant","message":{"content":[]}}"#.to_string()];
        assert!(classify_failed_result(&lines).is_none());
    }

    #[tokio::test]
    async fn persist_successful_cron_output_notify_sets_pending_delivery() {
        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        let spec = right_agent::cron_spec::CronSpec {
            schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: Some(-100),
            target_thread_id: Some(3),
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-notify",
            "job",
            "2026-01-01T00:00:00Z",
            "/log",
            &spec,
        )
        .await
        .unwrap();
        let output = CronReplyOutput {
            delivery: CronDeliveryDecision::Notify {
                content: right_rich_content::RichContent::literal("done").unwrap(),
                attachments: None,
            },
            run_note: "checked".into(),
        };
        let json = serde_json::to_string(&output.delivery).unwrap();

        let status = persist_successful_cron_output(&conn, "run-notify", &output, &json, false)
            .await
            .unwrap();

        assert_eq!(status, "pending");
        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT run_note, delivery_json, delivery_required, delivery_status FROM async_runs WHERE id = 'run-notify'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "checked");
        let stored: serde_json::Value = serde_json::from_str(&row.1).unwrap();
        assert_eq!(stored["kind"], "notify");
        assert_eq!(stored["content"]["text"], "done");
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "pending");
    }

    #[tokio::test]
    async fn persist_successful_cron_output_silent_sets_none_delivery() {
        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        let spec = right_agent::cron_spec::CronSpec {
            schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: Some(-100),
            target_thread_id: None,
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-silent",
            "job",
            "2026-01-01T00:00:00Z",
            "/log",
            &spec,
        )
        .await
        .unwrap();
        let output = CronReplyOutput {
            delivery: CronDeliveryDecision::Silent {
                reason: "no changes".into(),
            },
            run_note: "checked".into(),
        };
        let json = serde_json::to_string(&output.delivery).unwrap();

        let status = persist_successful_cron_output(&conn, "run-silent", &output, &json, false)
            .await
            .unwrap();

        assert_eq!(status, "none");
        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT run_note, delivery_json, delivery_required, delivery_status FROM async_runs WHERE id = 'run-silent'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "checked");
        assert_eq!(row.1, r#"{"kind":"silent","reason":"no changes"}"#);
        assert_eq!(row.2, 0);
        assert_eq!(row.3, "none");
    }

    #[tokio::test]
    async fn test_triggered_at_loaded_from_db() {
        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        right_agent::cron_spec::create_spec(
            &conn,
            "trig-test",
            "*/5 * * * *",
            "test prompt",
            None,
            None,
        )
        .await
        .unwrap();
        right_agent::cron_spec::trigger_spec(&conn, "trig-test", false, None, None, None, None)
            .await
            .unwrap();

        let specs = right_agent::cron_spec::load_specs_from_db(&conn)
            .await
            .unwrap();
        assert!(
            specs["trig-test"].triggered_at.is_some(),
            "triggered_at should be loaded"
        );
    }

    #[tokio::test]
    async fn test_clear_triggered_at_works() {
        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();

        right_agent::cron_spec::create_spec(&conn, "clr-test", "*/5 * * * *", "test", None, None)
            .await
            .unwrap();
        right_agent::cron_spec::trigger_spec(&conn, "clr-test", false, None, None, None, None)
            .await
            .unwrap();
        right_agent::cron_spec::clear_triggered_at(&conn, "clr-test")
            .await
            .unwrap();

        let specs = right_agent::cron_spec::load_specs_from_db(&conn)
            .await
            .unwrap();
        assert!(
            specs["clr-test"].triggered_at.is_none(),
            "triggered_at should be cleared"
        );
    }

    /// Regression: run_cron_task must exit promptly when shutdown token is cancelled.
    ///
    /// Before the fix, run_job_loop tasks sleep until next fire time (potentially hours).
    /// Shutdown awaited these handles with `handle.await`, causing a hang until
    /// process-compose SIGKILL'd the process after timeout_seconds (10s).
    #[tokio::test]
    async fn shutdown_completes_promptly_with_scheduled_jobs() {
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();

        // Create DB and register a job with a far-future schedule (once per year)
        let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
        right_agent::cron_spec::create_spec(
            &conn,
            "slow-job",
            "0 0 1 1 *", // Jan 1st at midnight — won't fire during test
            "echo test",
            None,
            None,
        )
        .await
        .unwrap();
        drop(conn);

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let ic = Arc::new(right_mcp::internal_client::InternalClient::new(
            "/nonexistent.sock",
        ));
        // No sandbox boots in a unit test: the handle carries bring-up's
        // failure, so every job fires against an unavailable backend.
        let (sandbox_runtime, _sandbox_rx) =
            crate::sandbox_runtime::SandboxRuntimeHandle::new(Err(Arc::new(
                right_sandbox::SandboxCause::HypervisorUnavailable.diagnose(),
            )));
        let model_cell = Arc::new(arc_swap::ArcSwap::from_pointee(None::<String>));
        let cron_handle = tokio::spawn(run_cron_task(
            agent_dir,
            "test-agent".to_string(),
            model_cell,
            sandbox_runtime,
            ic,
            shutdown_clone,
            Arc::new(tokio::sync::RwLock::new(())),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            right_agent_config::LearningConfig::default(),
            Arc::new(dashmap::DashMap::new()),
            crate::telegram::progress::ProgressState::default(),
        ));

        // Give cron engine time to reconcile and spawn the job loop
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Signal shutdown
        shutdown.cancel();

        // Must complete within 2 seconds — if it hangs, the bug is present
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), cron_handle).await;

        assert!(
            result.is_ok(),
            "run_cron_task must exit within 2s of shutdown — \
             job loop handles are likely blocking (not aborted on shutdown)"
        );
    }

    #[test]
    fn is_reconcile_tick_kind_includes_immediate() {
        use right_agent::cron_spec::ScheduleKind;
        assert!(is_reconcile_tick_kind(&ScheduleKind::Immediate));
    }

    #[test]
    fn is_reconcile_tick_kind_excludes_other_kinds() {
        use right_agent::cron_spec::ScheduleKind;
        assert!(!is_reconcile_tick_kind(&ScheduleKind::Recurring(
            "*/5 * * * *".into()
        )));
        assert!(!is_reconcile_tick_kind(&ScheduleKind::OneShotCron(
            "0 9 * * *".into()
        )));
        assert!(!is_reconcile_tick_kind(&ScheduleKind::RunAt(
            chrono::Utc::now()
        )));
    }

    #[test]
    fn is_run_job_loop_skip_kind_includes_runat_and_immediate() {
        use right_agent::cron_spec::ScheduleKind;
        assert!(is_run_job_loop_skip_kind(&ScheduleKind::RunAt(
            chrono::Utc::now()
        )));
        assert!(is_run_job_loop_skip_kind(&ScheduleKind::Immediate));
    }

    #[test]
    fn is_run_job_loop_skip_kind_excludes_recurring_and_oneshotcron() {
        use right_agent::cron_spec::ScheduleKind;
        // OneShotCron runs through run_job_loop (not skipped) — verify.
        assert!(!is_run_job_loop_skip_kind(&ScheduleKind::OneShotCron(
            "0 9 * * *".into()
        )));
        assert!(!is_run_job_loop_skip_kind(&ScheduleKind::Recurring(
            "*/5 * * * *".into()
        )));
    }
}

#[cfg(test)]
mod target_snapshot_tests {
    use super::*;
    use right_agent::cron_spec::{CronSpec, ScheduleKind};

    #[tokio::test]
    async fn consume_cron_stream_breaks_on_terminal_result_without_eof() {
        use std::time::Duration;
        // Prints a terminal result line, then holds stdout open (no EOF) via sleep.
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"UNIT-OK"}'; sleep 30"#,
        );
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");
        let stdout = child.stdout().expect("stdout piped");

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_lines(stdout))
            .await
            .expect("consume_cron_stream must return without waiting for EOF");

        match outcome {
            CronStreamOutcome::Success { collected_lines } => {
                assert!(
                    collected_lines.iter().any(|l| l.contains("UNIT-OK")),
                    "got: {collected_lines:?}"
                );
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn consume_cron_stream_eof_without_result_is_failed() {
        use std::time::Duration;
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c")
            .arg(r#"printf '%s\n' '{"type":"assistant","message":{"content":[]}}'"#);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");
        let stdout = child.stdout().expect("stdout piped");

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_lines(stdout))
            .await
            .expect("must return at EOF");
        assert!(
            matches!(outcome, CronStreamOutcome::Failed { .. }),
            "EOF without a terminal result must be Failed, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn consume_cron_stream_nested_result_is_not_terminal() {
        use std::time::Duration;
        // A result-shaped line carrying parent_tool_use_id is a sub-agent result,
        // NOT the terminal top-level result; EOF then yields Failed.
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(
            r#"printf '%s\n' '{"type":"result","parent_tool_use_id":"toolu_x","result":"NESTED"}'"#,
        );
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");
        let stdout = child.stdout().expect("stdout piped");

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_lines(stdout))
            .await
            .expect("must return at EOF");
        assert!(
            matches!(outcome, CronStreamOutcome::Failed { .. }),
            "nested result must not be treated as terminal, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn consume_cron_stream_is_error_result_is_failed() {
        use std::time::Duration;
        // A terminal top-level result with is_error:true (auth failure, budget
        // exceeded, turn limit) must still BREAK the loop without waiting for EOF
        // (sleep holds stdout open) AND classify as Failed, so the reflection /
        // failure-notify path runs instead of the success-parse path silently
        // dropping the user-facing failure.
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"Budget exceeded"}'; sleep 30"#,
        );
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");
        let stdout = child.stdout().expect("stdout piped");

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_lines(stdout))
            .await
            .expect("must break on the terminal result without waiting for EOF");

        match outcome {
            CronStreamOutcome::Failed { collected_lines } => {
                assert!(
                    collected_lines
                        .iter()
                        .any(|l| l.contains("Budget exceeded")),
                    "got: {collected_lines:?}"
                );
            }
            other => panic!("is_error:true terminal result must be Failed, got {other:?}"),
        }
    }

    async fn migrated_conn() -> (tempfile::TempDir, right_db::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        (dir, conn)
    }

    #[tokio::test]
    async fn insert_running_run_writes_async_runs() {
        let (_dir, conn) = migrated_conn().await;
        let spec = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: Some(-777),
            target_thread_id: Some(13),
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-1",
            "job-x",
            "2026-05-05T12:00:00Z",
            "/log/path",
            &spec,
        )
        .await
        .unwrap();

        let row: (String, String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT kind, producer_ref, target_chat_id, target_thread_id FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, ("cron".into(), "job-x".into(), Some(-777), Some(13)));
    }

    #[tokio::test]
    async fn insert_running_run_writes_zero_for_targetless_cron() {
        let (_dir, conn) = migrated_conn().await;
        let spec = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: None,
            target_thread_id: None,
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-2",
            "job-y",
            "2026-05-05T12:00:00Z",
            "/log/path",
            &spec,
        )
        .await
        .unwrap();

        let target_chat_id: i64 = conn
            .query_row(
                "SELECT target_chat_id FROM async_runs WHERE id = 'run-2'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(target_chat_id, 0);
    }

    #[tokio::test]
    async fn mark_cron_interrupted_by_shutdown_sets_failed_pending_delivery_for_target() {
        let (_dir, conn) = migrated_conn().await;
        let spec = right_agent::cron_spec::CronSpec {
            schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: Some(-777),
            target_thread_id: Some(13),
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-1",
            "job-x",
            "2026-05-05T12:00:00Z",
            "/sandbox/crons/logs/job-x-run-1.ndjson",
            &spec,
        )
        .await
        .unwrap();

        let updated = mark_cron_interrupted_by_shutdown(&conn, "job-x", "shutdown timeout")
            .await
            .unwrap();

        assert_eq!(updated, 1);
        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT status, delivery_status, COALESCE(run_note, ''), COALESCE(error_json, '') \
                 FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "pending");
        assert!(row.2.contains("job-x"));
        assert!(row.3.contains("shutdown timeout"));
    }

    #[tokio::test]
    async fn mark_cron_interrupted_by_shutdown_uses_none_delivery_for_targetless() {
        let (_dir, conn) = migrated_conn().await;
        let spec = right_agent::cron_spec::CronSpec {
            schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: None,
            target_thread_id: None,
            model: None,
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-2",
            "job-y",
            "2026-05-05T12:00:00Z",
            "/log/path",
            &spec,
        )
        .await
        .unwrap();

        let updated = mark_cron_interrupted_by_shutdown(&conn, "job-y", "shutdown timeout")
            .await
            .unwrap();

        assert_eq!(updated, 1);
        let row: (String, i64, String) = conn
            .query_row(
                "SELECT status, delivery_required, delivery_status FROM async_runs WHERE id = 'run-2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, ("failed".to_string(), 0, "none".to_string()));
    }

    #[tokio::test]
    async fn persist_force_notify_silent_delivers_pending() {
        let (_dir, conn) = migrated_conn().await;
        right_agent::async_runs::insert_running_cron_run(
            &conn,
            right_agent::async_runs::NewCronRun {
                id: "run-fns",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(7),
                target_thread_id: None,
                force_notify: true,
            },
        )
        .await
        .unwrap();

        let cron_output = CronReplyOutput {
            delivery: CronDeliveryDecision::Silent {
                reason: "no changes".into(),
            },
            run_note: "checked".into(),
        };
        let content = right_rich_content::RichContent::literal(
            "Verification run — nothing to report. no changes",
        )
        .unwrap();
        let delivery_json = notify_delivery_json(&content, None).unwrap();

        let status =
            persist_successful_cron_output(&conn, "run-fns", &cron_output, &delivery_json, true)
                .await
                .unwrap();
        assert_eq!(status, "pending");

        let required: i64 = conn
            .query_row(
                "SELECT delivery_required FROM async_runs WHERE id = 'run-fns'",
                [],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(required, 1, "forced silent run must require delivery");
    }

    #[tokio::test]
    async fn persist_non_forced_silent_stays_none() {
        let (_dir, conn) = migrated_conn().await;
        right_agent::async_runs::insert_running_cron_run(
            &conn,
            right_agent::async_runs::NewCronRun {
                id: "run-ns",
                job_name: "job",
                started_at: "2026-06-02T00:00:00Z",
                log_path: "/log",
                target_chat_id: Some(7),
                target_thread_id: None,
                force_notify: false,
            },
        )
        .await
        .unwrap();

        let cron_output = CronReplyOutput {
            delivery: CronDeliveryDecision::Silent {
                reason: "no changes".into(),
            },
            run_note: "checked".into(),
        };
        let delivery_json = serde_json::to_string(&cron_output.delivery).unwrap();

        let status =
            persist_successful_cron_output(&conn, "run-ns", &cron_output, &delivery_json, false)
                .await
                .unwrap();
        assert_eq!(
            status, "none",
            "non-forced silent run stays silent (regression)"
        );
    }

    /// Composition: a skill authored under a recurring cron's invocation is
    /// auto-linked, then named in the next run's prompt — with the stored prompt
    /// unchanged. Proves link_cron_authored → list_live_for_job → compose_run_prompt
    /// compose correctly (the live Claude round-trip is covered separately by the
    /// ignored live stub).
    #[tokio::test]
    async fn cron_learned_skill_is_linked_then_named_next_run() {
        let (_dir, conn) = migrated_conn().await;
        // A recurring cron.
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('writer','17 9 * * *','Find sources and write an article',2.0,'t','t')",
            [],
        )
        .await
        .unwrap();
        // Simulate the cron run authoring a skill inline under its invocation id:
        // a successful finish event + a live lifecycle row.
        conn.execute(
            "INSERT INTO skill_learning_events \
             (invocation_id, agent_name, action, skill_name, phase, status, created_at) \
             VALUES ('cron-inv','writer-agent','create','rightx-source-finder','finish','created','t')",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at) \
             VALUES ('rightx-source-finder','active','cron','t')",
            [],
        )
        .await
        .unwrap();

        // Auto-link seam (inline path) links the authored skill to the cron.
        let n = crate::learning_probe_writer::link_cron_authored(&conn, "writer", "cron-inv")
            .await
            .unwrap();
        assert_eq!(n, 1);

        // Next run resolves the live linked skills...
        let live = right_agent::cron_skill_link::list_live_for_job(&conn, "writer")
            .await
            .unwrap();
        assert_eq!(live, vec!["rightx-source-finder".to_string()]);

        // ...and names them in the run prompt, stored prompt unchanged.
        let prompt = compose_run_prompt(
            "Find sources and write an article",
            false,
            None,
            "tok",
            &live,
        );
        assert!(
            prompt.contains("Find sources and write an article"),
            "stored prompt preserved"
        );
        assert!(prompt.contains("## Linked skills"));
        assert!(prompt.contains("rightx-source-finder"));
    }

    #[test]
    fn resolve_cron_model_prefers_spec_then_global() {
        let global = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(Some("opus".to_string())));

        let spec_with = CronSpec {
            schedule_kind: right_agent::cron_spec::ScheduleKind::Recurring("17 9 * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 5.0,
            triggered_at: None,
            trigger_force_notify: false,
            target_chat_id: None,
            target_thread_id: None,
            model: Some("haiku".into()),
            trigger_extra_instruction: None,
            then: None,
            trigger_origin_chat_id: None,
            trigger_origin_thread_id: None,
        };
        assert_eq!(
            resolve_cron_model(&spec_with, &global).as_deref(),
            Some("haiku")
        );

        let mut spec_without = spec_with.clone();
        spec_without.model = None;
        assert_eq!(
            resolve_cron_model(&spec_without, &global).as_deref(),
            Some("opus")
        );

        let empty_global = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(None::<String>));
        assert_eq!(resolve_cron_model(&spec_without, &empty_global), None);
    }
}
