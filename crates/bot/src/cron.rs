use std::collections::HashMap;
use teloxide::prelude::Requester as _;
use tokio::task::JoinHandle;

use crate::telegram::{worker::parse_reply_output, BotType};

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
/// If CC exits successfully and produces a reply tool call, the content is sent to all notify_chat_ids.
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    bot: &BotType,
    notify_chat_ids: &[i64],
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

    // Read reply schema — same schema as Telegram worker uses (D-01 / CRON-reply).
    // If missing, cron still runs but CC output won't be parsed or sent to Telegram.
    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    let reply_schema = std::fs::read_to_string(&reply_schema_path).ok();
    if reply_schema.is_none() {
        tracing::warn!(
            job = %job_name,
            path = %reply_schema_path.display(),
            "reply-schema.json not found — cron output will NOT be delivered to Telegram"
        );
    }

    // Build command (D-01: --agent <name>, --output-format json for structured reply parsing)
    let mut cmd = tokio::process::Command::new(&cc_bin);
    cmd.arg("-p");
    cmd.arg("--dangerously-skip-permissions");
    cmd.arg("--agent").arg(agent_name);
    if let Some(max_turns) = spec.max_turns {
        cmd.arg("--max-turns").arg(max_turns.to_string());
    }
    // --output-format json is always required: it enables the structured reply parsing path
    // below. Even when reply-schema.json is absent, JSON mode is needed so parse_reply_output
    // can attempt to extract a plain-string result field.
    cmd.arg("--output-format").arg("json");
    if let Some(ref schema) = reply_schema {
        cmd.arg("--json-schema").arg(schema);
    }
    cmd.arg("--").arg(&spec.prompt);
    cmd.env("HOME", agent_dir);
    // Use system rg instead of CC's bundled vendor binary (nix store rg lacks execute bit).
    cmd.env("USE_BUILTIN_RIPGREP", "1");
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

    // CRON-reply: parse CC structured output and send to Telegram if content is non-null.
    // Only on success — non-zero exit means no valid structured JSON to parse.
    // Silent by default: content:null → no message sent. Failures are silent too.
    if output.status.success() && reply_schema.is_some() && !notify_chat_ids.is_empty() {
        if let Some(content) = parse_cron_reply_content(&output.stdout, reply_schema.is_some()) {
            for &chat_id in notify_chat_ids {
                // best-effort: cron Telegram delivery is fire-and-forget; send failures are
                // logged but do not change job status (D-02 analogue for the delivery path)
                if let Err(e) = bot
                    .send_message(teloxide::types::ChatId(chat_id), &content)
                    .await
                {
                    tracing::error!(
                        job = %job_name,
                        chat_id,
                        "failed to send cron reply to Telegram: {e:#}"
                    );
                }
            }
        }
    }
}

/// Extract reply content from CC stdout for Telegram delivery.
///
/// Returns `Some(content)` when CC produced a non-empty reply, `None` otherwise.
/// Called only when `has_schema` is true (gating is the caller's responsibility).
/// Parses via `parse_reply_output` so both `structured_output` and plain-string `result`
/// fields are handled (CC does not always comply with `--json-schema` after MCP tool use).
pub(crate) fn parse_cron_reply_content(stdout: &[u8], has_schema: bool) -> Option<String> {
    if !has_schema {
        return None;
    }
    let raw = String::from_utf8_lossy(stdout);
    match parse_reply_output(&raw) {
        Ok((reply_output, _)) => reply_output.content,
        Err(reason) => {
            tracing::warn!(reason, "CC cron output parse failed — no Telegram notification sent");
            None
        }
    }
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
/// `bot` and `notify_chat_ids` are threaded down to `execute_job` so that CC output
/// containing a `reply` tool call is delivered to Telegram after each successful run.
///
/// Signature expected by lib.rs spawn site (CRON-01, CRON-02, CRON-06).
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    bot: BotType,
    notify_chat_ids: Vec<i64>,
) {
    tracing::info!(agent = %agent_name, "cron task started");
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(&mut handles, &agent_dir, &agent_name, &bot, &notify_chat_ids).await;

    loop {
        interval.tick().await;
        reconcile_jobs(&mut handles, &agent_dir, &agent_name, &bot, &notify_chat_ids).await;
    }
}

async fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    agent_dir: &std::path::Path,
    agent_name: &str,
    bot: &BotType,
    notify_chat_ids: &[i64],
) {
    let new_specs = load_specs(agent_dir);

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
        let job_bot = bot.clone();
        let job_chat_ids = notify_chat_ids.to_vec();

        let handle = tokio::spawn(async move {
            run_job_loop(job_name, job_spec, job_agent_dir, job_agent_name, job_bot, job_chat_ids)
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
    bot: BotType,
    notify_chat_ids: Vec<i64>,
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
        let bt = bot.clone();
        let nc = notify_chat_ids.clone();
        tokio::spawn(async move {
            execute_job(&jn, &sp, &ad, &an, &bt, &nc).await;
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

    // parse_cron_reply_content tests — cover gating logic for CRON-reply delivery

    #[test]
    fn parse_cron_reply_content_no_schema_returns_none() {
        // Even if stdout has valid CC JSON, no schema → no delivery
        let json = r#"{"result":{"content":"hello","reply_to_message_id":null,"media_paths":null}}"#;
        assert!(parse_cron_reply_content(json.as_bytes(), false).is_none());
    }

    #[test]
    fn parse_cron_reply_content_with_schema_returns_content() {
        let json = r#"{"result":{"content":"cron says hi","reply_to_message_id":null,"media_paths":null}}"#;
        let result = parse_cron_reply_content(json.as_bytes(), true);
        assert_eq!(result.as_deref(), Some("cron says hi"));
    }

    #[test]
    fn parse_cron_reply_content_null_content_returns_none() {
        // content: null → silent job, nothing sent to Telegram
        let json = r#"{"result":{"content":null,"reply_to_message_id":null,"media_paths":null}}"#;
        assert!(parse_cron_reply_content(json.as_bytes(), true).is_none());
    }

    #[test]
    fn parse_cron_reply_content_plain_string_result_wrapped() {
        // CC sometimes returns result as plain string after MCP tool use
        let json = r#"{"result":"market update: BTC up 2%"}"#;
        let result = parse_cron_reply_content(json.as_bytes(), true);
        assert_eq!(result.as_deref(), Some("market update: BTC up 2%"));
    }

    #[test]
    fn parse_cron_reply_content_unparseable_json_returns_none() {
        let result = parse_cron_reply_content(b"not json at all", true);
        assert!(result.is_none());
    }

    #[test]
    fn parse_cron_reply_content_structured_output_preferred_over_result() {
        let json = r#"{"result":"ignored","structured_output":{"content":"from structured","reply_to_message_id":null,"media_paths":null}}"#;
        let result = parse_cron_reply_content(json.as_bytes(), true);
        assert_eq!(result.as_deref(), Some("from structured"));
    }
}
