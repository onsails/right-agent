use std::collections::HashMap;
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
    claude_args.push("--output-format".into());
    claude_args.push("json".into());
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
        let script = format!("cd /sandbox && {claude_cmd}");

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
        c.current_dir(agent_dir);
        c
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    tracing::info!(job = %job_name, run_id = %run_id, "executing cron job");

    let spawn_result = cmd.spawn();
    let output = match spawn_result {
        Err(e) => {
            tracing::error!(job = %job_name, "spawn failed: {e:#}");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
        Ok(child) => match child.wait_with_output().await {
            Err(e) => {
                tracing::error!(job = %job_name, "wait_with_output failed: {e:#}");
                update_run_record(&conn, &run_id, None, "failed");
                std::fs::remove_file(&lock_path).ok();
                return;
            }
            Ok(o) => o,
        },
    };

    // Write log file (D-04)
    let mut log_content = String::new();
    log_content.push_str("=== stdout ===\n");
    log_content.push_str(&String::from_utf8_lossy(&output.stdout));
    log_content.push_str("\n=== stderr ===\n");
    log_content.push_str(&String::from_utf8_lossy(&output.stderr));
    if let Err(e) = std::fs::write(&log_path, &log_content) {
        tracing::error!(job = %job_name, "failed to write log file: {e:#}");
        // Continue — still update DB even if log write fails
    }

    // Determine status (D-02)
    let exit_code = output.status.code();
    let status = if output.status.success() {
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
    if output.status.success() {
        match parse_cron_output(&output.stdout) {
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
    }
}

/// Parse CC stdout into `CronReplyOutput`.
///
/// Tries `structured_output` first, falls back to `result`.
/// Returns `Err` if neither field is present or JSON is invalid.
pub(crate) fn parse_cron_output(stdout: &[u8]) -> Result<CronReplyOutput, String> {
    let raw = String::from_utf8_lossy(stdout);

    let envelope: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("CC output is not valid JSON: {e}"))?;

    let payload = if let Some(so) = envelope.get("structured_output") {
        if !so.is_null() { so } else { envelope.get("result").unwrap_or(so) }
    } else if let Some(r) = envelope.get("result") {
        r
    } else {
        return Err("CC output has neither 'structured_output' nor 'result' field".into());
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

    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(&mut handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path);
            }
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "cron shutdown: stopping reconciler, waiting for running jobs");
                break;
            }
        }
    }

    // Wait for all running job handles (don't abort — let in-flight jobs finish)
    for (name, (_, handle)) in handles {
        tracing::info!(job = %name, "cron shutdown: waiting for job to finish");
        let _ = handle.await;
    }
    tracing::info!(agent = %agent_name, "cron shutdown complete — all jobs finished");
}

fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    conn: &rusqlite::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    ssh_config_path: &Option<std::path::PathBuf>,
) {
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

        let handle = tokio::spawn(async move {
            run_job_loop(job_name, job_spec, job_agent_dir, job_agent_name, job_model, job_ssh_config)
                .await;
        });
        handles.insert(name.clone(), (spec.clone(), handle));
        tracing::info!(job = %name, schedule = %spec.schedule, "cron job scheduled");
    }
}

/// Per-job loop: sleep until next scheduled time, then execute. (CRON-03, D-03)
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
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
        // Note: if reconcile_jobs aborts this loop handle (spec changed/removed), any
        // in-flight execute_job spawn runs to completion as an orphan — this is intentional.
        // The lock expires naturally and the next reconcile tick picks up the updated spec.
        let jn = job_name.clone();
        let sp = spec.clone();
        let ad = agent_dir.clone();
        let an = agent_name.clone();
        let md = model.clone();
        let sc = ssh_config_path.clone();
        tokio::spawn(async move {
            execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref()).await;
        });
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

    // -- CronReplyOutput parser tests --

    #[test]
    fn parse_cron_output_full_notify() {
        let json = r#"{"result":{"notify":{"content":"BTC broke 100k","attachments":null},"summary":"Checked 5 pairs"}}"#;
        let out = parse_cron_output(json.as_bytes()).unwrap();
        assert_eq!(out.summary, "Checked 5 pairs");
        let notify = out.notify.unwrap();
        assert_eq!(notify.content, "BTC broke 100k");
        assert!(notify.attachments.is_none());
    }

    #[test]
    fn parse_cron_output_silent_null_notify() {
        let json = r#"{"result":{"notify":null,"summary":"Nothing interesting"}}"#;
        let out = parse_cron_output(json.as_bytes()).unwrap();
        assert!(out.notify.is_none());
        assert_eq!(out.summary, "Nothing interesting");
    }

    #[test]
    fn parse_cron_output_with_attachments() {
        let json = r#"{"result":{"notify":{"content":"Chart","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png"}]},"summary":"Generated chart"}}"#;
        let out = parse_cron_output(json.as_bytes()).unwrap();
        let notify = out.notify.unwrap();
        assert_eq!(notify.attachments.as_ref().unwrap().len(), 1);
        assert_eq!(notify.attachments.unwrap()[0].path, "/sandbox/outbox/chart.png");
    }

    #[test]
    fn parse_cron_output_structured_output_preferred() {
        let json = r#"{"result":"ignored","structured_output":{"notify":null,"summary":"from structured"}}"#;
        let out = parse_cron_output(json.as_bytes()).unwrap();
        assert_eq!(out.summary, "from structured");
    }

    #[test]
    fn parse_cron_output_unparseable_returns_err() {
        let result = parse_cron_output(b"not json");
        assert!(result.is_err());
    }

}
