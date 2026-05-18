use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub(crate) struct BackgroundRunRequest {
    pub run_id: String,
    pub source_session_id: String,
    pub target_chat_id: i64,
    pub target_thread_id: Option<i64>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandoffStatus {
    Spawned,
    Failed(String),
}

pub(crate) fn bg_log_path(agent_dir: &Path, run_id: &str) -> PathBuf {
    agent_dir
        .join("background")
        .join("logs")
        .join(format!("{run_id}.ndjson"))
}

const HANDOFF_INIT_TIMEOUT: Duration = Duration::from_secs(30);
const POST_STDOUT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const BACKGROUND_FAILURE_NOTIFY_CONTENT: &str =
    "Background work failed before it could produce a result.";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_background_continuation(
    request: BackgroundRunRequest,
    agent_dir: PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<PathBuf>,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    session_locks: crate::telegram::SessionLocks,
    debug: Arc<std::sync::atomic::AtomicBool>,
) -> HandoffStatus {
    let log_path = bg_log_path(&agent_dir, &request.run_id);
    if let Some(parent) = log_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let reason = format!("failed to create background log dir: {e:#}");
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
        return HandoffStatus::Failed(reason);
    }

    let _upgrade_guard = upgrade_lock.read().await;

    let mcp_instructions =
        fetch_mcp_instructions(&internal_client, &agent_name, &request.run_id).await;

    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            ssh_config_path.as_deref(),
            &agent_dir,
        )),
        json_schema: Some(right_codegen::BG_CONTINUATION_SCHEMA_JSON.into()),
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model,
        max_budget_usd: None,
        max_turns: None,
        resume_session_id: Some(request.source_session_id.clone()),
        new_session_id: Some(request.run_id.clone()),
        fork_session: true,
        allowed_tools: vec![],
        disallowed_tools: crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        ),
        extra_args: vec![],
        prompt: Some(request.prompt.clone()),
        debug_flag: Some(debug),
    };
    let claude_args = invocation.into_args();

    let mut cmd = match build_background_command(
        &agent_dir,
        &agent_name,
        ssh_config_path.as_deref(),
        resolved_sandbox.as_deref(),
        &claude_args,
        mcp_instructions.as_deref(),
    ) {
        Ok(cmd) => cmd,
        Err(reason) => {
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
            return HandoffStatus::Failed(reason);
        }
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let _session_guard: tokio::sync::OwnedMutexGuard<()> = {
        let entry = session_locks
            .entry(request.source_session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        entry.lock_owned().await
    };

    tracing::info!(
        run_id = %request.run_id,
        source_session_id = %request.source_session_id,
        target_chat_id = request.target_chat_id,
        target_thread_id = ?request.target_thread_id,
        "spawning immediate background continuation"
    );
    let mut child = match right_process::ProcessGroupChild::spawn(cmd) {
        Ok(child) => child,
        Err(e) => {
            let reason = format!("spawn failed: {e:#}");
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
            return HandoffStatus::Failed(reason);
        }
    };

    let stdout = match child.stdout() {
        Some(stdout) => stdout,
        None => {
            let reason = "spawned background child without stdout".to_string();
            kill_child(child).await;
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
            return HandoffStatus::Failed(reason);
        }
    };
    let (init_tx, init_rx) = tokio::sync::oneshot::channel();
    let reader_handle = tokio::spawn(read_background_stdout(
        stdout,
        log_path.clone(),
        request.run_id.clone(),
        init_tx,
    ));

    let confirmed = match tokio::time::timeout(HANDOFF_INIT_TIMEOUT, init_rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(reason))) => Err(reason),
        Ok(Err(_)) => Err("stdout reader ended before handoff init confirmation".to_string()),
        Err(_) => Err(format!(
            "timed out after {}s waiting for background system/init",
            HANDOFF_INIT_TIMEOUT.as_secs()
        )),
    };
    if let Err(reason) = confirmed {
        kill_unconfirmed_child(child, reader_handle).await;
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
        return HandoffStatus::Failed(reason);
    }

    let started_at = chrono::Utc::now().to_rfc3339();
    let log_path_str = log_path.to_string_lossy().into_owned();
    let mark_spawned = {
        match right_db::open_connection(&agent_dir, false) {
            Ok(conn) => right_agent::async_runs::mark_background_spawned(
                &conn,
                &request.run_id,
                &started_at,
                &log_path_str,
            )
            .map_err(|e| format!("mark background spawned: {e:#}")),
            Err(e) => Err(format!("open DB to mark background spawned: {e:#}")),
        }
    };
    if let Err(reason) = mark_spawned {
        kill_unconfirmed_child(child, reader_handle).await;
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason);
        return HandoffStatus::Failed(reason);
    }

    tokio::spawn(complete_background_run(
        request,
        agent_dir,
        resolved_sandbox,
        child,
        reader_handle,
    ));
    HandoffStatus::Spawned
}

fn is_handoff_init_for_run(line: &str, run_id: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    value.get("type").and_then(|v| v.as_str()) == Some("system")
        && value.get("subtype").and_then(|v| v.as_str()) == Some("init")
        && value.get("session_id").and_then(|v| v.as_str()) == Some(run_id)
}

async fn fetch_mcp_instructions(
    internal_client: &right_mcp::internal_client::InternalClient,
    agent_name: &str,
    run_id: &str,
) -> Option<String> {
    match internal_client.mcp_instructions(agent_name).await {
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
            tracing::warn!(
                run_id,
                "failed to fetch MCP instructions for background run: {e:#}"
            );
            None
        }
    }
}

fn build_background_command(
    agent_dir: &Path,
    agent_name: &str,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
) -> Result<tokio::process::Command, String> {
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
    let memory_mode: Option<crate::cc::prompt::MemoryMode> = None;

    if let Some(ssh_config) = ssh_config_path {
        let Some(sandbox_name) = resolved_sandbox else {
            return Err("resolved sandbox missing for sandboxed background run".to_string());
        };
        let mut assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Cron,
            "/sandbox",
            "/tmp/right-system-prompt.md",
            "/sandbox",
            claude_args,
            mcp_instructions,
            memory_mode.as_ref(),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(sandbox_name);
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-F").arg(ssh_config);
        cmd.arg("-o").arg("ControlMaster=no");
        cmd.arg("-o").arg("ControlPath=none");
        cmd.arg(&ssh_host);
        cmd.arg("--");
        cmd.arg(assembly_script);
        Ok(cmd)
    } else {
        if which::which("claude").is_err() && which::which("claude-bun").is_err() {
            return Err("claude binary not found in PATH".to_string());
        }
        let claude_dir = agent_dir.join(".claude");
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| format!("failed to create .claude dir: {e:#}"))?;
        let agent_dir_str = agent_dir.to_string_lossy();
        let prompt_path = claude_dir.join("background-system-prompt.md");
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
            &base_prompt,
            crate::cc::prompt::PromptMode::Cron,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            claude_args,
            mcp_instructions,
            memory_mode.as_ref(),
        );
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(&assembly_script);
        cmd.env("HOME", agent_dir);
        cmd.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        cmd.current_dir(agent_dir);
        Ok(cmd)
    }
}

async fn read_background_stdout(
    stdout: tokio::process::ChildStdout,
    log_path: PathBuf,
    run_id: String,
    init_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
) -> Result<Vec<String>, String> {
    let mut init_tx = Some(init_tx);
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
    {
        Ok(file) => file,
        Err(e) => {
            let reason = format!("open background log {}: {e:#}", log_path.display());
            send_init_failure(&mut init_tx, &reason);
            return Err(reason);
        }
    };
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut collected = Vec::new();
    let mut saw_init = false;

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                let reason = format!("read background stdout: {e:#}");
                send_init_failure(&mut init_tx, &reason);
                return Err(reason);
            }
        };
        if let Err(e) = file.write_all(line.as_bytes()).await {
            let reason = format!("write background log {}: {e:#}", log_path.display());
            send_init_failure(&mut init_tx, &reason);
            return Err(reason);
        }
        if let Err(e) = file.write_all(b"\n").await {
            let reason = format!("write background log newline {}: {e:#}", log_path.display());
            send_init_failure(&mut init_tx, &reason);
            return Err(reason);
        }

        if !saw_init && is_handoff_init_for_run(&line, &run_id) {
            saw_init = true;
            if let Some(tx) = init_tx.take() {
                let _ = tx.send(Ok(()));
            }
        }
        collected.push(line);
    }

    if !saw_init {
        let reason = format!("background stdout ended before system/init for run {run_id}");
        send_init_failure(&mut init_tx, &reason);
    }
    Ok(collected)
}

fn send_init_failure(
    init_tx: &mut Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    reason: &str,
) {
    if let Some(tx) = init_tx.take() {
        let _ = tx.send(Err(reason.to_string()));
    }
}

async fn kill_child(mut child: right_process::ProcessGroupChild) {
    if let Err(e) = child.kill().await {
        tracing::warn!("failed to kill background child: {e:#}");
    }
}

async fn kill_unconfirmed_child(
    mut child: right_process::ProcessGroupChild,
    reader_handle: JoinHandle<Result<Vec<String>, String>>,
) {
    if let Err(e) = child.kill().await {
        tracing::warn!("failed to kill unconfirmed background child: {e:#}");
    }
    drop(child);
    if tokio::time::timeout(POST_STDOUT_WAIT_TIMEOUT, reader_handle)
        .await
        .is_err()
    {
        tracing::warn!("timed out waiting for unconfirmed background stdout reader");
    }
}

async fn complete_background_run(
    request: BackgroundRunRequest,
    agent_dir: PathBuf,
    resolved_sandbox: Option<String>,
    mut child: right_process::ProcessGroupChild,
    reader_handle: JoinHandle<Result<Vec<String>, String>>,
) {
    let lines = match reader_handle.await {
        Ok(Ok(lines)) => lines,
        Ok(Err(reason)) => {
            let _ = child.kill().await;
            persist_completion_failed_at_agent(&agent_dir, &request.run_id, None, &reason);
            return;
        }
        Err(e) => {
            let _ = child.kill().await;
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                &format!("background stdout reader task failed: {e:#}"),
            );
            return;
        }
    };

    let exit_status = match tokio::time::timeout(POST_STDOUT_WAIT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                &format!("background child wait failed: {e:#}"),
            );
            return;
        }
        Err(_) => {
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                "background child wait timed out after stdout closed",
            );
            return;
        }
    };
    let exit_code = exit_status.code();
    let stderr = drain_stderr(&mut child).await;

    if !exit_status.success() {
        let reason = if stderr.trim().is_empty() {
            format!("background subprocess failed with exit code {exit_code:?}")
        } else {
            format!("background subprocess failed with exit code {exit_code:?}: {stderr}")
        };
        persist_completion_failed_at_agent(&agent_dir, &request.run_id, exit_code, &reason);
        return;
    }

    match crate::cron::parse_cron_output(&lines) {
        Ok(output) => {
            if let Err(reason) = persist_successful_background_output(
                &agent_dir,
                &request.run_id,
                exit_code,
                &output,
                resolved_sandbox.as_deref(),
            )
            .await
            {
                persist_completion_failed_at_agent(&agent_dir, &request.run_id, exit_code, &reason);
            }
        }
        Err(reason) => {
            persist_completion_failed_at_agent(&agent_dir, &request.run_id, exit_code, &reason);
        }
    }
}

async fn drain_stderr(child: &mut right_process::ProcessGroupChild) -> String {
    let Some(mut stderr) = child.stderr() else {
        return String::new();
    };
    let mut buf = Vec::new();
    match tokio::time::timeout(STDERR_DRAIN_TIMEOUT, stderr.read_to_end(&mut buf)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("failed to read background stderr: {e:#}"),
        Err(_) => tracing::warn!("timed out reading background stderr"),
    }
    String::from_utf8_lossy(&buf).into_owned()
}

async fn persist_successful_background_output(
    agent_dir: &Path,
    run_id: &str,
    exit_code: Option<i32>,
    output: &crate::cron::CronReplyOutput,
    resolved_sandbox: Option<&str>,
) -> Result<(), String> {
    let notify = output
        .notify
        .as_ref()
        .ok_or_else(|| "background continuation returned notify: null".to_string())?;
    if notify.content.trim().is_empty() {
        return Err("background continuation returned empty notify.content".to_string());
    }
    let notify_json =
        serialize_notify_for_host(agent_dir, run_id, notify, resolved_sandbox).await?;

    let conn = right_db::open_connection(agent_dir, false)
        .map_err(|e| format!("open DB to persist background output: {e:#}"))?;
    right_agent::async_runs::persist_run_output(
        &conn,
        run_id,
        right_agent::async_runs::RunOutput {
            summary: Some(&output.summary),
            notify_json: Some(&notify_json),
            no_notify_reason: output.no_notify_reason.as_deref(),
            error_json: None,
            delivery_required: true,
        },
    )
    .map_err(|e| format!("persist background output: {e:#}"))?;
    right_agent::async_runs::finish_run(&conn, run_id, exit_code, "success")
        .map_err(|e| format!("finish background run: {e:#}"))?;
    Ok(())
}

async fn serialize_notify_for_host(
    agent_dir: &Path,
    run_id: &str,
    notify: &crate::cron::CronNotify,
    resolved_sandbox: Option<&str>,
) -> Result<String, String> {
    let Some(attachments) = notify.attachments.as_ref() else {
        return serde_json::to_string(notify)
            .map_err(|e| format!("serialize background notify_json: {e:#}"));
    };
    if attachments.is_empty() || resolved_sandbox.is_none() {
        return serde_json::to_string(notify)
            .map_err(|e| format!("serialize background notify_json: {e:#}"));
    }

    let sandbox = resolved_sandbox.expect("checked above");
    let outbox_dir = agent_dir.join("outbox").join("background").join(run_id);
    std::fs::create_dir_all(&outbox_dir)
        .map_err(|e| format!("create background outbox dir: {e:#}"))?;

    let mut host_attachments = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let dest = outbox_dir.join(attachment_filename(&attachment.path));
        right_openshell::openshell::download_file(sandbox, &attachment.path, &dest)
            .await
            .map_err(|e| {
                format!(
                    "download background attachment {} to {}: {e:#}",
                    attachment.path,
                    dest.display()
                )
            })?;
        host_attachments.push(crate::cc::attachments_dto::OutboundAttachment {
            kind: attachment.kind,
            path: dest.to_string_lossy().into_owned(),
            filename: attachment.filename.clone(),
            caption: attachment.caption.clone(),
            media_group_id: attachment.media_group_id.clone(),
        });
    }

    let host_notify = crate::cron::CronNotify {
        content: notify.content.clone(),
        attachments: Some(host_attachments),
    };
    serde_json::to_string(&host_notify)
        .map_err(|e| format!("serialize background host notify_json: {e:#}"))
}

fn attachment_filename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn persist_background_failure_notify(
    conn: &rusqlite::Connection,
    run_id: &str,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    let notify = crate::cron::CronNotify {
        content: BACKGROUND_FAILURE_NOTIFY_CONTENT.to_string(),
        attachments: None,
    };
    let notify_json = serde_json::to_string(&notify)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let summary = format!("Background run `{run_id}` failed before producing a result");
    let error_json = serde_json::json!({
        "kind": "background_result_unavailable",
        "run_id": run_id,
        "reason": reason,
    })
    .to_string();

    right_agent::async_runs::persist_run_output(
        conn,
        run_id,
        right_agent::async_runs::RunOutput {
            summary: Some(&summary),
            notify_json: Some(&notify_json),
            no_notify_reason: None,
            error_json: Some(&error_json),
            delivery_required: true,
        },
    )
}

fn mark_handoff_failed(
    conn: &rusqlite::Connection,
    run_id: &str,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    persist_background_failure_notify(conn, run_id, reason)?;
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn.execute(
        "UPDATE async_runs
         SET handoff_state = 'failed',
             updated_at = ?2
         WHERE id = ?1",
        rusqlite::params![run_id, now],
    )?;
    if rows == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    right_agent::async_runs::finish_run(conn, run_id, None, "failed")
}

fn mark_completion_failed(
    conn: &rusqlite::Connection,
    run_id: &str,
    exit_code: Option<i32>,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    persist_background_failure_notify(conn, run_id, reason)?;
    right_agent::async_runs::finish_run(conn, run_id, exit_code, "failed")
}

fn persist_handoff_failed_at_agent(agent_dir: &Path, run_id: &str, reason: &str) {
    match right_db::open_connection(agent_dir, false) {
        Ok(conn) => {
            if let Err(e) = mark_handoff_failed(&conn, run_id, reason) {
                tracing::error!(
                    run_id,
                    "failed to persist background handoff failure: {e:#}"
                );
            }
        }
        Err(e) => tracing::error!(
            run_id,
            "failed to open DB for background handoff failure: {e:#}"
        ),
    }
}

fn persist_completion_failed_at_agent(
    agent_dir: &Path,
    run_id: &str,
    exit_code: Option<i32>,
    reason: &str,
) {
    match right_db::open_connection(agent_dir, false) {
        Ok(conn) => {
            if let Err(e) = mark_completion_failed(&conn, run_id, exit_code, reason) {
                tracing::error!(
                    run_id,
                    "failed to persist background completion failure: {e:#}"
                );
            }
        }
        Err(e) => {
            tracing::error!(
                run_id,
                "failed to open DB for background completion failure: {e:#}"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_log_path_uses_background_logs_dir() {
        assert_eq!(
            bg_log_path(Path::new("/agent"), "run-1"),
            PathBuf::from("/agent/background/logs/run-1.ndjson")
        );
    }

    #[test]
    fn handoff_init_parser_requires_matching_system_init_session_id() {
        let line = r#"{"type":"system","subtype":"init","session_id":"run-1"}"#;
        assert!(is_handoff_init_for_run(line, "run-1"));
        assert!(!is_handoff_init_for_run(line, "run-2"));
        assert!(!is_handoff_init_for_run(
            r#"{"type":"system","subtype":"result","session_id":"run-1"}"#,
            "run-1"
        ));
        assert!(!is_handoff_init_for_run("not json", "run-1"));
    }

    #[test]
    fn mark_handoff_failed_sets_failed_pending_delivery() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).unwrap();
        right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: "run-1",
                producer_ref: Some("background"),
                source_session_id: "main-1",
                run_session_id: "run-1",
                target_chat_id: -42,
                target_thread_id: Some(7),
                created_at: "2026-05-18T10:00:00Z",
            },
        )
        .unwrap();

        mark_handoff_failed(&conn, "run-1", "init timeout").unwrap();

        let row: (String, String, i64, String, String, String) = conn
            .query_row(
                "SELECT status, handoff_state, delivery_required, delivery_status, notify_json, error_json \
                 FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "failed");
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "pending");
        assert!(row.4.contains("Background work failed"));
        assert!(row.5.contains("init timeout"));
    }
}
