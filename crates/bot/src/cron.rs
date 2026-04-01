use std::collections::HashMap;
use tokio::task::JoinHandle;

/// Deserialized from crons/*.yaml
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>, // default "30m"
    pub max_turns: Option<u32>,
}

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

/// Scan `agent_dir/crons/*.yaml` and return a map of job_name -> CronSpec.
///
/// The job_name is the YAML file stem (e.g. "deploy-check" from "deploy-check.yaml").
/// Files that fail to parse are skipped with a `tracing::warn`.
pub fn load_specs(agent_dir: &std::path::Path) -> HashMap<String, CronSpec> {
    let crons_dir = agent_dir.join("crons");
    let mut map = HashMap::new();
    for entry in walkdir::WalkDir::new(&crons_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(job = %stem, "failed to read cron spec: {e:#}");
                continue;
            }
        };
        match serde_saphyr::from_str::<CronSpec>(&raw) {
            Ok(spec) => {
                map.insert(stem.to_string(), spec);
            }
            Err(e) => tracing::warn!(job = %stem, "failed to parse cron spec: {e:#}"),
        }
    }
    map
}

/// Execute one cron job: lock check → DB insert → subprocess → log write → DB update → lock delete.
///
/// Per D-02: subprocess failures log `tracing::error` only, do not propagate.
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
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

    // Resolve CC binary (same as worker.rs)
    let cc_bin = match which::which("claude").or_else(|_| which::which("claude-bun")) {
        Ok(b) => b,
        Err(_) => {
            tracing::error!(job = %job_name, "claude binary not found in PATH");
            update_run_record(&conn, &run_id, None, "failed");
            std::fs::remove_file(&lock_path).ok();
            return;
        }
    };

    // Build command (D-01: --agent <name>, no session, no JSON output)
    let mut cmd = tokio::process::Command::new(&cc_bin);
    cmd.arg("-p");
    cmd.arg("--agent").arg(agent_name);
    if let Some(max_turns) = spec.max_turns {
        cmd.arg("--max-turns").arg(max_turns.to_string());
    }
    cmd.arg("--").arg(&spec.prompt);
    cmd.env("HOME", agent_dir);
    cmd.current_dir(agent_dir);
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
/// Signature expected by lib.rs spawn site (CRON-01, CRON-02, CRON-06).
pub async fn run_cron_task(agent_dir: std::path::PathBuf, agent_name: String) {
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(&mut handles, &agent_dir, &agent_name).await;

    loop {
        interval.tick().await;
        reconcile_jobs(&mut handles, &agent_dir, &agent_name).await;
    }
}

async fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    agent_dir: &std::path::Path,
    agent_name: &str,
) {
    let new_specs = load_specs(agent_dir);

    // Abort handles for removed or changed jobs (CRON-06)
    let to_remove: Vec<String> = handles
        .iter()
        .filter(|(name, (old_spec, _))| {
            new_specs
                .get(*name)
                .map_or(true, |new_spec| new_spec != old_spec)
        })
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

        let handle = tokio::spawn(async move {
            run_job_loop(job_name, job_spec, job_agent_dir, job_agent_name).await;
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

        // D-03: no drift check — if we woke late, still execute once and loop
        execute_job(&job_name, &spec, &agent_dir, &agent_name).await;
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

    #[test]
    fn test_load_specs_empty_dir() {
        let dir = tempdir().unwrap();
        // crons/ dir doesn't exist → empty map
        let specs = load_specs(dir.path());
        assert!(specs.is_empty());
    }

    #[test]
    fn test_load_specs_valid_yaml() {
        let dir = tempdir().unwrap();
        let crons_dir = dir.path().join("crons");
        std::fs::create_dir_all(&crons_dir).unwrap();

        let yaml = r#"
schedule: "*/5 * * * *"
prompt: "Check system health"
lock_ttl: "1h"
max_turns: 10
"#;
        std::fs::write(crons_dir.join("health-check.yaml"), yaml).unwrap();

        let specs = load_specs(dir.path());
        assert_eq!(specs.len(), 1);
        let spec = specs.get("health-check").expect("health-check spec should exist");
        assert_eq!(spec.schedule, "*/5 * * * *");
        assert_eq!(spec.prompt, "Check system health");
        assert_eq!(spec.lock_ttl.as_deref(), Some("1h"));
        assert_eq!(spec.max_turns, Some(10));
    }
}
