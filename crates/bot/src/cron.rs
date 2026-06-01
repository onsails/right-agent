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
    pub content: String,
    pub attachments: Option<Vec<OutboundAttachment>>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CronDeliveryDecision {
    Notify {
        content: String,
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
            Self::Notify { content, .. } if content.trim().is_empty() => {
                Err("empty notify content".to_string())
            }
            Self::Silent { reason } if reason.trim().is_empty() => {
                Err("empty silent reason".to_string())
            }
            _ => Ok(()),
        }
    }
}

pub(crate) fn notify_delivery_json(
    content: &str,
    attachments: Option<&[OutboundAttachment]>,
) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum DeliveryRef<'a> {
        Notify {
            content: &'a str,
            attachments: Option<&'a [OutboundAttachment]>,
        },
    }
    serde_json::to_string(&DeliveryRef::Notify {
        content,
        attachments,
    })
}

pub(crate) fn notify_from_delivery_json(raw: &str) -> Result<CronNotify, String> {
    let decision: CronDeliveryDecision =
        serde_json::from_str(raw).map_err(|e| format!("parse delivery_json: {e}"))?;
    decision.validate()?;
    decision
        .as_notify()
        .ok_or_else(|| "delivery_json is not a notify decision".to_string())
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

/// Delete old cron log files for a job, keeping the most recent `keep` files.
async fn cleanup_old_logs(
    job_name: &str,
    log_dir: &str,
    keep: usize,
    ssh_config_path: Option<&std::path::Path>,
    resolved_sandbox: Option<&str>,
) {
    // Defense-in-depth: job names should be alphanumeric + hyphens only (validated at creation).
    if !job_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        tracing::error!(job = %job_name, "job name contains unsafe characters, skipping log cleanup");
        return;
    }
    if let Some(ssh_config) = ssh_config_path {
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        // List matching files sorted newest-first, skip `keep`, delete the rest.
        // Using find+stat avoids ls parsing pitfalls with special characters in filenames.
        let cleanup_cmd = format!(
            "find {log_dir} -maxdepth 1 -name '{job_name}-*.ndjson' -printf '%T@ %p\\n' 2>/dev/null | sort -rn | tail -n +{} | cut -d' ' -f2- | xargs -r rm -f",
            keep + 1
        );
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F")
            .arg(ssh_config)
            .arg(&ssh_host)
            .arg("--")
            .arg(&cleanup_cmd);
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        let output = match right_process::ProcessGroupChild::spawn(c) {
            Ok(mut child) => child.wait_with_output().await,
            Err(e) => Err(e),
        };
        match output {
            Ok(o) if !o.status.success() => {
                tracing::warn!(
                    job = %job_name,
                    "log cleanup via SSH failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                tracing::warn!(job = %job_name, "log cleanup SSH command failed: {e:#}");
            }
            _ => {}
        }
    } else {
        let pattern = format!("{job_name}-");
        let dir = match std::fs::read_dir(log_dir) {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = dir
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&pattern) && n.ends_with(".ndjson"))
            })
            .filter_map(|e| {
                let path = e.path();
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((path, mtime))
            })
            .collect();
        files.sort_by(|a, b| b.1.cmp(&a.1));
        for (old, _) in files.into_iter().skip(keep) {
            if let Err(e) = std::fs::remove_file(&old) {
                tracing::warn!(job = %job_name, path = %old.display(), "failed to delete old log: {e:#}");
            }
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
    conn: &right_db::Connection,
    run_id: &str,
    exit_code: Option<i32>,
) {
    update_run_record(conn, run_id, exit_code, "failed").await;
}

async fn persist_successful_cron_output(
    conn: &right_db::Connection,
    run_id: &str,
    cron_output: &CronReplyOutput,
    delivery_json: &str,
) -> Result<&'static str, right_db::DbError> {
    let (delivery_required, delivery_status) = match cron_output.delivery {
        CronDeliveryDecision::Notify { .. } => (true, "pending"),
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
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &right_mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::process::Stdio;

    // Lock check (CRON-04)
    let lock_ttl = effective_lock_ttl(spec);
    if is_lock_fresh(agent_dir, job_name, lock_ttl) {
        tracing::info!(job = %job_name, "skipping — previous run still active (lock fresh)");
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

    // Compute sandbox-relative log path (agents read this via Read tool).
    // For sandbox mode: /sandbox/crons/logs/{job_name}-{run_id}.ndjson
    // For no-sandbox: {agent_dir}/crons/logs/{job_name}-{run_id}.ndjson
    let log_filename = format!("{job_name}-{run_id}.ndjson");
    let sandbox_log_dir = if ssh_config_path.is_some() {
        "/sandbox/crons/logs".to_owned()
    } else {
        agent_dir
            .join("crons")
            .join("logs")
            .to_string_lossy()
            .into_owned()
    };
    let log_path_str = format!("{sandbox_log_dir}/{log_filename}");

    // DB insert: status='running' (D-04)
    // Open per job so DB resource lifetime is bounded to the job run.
    let conn = match right_db::open_connection(agent_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(job = %job_name, "DB open failed: {e:#}");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    if let Err(e) =
        insert_running_run(&conn, &run_id, job_name, &started_at, &log_path_str, spec).await
    {
        tracing::error!(job = %job_name, "DB insert failed: {e:#}");
        std::fs::remove_file(&lock_path).ok();
        return;
    }

    let disallowed_tools = crate::cc::invocation::disallow_foreground_only_tools(
        crate::cc::invocation::baseline_disallowed_tools(),
    );

    let prompt_for_cc = spec.prompt.clone();

    let mcp_path = crate::cc::invocation::mcp_config_path(ssh_config_path, agent_dir);

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
        allowed_tools: vec![],
        disallowed_tools,
        extra_args: vec![],
        prompt: Some(prompt_for_cc),
        debug_flag: Some(std::sync::Arc::clone(&debug)),
    };

    let claude_args = invocation.into_args();

    // Derive sandbox_mode and home_dir from ssh_config_path (same as worker).
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

    if let Err(e) =
        crate::cc::invocation::guard_no_sandboxed_host_exec(resolved_sandbox, ssh_config_path)
    {
        tracing::error!(job = %job_name, "{e:#}");
        update_failed_run_record(&conn, &run_id, None).await;
        std::fs::remove_file(&lock_path).ok();
        return;
    }

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        // Sandbox mode: assemble system prompt via shell script (same as worker).
        let mut assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Cron,
            "/sandbox",
            "/tmp/right-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir).await {
            let escaped = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        assembly_script = format!(
            "set -o pipefail\nmkdir -p /sandbox/crons/logs\n{assembly_script} | tee /sandbox/crons/logs/{log_filename}"
        );
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        // Opt out of multiplexing — see worker.rs `invoke_cc` for the
        // rationale. Cron jobs are long-lived just like worker turns and hit
        // the same hang if the master holds forwarded FDs after we kill the
        // slave.
        c.arg("-o").arg("ControlMaster=no");
        c.arg("-o").arg("ControlPath=none");
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        // Direct exec (no sandbox): verify claude binary exists for clear error.
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = agent_dir.join(".claude").join("cron-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Cron,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        if which::which("claude").is_err() && which::which("claude-bun").is_err() {
            tracing::error!(job = %job_name, "claude binary not found in PATH");
            update_failed_run_record(&conn, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        let host_log_dir = agent_dir.join("crons").join("logs");
        if let Err(e) = std::fs::create_dir_all(&host_log_dir) {
            tracing::error!(job = %job_name, "failed to create log dir: {e:#}");
            update_failed_run_record(&conn, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        let assembly_script =
            format!("set -o pipefail\n{assembly_script} | tee {sandbox_log_dir}/{log_filename}");
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", agent_dir);
        // CC internal env var — "0" = skip bundled rg, use system rg from PATH (D-05, D-06, SBOX-02).
        // Counterintuitive: A_("0")=true means "builtin disabled" -> falls through to system rg.
        // "1" = use CC's vendored rg (default; broken in nix — vendor binary lacks execute bit).
        // UNDOCUMENTED: re-verify after CC version bumps.
        // See: https://github.com/anthropics/claude-code/issues/6415
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir).await {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let mut child = match right_process::ProcessGroupChild::spawn(cmd) {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_failed_run_record(&conn, &run_id, None).await;
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(c) => c,
    };

    // Stream stdout; break on the terminal result event (do not wait for EOF —
    // the SSH stdout pipe can linger open after CC exits).
    let outcome = consume_cron_stream(&mut child).await;
    let collected_lines: Vec<String> = match &outcome {
        CronStreamOutcome::Success {
            collected_lines, ..
        }
        | CronStreamOutcome::Failed { collected_lines } => collected_lines.clone(),
    };

    // Post-stream-loop cleanup. ProcessGroupChild::Drop kills the slave's
    // group on function return, so a hang here can never outlive `execute_job`.
    // Inside the function we still bound each blocking syscall (the same
    // wedged-pipe defense the worker uses).
    let child_pid = child.id();

    let wait_started = tokio::time::Instant::now();
    let exit_status = match tokio::time::timeout(
        std::time::Duration::from_secs(POST_BREAK_WAIT_TIMEOUT_SECS),
        child.wait(),
    )
    .await
    {
        Ok(Ok(s)) => Some(s),
        Ok(Err(e)) => {
            tracing::error!(job = %job_name, child_pid, "wait failed: {e:#}");
            None
        }
        Err(_) => {
            tracing::error!(
                job = %job_name,
                child_pid,
                elapsed_ms = wait_started.elapsed().as_millis() as u64,
                "child.wait timed out — slave wedged; ProcessGroupChild::Drop will killpg on return",
            );
            None
        }
    };
    // Best-effort only: the outcome (Task 1) decides success/failure, not the
    // exit code. A wedged transport leaves `exit_status` None; we still deliver
    // a Success outcome. ProcessGroupChild::Drop killpg's the group on return.
    let exit_code: Option<i32> = exit_status.and_then(|s| s.code());
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
            Err(_) => tracing::error!(
                job = %job_name,
                child_pid,
                bytes_so_far = buf.len(),
                elapsed_ms = read_started.elapsed().as_millis() as u64,
                "stderr read timed out — pipe write-end held by another process",
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
        tracing::error!(job = %job_name, exit_code = ?exit_code, "cron job produced no terminal result");
    }

    // The terminal `status='success'` transition is deferred to the success branch
    // below so it stays atomic with the output persist. The failure branch performs
    // its own `update_run_record(failed)` before reflection. This prevents a state
    // where `status='success'` is committed but output preservation fails, leaving
    // the row at `delivery_status='none'` and silently dropping user-visible output.

    // Delete lock on completion (CRON-04)
    std::fs::remove_file(&lock_path).ok();

    // Retention: keep last 10 log files per job (fire-and-forget to avoid SSH overhead on hot path)
    let job_name_owned = job_name.to_owned();
    let log_dir_owned = sandbox_log_dir.clone();
    let ssh_config_owned = ssh_config_path.map(|p| p.to_owned());
    let sandbox_owned = resolved_sandbox.map(|s| s.to_owned());
    tokio::spawn(async move {
        cleanup_old_logs(
            &job_name_owned,
            &log_dir_owned,
            10,
            ssh_config_owned.as_deref(),
            sandbox_owned.as_deref(),
        )
        .await;
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
                        } else if ssh_config_path.is_some() {
                            let sandbox = resolved_sandbox.unwrap();
                            for att in atts {
                                let dest = outbox_dir.join(attachment_filename(&att.path));
                                if let Err(e) = right_openshell::openshell::download_file(
                                    sandbox, &att.path, &dest,
                                )
                                .await
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
                    other => serde_json::to_string(other)
                        .map_err(|e| {
                            tracing::error!(job = %job_name, "failed to serialize delivery_json: {e:#}");
                        })
                        .ok(),
                };

                    // Persist output and flip status='success' atomically. If either
                    // write fails, roll back and mark the row 'failed' so the operator
                    // sees the run as broken instead of stuck at 'success' with no
                    // delivery payload.
                    if let Some(delivery_json) = delivery_json {
                        let tx_result: Result<&'static str, right_db::DbError> = async {
                            let tx = conn.transaction().await?;
                            let delivery_status = persist_successful_cron_output(
                                &tx,
                                &run_id,
                                &cron_output,
                                &delivery_json,
                            )
                            .await?;
                            right_agent::async_runs::finish_run(&tx, &run_id, exit_code, "success")
                                .await?;
                            tx.commit().await?;
                            Ok(delivery_status)
                        }
                        .await;

                        match tx_result {
                            Ok(delivery_status) => {
                                tracing::info!(
                                    job = %job_name,
                                    has_notify = matches!(cron_output.delivery, CronDeliveryDecision::Notify { .. }),
                                    delivery_status,
                                    silent_reason = cron_output.delivery.silent_reason().unwrap_or("-"),
                                    "cron output persisted to DB"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    job = %job_name,
                                    "failed to persist cron output atomically; marking run failed: {e:#}"
                                );
                                update_failed_run_record(&conn, &run_id, exit_code).await;
                            }
                        }
                    } else {
                        tracing::error!(
                            job = %job_name,
                            "failed to produce delivery_json; marking run failed"
                        );
                        update_failed_run_record(&conn, &run_id, exit_code).await;
                    }
                }
                Err(reason) => {
                    tracing::warn!(job = %job_name, reason, "failed to parse cron output");
                    let error_json_str = serde_json::json!({
                        "kind": "cron_parse_failed",
                        "reason": reason,
                    })
                    .to_string();
                    let tx_result: Result<(), right_db::DbError> = async {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::persist_run_output(
                            &tx,
                            &run_id,
                            right_agent::async_runs::RunOutput {
                                run_note: None,
                                delivery_json: None,
                                error_json: Some(&error_json_str),
                                delivery_required: false,
                            },
                        )
                        .await?;
                        right_agent::async_runs::finish_run(&tx, &run_id, exit_code, "failed")
                            .await?;
                        tx.commit().await?;
                        Ok(())
                    }
                    .await;

                    if let Err(e) = tx_result {
                        tracing::error!(
                            job = %job_name,
                            "failed to persist cron parse error atomically: {e:#}"
                        );
                        update_failed_run_record(&conn, &run_id, exit_code).await;
                    }
                }
            }
        }
        CronStreamOutcome::Failed { collected_lines } => {
            // Failure path: commit terminal status='failed' before reflection runs.
            // Reflection then writes its own failure notify via persist_run_output,
            // which is consistent with status='failed'.
            update_run_record(&conn, &run_id, exit_code, "failed").await;
            let exit_str = exit_code.map_or("unknown".to_string(), |c| c.to_string());
            let raw_detail = find_last_result_line(&collected_lines)
                .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(String::from))
                .unwrap_or_else(|| stderr_str.to_string());
            let raw_content =
                format!("Cron job `{job_name}` failed (exit code {exit_str}):\n{raw_detail}");

            let failure_kind =
                classify_cron_failure(exit_code, &raw_detail, spec.max_budget_usd, None);

            // Best-effort ring buffer: parse last ~5 stream-json lines from collected_lines,
            // keeping only displayable events. Chronological order (oldest → newest) to
            // match worker's EventRingBuffer convention.
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
                failure: failure_kind,
                ring_buffer_tail: ring_tail,
                limits: crate::reflection::ReflectionLimits::CRON,
                agent_name: agent_name.to_string(),
                agent_dir: agent_dir.to_path_buf(),
                ssh_config_path: ssh_config_path.map(std::path::PathBuf::from),
                resolved_sandbox: resolved_sandbox.map(String::from),
                parent_source: crate::reflection::ParentSource::Cron {
                    job_name: job_name.to_string(),
                },
                model: model.map(String::from),
                debug: Some(std::sync::Arc::clone(&debug)),
            };

            let reflected_content = match crate::reflection::reflect_on_failure(refl_ctx).await {
                Ok(text) => {
                    tracing::info!(job = %job_name, "cron reflection reply produced");
                    text
                }
                Err(e) => {
                    tracing::warn!(job = %job_name, "cron reflection failed: {e:#}; using raw content");
                    raw_content
                }
            };

            match notify_delivery_json(&reflected_content, None) {
                Ok(json) => {
                    if let Err(e) = right_agent::async_runs::persist_run_output(
                        &conn,
                        &run_id,
                        right_agent::async_runs::RunOutput {
                            run_note: Some("failed"),
                            delivery_json: Some(&json),
                            error_json: None,
                            delivery_required: true,
                        },
                    )
                    .await
                    {
                        tracing::error!(job = %job_name, "failed to persist failure notify to DB: {e:#}");
                    }
                }
                Err(e) => {
                    tracing::error!(job = %job_name, "failed to serialize failure notify: {e:#}");
                }
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
                if let Err(e) =
                    right_agent::usage::insert::insert_cron(&conn, &breakdown, job_name).await
                {
                    tracing::warn!(job = %job_name, "usage insert failed: {e:#}");
                }
            }
            None => {
                tracing::warn!(job = %job_name, "result event missing required usage fields");
            }
        }
    }
}

/// Return the last NDJSON line whose `type` field equals `"result"`.
fn find_last_result_line(lines: &[String]) -> Option<&str> {
    lines.iter().rev().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("type").and_then(|t| t.as_str()) == Some("result")).then_some(line.as_str())
    })
}

/// Outcome of consuming the cron CC subprocess stdout stream.
///
/// `collected_lines` carries every NDJSON line read from stdout so the caller
/// can run [`parse_cron_output`].
#[derive(Debug)]
pub(crate) enum CronStreamOutcome {
    /// A terminal top-level `{"type":"result"}` event was observed (the loop
    /// broke on it, or it was found at EOF).
    Success { collected_lines: Vec<String> },
    /// Stdout reached EOF without a terminal `result` event.
    Failed { collected_lines: Vec<String> },
}

/// Return `true` iff the line is the terminal top-level CC result event.
///
/// CC emits exactly one top-level `{"type":"result"}` summary at the end of a
/// turn. Sub-agent (Task tool) results arrive as nested `assistant`/`user`
/// messages carrying `parent_tool_use_id`, so `type == "result"` is already
/// terminal; the `parent_tool_use_id` absent/null check is defense-in-depth.
///
/// This shares the `type == "result"` test with [`find_last_result_line`] but
/// deliberately differs: this predicate adds the top-level `parent_tool_use_id`
/// guard and is forward (it drives the live break-on-terminal loop), while
/// `find_last_result_line` is reverse/borrowing for `parse_cron_output`'s
/// fallback. A future change to "what counts as a result" likely touches both.
fn is_terminal_result_line(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    let is_result = v.get("type").and_then(|t| t.as_str()) == Some("result");
    let top_level = v.get("parent_tool_use_id").is_none_or(|p| p.is_null());
    is_result && top_level
}

/// Consume the cron CC subprocess stdout line-by-line and classify the outcome.
///
/// Breaks immediately on the terminal top-level `result` event (does NOT wait
/// for EOF — the SSH stdout pipe can linger open after CC exits). On EOF,
/// returns `Success` iff a terminal result was seen, else `Failed`. There is no
/// wall-clock bound here; a turn that never emits a result is bounded by the
/// shutdown-drain (`SHUTDOWN_JOB_TIMEOUT`).
pub(crate) async fn consume_cron_stream(
    child: &mut right_process::ProcessGroupChild,
) -> CronStreamOutcome {
    let stdout = child.stdout().expect("stdout piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut collected_lines: Vec<String> = Vec::new();
    let mut saw_terminal_result = false;
    while let Ok(Some(line)) = lines.next_line().await {
        if !saw_terminal_result && is_terminal_result_line(&line) {
            saw_terminal_result = true;
        }
        collected_lines.push(line);
        if saw_terminal_result {
            break; // terminal result seen — do not wait for EOF
        }
    }

    if saw_terminal_result {
        CronStreamOutcome::Success { collected_lines }
    } else {
        CronStreamOutcome::Failed { collected_lines }
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
            if v.get("type").and_then(|t| t.as_str()) == Some("result") {
                Some(v)
            } else {
                None
            }
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
    conn: &right_db::Connection,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) {
    if let Err(e) = right_agent::async_runs::finish_run(conn, run_id, exit_code, status).await {
        tracing::error!("DB update for run {run_id} failed: {e:#}");
    }
}

/// Timeout for waiting on in-flight execute_job tasks during shutdown.
///
pub(crate) const SHUTDOWN_JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    tracing::info!(agent = %agent_name, "cron task started");

    let conn = match right_db::open_connection(&agent_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(agent = %agent_name, "cron task: DB open failed: {e:#}");
            return;
        }
    };

    let execute_handles: ExecuteHandles = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut triggered_handles: Vec<JoinHandle<()>> = Vec::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(
        &mut handles,
        &mut triggered_handles,
        &conn,
        &agent_dir,
        &agent_name,
        &model,
        &ssh_config_path,
        &internal_client,
        &execute_handles,
        &resolved_sandbox,
        &upgrade_lock,
        &debug,
    )
    .await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &mut triggered_handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path, &internal_client, &execute_handles, &resolved_sandbox, &upgrade_lock, &debug).await;
            }
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "cron shutdown: stopping reconciler");
                break;
            }
        }
    }

    // Phase 1: Abort all job scheduler loops (sleeping until next fire time).
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
                    match right_db::open_connection(&agent_dir, false).await {
                        Ok(conn) => {
                            if let Err(e) =
                                mark_cron_interrupted_by_shutdown(&conn, &name, "shutdown timeout")
                                    .await
                            {
                                tracing::error!(
                                    job = %name,
                                    "cron shutdown: mark interrupted failed: {e:#}"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                job = %name,
                                "cron shutdown: DB open to mark interrupted failed: {e:#}"
                            );
                        }
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

/// Delete a one-shot spec after it has fired. Opens a fresh DB connection
/// (callers are inside `tokio::spawn` and cannot share the reconciler's connection).
async fn delete_one_shot_spec(agent_dir: &std::path::Path, job_name: &str) {
    let conn = match right_db::open_connection(agent_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(job = %job_name, "failed to open DB for post-fire delete: {e:#}");
            return;
        }
    };
    if let Err(e) = right_agent::cron_spec::delete_spec(&conn, job_name, agent_dir).await {
        tracing::error!(job = %job_name, "failed to delete one-shot spec after fire: {e}");
    } else {
        tracing::info!(job = %job_name, "one-shot spec auto-deleted after fire");
    }
}

/// Fire a batch of one-shot specs (RunAt or Immediate). Each becomes a spawned
/// `execute_job` followed by `delete_one_shot_spec`. The lock check is best-effort —
/// `execute_job` re-checks under the upgrade-lock guard before writing the lock file.
#[allow(clippy::too_many_arguments)]
fn fire_one_shot_specs(
    specs: Vec<(String, CronSpec)>,
    kind_label: &'static str,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    ssh_config_path: &Option<std::path::PathBuf>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    resolved_sandbox: &Option<String>,
    upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: &std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        let md: Option<String> = crate::snapshot_model(model);
        let sc = ssh_config_path.clone();
        let ic = Arc::clone(internal_client);
        let rs = resolved_sandbox.clone();
        let ul = Arc::clone(upgrade_lock);
        let dbg = debug.clone();
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
                sc.as_deref(),
                &ic,
                rs.as_deref(),
                ul,
                dbg,
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
    conn: &right_db::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Arc<arc_swap::ArcSwap<Option<String>>>,
    ssh_config_path: &Option<std::path::PathBuf>,
    internal_client: &Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    resolved_sandbox: &Option<String>,
    upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    // Clean up finished triggered handles
    triggered_handles.retain(|h| !h.is_finished());
    let new_specs = match right_agent::cron_spec::load_specs_from_db(conn).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to load cron specs from DB: {e}");
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
        ssh_config_path,
        internal_client,
        execute_handles,
        resolved_sandbox,
        upgrade_lock,
        debug,
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
        ssh_config_path,
        internal_client,
        execute_handles,
        resolved_sandbox,
        upgrade_lock,
        debug,
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
        let job_ssh_config = ssh_config_path.clone();
        let job_execute_handles = Arc::clone(execute_handles);
        let job_internal_client = Arc::clone(internal_client);
        let job_sandbox = resolved_sandbox.clone();
        let job_upgrade_lock = Arc::clone(upgrade_lock);
        let job_debug = debug.clone();

        let handle = tokio::spawn(async move {
            run_job_loop(
                job_name,
                job_spec,
                job_agent_dir,
                job_agent_name,
                job_model,
                job_ssh_config,
                job_internal_client,
                job_execute_handles,
                job_sandbox,
                job_upgrade_lock,
                job_debug,
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
            if let Err(e) = right_agent::cron_spec::clear_triggered_at(conn, name).await {
                tracing::error!(job = %name, "failed to clear triggered_at: {e}");
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
            let md: Option<String> = crate::snapshot_model(model);
            let sc = ssh_config_path.clone();
            let ic = Arc::clone(internal_client);
            let rs = resolved_sandbox.clone();
            let ul = Arc::clone(upgrade_lock);
            let dbg = debug.clone();
            tracing::info!(job = %name, "executing triggered job");
            let trigger_name = name.clone();
            let handle = tokio::spawn(async move {
                execute_job(
                    &jn,
                    &sp,
                    &ad,
                    &an,
                    md.as_deref(),
                    sc.as_deref(),
                    &ic,
                    rs.as_deref(),
                    ul,
                    dbg,
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
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    execute_handles: ExecuteHandles,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        let md: Option<String> = crate::snapshot_model(&model);
        let sc = ssh_config_path.clone();
        let ic = Arc::clone(&internal_client);
        let rs = resolved_sandbox.clone();
        let ul = Arc::clone(&upgrade_lock);
        let dbg = debug.clone();
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
                sc.as_deref(),
                &ic,
                rs.as_deref(),
                ul,
                dbg,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
            target_chat_id: None,
            target_thread_id: None,
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
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":"BTC broke 100k","attachments":null},"run_note":"Checked 5 pairs"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.run_note, "Checked 5 pairs");
        let notify = out.delivery.as_notify().unwrap();
        assert_eq!(notify.content, "BTC broke 100k");
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
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":"   "},"run_note":"bad"}}"#.to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("empty notify content"));
    }

    #[test]
    fn notify_from_delivery_json_rejects_empty_content() {
        let err = notify_from_delivery_json(r#"{"kind":"notify","content":"   "}"#).unwrap_err();
        assert!(err.contains("empty notify content"));
    }

    #[test]
    fn parse_cron_output_empty_silent_reason_is_invalid() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"silent","reason":" "},"run_note":"bad"}}"#.to_string(),
        ];
        let err = parse_cron_output(&lines).unwrap_err();
        assert!(err.contains("empty silent reason"));
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
            target_chat_id: Some(-100),
            target_thread_id: Some(3),
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
                content: "done".into(),
                attachments: None,
            },
            run_note: "checked".into(),
        };
        let json = serde_json::to_string(&output.delivery).unwrap();

        let status = persist_successful_cron_output(&conn, "run-notify", &output, &json)
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
        assert_eq!(
            row.1,
            r#"{"kind":"notify","content":"done","attachments":null}"#
        );
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
            target_chat_id: Some(-100),
            target_thread_id: None,
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

        let status = persist_successful_cron_output(&conn, "run-silent", &output, &json)
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
        right_agent::cron_spec::trigger_spec(&conn, "trig-test")
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
        right_agent::cron_spec::trigger_spec(&conn, "clr-test")
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
        let model_cell = Arc::new(arc_swap::ArcSwap::from_pointee(None::<String>));
        let cron_handle = tokio::spawn(run_cron_task(
            agent_dir,
            "test-agent".to_string(),
            model_cell,
            None,
            ic,
            shutdown_clone,
            None,
            Arc::new(tokio::sync::RwLock::new(())),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
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

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
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

        let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
            .await
            .expect("must return at EOF");
        assert!(
            matches!(outcome, CronStreamOutcome::Failed { .. }),
            "nested result must not be treated as terminal, got {outcome:?}"
        );
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
            target_chat_id: Some(-777),
            target_thread_id: Some(13),
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
            target_chat_id: None,
            target_thread_id: None,
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
            target_chat_id: Some(-777),
            target_thread_id: Some(13),
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
            target_chat_id: None,
            target_thread_id: None,
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

    /// Regression guard for the wedged-stdout hang (see `consume_cron_stream`).
    ///
    /// Builds the SSH command exactly like cron's sandbox branch
    /// (`-F <cfg> -o ControlMaster=no -o ControlPath=none <host> -- <script>`)
    /// and runs a remote script that prints one canonical CC `result` line and
    /// then holds stdout open without closing it (`sleep`). The SSH stdout pipe
    /// therefore never reaches EOF.
    ///
    /// The fix: `consume_cron_stream` breaks the read loop on the terminal
    /// top-level `result` event (detected via `is_terminal_result_line`) instead
    /// of waiting for EOF. There is intentionally NO wall-clock deadline — the
    /// 60s shutdown-drain (`SHUTDOWN_JOB_TIMEOUT`) is the only backstop.
    ///
    /// So `consume_cron_stream` returns `Success` carrying `REPRO-OK` well
    /// within the outer 25s timeout. If the break-on-terminal-result logic
    /// regressed to an unbounded `next_line()` EOF loop, the inner call would
    /// hang on the never-closing pipe, the outer 25s timeout would fire, and
    /// the `.expect()` below would fail — that timeout IS the reproduction.
    #[ignore = "ci-openshell: creates real sandbox"]
    #[tokio::test]
    async fn ci_openshell_cron_stream_survives_wedged_stdout() {
        use std::time::Duration;

        let sandbox =
            right_openshell::test_support::TestSandbox::create("cron-wedged-stdout").await;

        // Materialize the per-sandbox ssh-config the same way the bot does for
        // cron's sandbox branch (`openshell sandbox ssh-config NAME`).
        let cfg_dir = tempfile::tempdir().expect("tempdir");
        let ssh_config =
            right_openshell::openshell::generate_ssh_config(sandbox.name(), cfg_dir.path())
                .await
                .expect("generate ssh-config");
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(sandbox.name());

        // Deterministic "result emitted, then stdout never EOFs": print one
        // canonical CC stream-json result line, then hold the remote shell
        // (and thus the SSH stdout channel) open so `next_line()` never sees EOF.
        let remote_script = r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"REPRO-OK","session_id":"repro","terminal_reason":"completed"}'
sleep 120"#;

        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-F").arg(&ssh_config);
        cmd.arg("-o").arg("ControlMaster=no");
        cmd.arg("-o").arg("ControlPath=none");
        cmd.arg(&ssh_host);
        cmd.arg("--");
        cmd.arg(remote_script);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn ssh");

        // Bound the whole test with an outer timeout so the RED fails fast
        // instead of hanging CI. ProcessGroupChild Drop killpg's the lingering
        // ssh/sleep when `child` drops on return.
        let outcome =
            tokio::time::timeout(Duration::from_secs(25), consume_cron_stream(&mut child)).await;

        let outcome = outcome.expect(
            "consume_cron_stream did not return within 25s — wedged-stdout hang reproduced \
             (the unbounded next_line() loop never sees EOF). This is the RED repro: it goes \
             GREEN once consume_cron_stream breaks on the terminal result event.",
        );

        match outcome {
            CronStreamOutcome::Success { collected_lines } => {
                assert!(
                    collected_lines.iter().any(|l| l.contains("REPRO-OK")),
                    "collected_lines should contain the REPRO-OK result line, got: {collected_lines:?}"
                );
            }
            other => panic!("expected Success with REPRO-OK result, got: {other:?}"),
        }
    }
}
