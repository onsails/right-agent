use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension as _;
use teloxide::payloads::SendMessageSetters as _;

use crate::telegram::handler::IdleTimestamp;

/// A pending cron result ready for delivery.
#[derive(Debug)]
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}

/// Query the oldest undelivered cron result with a non-null notify_json.
pub fn fetch_pending(
    conn: &rusqlite::Connection,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT cr.id, cr.job_name, cr.notify_json, cr.summary, cr.finished_at, cr.status, \
                cs.target_chat_id, cs.target_thread_id \
         FROM cron_runs cr \
         LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name \
         WHERE cr.status IN ('success', 'failed') AND cr.notify_json IS NOT NULL AND cr.delivered_at IS NULL \
         ORDER BY cr.finished_at ASC LIMIT 1",
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
            status: row.get(5)?,
            target_chat_id: row.get(6)?,
            target_thread_id: row.get(7)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mark a cron run delivery as complete with a given status.
///
/// Single UPDATE sets both `delivery_status` and `delivered_at` atomically.
fn mark_delivery_outcome(
    conn: &rusqlite::Connection,
    run_id: &str,
    status: &str,
) -> Result<(), rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE cron_runs SET delivery_status = ?1, delivered_at = ?2 WHERE id = ?3",
        rusqlite::params![status, now, run_id],
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
            "SELECT cr.id, cr.job_name, cr.notify_json, cr.summary, cr.finished_at, cr.status, \
                    cs.target_chat_id, cs.target_thread_id \
             FROM cron_runs cr \
             LEFT JOIN cron_specs cs ON cs.job_name = cr.job_name \
             WHERE cr.job_name = ?1 AND cr.status IN ('success', 'failed') AND cr.notify_json IS NOT NULL AND cr.delivered_at IS NULL \
             ORDER BY cr.finished_at DESC LIMIT 1",
            rusqlite::params![job_name],
            |row| {
                Ok(PendingCronResult {
                    id: row.get(0)?,
                    job_name: row.get(1)?,
                    notify_json: row.get(2)?,
                    summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    status: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    target_chat_id: row.get(6)?,
                    target_thread_id: row.get(7)?,
                })
            },
        )
        .optional()?;

    let Some(latest) = latest else {
        return Ok(None);
    };

    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1, delivery_status = 'superseded' \
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

/// Instruction prefix for the delivery CC session (success path).
///
/// This is approach A: instruction in stdin. If Haiku ignores these instructions
/// (summarizes instead of relaying verbatim), migrate to approach B: add a
/// delivery-specific block to the system prompt via `build_prompt_assembly_script()`.
/// See docs/superpowers/specs/2026-04-15-cron-delivery-verbatim-relay.md.
const DELIVERY_INSTRUCTION_SUCCESS: &str = "\
You are delivering a cron job result to the user.
The `content` field below is the FINAL user-facing message — send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Ignore the attachments field — attachments are sent separately.

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

/// Format a pending cron result as YAML for the main CC session.
///
/// The output begins with a [`DELIVERY_INSTRUCTION_SUCCESS`] or
/// [`DELIVERY_INSTRUCTION_FAILURE`] prefix (depending on `pending.status`),
/// followed by the YAML payload.
pub fn format_cron_yaml(pending: &PendingCronResult, skipped: u32) -> String {
    let total = skipped + 1;
    let instruction = match pending.status.as_str() {
        "failed" => DELIVERY_INSTRUCTION_FAILURE,
        _ => DELIVERY_INSTRUCTION_SUCCESS,
    };
    let mut output = String::from(instruction);
    output.push_str("\ncron_result:\n");
    output.push_str(&format!("  job: \"{}\"\n", yaml_escape(&pending.job_name)));
    output.push_str(&format!("  runs_total: {total}\n"));
    if skipped > 0 {
        output.push_str(&format!("  skipped_runs: {skipped}\n"));
    }

    if let Ok(notify) = serde_json::from_str::<crate::cron::CronNotify>(&pending.notify_json) {
        output.push_str("  result:\n");
        output.push_str("    notify:\n");
        output.push_str(&format!(
            "      content: \"{}\"\n",
            yaml_escape(&notify.content)
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
                output.push_str(&format!("        - type: \"{}\"\n", yaml_escape(&kind_str)));
                output.push_str(&format!("          path: \"{}\"\n", yaml_escape(&att.path)));
                if let Some(ref caption) = att.caption {
                    output.push_str(&format!(
                        "          caption: \"{}\"\n",
                        yaml_escape(caption)
                    ));
                }
            }
        }
        output.push_str(&format!(
            "    summary: \"{}\"\n",
            yaml_escape(&pending.summary)
        ));
    }

    output
}

const IDLE_THRESHOLD_SECS: i64 = 180; // 3 minutes — within CC's 5-min prompt cache TTL
const POLL_INTERVAL_SECS: u64 = 30; // Check every 30s

/// Outcome of resolving a pending cron's delivery target against the live allowlist.
#[derive(Debug)]
pub(crate) enum TargetClassification {
    NoTarget,
    Denied,
    Ready {
        chat_id: i64,
        thread_id: Option<i64>,
    },
}

/// Classify a pending cron result. Pure function; no side effects.
pub(crate) fn classify_pending_target(
    pending: &PendingCronResult,
    allowlist: &rightclaw::agent::allowlist::AllowlistState,
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

/// Main delivery loop. Runs as a tokio task.
#[allow(clippy::too_many_arguments)]
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    resolved_sandbox: Option<String>,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
) {
    tracing::info!(agent = %agent_name, "cron delivery loop started");

    let conn = match rightclaw::memory::open_connection(&agent_dir, false) {
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

        let allowlist_snapshot = {
            let guard = allowlist.0.read().expect("allowlist lock poisoned");
            guard.clone()
        };

        let (target_chat_id, target_thread_id) = match classify_pending_target(&to_deliver, &allowlist_snapshot) {
            TargetClassification::NoTarget => {
                tracing::warn!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    "cron has no target_chat_id — call cron_update to set one or recreate the cron in the desired chat"
                );
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "no_target") {
                    tracing::error!(run_id = %to_deliver.id, "mark no_target failed: {e:#}");
                    delivered_in_memory.insert(to_deliver.id.clone());
                }
                continue;
            }
            TargetClassification::Denied => {
                tracing::warn!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    target_chat_id = ?to_deliver.target_chat_id,
                    "cron target chat is not in allowlist — skipping delivery"
                );
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "denied") {
                    tracing::error!(run_id = %to_deliver.id, "mark denied failed: {e:#}");
                    delivered_in_memory.insert(to_deliver.id.clone());
                }
                continue;
            }
            TargetClassification::Ready { chat_id, thread_id } => (chat_id, thread_id),
        };

        let session_id = match crate::telegram::session::get_active_session(&conn, target_chat_id, target_thread_id.unwrap_or(0)) {
            Ok(s) => s.map(|s| s.root_session_id),
            Err(e) => {
                tracing::error!("cron delivery: session lookup failed: {e:#}");
                None
            }
        };

        let yaml = format_cron_yaml(&to_deliver, skipped);
        tracing::info!(
            job = %to_deliver.job_name,
            run_id = %to_deliver.id,
            skipped,
            target_chat_id,
            ?target_thread_id,
            "delivering cron result through main session"
        );

        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            target_chat_id,
            target_thread_id,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
            resolved_sandbox.as_deref(),
            &upgrade_lock,
        )
        .await
        {
            Ok(()) => {
                // TODO(usage): delivery stream capture lives elsewhere — follow up.
                // deliver_through_session uses OutputFormat::Json (single JSON blob, not stream-json
                // NDJSON), so there is no "result" event line to feed parse_usage_full. Usage
                // tracking for delivery sessions requires either switching to stream-json output
                // or extracting cost from the non-streaming JSON response format.
                if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "delivered") {
                    tracing::error!(run_id = %to_deliver.id, "delivery DB update failed: {e:#}");
                    delivered_in_memory.insert(to_deliver.id.clone());
                }
                let outbox_dir = agent_dir.join("outbox").join("cron").join(&to_deliver.id);
                if outbox_dir.exists()
                    && let Err(e) = std::fs::remove_dir_all(&outbox_dir)
                {
                    tracing::warn!(run_id = %to_deliver.id, "outbox cleanup failed: {e:#}");
                }
                idle_ts.0.store(
                    chrono::Utc::now().timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
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
                    if let Err(e) = mark_delivery_outcome(&conn, &to_deliver.id, "failed") {
                        tracing::error!(run_id = %to_deliver.id, "delivery-failure DB update failed: {e:#}");
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
    target_chat_id: i64,
    target_thread_id: Option<i64>,
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    upgrade_lock: &tokio::sync::RwLock<()>,
) -> Result<(), String> {
    use std::process::Stdio;

    // Block while upgrade is running.
    let _upgrade_guard = upgrade_lock.read().await;

    // Delivery always uses Haiku — cheap relay task.
    const DELIVERY_MODEL: &str = "claude-haiku-4-5-20251001";

    let mcp_path = crate::telegram::invocation::mcp_config_path(ssh_config_path, agent_dir);

    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    let json_schema = std::fs::read_to_string(&reply_schema_path).unwrap_or_default();

    let invocation = crate::telegram::invocation::ClaudeInvocation {
        mcp_config_path: mcp_path,
        json_schema,
        output_format: crate::telegram::invocation::OutputFormat::Json,
        model: Some(DELIVERY_MODEL.into()),
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: session_id,
        new_session_id: None,
        disallowed_tools: vec![], // delivery is a relay — no tools to disable
        extra_args: vec![],
        prompt: None, // stdin-piped
    };

    let claude_args = invocation.into_args();

    // Derive sandbox_mode and home_dir from ssh_config_path.
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
            tracing::warn!("delivery: failed to fetch MCP instructions: {e:#}");
            None
        }
    };

    // Delivery sessions skip memory injection — same rationale as cron jobs.
    let memory_mode: Option<crate::telegram::prompt::MemoryMode> = None;

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
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
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
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
            memory_mode.as_ref(),
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

    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn failed: {e:#}"))?;

    if let Some(mut stdin) = child.stdin() {
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
    let (reply, _) = crate::telegram::worker::parse_reply_output(&raw)
        .map_err(|e| format!("reply parse: {e}"))?;

    if let Some(ref content) = reply.content {
        use teloxide::prelude::Requester as _;
        use teloxide::types::{ChatId, MessageId, ThreadId};
        let html = crate::telegram::markdown::md_to_telegram_html(content);
        let parts = crate::telegram::markdown::split_html_message(&html);
        let chat_id = ChatId(target_chat_id);
        for part in &parts {
            let mut send = bot
                .send_message(chat_id, part)
                .parse_mode(teloxide::types::ParseMode::Html);
            if let Some(t) = target_thread_id {
                send = send.message_thread_id(ThreadId(MessageId(t as i32)));
            }
            if let Err(e) = send.await {
                tracing::warn!(
                    chat_id = target_chat_id,
                    "cron delivery: HTML send failed, retrying plain: {e:#}"
                );
                let plain = crate::telegram::markdown::strip_html_tags(part);
                let mut fallback = bot.send_message(chat_id, &plain);
                if let Some(t) = target_thread_id {
                    fallback = fallback.message_thread_id(ThreadId(MessageId(t as i32)));
                }
                if let Err(e2) = fallback.await {
                    tracing::error!(
                        chat_id = target_chat_id,
                        "cron delivery: plain text fallback also failed: {e2:#}"
                    );
                }
            }
        }
    }

    if let Some(ref atts) = reply.attachments
        && !atts.is_empty()
        && let Err(e) = crate::telegram::attachments::send_attachments(
            atts,
            bot,
            teloxide::types::ChatId(target_chat_id),
            target_thread_id.unwrap_or(0),
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
        )
        .await
    {
        tracing::error!(
            chat_id = target_chat_id,
            "cron delivery: attachment send failed: {e:#}"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rightclaw::memory::open_connection(dir.path(), true).unwrap();
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
    fn deduplicate_sets_superseded_status() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivery_status) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"old\"}', 'pending')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivery_status) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"new\"}', 'pending')",
            [],
        ).unwrap();
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);

        let status: Option<String> = conn
            .query_row(
                "SELECT delivery_status FROM cron_runs WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status.as_deref(), Some("superseded"));
    }

    #[test]
    fn format_cron_yaml_basic() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "health-check".into(),
            notify_json: r#"{"content":"BTC up 2%"}"#.into(),
            summary: "Checked 5 pairs".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let output = format_cron_yaml(&pending, 2);
        // Instruction prefix assertions
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("VERBATIM"));
        assert!(output.contains("attachments are sent separately"));
        assert!(output.contains("Here is the YAML report of the cron job:"));
        // YAML content assertions
        assert!(output.contains("job: \"health-check\""));
        assert!(output.contains("runs_total: 3"));
        assert!(output.contains("skipped_runs: 2"));
        assert!(output.contains("BTC up 2%"));
        assert!(output.contains("Checked 5 pairs"));
    }

    #[test]
    fn format_cron_yaml_no_skipped() {
        let pending = PendingCronResult {
            id: "abc".into(),
            job_name: "job1".into(),
            notify_json: r#"{"content":"hello"}"#.into(),
            summary: "done".into(),
            finished_at: "2026-01-01T00:01:00Z".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let output = format_cron_yaml(&pending, 0);
        assert!(output.starts_with("You are delivering a cron job result"));
        assert!(output.contains("runs_total: 1"));
        assert!(!output.contains("skipped_runs"));
    }

    #[test]
    fn format_cron_yaml_uses_failure_instruction_when_status_failed() {
        let pending = PendingCronResult {
            id: "r1".into(),
            job_name: "watcher".into(),
            notify_json: r#"{"content":"Partial data fetched then hit budget"}"#.into(),
            summary: "failed".into(),
            finished_at: "2026-04-21T10:00:00Z".into(),
            status: "failed".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let out = format_cron_yaml(&pending, 0);
        assert!(out.contains("did not complete successfully"));
        assert!(!out.contains("send it VERBATIM"));
    }

    #[test]
    fn format_cron_yaml_uses_success_instruction_when_status_success() {
        let pending = PendingCronResult {
            id: "r2".into(),
            job_name: "watcher".into(),
            notify_json: r#"{"content":"BTC up 2%"}"#.into(),
            summary: "ok".into(),
            finished_at: "2026-04-21T10:00:00Z".into(),
            status: "success".into(),
            target_chat_id: None,
            target_thread_id: None,
        };
        let out = format_cron_yaml(&pending, 0);
        assert!(out.contains("VERBATIM"));
    }

    #[test]
    fn fetch_pending_carries_target_fields() {
        let (_dir, conn) = setup_db();
        // Seed cron_specs with target so the JOIN finds it.
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'p', 1.0, -555, 9, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-555));
        assert_eq!(pending.target_thread_id, Some(9));
    }

    #[test]
    fn fetch_pending_returns_none_target_when_spec_has_none() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert!(pending.target_chat_id.is_none());
        assert!(pending.target_thread_id.is_none());
    }

    #[test]
    fn null_target_classifies_as_no_target() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('legacy', '*/5 * * * *', 'p', 1.0, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[]));
        assert!(matches!(outcome, TargetClassification::NoTarget), "got: {outcome:?}");
    }

    #[test]
    fn target_not_in_allowlist_classifies_as_denied() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, created_at, updated_at) \
             VALUES ('agenda', '*/5 * * * *', 'p', 1.0, -777, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'agenda', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[100], &[-200]));
        assert!(matches!(outcome, TargetClassification::Denied), "got: {outcome:?}");
    }

    #[test]
    fn target_in_allowlist_classifies_as_ready() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('agenda', '*/5 * * * *', 'p', 1.0, -200, 5, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'agenda', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        let outcome = classify_pending_target(&pending, &fake_allowlist(&[], &[-200]));
        assert!(
            matches!(outcome, TargetClassification::Ready { chat_id: -200, thread_id: Some(5) }),
            "got: {outcome:?}"
        );
    }

    fn fake_allowlist(users: &[i64], groups: &[i64]) -> rightclaw::agent::allowlist::AllowlistState {
        use rightclaw::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistState};
        let now = chrono::Utc::now();
        let mut state = AllowlistState::default();
        for &id in users {
            state.add_user(AllowedUser { id, label: None, added_by: None, added_at: now });
        }
        for &id in groups {
            state.add_group(AllowedGroup { id, label: None, opened_by: None, opened_at: now });
        }
        state
    }

    #[test]
    fn deduplicate_job_carries_target_fields() {
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'p', 1.0, -100, NULL, ?1, ?1)",
            [&now],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"y\"}')",
            [],
        ).unwrap();
        let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest.id, "b");
        assert_eq!(skipped, 1);
        assert_eq!(latest.target_chat_id, Some(-100));
    }
}
