use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension as _;

use crate::telegram::handler::IdleTimestamp;

/// A pending cron result ready for delivery.
#[derive(Debug)]
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
}

/// Query the oldest undelivered cron result with a non-null notify_json.
pub fn fetch_pending(
    conn: &rusqlite::Connection,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs \
         WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at ASC LIMIT 1",
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mark a cron run as delivered.
pub fn mark_delivered(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<(), rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 WHERE id = ?2",
        rusqlite::params![now, run_id],
    )?;
    Ok(())
}

/// Deduplicate: for a given job, find the latest undelivered result and mark all
/// older undelivered results as delivered. Returns (latest_result, skipped_count).
pub fn deduplicate_job(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<(PendingCronResult, u32)>, rusqlite::Error> {
    let latest = conn
        .query_row(
            "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs \
             WHERE job_name = ?1 AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
             ORDER BY finished_at DESC LIMIT 1",
            rusqlite::params![job_name],
            |row| {
                Ok(PendingCronResult {
                    id: row.get(0)?,
                    job_name: row.get(1)?,
                    notify_json: row.get(2)?,
                    summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                })
            },
        )
        .optional()?;

    let Some(latest) = latest else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 \
         WHERE job_name = ?2 AND id != ?3 \
         AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL",
        rusqlite::params![now, job_name, latest.id],
    )?;

    Ok(Some((latest, count as u32)))
}

/// Escape a string for use inside YAML double-quoted scalars.
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Format a pending cron result as YAML for the main CC session.
pub fn format_cron_yaml(pending: &PendingCronResult, skipped: u32) -> String {
    let total = skipped + 1;
    let mut yaml = String::new();
    yaml.push_str("cron_result:\n");
    yaml.push_str(&format!("  job: \"{}\"\n", yaml_escape(&pending.job_name)));
    yaml.push_str(&format!("  runs_total: {total}\n"));
    if skipped > 0 {
        yaml.push_str(&format!("  skipped_runs: {skipped}\n"));
    }

    if let Ok(notify) = serde_json::from_str::<crate::cron::CronNotify>(&pending.notify_json) {
        yaml.push_str("  result:\n");
        yaml.push_str("    notify:\n");
        yaml.push_str(&format!(
            "      content: \"{}\"\n",
            yaml_escape(&notify.content)
        ));
        if let Some(ref atts) = notify.attachments
            && !atts.is_empty()
        {
            yaml.push_str("      attachments:\n");
            for att in atts {
                let kind_str = serde_json::to_value(att.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "document".to_string());
                yaml.push_str(&format!(
                    "        - type: \"{}\"\n",
                    yaml_escape(&kind_str)
                ));
                yaml.push_str(&format!(
                    "          path: \"{}\"\n",
                    yaml_escape(&att.path)
                ));
                if let Some(ref caption) = att.caption {
                    yaml.push_str(&format!(
                        "          caption: \"{}\"\n",
                        yaml_escape(caption)
                    ));
                }
            }
        }
        yaml.push_str(&format!(
            "    summary: \"{}\"\n",
            yaml_escape(&pending.summary)
        ));
    }

    yaml
}

const IDLE_THRESHOLD_SECS: i64 = 180; // 3 minutes — within CC's 5-min prompt cache TTL
const POLL_INTERVAL_SECS: u64 = 30; // Check every 30s

/// Main delivery loop. Runs as a tokio task.
#[allow(clippy::too_many_arguments)]
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tracing::info!(agent = %agent_name, "cron delivery loop started");

    let conn = match rightclaw::memory::open_connection(&agent_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("cron delivery: DB open failed: {e:#}");
            return;
        }
    };

    // Track run IDs that were successfully sent to Telegram but failed to be marked
    // as delivered in the DB. Prevents duplicate sends on subsequent delivery ticks.
    let mut delivered_in_memory: HashSet<String> = HashSet::new();

    // Track delivery attempt counts per run_id. After MAX_DELIVERY_ATTEMPTS failures,
    // mark as delivered to avoid infinite retry loops.
    const MAX_DELIVERY_ATTEMPTS: u32 = 3;
    let mut attempt_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    loop {
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)) => {}
            () = shutdown.cancelled() => {
                tracing::info!("cron delivery loop shutting down");
                return;
            }
        }

        let pending = match fetch_pending(&conn) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("cron delivery: fetch_pending failed: {e:#}");
                continue;
            }
        };

        let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
        let now = chrono::Utc::now().timestamp();
        let idle_for = now - last;
        if idle_for < IDLE_THRESHOLD_SECS {
            let wait = IDLE_THRESHOLD_SECS - idle_for;
            tracing::info!(
                job = %pending.job_name,
                run_id = %pending.id,
                idle_secs = idle_for,
                wait_secs = wait,
                "cron delivery: result pending, waiting for chat idle ({IDLE_THRESHOLD_SECS}s)"
            );
            continue;
        }

        let (to_deliver, skipped) = match deduplicate_job(&conn, &pending.job_name) {
            Ok(Some((result, s))) => (result, s),
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("cron delivery: deduplicate failed: {e:#}");
                continue;
            }
        };

        if delivered_in_memory.contains(&to_deliver.id) {
            tracing::debug!(run_id = %to_deliver.id, "skipping already-delivered run (in-memory dedup)");
            continue;
        }

        let yaml = format_cron_yaml(&to_deliver, skipped);
        tracing::info!(
            job = %to_deliver.job_name,
            run_id = %to_deliver.id,
            skipped,
            "delivering cron result through main session"
        );

        let session_id = if notify_chat_ids.is_empty() {
            None
        } else {
            let chat_id = notify_chat_ids[0];
            match crate::telegram::session::get_active_session(&conn, chat_id, 0) {
                Ok(s) => s.map(|s| s.root_session_id),
                Err(e) => {
                    tracing::error!("cron delivery: session lookup failed: {e:#}");
                    None
                }
            }
        };

        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            &notify_chat_ids,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
        )
        .await
        {
            Ok(()) => {
                if let Err(e) = mark_delivered(&conn, &to_deliver.id) {
                    tracing::error!(run_id = %to_deliver.id, "mark_delivered failed: {e:#}");
                    delivered_in_memory.insert(to_deliver.id.clone());
                }
                let outbox_dir = agent_dir.join("outbox").join("cron").join(&to_deliver.id);
                if outbox_dir.exists()
                    && let Err(e) = std::fs::remove_dir_all(&outbox_dir)
                {
                    tracing::warn!(run_id = %to_deliver.id, "outbox cleanup failed: {e:#}");
                }
                idle_ts
                    .0
                    .store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => {
                let attempts = attempt_counts
                    .entry(to_deliver.id.clone())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                tracing::error!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    attempt = *attempts,
                    max = MAX_DELIVERY_ATTEMPTS,
                    "cron delivery failed: {e:#}"
                );
                if *attempts >= MAX_DELIVERY_ATTEMPTS {
                    tracing::warn!(
                        job = %to_deliver.job_name,
                        run_id = %to_deliver.id,
                        "giving up after {MAX_DELIVERY_ATTEMPTS} attempts, marking as delivered"
                    );
                    if let Err(db_err) = mark_delivered(&conn, &to_deliver.id) {
                        tracing::error!(run_id = %to_deliver.id, "mark_delivered failed: {db_err:#}");
                        delivered_in_memory.insert(to_deliver.id.clone());
                    }
                    attempt_counts.remove(&to_deliver.id);
                }
            }
        }
    }
}

/// Invoke the main CC session with cron result YAML and send the reply to Telegram.
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    notify_chat_ids: &[i64],
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
) -> Result<(), String> {
    use std::process::Stdio;

    if notify_chat_ids.is_empty() {
        return Err("no notify_chat_ids configured".into());
    }

    // Delivery always uses Haiku — cheap relay task.
    const DELIVERY_MODEL: &str = "claude-haiku-4-5-20251001";

    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
        "--model".into(),
        DELIVERY_MODEL.into(),
    ];
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    if let Some(ref sid) = session_id {
        claude_args.push("--resume".into());
        claude_args.push(sid.clone());
    }

    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    if let Ok(schema) = std::fs::read_to_string(&reply_schema_path) {
        claude_args.push("--json-schema".into());
        claude_args.push(schema);
    }

    // Derive sandbox_mode and home_dir from ssh_config_path.
    let (sandbox_mode, home_dir) = if ssh_config_path.is_some() {
        (rightclaw::agent::types::SandboxMode::Openshell, "/sandbox".to_owned())
    } else {
        (rightclaw::agent::types::SandboxMode::None, agent_dir.to_string_lossy().into_owned())
    };
    let base_prompt = rightclaw::codegen::generate_system_prompt(agent_name, &sandbox_mode, &home_dir);

    // Fetch MCP instructions from aggregator (non-fatal).
    let mcp_instructions: Option<String> = match internal_client.mcp_instructions(agent_name).await {
        Ok(resp) => {
            if resp.instructions.trim().len() > rightclaw::codegen::mcp_instructions::MCP_INSTRUCTIONS_HEADER.trim().len() {
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

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            "/sandbox",
            "/tmp/rightclaw-system-prompt.md",
            "/sandbox",
            &claude_args,
            mcp_instructions.as_deref(),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script = format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
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
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            mcp_instructions.as_deref(),
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
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e:#}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(yaml_input.as_bytes())
            .await
            .map_err(|e| format!("stdin write: {e:#}"))?;
    }

    const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
    let output = tokio::time::timeout(DELIVERY_TIMEOUT, child.wait_with_output())
        .await
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
    let (reply, _) =
        crate::telegram::worker::parse_reply_output(&raw).map_err(|e| format!("reply parse: {e}"))?;

    if let Some(ref content) = reply.content {
        use teloxide::prelude::Requester as _;
        for &cid in notify_chat_ids {
            if let Err(e) = bot.send_message(teloxide::types::ChatId(cid), content).await {
                tracing::error!(chat_id = cid, "cron delivery: Telegram send failed: {e:#}");
            }
        }
    }

    if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
    {
        for &cid in notify_chat_ids {
            if let Err(e) = crate::telegram::attachments::send_attachments(
                atts,
                bot,
                teloxide::types::ChatId(cid),
                0,
                agent_dir,
                ssh_config_path,
                agent_name,
            )
            .await
            {
                tracing::error!(chat_id = cid, "cron delivery: attachment send failed: {e:#}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rightclaw::memory::open_connection(dir.path()).unwrap();
        (dir, conn)
    }

    #[test]
    fn fetch_pending_empty_db() {
        let (_dir, conn) = setup_db();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_returns_oldest() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"first\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"second\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "a", "should return oldest first");
    }

    #[test]
    fn fetch_pending_skips_null_notify() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'silent')",
            [],
        )
        .unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_skips_delivered() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivered_at) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"done\"}', '2026-01-01T00:10:00Z')",
            [],
        ).unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn deduplicate_keeps_latest_marks_older() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"old\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"new\"}')",
            [],
        ).unwrap();
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        let delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM cron_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(delivered.is_some());
        let not_delivered: Option<String> = conn
            .query_row(
                "SELECT delivered_at FROM cron_runs WHERE id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(not_delivered.is_none());
    }

    #[test]
    fn deduplicate_does_not_touch_other_jobs() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job2', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"y\"}')",
            [],
        ).unwrap();
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "a");
        assert_eq!(skipped, 0);
    }

    #[test]
    fn format_cron_yaml_basic() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "health-check".into(),
            notify_json: r#"{"content":"BTC up 2%"}"#.into(),
            summary: "Checked 5 pairs".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
        };
        let yaml = format_cron_yaml(&pending, 2);
        assert!(yaml.contains("job: \"health-check\""));
        assert!(yaml.contains("runs_total: 3"));
        assert!(yaml.contains("skipped_runs: 2"));
        assert!(yaml.contains("BTC up 2%"));
        assert!(yaml.contains("Checked 5 pairs"));
    }

    #[test]
    fn format_cron_yaml_no_skipped() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "job1".into(),
            notify_json: r#"{"content":"hello"}"#.into(),
            summary: "done".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
        };
        let yaml = format_cron_yaml(&pending, 0);
        assert!(yaml.contains("runs_total: 1"));
        assert!(!yaml.contains("skipped_runs"));
    }
}
