use std::collections::HashMap;
use std::io::Write as _;
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
) {
    use std::process::Stdio;

    // Lock check (CRON-04)
    let lock_ttl = spec.lock_ttl.as_deref().unwrap_or("30m");
    if is_lock_fresh(agent_dir, job_name, lock_ttl) {
        tracing::info!(job = %job_name, "skipping — previous run still active (lock fresh)");
        return;
    }

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
    let log_dir = agent_dir.join("crons").join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::error!(job = %job_name, "failed to create log dir: {e:#}");
        std::fs::remove_file(&lock_path).ok();
        return;
    }
    let log_path = log_dir.join(format!("{job_name}-{run_id}.txt"));
    let log_path_str = log_path.display().to_string();

    // DB insert: status='running' (D-04)
    // Open connection per-job — rusqlite::Connection is !Send
    let conn = match rightclaw::memory::open_connection(agent_dir) {
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

    // Build claude CLI arguments (first element is "claude" for SSH mode).
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
        "--agent".into(),
        agent_name.into(),
    ];
    if let Some(model) = model {
        claude_args.push("--model".into());
        claude_args.push(model.into());
    }
    claude_args.push("--max-budget-usd".into());
    claude_args.push(format!("{:.2}", spec.max_budget_usd));
    claude_args.push("--verbose".into());
    claude_args.push("--output-format".into());
    claude_args.push("stream-json".into());
    claude_args.push("--json-schema".into());
    claude_args.push(rightclaw::codegen::CRON_SCHEMA_JSON.into());
    claude_args.push("--".into());
    claude_args.push(spec.prompt.clone());

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        // Sandbox mode: route through SSH like deliver_through_session does.
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        let escaped_args: Vec<String> = claude_args
            .iter()
            .map(|a| shlex::try_quote(a).expect("valid UTF-8").into_owned())
            .collect();
        let claude_cmd = escaped_args.join(" ");
        let mut script = String::new();
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
        }
        script.push_str(&format!("cd /sandbox && {claude_cmd}"));

        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(script);
        c
    } else {
        // Direct exec (no sandbox).
        let cc_bin = match which::which("claude").or_else(|_| which::which("claude-bun")) {
            Ok(b) => b,
            Err(_) => {
                tracing::error!(job = %job_name, "claude binary not found in PATH");
                update_run_record(&conn, &run_id, None, "failed");
                std::fs::remove_file(&lock_path).ok();
                return;
            }
        };
        let mut c = tokio::process::Command::new(&cc_bin);
        // Skip "claude" (first element) — it's the binary name for SSH mode.
        for arg in &claude_args[1..] {
            c.arg(arg);
        }
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
    cmd.kill_on_drop(true);

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let mut child = match cmd.spawn() {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(c) => c,
    };

    // Stream stdout line-by-line into NDJSON log and collected_lines vec.
    let stdout = child.stdout.take().expect("stdout piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let stream_log_dir = agent_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(agent_dir)
        .join("logs")
        .join("streams");
    if let Err(e) = std::fs::create_dir_all(&stream_log_dir) {
        tracing::warn!(job = %job_name, "failed to create stream log dir: {e:#}");
    }
    let stream_log_path = stream_log_dir.join(format!("{job_name}-{run_id}.ndjson"));
    let mut stream_log = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stream_log_path)
    {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(job = %job_name, "failed to open stream log: {e:#}");
            None
        }
    };

    let mut collected_lines: Vec<String> = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(ref mut log) = stream_log {
            let _ = writeln!(log, "{line}");
        }
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
    let stderr_bytes = if let Some(mut stderr) = child.stderr.take() {
        let mut buf = Vec::new();
        if let Err(e) = stderr.read_to_end(&mut buf).await {
            tracing::warn!(job = %job_name, "failed to read stderr: {e:#}");
        }
        buf
    } else {
        Vec::new()
    };
    let stderr_str = String::from_utf8_lossy(&stderr_bytes);

    // Write text log file (D-04)
    let mut log_content = String::new();
    log_content.push_str(&format!("=== stream log: {} ===\n", stream_log_path.display()));
    if !stderr_str.is_empty() {
        log_content.push_str("=== stderr ===\n");
        log_content.push_str(&stderr_str);
    }
    if let Err(e) = std::fs::write(&log_path, &log_content) {
        tracing::error!(job = %job_name, "failed to write log file: {e:#}");
        // Continue — still update DB even if log write fails
    }

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
                            let sandbox = rightclaw::openshell::sandbox_name(agent_name);
                            for att in atts {
                                let dest = outbox_dir.join(attachment_filename(&att.path));
                                if let Err(e) = rightclaw::openshell::download_file(
                                    &sandbox, &att.path, &dest,
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

                        // Rewrite paths to host-side
                        let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                        let host_notify = CronNotify {
                            content: notify.content.clone(),
                            attachments: Some(
                                atts.iter()
                                    .map(|att| {
                                        crate::telegram::attachments::OutboundAttachment {
                                            kind: att.kind,
                                            path: outbox_dir
                                                .join(attachment_filename(&att.path))
                                                .to_string_lossy()
                                                .into_owned(),
                                            filename: att.filename.clone(),
                                            caption: att.caption.clone(),
                                        }
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

                if let Err(e) = conn.execute(
                    "UPDATE cron_runs SET summary = ?1, notify_json = ?2 WHERE id = ?3",
                    rusqlite::params![cron_output.summary, notify_json, run_id],
                ) {
                    tracing::error!(job = %job_name, "failed to persist cron output to DB: {e:#}");
                }

                tracing::info!(
                    job = %job_name,
                    has_notify = cron_output.notify.is_some(),
                    "cron output persisted to DB"
                );
            }
            Err(reason) => {
                tracing::warn!(job = %job_name, reason, "failed to parse cron output");
            }
        }
    } else {
        // Build failure notification (Fix 3)
        let exit_str = exit_code.map_or("unknown".to_string(), |c| c.to_string());
        let error_detail = collected_lines
            .iter()
            .rev()
            .find_map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
                    .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(String::from))
            })
            .unwrap_or_else(|| stderr_str.to_string());
        let content = format!("Cron job `{job_name}` failed (exit code {exit_str}):\n{error_detail}");
        let notify = CronNotify {
            content,
            attachments: None,
        };
        match serde_json::to_string(&notify) {
            Ok(json) => {
                if let Err(e) = conn.execute(
                    "UPDATE cron_runs SET summary = ?1, notify_json = ?2 WHERE id = ?3",
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
        if !so.is_null() { so } else { envelope.get("result").unwrap_or(so) }
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
    shutdown: CancellationToken,
) {
    tracing::info!(agent = %agent_name, "cron task started");

    let conn = match rightclaw::memory::open_connection(&agent_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(agent = %agent_name, "cron task: DB open failed: {e:#}");
            return;
        }
    };

    let execute_handles: ExecuteHandles = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut triggered_handles: Vec<JoinHandle<()>> = Vec::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(&mut handles, &mut triggered_handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path, &execute_handles);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &mut triggered_handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path, &execute_handles);
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
        let mut guard = execute_handles.lock().expect("execute_handles mutex poisoned");
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

fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    conn: &rusqlite::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    ssh_config_path: &Option<std::path::PathBuf>,
    execute_handles: &ExecuteHandles,
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

        let handle = tokio::spawn(async move {
            run_job_loop(job_name, job_spec, job_agent_dir, job_agent_name, job_model, job_ssh_config, job_execute_handles)
                .await;
        });
        handles.insert(name.clone(), (spec.clone(), handle));
        tracing::info!(job = %name, schedule = %spec.schedule, "cron job scheduled");
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
            tracing::info!(job = %name, "executing triggered job");
            let trigger_name = name.clone();
            let handle = tokio::spawn(async move {
                execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref()).await;
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
    execute_handles: ExecuteHandles,
) {
    use cron::Schedule;
    use std::str::FromStr;

    let seven_field = to_7field(&spec.schedule);
    let schedule = match Schedule::from_str(&seven_field) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(job = %job_name, "invalid cron schedule '{}': {e:#}", spec.schedule);
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

        // Spawn execution so the loop continues counting ticks while the job runs.
        // The lock in execute_job prevents concurrent executions of the same job.
        let jn = job_name.clone();
        let sp = spec.clone();
        let ad = agent_dir.clone();
        let an = agent_name.clone();
        let md = model.clone();
        let sc = ssh_config_path.clone();
        let handle = tokio::spawn(async move {
            execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref()).await;
        });
        // Register for shutdown tracking. Lock is brief — just a Vec push.
        if let Ok(mut guard) = execute_handles.lock() {
            // Clean up finished handles to prevent unbounded growth
            guard.retain(|(_, h)| !h.is_finished());
            guard.push((job_name.clone(), handle));
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
    fn parse_cron_output_with_attachments() {
        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":{"content":"Chart","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png"}]},"summary":"Generated chart"}}"#.to_string(),
        ];
        let out = parse_cron_output(&lines).unwrap();
        let notify = out.notify.unwrap();
        assert_eq!(notify.attachments.as_ref().unwrap().len(), 1);
        assert_eq!(notify.attachments.unwrap()[0].path, "/sandbox/outbox/chart.png");
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
        let conn = rightclaw::memory::open_connection(dir.path()).unwrap();

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
        let conn = rightclaw::memory::open_connection(dir.path()).unwrap();

        rightclaw::cron_spec::create_spec(
            &conn,
            "clr-test",
            "*/5 * * * *",
            "test",
            None,
            None,
        )
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
        let conn = rightclaw::memory::open_connection(&agent_dir).unwrap();
        rightclaw::cron_spec::create_spec(
            &conn,
            "slow-job",
            "0 0 1 1 *",  // Jan 1st at midnight — won't fire during test
            "echo test",
            None,
            None,
        )
        .unwrap();
        drop(conn);

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        let cron_handle = tokio::spawn(run_cron_task(
            agent_dir,
            "test-agent".to_string(),
            None,
            None,
            shutdown_clone,
        ));

        // Give cron engine time to reconcile and spawn the job loop
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Signal shutdown
        shutdown.cancel();

        // Must complete within 2 seconds — if it hangs, the bug is present
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            cron_handle,
        )
        .await;

        assert!(
            result.is_ok(),
            "run_cron_task must exit within 2s of shutdown — \
             job loop handles are likely blocking (not aborted on shutdown)"
        );
    }
}
