use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt as _;
use tokio::io::AsyncReadExt as _;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use rightclaw::cron_spec::CronSpec;

/// Lock file JSON: {"heartbeat": "2026-...Z"}
#[derive(serde::Deserialize, serde::Serialize)]
struct LockFile {
    heartbeat: chrono::DateTime<chrono::Utc>,
}

/// Errors produced by the cron engine.
#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("claude binary not found in PATH")]
    BinaryNotFound,
    #[error("invalid lock_ttl format '{0}' — expected e.g. '30m' or '1h'")]
    InvalidLockTtl(String),
    #[error("cron expression parse error: {0}")]
    ScheduleParse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("db error: {0:#}")]
    Db(#[from] rightclaw::memory::MemoryError),
}

/// Structured output from a cron CC invocation.
#[derive(Debug, serde::Deserialize)]
pub struct CronReplyOutput {
    pub notify: Option<CronNotify>,
    pub summary: String,
    pub no_notify_reason: Option<String>,
}

/// User-facing notification from a cron job.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct CronNotify {
    pub content: String,
    pub attachments: Option<Vec<crate::telegram::attachments::OutboundAttachment>>,
}

/// Extract the filename component from a sandbox attachment path.
fn attachment_filename(path: &str) -> String {
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
pub fn to_7field(expr: &str) -> String {
    format!("0 {} *", expr.trim())
}

/// Parse a lock_ttl string ("30m", "1h") into a `chrono::Duration`.
pub fn parse_lock_ttl(s: &str) -> Result<chrono::Duration, CronError> {
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
pub fn is_lock_fresh(agent_dir: &std::path::Path, job_name: &str, lock_ttl_str: &str) -> bool {
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
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
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
        let output = match rightclaw::process_group::ProcessGroupChild::spawn(c) {
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

/// Execute one cron job: lock check → DB insert → subprocess → log write → DB update → lock delete.
///
/// Per D-02: subprocess failures log `tracing::error` only, do not propagate.
/// Results are persisted to the `cron_runs` table (summary + notify_json).
/// A separate Telegram delivery loop reads pending rows and sends notifications.
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
) {
    use std::process::Stdio;

    // Lock check (CRON-04)
    let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
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
    // Open connection per-job — rusqlite::Connection is !Send
    let conn = match rightclaw::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(job = %job_name, "DB open failed: {e:#}");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    if let Err(e) = conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path) VALUES (?1, ?2, ?3, 'running', ?4)",
        rusqlite::params![run_id, job_name, started_at, log_path_str],
    ) {
        tracing::error!(job = %job_name, "DB insert failed: {e:#}");
        std::fs::remove_file(&lock_path).ok();
        return;
    }

    // Disallow CC built-in tools — cron jobs must not self-schedule, manage tasks, or spawn subagents.
    // Agent is disabled to prevent budget waste on parallel subagent branches.
    let disallowed_tools: Vec<String> = [
        "Agent",
        "CronCreate",
        "CronList",
        "CronDelete",
        "TaskCreate",
        "TaskUpdate",
        "TaskList",
        "TaskGet",
        "TaskOutput",
        "TaskStop",
        "EnterPlanMode",
        "ExitPlanMode",
        "RemoteTrigger",
    ]
    .iter()
    .map(|&s| s.into())
    .collect();

    let mcp_path = crate::telegram::invocation::mcp_config_path(ssh_config_path, agent_dir);

    let invocation = crate::telegram::invocation::ClaudeInvocation {
        mcp_config_path: mcp_path,
        json_schema: rightclaw::codegen::CRON_SCHEMA_JSON.into(),
        output_format: crate::telegram::invocation::OutputFormat::StreamJson,
        model: model.map(|s| s.to_owned()),
        max_budget_usd: Some(spec.max_budget_usd),
        max_turns: None,
        resume_session_id: None,
        new_session_id: Some(run_id.clone()),
        disallowed_tools,
        extra_args: vec![],
        prompt: Some(spec.prompt.clone()),
    };

    let claude_args = invocation.into_args();

    // Derive sandbox_mode and home_dir from ssh_config_path (same as worker).
    let (sandbox_mode, home_dir) = if ssh_config_path.is_some() {
        (
            rightclaw::agent::types::SandboxMode::Openshell,
            "/sandbox".to_owned(),
        )
    } else {
        (
            rightclaw::agent::types::SandboxMode::None,
            agent_dir.to_string_lossy().into_owned(),
        )
    };
    let base_prompt =
        rightclaw::codegen::generate_system_prompt(agent_name, &sandbox_mode, &home_dir);

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> = match internal_client.mcp_instructions(agent_name).await
    {
        Ok(resp) => {
            if resp.instructions.trim().len()
                > rightclaw::codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER
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
    let memory_mode: Option<crate::telegram::prompt::MemoryMode> = None;

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        // Sandbox mode: assemble system prompt via shell script (same as worker).
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            "/sandbox",
            "/tmp/rightclaw-system-prompt.md",
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
        assembly_script = format!(
            "set -o pipefail\nmkdir -p /sandbox/crons/logs\n{assembly_script} | tee /sandbox/crons/logs/{log_filename}"
        );
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        // Direct exec (no sandbox): verify claude binary exists for clear error.
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = agent_dir.join(".claude").join("cron-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
            memory_mode.as_ref(),
        );
        if which::which("claude").is_err() && which::which("claude-bun").is_err() {
            tracing::error!(job = %job_name, "claude binary not found in PATH");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        let host_log_dir = agent_dir.join("crons").join("logs");
        if let Err(e) = std::fs::create_dir_all(&host_log_dir) {
            tracing::error!(job = %job_name, "failed to create log dir: {e:#}");
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
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let mut child = match rightclaw::process_group::ProcessGroupChild::spawn(cmd) {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(c) => c,
    };

    // Stream stdout line-by-line; tee inside the subprocess writes the NDJSON log.
    let stdout = child.stdout().expect("stdout piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut collected_lines: Vec<String> = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        collected_lines.push(line);
    }

    // Wait for child exit and capture stderr.
    let exit_status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(job = %job_name, "wait failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };
    // stderr is still owned by child — read it via the handle.
    let stderr_bytes = if let Some(mut stderr) = child.stderr() {
        let mut buf = Vec::new();
        if let Err(e) = stderr.read_to_end(&mut buf).await {
            tracing::warn!(job = %job_name, "failed to read stderr: {e:#}");
        }
        buf
    } else {
        Vec::new()
    };
    let stderr_str = String::from_utf8_lossy(&stderr_bytes);

    // Determine status (D-02)
    let exit_code = exit_status.code();
    let status = if exit_status.success() {
        "success"
    } else {
        tracing::error!(
            job = %job_name,
            exit_code = ?exit_code,
            "cron job subprocess failed"
        );
        "failed"
    };

    // DB update on completion (D-04)
    update_run_record(&conn, &run_id, exit_code, status);

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
    if exit_status.success() {
        match parse_cron_output(&collected_lines) {
            Ok(cron_output) => {
                // Download attachments from sandbox to host outbox
                let notify_json = if let Some(ref notify) = cron_output.notify {
                    if let Some(ref atts) = notify.attachments {
                        let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                        if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
                            tracing::error!(job = %job_name, "failed to create cron outbox dir: {e:#}");
                        } else if ssh_config_path.is_some() {
                            let sandbox = resolved_sandbox.unwrap();
                            for att in atts {
                                let dest = outbox_dir.join(attachment_filename(&att.path));
                                if let Err(e) =
                                    rightclaw::openshell::download_file(&sandbox, &att.path, &dest)
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

                        // Rewrite paths to host-side
                        let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                        let host_notify = CronNotify {
                            content: notify.content.clone(),
                            attachments: Some(
                                atts.iter()
                                    .map(|att| crate::telegram::attachments::OutboundAttachment {
                                        kind: att.kind,
                                        path: outbox_dir
                                            .join(attachment_filename(&att.path))
                                            .to_string_lossy()
                                            .into_owned(),
                                        filename: att.filename.clone(),
                                        caption: att.caption.clone(),
                                        media_group_id: att.media_group_id.clone(),
                                    })
                                    .collect(),
                            ),
                        };
                        match serde_json::to_string(&host_notify) {
                            Ok(json) => Some(json),
                            Err(e) => {
                                tracing::error!(job = %job_name, "failed to serialize notify_json: {e:#}");
                                None
                            }
                        }
                    } else {
                        match serde_json::to_string(notify) {
                            Ok(json) => Some(json),
                            Err(e) => {
                                tracing::error!(job = %job_name, "failed to serialize notify_json: {e:#}");
                                None
                            }
                        }
                    }
                } else {
                    None
                };

                let delivery_status = if cron_output.notify.is_some() {
                    "pending"
                } else {
                    "silent"
                };
                if let Err(e) = conn.execute(
                    "UPDATE cron_runs SET summary = ?1, notify_json = ?2, delivery_status = ?3, no_notify_reason = ?4 WHERE id = ?5",
                    rusqlite::params![cron_output.summary, notify_json, delivery_status, cron_output.no_notify_reason, run_id],
                ) {
                    tracing::error!(job = %job_name, "failed to persist cron output to DB: {e:#}");
                }

                tracing::info!(
                    job = %job_name,
                    has_notify = cron_output.notify.is_some(),
                    delivery_status,
                    no_notify_reason = cron_output.no_notify_reason.as_deref().unwrap_or("-"),
                    "cron output persisted to DB"
                );
            }
            Err(reason) => {
                tracing::warn!(job = %job_name, reason, "failed to parse cron output");
            }
        }
    } else {
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
            .map(|line| crate::telegram::stream::parse_stream_event(line))
            .filter(|e| {
                matches!(
                    e,
                    crate::telegram::stream::StreamEvent::Text(_)
                        | crate::telegram::stream::StreamEvent::Thinking
                        | crate::telegram::stream::StreamEvent::ToolUse { .. }
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

        let notify = CronNotify {
            content: reflected_content,
            attachments: None,
        };
        match serde_json::to_string(&notify) {
            Ok(json) => {
                if let Err(e) = conn.execute(
                    "UPDATE cron_runs SET summary = ?1, notify_json = ?2, delivery_status = 'pending' WHERE id = ?3",
                    rusqlite::params!["failed", json, run_id],
                ) {
                    tracing::error!(job = %job_name, "failed to persist failure notify to DB: {e:#}");
                }
            }
            Err(e) => {
                tracing::error!(job = %job_name, "failed to serialize failure notify: {e:#}");
            }
        }
    }

    if let Some(result_line) = find_last_result_line(&collected_lines) {
        match crate::telegram::stream::parse_usage_full(result_line) {
            Some(mut breakdown) => {
                // Scan all lines for the init event (first line that matches wins).
                breakdown.api_key_source = collected_lines
                    .iter()
                    .find_map(|l| crate::telegram::stream::parse_api_key_source(l))
                    .unwrap_or_else(|| "none".into());
                if let Err(e) = rightclaw::usage::insert::insert_cron(&conn, &breakdown, job_name) {
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

    serde_json::from_value(payload.clone())
        .map_err(|e| format!("failed to parse CronReplyOutput: {e}"))
}

fn update_run_record(
    conn: &rusqlite::Connection,
    run_id: &str,
    exit_code: Option<i32>,
    status: &str,
) {
    let finished_at = chrono::Utc::now().to_rfc3339();
    if let Err(e) = conn.execute(
        "UPDATE cron_runs SET finished_at=?1, exit_code=?2, status=?3 WHERE id=?4",
        rusqlite::params![finished_at, exit_code, status, run_id],
    ) {
        tracing::error!("DB update for run {run_id} failed: {e:#}");
    }
}

/// Timeout for waiting on in-flight execute_job tasks during shutdown.
const SHUTDOWN_JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Shared storage for in-flight execute_job handles.
///
/// `run_job_loop` and triggered job spawns push handles here.
/// Shutdown collects and awaits them with a timeout.
type ExecuteHandles = Arc<std::sync::Mutex<Vec<(String, JoinHandle<()>)>>>;

/// Main reconciler loop. Polls `crons/*.yaml` every 60s, spawning per-job loops.
///
/// Cron results are persisted to DB. A separate delivery loop reads pending rows
/// and sends Telegram notifications.
///
/// Signature expected by lib.rs spawn site (CRON-01, CRON-02, CRON-06).
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
) {
    tracing::info!(agent = %agent_name, "cron task started");

    let conn = match rightclaw::memory::open_connection(&agent_dir, false) {
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
    );

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &mut triggered_handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path, &internal_client, &execute_handles, &resolved_sandbox, &upgrade_lock);
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
    let pending: Vec<(String, JoinHandle<()>)> = {
        let mut guard = execute_handles
            .lock()
            .expect("execute_handles mutex poisoned");
        guard.drain(..).filter(|(_, h)| !h.is_finished()).collect()
    };

    if pending.is_empty() {
        tracing::info!(agent = %agent_name, "cron shutdown: no running jobs");
    } else {
        let names: Vec<&str> = pending.iter().map(|(n, _)| n.as_str()).collect();
        tracing::info!(
            agent = %agent_name,
            count = pending.len(),
            jobs = ?names,
            "cron shutdown: waiting for running job(s) (timeout {}s)",
            SHUTDOWN_JOB_TIMEOUT.as_secs()
        );

        for (name, handle) in pending {
            match tokio::time::timeout(SHUTDOWN_JOB_TIMEOUT, handle).await {
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
                        "cron shutdown: job timed out, aborting"
                    );
                    // handle is dropped here → task continues as orphan
                    // (abort requires owning the handle, which timeout consumed)
                }
            }
        }
    }

    tracing::info!(agent = %agent_name, "cron shutdown complete");
}

/// Delete a one-shot spec after it has fired. Opens a fresh DB connection
/// (callers are inside `tokio::spawn` and cannot share the reconciler's connection).
fn delete_one_shot_spec(agent_dir: &std::path::Path, job_name: &str) {
    let conn = match rightclaw::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(job = %job_name, "failed to open DB for post-fire delete: {e:#}");
            return;
        }
    };
    if let Err(e) = rightclaw::cron_spec::delete_spec(&conn, job_name, agent_dir) {
        tracing::error!(job = %job_name, "failed to delete one-shot spec after fire: {e}");
    } else {
        tracing::info!(job = %job_name, "one-shot spec auto-deleted after fire");
    }
}

fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    conn: &rusqlite::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    ssh_config_path: &Option<std::path::PathBuf>,
    internal_client: &Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    resolved_sandbox: &Option<String>,
    upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>,
) {
    // Clean up finished triggered handles
    triggered_handles.retain(|h| !h.is_finished());
    let new_specs = match rightclaw::cron_spec::load_specs_from_db(conn) {
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
        .filter(|(_, spec)| matches!(&spec.schedule_kind, rightclaw::cron_spec::ScheduleKind::RunAt(dt) if *dt <= now))
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();

    for (name, spec) in overdue_run_at {
        let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
        if is_lock_fresh(agent_dir, &name, lock_ttl) {
            tracing::info!(job = %name, "run_at overdue but locked — skipping until next tick");
            continue;
        }

        tracing::info!(job = %name, "firing overdue run_at job");
        let jn = name.clone();
        let sp = spec.clone();
        let ad = agent_dir.to_path_buf();
        let an = agent_name.to_string();
        let md = model.clone();
        let sc = ssh_config_path.clone();
        let ic = Arc::clone(internal_client);
        let rs = resolved_sandbox.clone();
        let ul = Arc::clone(upgrade_lock);
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
            )
            .await;
            delete_one_shot_spec(&ad, &jn);
        });
        if let Ok(mut guard) = execute_handles.lock() {
            guard.push((name, handle));
        } else {
            triggered_handles.push(handle);
        }
    }

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
        // Skip RunAt specs — they are handled above via reconcile tick, not run_job_loop
        if matches!(
            spec.schedule_kind,
            rightclaw::cron_spec::ScheduleKind::RunAt(_)
        ) {
            continue;
        }
        if handles.contains_key(name) {
            continue; // unchanged, already running
        }
        let job_name = name.clone();
        let job_spec = spec.clone();
        let job_agent_dir = agent_dir.to_path_buf();
        let job_agent_name = agent_name.to_string();
        let job_model = model.clone();
        let job_ssh_config = ssh_config_path.clone();
        let job_execute_handles = Arc::clone(execute_handles);
        let job_internal_client = Arc::clone(internal_client);
        let job_sandbox = resolved_sandbox.clone();
        let job_upgrade_lock = Arc::clone(upgrade_lock);

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
            )
            .await;
        });
        handles.insert(name.clone(), (spec.clone(), handle));
        let sched_display = spec.schedule_kind.cron_schedule().unwrap_or("<run_at>");
        tracing::info!(job = %name, schedule = %sched_display, "cron job scheduled");
    }

    // Check for triggered jobs (manual trigger via cron_trigger MCP tool)
    for (name, spec) in &new_specs {
        if spec.triggered_at.is_some() {
            // Clear trigger immediately to prevent re-firing on next tick
            if let Err(e) = rightclaw::cron_spec::clear_triggered_at(conn, name) {
                tracing::error!(job = %name, "failed to clear triggered_at: {e}");
                continue;
            }

            // Check lock — if locked, skip (trigger lost, same as schedule miss while locked)
            let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
            if is_lock_fresh(agent_dir, name, lock_ttl) {
                tracing::info!(job = %name, "triggered but locked — skipping");
                continue;
            }

            let jn = name.clone();
            let sp = spec.clone();
            let ad = agent_dir.to_path_buf();
            let an = agent_name.to_string();
            let md = model.clone();
            let sc = ssh_config_path.clone();
            let ic = Arc::clone(internal_client);
            let rs = resolved_sandbox.clone();
            let ul = Arc::clone(upgrade_lock);
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
                )
                .await;
            });
            // Register for shutdown tracking
            if let Ok(mut guard) = execute_handles.lock() {
                guard.push((trigger_name, handle));
            } else {
                triggered_handles.push(handle);
            }
        }
    }
}

/// Per-job loop: sleep until next scheduled time, then execute. (CRON-03, D-03)
///
/// Execute handles are pushed to `execute_handles` so shutdown can await them.
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: ExecuteHandles,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
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

        if let Ok(Some(warning)) = rightclaw::cron_spec::validate_schedule(cron_expr) {
            tracing::warn!(job = %job_name, "{warning}");
        }

        // Spawn execution so the loop continues counting ticks while the job runs.
        // The lock in execute_job prevents concurrent executions of the same job.
        let jn = job_name.clone();
        let sp = spec.clone();
        let ad = agent_dir.clone();
        let an = agent_name.clone();
        let md = model.clone();
        let sc = ssh_config_path.clone();
        let ic = Arc::clone(&internal_client);
        let rs = resolved_sandbox.clone();
        let ul = Arc::clone(&upgrade_lock);
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
            )
            .await;
        });
        if spec.schedule_kind.is_one_shot() {
            // Wait for execution, then delete and exit loop
            if let Err(e) = handle.await {
                tracing::error!(job = %job_name, "one-shot job panicked: {e}");
            }
            delete_one_shot_spec(&agent_dir, &job_name);
            break;
        }
        // Register for shutdown tracking (only for recurring jobs that continue the loop)
        if let Ok(mut guard) = execute_handles.lock() {
            // Clean up finished handles to prevent unbounded growth
            guard.retain(|(_, h)| !h.is_finished());
            guard.push((job_name.clone(), handle));
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use crate::reflection::FailureKind;

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

    // -- CronReplyOutput parser tests (stream-json NDJSON format) --

    #[test]
    fn parse_cron_output_full_notify() {
        let lines = vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#.to_string(),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":{"content":"BTC broke 100k","attachments":null},"summary":"Checked 5 pairs"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.summary, "Checked 5 pairs");
        let notify = out.notify.unwrap();
        assert_eq!(notify.content, "BTC broke 100k");
        assert!(notify.attachments.is_none());
    }

    #[test]
    fn parse_cron_output_silent_null_notify() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":null,"summary":"Nothing interesting"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert!(out.notify.is_none());
        assert_eq!(out.summary, "Nothing interesting");
    }

    #[test]
    fn parse_cron_output_silent_with_reason() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":null,"summary":"Nothing interesting","no_notify_reason":"No changes since last run"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert!(out.notify.is_none());
        assert_eq!(out.summary, "Nothing interesting");
        assert_eq!(
            out.no_notify_reason.as_deref(),
            Some("No changes since last run")
        );
    }

    #[test]
    fn parse_cron_output_notify_present_no_reason() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":{"content":"BTC broke 100k"},"summary":"Checked pairs","no_notify_reason":null}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert!(out.notify.is_some());
        assert!(out.no_notify_reason.is_none());
    }

    #[test]
    fn parse_cron_output_with_attachments() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":{"content":"Chart","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png"}]},"summary":"Generated chart"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        let notify = out.notify.unwrap();
        assert_eq!(notify.attachments.as_ref().unwrap().len(), 1);
        assert_eq!(
            notify.attachments.unwrap()[0].path,
            "/sandbox/outbox/chart.png"
        );
    }

    #[test]
    fn parse_cron_output_structured_output_preferred() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ignored","structured_output":{"notify":null,"summary":"from structured"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.summary, "from structured");
    }

    #[test]
    fn parse_cron_output_falls_back_to_result() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":{"notify":null,"summary":"from result field"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        assert_eq!(out.summary, "from result field");
    }

    #[test]
    fn parse_cron_output_no_result_line_returns_err() {
        let lines = vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#.to_string(),
            "not json".to_string(),
        ];
        let result = parse_cron_output(&lines);
        assert!(result.is_err());
    }

    #[test]
    fn parse_cron_output_empty_returns_err() {
        let result = parse_cron_output(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_triggered_at_loaded_from_db() {
        let dir = tempdir().unwrap();
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();

        rightclaw::cron_spec::create_spec(
            &conn,
            "trig-test",
            "*/5 * * * *",
            "test prompt",
            None,
            None,
        )
        .unwrap();
        rightclaw::cron_spec::trigger_spec(&conn, "trig-test").unwrap();

        let specs = rightclaw::cron_spec::load_specs_from_db(&conn).unwrap();
        assert!(
            specs["trig-test"].triggered_at.is_some(),
            "triggered_at should be loaded"
        );
    }

    #[test]
    fn test_clear_triggered_at_works() {
        let dir = tempdir().unwrap();
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();

        rightclaw::cron_spec::create_spec(&conn, "clr-test", "*/5 * * * *", "test", None, None)
            .unwrap();
        rightclaw::cron_spec::trigger_spec(&conn, "clr-test").unwrap();
        rightclaw::cron_spec::clear_triggered_at(&conn, "clr-test").unwrap();

        let specs = rightclaw::cron_spec::load_specs_from_db(&conn).unwrap();
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
        let conn = rightclaw::memory::open_connection(&agent_dir, true).unwrap();
        rightclaw::cron_spec::create_spec(
            &conn,
            "slow-job",
            "0 0 1 1 *", // Jan 1st at midnight — won't fire during test
            "echo test",
            None,
            None,
        )
        .unwrap();
        drop(conn);

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let ic = Arc::new(rightclaw::mcp::internal_client::InternalClient::new(
            "/nonexistent.sock",
        ));
        let cron_handle = tokio::spawn(run_cron_task(
            agent_dir,
            "test-agent".to_string(),
            None,
            None,
            ic,
            shutdown_clone,
            None,
            Arc::new(tokio::sync::RwLock::new(())),
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
}
