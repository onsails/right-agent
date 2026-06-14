use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWriteExt as _};
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
const INTERRUPTED_HANDOFF_REASON: &str =
    "background handoff interrupted while queued before startup recovery";

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
    _session_guard: tokio::sync::OwnedMutexGuard<()>,
    debug: Arc<std::sync::atomic::AtomicBool>,
) -> HandoffStatus {
    let log_path = bg_log_path(&agent_dir, &request.run_id);
    if let Some(parent) = log_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let reason = format!("failed to create background log dir: {e:#}");
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
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
        debug_flag: Some(Arc::clone(&debug)),
    };
    let claude_args = invocation.into_args();

    let mut cmd = match build_background_command(
        &agent_dir,
        &agent_name,
        ssh_config_path.as_deref(),
        resolved_sandbox.as_deref(),
        &claude_args,
        mcp_instructions.as_deref(),
    )
    .await
    {
        Ok(cmd) => cmd,
        Err(reason) => {
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
            return HandoffStatus::Failed(reason);
        }
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

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
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
            return HandoffStatus::Failed(reason);
        }
    };

    let stderr_handle =
        spawn_stderr_reader(child.stderr(), log_path.clone(), request.run_id.clone());

    let stdout = match child.stdout() {
        Some(stdout) => stdout,
        None => {
            let reason = "spawned background child without stdout".to_string();
            let stderr = kill_child_and_collect_stderr(child, stderr_handle).await;
            let reason = append_stderr_to_reason(&reason, &stderr);
            persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
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
        let stderr = kill_unconfirmed_child(child, reader_handle, stderr_handle).await;
        let reason = append_stderr_to_reason(&reason, &stderr);
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
        return HandoffStatus::Failed(reason);
    }

    let started_at = chrono::Utc::now().to_rfc3339();
    let log_path_str = log_path.to_string_lossy().into_owned();
    let mark_spawned = {
        match right_db::open_connection(&agent_dir, false).await {
            Ok(conn) => right_agent::async_runs::mark_background_spawned(
                &conn,
                &request.run_id,
                &started_at,
                &log_path_str,
            )
            .await
            .map_err(|e| format!("mark background spawned: {e:#}")),
            Err(e) => Err(format!("open DB to mark background spawned: {e:#}")),
        }
    };
    if let Err(reason) = mark_spawned {
        let stderr = kill_unconfirmed_child(child, reader_handle, stderr_handle).await;
        let reason = append_stderr_to_reason(&reason, &stderr);
        persist_handoff_failed_at_agent(&agent_dir, &request.run_id, &reason).await;
        return HandoffStatus::Failed(reason);
    }

    tokio::spawn(complete_background_run(
        request,
        agent_dir,
        resolved_sandbox,
        child,
        reader_handle,
        stderr_handle,
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

async fn build_background_command(
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

    crate::cc::invocation::guard_no_sandboxed_host_exec(resolved_sandbox, ssh_config_path)
        .map_err(|e| format!("{e:#}"))?;

    // Per-agent notice token for the trusted `## Platform Notice Token` prompt
    // section, so the agent can verify SYSTEM_NOTICE markers.
    let notice_token = {
        let conn = right_db::open_connection(agent_dir, false)
            .await
            .map_err(|e| format!("open DB for notice token: {e:#}"))?;
        right_mcp::credentials::get_or_create_notice_token(&conn)
            .await
            .map_err(|e| format!("fetch notice token: {e:#}"))?
    };

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
            None,
            None,
            Some(&notice_token),
        );
        if let Some(token) = crate::login::load_auth_token(agent_dir).await {
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
            None,
            None,
            Some(&notice_token),
        );
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(&assembly_script);
        cmd.env("HOME", agent_dir);
        cmd.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir).await {
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
        if let Err(reason) = append_log_line(&mut file, &line, &log_path).await {
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

fn format_stderr_log_line(run_id: &str, content: &str) -> String {
    serde_json::json!({
        "type": "stream",
        "stream": "stderr",
        "session_id": run_id,
        "content": content,
    })
    .to_string()
}

async fn append_log_line(
    file: &mut tokio::fs::File,
    line: &str,
    log_path: &Path,
) -> Result<(), String> {
    let mut record = String::with_capacity(line.len() + 1);
    record.push_str(line);
    record.push('\n');
    file.write_all(record.as_bytes())
        .await
        .map_err(|e| format!("write background log {}: {e:#}", log_path.display()))
}

fn spawn_stderr_reader(
    stderr: Option<tokio::process::ChildStderr>,
    log_path: PathBuf,
    run_id: String,
) -> Option<JoinHandle<Result<String, String>>> {
    stderr.map(|stderr| tokio::spawn(read_background_stderr_from_reader(stderr, log_path, run_id)))
}

async fn read_background_stderr_from_reader<R>(
    stderr: R,
    log_path: PathBuf,
    run_id: String,
) -> Result<String, String>
where
    R: AsyncRead + Unpin,
{
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
    {
        Ok(file) => Some(file),
        Err(e) => {
            tracing::warn!(
                path = %log_path.display(),
                "failed to open background log for stderr; draining stderr without host log: {e:#}"
            );
            None
        }
    };
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    let mut captured = String::new();

    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| format!("read background stderr: {e:#}"))?;
        let Some(line) = line else {
            break;
        };
        captured.push_str(&line);
        captured.push('\n');
        if let Some(open_file) = file.as_mut() {
            let log_line = format_stderr_log_line(&run_id, &line);
            if let Err(reason) = append_log_line(open_file, &log_line, &log_path).await {
                tracing::warn!("{reason}; continuing to drain stderr without host log");
                file = None;
            }
        }
    }

    Ok(captured)
}

fn send_init_failure(
    init_tx: &mut Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    reason: &str,
) {
    if let Some(tx) = init_tx.take() {
        let _ = tx.send(Err(reason.to_string()));
    }
}

async fn kill_child_and_collect_stderr(
    mut child: right_process::ProcessGroupChild,
    stderr_handle: Option<JoinHandle<Result<String, String>>>,
) -> String {
    if let Err(e) = child.kill().await {
        tracing::warn!("failed to kill background child: {e:#}");
    }
    drop(child);
    await_stderr_reader(stderr_handle).await
}

async fn kill_unconfirmed_child(
    mut child: right_process::ProcessGroupChild,
    reader_handle: JoinHandle<Result<Vec<String>, String>>,
    stderr_handle: Option<JoinHandle<Result<String, String>>>,
) -> String {
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
    await_stderr_reader(stderr_handle).await
}

async fn complete_background_run(
    request: BackgroundRunRequest,
    agent_dir: PathBuf,
    resolved_sandbox: Option<String>,
    mut child: right_process::ProcessGroupChild,
    reader_handle: JoinHandle<Result<Vec<String>, String>>,
    stderr_handle: Option<JoinHandle<Result<String, String>>>,
) {
    let lines = match reader_handle.await {
        Ok(Ok(lines)) => lines,
        Ok(Err(reason)) => {
            let _ = child.kill().await;
            drop(child);
            let stderr = await_stderr_reader(stderr_handle).await;
            let reason = append_stderr_to_reason(&reason, &stderr);
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                BACKGROUND_FAILURE_NOTIFY_CONTENT,
                &reason,
            )
            .await;
            return;
        }
        Err(e) => {
            let _ = child.kill().await;
            drop(child);
            let stderr = await_stderr_reader(stderr_handle).await;
            let reason = append_stderr_to_reason(
                &format!("background stdout reader task failed: {e:#}"),
                &stderr,
            );
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                BACKGROUND_FAILURE_NOTIFY_CONTENT,
                &reason,
            )
            .await;
            return;
        }
    };

    let exit_status = match tokio::time::timeout(POST_STDOUT_WAIT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            let stderr = await_stderr_reader(stderr_handle).await;
            let reason =
                append_stderr_to_reason(&format!("background child wait failed: {e:#}"), &stderr);
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                BACKGROUND_FAILURE_NOTIFY_CONTENT,
                &reason,
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = child.kill().await;
            drop(child);
            let stderr = await_stderr_reader(stderr_handle).await;
            let reason = append_stderr_to_reason(
                "background child wait timed out after stdout closed",
                &stderr,
            );
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                None,
                BACKGROUND_FAILURE_NOTIFY_CONTENT,
                &reason,
            )
            .await;
            return;
        }
    };
    let exit_code = exit_status.code();
    let stderr = await_stderr_reader(stderr_handle).await;
    if !stderr.trim().is_empty() {
        tracing::warn!(
            run_id = %request.run_id,
            stderr = %stderr,
            "background stderr"
        );
    }

    // CC's terminal result line is authoritative. An `is_error` result — e.g. a
    // 529 overload that exhausted retries — can accompany either exit code and
    // never carries a parseable delivery, so classify it into a user-facing
    // message before falling back to generic exit-code / parse handling.
    if let Some(classified) = crate::cron::classify_failed_result(&lines) {
        tracing::warn!(
            run_id = %request.run_id,
            detail = %classified.detail,
            "background run reported an error result"
        );
        persist_completion_failed_at_agent(
            &agent_dir,
            &request.run_id,
            exit_code,
            &classified.user_message,
            &classified.detail,
        )
        .await;
        return;
    }

    if !exit_status.success() {
        let reason = if stderr.trim().is_empty() {
            format!("background subprocess failed with exit code {exit_code:?}")
        } else {
            format!("background subprocess failed with exit code {exit_code:?}: {stderr}")
        };
        persist_completion_failed_at_agent(
            &agent_dir,
            &request.run_id,
            exit_code,
            BACKGROUND_FAILURE_NOTIFY_CONTENT,
            &reason,
        )
        .await;
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
                persist_completion_failed_at_agent(
                    &agent_dir,
                    &request.run_id,
                    exit_code,
                    BACKGROUND_FAILURE_NOTIFY_CONTENT,
                    &reason,
                )
                .await;
            }
        }
        Err(reason) => {
            persist_completion_failed_at_agent(
                &agent_dir,
                &request.run_id,
                exit_code,
                BACKGROUND_FAILURE_NOTIFY_CONTENT,
                &reason,
            )
            .await;
        }
    }
}

async fn await_stderr_reader(stderr_handle: Option<JoinHandle<Result<String, String>>>) -> String {
    let Some(stderr_handle) = stderr_handle else {
        return String::new();
    };
    match tokio::time::timeout(STDERR_DRAIN_TIMEOUT, stderr_handle).await {
        Ok(Ok(Ok(stderr))) => stderr,
        Ok(Ok(Err(reason))) => {
            tracing::warn!("background stderr reader failed: {reason}");
            format!("[stderr reader failed: {reason}]")
        }
        Ok(Err(e)) => {
            tracing::warn!("background stderr reader task failed: {e:#}");
            format!("[stderr reader task failed: {e:#}]")
        }
        Err(_) => {
            tracing::warn!("timed out waiting for background stderr reader");
            "[stderr reader timed out]".to_string()
        }
    }
}

fn append_stderr_to_reason(reason: &str, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        reason.to_string()
    } else {
        format!("{reason}; stderr: {stderr}")
    }
}

async fn persist_successful_background_output(
    agent_dir: &Path,
    run_id: &str,
    exit_code: Option<i32>,
    output: &crate::cron::CronReplyOutput,
    resolved_sandbox: Option<&str>,
) -> Result<(), String> {
    let notify = output
        .delivery
        .as_notify()
        .ok_or_else(|| "background continuation returned non-notify delivery".to_string())?;
    let delivery_json =
        serialize_notify_delivery_for_host(agent_dir, run_id, &notify, resolved_sandbox).await?;

    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| format!("open DB to persist background output: {e:#}"))?;
    let tx = conn
        .transaction()
        .await
        .map_err(|e| format!("begin transaction for background output: {e:#}"))?;
    right_agent::async_runs::persist_run_output(
        &tx,
        run_id,
        right_agent::async_runs::RunOutput {
            run_note: Some(&output.run_note),
            delivery_json: Some(&delivery_json),
            error_json: None,
            delivery_required: true,
        },
    )
    .await
    .map_err(|e| format!("persist background output: {e:#}"))?;
    right_agent::async_runs::finish_run(&tx, run_id, exit_code, "success")
        .await
        .map_err(|e| format!("finish background run: {e:#}"))?;
    tx.commit()
        .await
        .map_err(|e| format!("commit background output: {e:#}"))?;
    Ok(())
}

async fn serialize_notify_delivery_for_host(
    agent_dir: &Path,
    run_id: &str,
    notify: &crate::cron::CronNotify,
    resolved_sandbox: Option<&str>,
) -> Result<String, String> {
    let Some(attachments) = notify.attachments.as_ref() else {
        return crate::cron::notify_delivery_json(&notify.content, notify.attachments.as_deref())
            .map_err(|e| format!("serialize background delivery_json: {e:#}"));
    };
    if attachments.is_empty() || resolved_sandbox.is_none() {
        return crate::cron::notify_delivery_json(&notify.content, notify.attachments.as_deref())
            .map_err(|e| format!("serialize background delivery_json: {e:#}"));
    }

    let sandbox = resolved_sandbox.expect("checked above");
    let outbox_dir = agent_dir.join("outbox").join("background").join(run_id);
    std::fs::create_dir_all(&outbox_dir)
        .map_err(|e| format!("create background outbox dir: {e:#}"))?;

    let mut host_attachments = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let dest = outbox_dir.join(crate::cron::attachment_filename(&attachment.path));
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

    crate::cron::notify_delivery_json(&notify.content, Some(&host_attachments))
        .map_err(|e| format!("serialize background host delivery_json: {e:#}"))
}

async fn persist_background_failure_notify(
    conn: &right_db::Connection,
    run_id: &str,
    user_content: &str,
    reason: &str,
) -> Result<(), right_db::DbError> {
    let (run_note, delivery_json, error_json) =
        background_failure_payload(run_id, user_content, reason)?;
    right_agent::async_runs::persist_run_output(
        conn,
        run_id,
        right_agent::async_runs::RunOutput {
            run_note: Some(&run_note),
            delivery_json: Some(&delivery_json),
            error_json: Some(&error_json),
            delivery_required: true,
        },
    )
    .await
}

fn background_failure_payload(
    run_id: &str,
    user_content: &str,
    reason: &str,
) -> Result<(String, String, String), right_db::DbError> {
    let delivery_json = crate::cron::notify_delivery_json(user_content, None)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let run_note = format!("Background run `{run_id}` failed before producing a result");
    let error_json = serde_json::json!({
        "kind": "background_result_unavailable",
        "run_id": run_id,
        "reason": reason,
    })
    .to_string();
    Ok((run_note, delivery_json, error_json))
}

async fn mark_handoff_failed(
    conn: &right_db::Connection,
    run_id: &str,
    reason: &str,
) -> Result<(), right_db::DbError> {
    persist_background_failure_notify(conn, run_id, BACKGROUND_FAILURE_NOTIFY_CONTENT, reason)
        .await?;
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE async_runs
         SET handoff_state = 'failed',
             updated_at = ?2
        WHERE id = ?1",
            right_db::params![run_id, now],
        )
        .await?;
    if rows == 0 {
        return Err(right_db::DbError::NotFound);
    }
    right_agent::async_runs::finish_run(conn, run_id, None, "failed").await
}

pub(crate) async fn mark_interrupted_handoffs(
    conn: &right_db::Connection,
) -> Result<usize, right_db::DbError> {
    let tx = conn.transaction().await?;
    let run_ids = {
        let mut stmt = tx.prepare(
            "SELECT id
             FROM async_runs
             WHERE kind = 'background'
               AND status = 'queued'
               AND handoff_state = 'queued'",
        )?;
        stmt.query_map([], |r| r.get::<_, String>(0))
            .await?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut converted = 0usize;
    for run_id in run_ids {
        if mark_interrupted_handoff_failed_if_still_queued(&tx, &run_id).await? {
            converted += 1;
        }
    }
    tx.commit().await?;
    Ok(converted)
}

async fn mark_interrupted_handoff_failed_if_still_queued(
    conn: &right_db::Connection,
    run_id: &str,
) -> Result<bool, right_db::DbError> {
    let (run_note, delivery_json, error_json) = background_failure_payload(
        run_id,
        BACKGROUND_FAILURE_NOTIFY_CONTENT,
        INTERRUPTED_HANDOFF_REASON,
    )?;
    let now = chrono::Utc::now().to_rfc3339();
    let rows = conn
        .execute(
            "UPDATE async_runs
         SET run_note = ?2,
             delivery_json = ?3,
             error_json = ?4,
             delivery_required = 1,
             delivery_status = 'pending',
             handoff_state = 'failed',
             finished_at = ?5,
             exit_code = NULL,
             status = 'failed',
             updated_at = ?5
        WHERE id = ?1
           AND kind = 'background'
           AND status = 'queued'
           AND handoff_state = 'queued'",
            right_db::params![run_id, run_note, delivery_json, error_json, now],
        )
        .await?;
    Ok(rows > 0)
}

async fn mark_completion_failed(
    conn: &right_db::Connection,
    run_id: &str,
    exit_code: Option<i32>,
    user_content: &str,
    reason: &str,
) -> Result<(), right_db::DbError> {
    let tx = conn.transaction().await?;
    persist_background_failure_notify(&tx, run_id, user_content, reason).await?;
    right_agent::async_runs::finish_run(&tx, run_id, exit_code, "failed").await?;
    tx.commit().await
}

async fn persist_handoff_failed_at_agent(agent_dir: &Path, run_id: &str, reason: &str) {
    match right_db::open_connection(agent_dir, false).await {
        Ok(conn) => {
            if let Err(e) = mark_handoff_failed(&conn, run_id, reason).await {
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

async fn persist_completion_failed_at_agent(
    agent_dir: &Path,
    run_id: &str,
    exit_code: Option<i32>,
    user_content: &str,
    reason: &str,
) {
    match right_db::open_connection(agent_dir, false).await {
        Ok(conn) => {
            if let Err(e) =
                mark_completion_failed(&conn, run_id, exit_code, user_content, reason).await
            {
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

    #[tokio::test]
    async fn build_background_command_refuses_sandboxed_host_exec() {
        // Sandboxed agent (resolved_sandbox Some) with no sandbox connection
        // (ssh_config_path None) must refuse — never build a host command.
        let tmp = tempfile::tempdir().unwrap();
        let result =
            build_background_command(tmp.path(), "agent", None, Some("sbx"), &[], None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bg_log_path_uses_background_logs_dir() {
        assert_eq!(
            bg_log_path(Path::new("/agent"), "run-1"),
            PathBuf::from("/agent/background/logs/run-1.ndjson")
        );
    }

    #[tokio::test]
    async fn handoff_init_parser_requires_matching_system_init_session_id() {
        let line = r#"{"type":"system","subtype":"init","session_id":"run-1"}"#;
        assert!(is_handoff_init_for_run(line, "run-1"));
        assert!(!is_handoff_init_for_run(line, "run-2"));
        assert!(!is_handoff_init_for_run(
            r#"{"type":"system","subtype":"result","session_id":"run-1"}"#,
            "run-1"
        ));
        assert!(!is_handoff_init_for_run("not json", "run-1"));
    }

    #[tokio::test]
    async fn format_stderr_log_line_marks_stderr_stream() {
        let line = format_stderr_log_line("run-1", "warning: bad \"thing\"");
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["type"], "stream");
        assert_eq!(parsed["stream"], "stderr");
        assert_eq!(parsed["session_id"], "run-1");
        assert_eq!(parsed["content"], "warning: bad \"thing\"");
    }

    #[tokio::test]
    async fn stderr_reader_writes_structured_lines_and_returns_text() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("bg.ndjson");
        let (mut writer, reader) = tokio::io::duplex(64);
        let writer_task = tokio::spawn(async move {
            writer.write_all(b"first\nsecond\n").await.unwrap();
        });

        let captured = read_background_stderr_from_reader(reader, log_path.clone(), "run-1".into())
            .await
            .unwrap();
        writer_task.await.unwrap();

        assert_eq!(captured, "first\nsecond\n");
        let raw = tokio::fs::read_to_string(log_path).await.unwrap();
        let lines: Vec<_> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["stream"], "stderr");
        assert_eq!(first["content"], "first");
        assert_eq!(second["stream"], "stderr");
        assert_eq!(second["content"], "second");
    }

    #[tokio::test]
    async fn mark_handoff_failed_sets_failed_pending_delivery() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
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
        .await
        .unwrap();

        mark_handoff_failed(&conn, "run-1", "init timeout")
            .await
            .unwrap();

        let row: (String, String, i64, String, String, String) = conn
            .query_row(
                "SELECT status, handoff_state, delivery_required, delivery_status, delivery_json, error_json \
                 FROM async_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "failed");
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "pending");
        assert!(row.4.contains("Background work failed"));
        assert!(row.5.contains("init timeout"));
    }

    #[tokio::test]
    async fn completion_failure_delivers_classified_529_message_to_user() {
        // Regression for the contentless "failed without result" notification:
        // a 529 overload result line must reach the user as a transient-overload
        // message, with the raw status preserved only in the internal error_json.
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: "run-529",
                producer_ref: Some("background"),
                source_session_id: "main-1",
                run_session_id: "run-529",
                target_chat_id: -42,
                target_thread_id: Some(7),
                created_at: "2026-06-02T13:39:00Z",
            },
        )
        .await
        .unwrap();

        let lines = vec![
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":529,"result":"API Error: 529 Overloaded."}"#.to_string(),
        ];
        let classified =
            crate::cron::classify_failed_result(&lines).expect("529 classifies as failure");
        mark_completion_failed(
            &conn,
            "run-529",
            Some(1),
            &classified.user_message,
            &classified.detail,
        )
        .await
        .unwrap();

        let row: (String, i64, String, String, String) = conn
            .query_row(
                "SELECT status, delivery_required, delivery_status, delivery_json, error_json \
                 FROM async_runs WHERE id = 'run-529'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, 1);
        assert_eq!(row.2, "pending");
        // User-facing delivery carries the transient-overload explanation, not
        // the generic "Background work failed" string.
        assert!(row.3.contains("overloaded"), "delivery_json: {}", row.3);
        assert!(
            !row.3.contains("Background work failed"),
            "should not be the generic message: {}",
            row.3
        );
        // Raw status is preserved internally for debugging.
        assert!(row.4.contains("529"), "error_json: {}", row.4);
    }

    #[tokio::test]
    async fn mark_interrupted_handoffs_converts_queued_background_to_failed_pending_delivery() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: "bg-queued",
                producer_ref: Some("background"),
                source_session_id: "main-1",
                run_session_id: "bg-queued",
                target_chat_id: -42,
                target_thread_id: Some(7),
                created_at: "2026-05-18T10:00:00Z",
            },
        )
        .await
        .unwrap();
        right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: "bg-spawned",
                producer_ref: Some("background"),
                source_session_id: "main-2",
                run_session_id: "bg-spawned",
                target_chat_id: -42,
                target_thread_id: Some(7),
                created_at: "2026-05-18T10:00:00Z",
            },
        )
        .await
        .unwrap();
        right_agent::async_runs::mark_background_spawned(
            &conn,
            "bg-spawned",
            "2026-05-18T10:01:00Z",
            "/log/bg-spawned.ndjson",
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id, target_thread_id,
                status, handoff_state, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'cron-queued', 'cron', 'cron-job', 'cron-queued', -42, NULL,
                'queued', 'queued', 0, 'none',
                '2026-05-18T10:00:00Z', '2026-05-18T10:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();

        let converted = mark_interrupted_handoffs(&conn).await.unwrap();

        assert_eq!(converted, 1);
        let bg: (String, String, i64, String, String, String) = conn
            .query_row(
                "SELECT status, handoff_state, delivery_required, delivery_status, delivery_json, error_json \
                 FROM async_runs WHERE id = 'bg-queued'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .await
            .unwrap();
        assert_eq!(bg.0, "failed");
        assert_eq!(bg.1, "failed");
        assert_eq!(bg.2, 1);
        assert_eq!(bg.3, "pending");
        assert!(bg.4.contains("Background work failed"));
        assert!(bg.5.contains("interrupted"));

        let spawned: (String, String) = conn
            .query_row(
                "SELECT status, handoff_state FROM async_runs WHERE id = 'bg-spawned'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(spawned.0, "running");
        assert_eq!(spawned.1, "spawned");

        let cron: (String, String, String) = conn
            .query_row(
                "SELECT kind, status, handoff_state FROM async_runs WHERE id = 'cron-queued'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(cron.0, "cron");
        assert_eq!(cron.1, "queued");
        assert_eq!(cron.2, "queued");
    }

    #[tokio::test]
    async fn mark_interrupted_handoffs_skip_rows_no_longer_queued_at_write_time() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        right_agent::async_runs::insert_queued_background_run(
            &conn,
            right_agent::async_runs::NewBackgroundRun {
                id: "bg-raced",
                producer_ref: Some("background"),
                source_session_id: "main-1",
                run_session_id: "bg-raced",
                target_chat_id: -42,
                target_thread_id: Some(7),
                created_at: "2026-05-18T10:00:00Z",
            },
        )
        .await
        .unwrap();
        right_agent::async_runs::mark_background_spawned(
            &conn,
            "bg-raced",
            "2026-05-18T10:01:00Z",
            "/log/bg-raced.ndjson",
        )
        .await
        .unwrap();

        let converted = mark_interrupted_handoff_failed_if_still_queued(&conn, "bg-raced")
            .await
            .unwrap();

        assert!(!converted);
        let row: (String, String, i64, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, handoff_state, delivery_required, delivery_status, delivery_json, error_json \
                 FROM async_runs WHERE id = 'bg-raced'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .await
            .unwrap();
        assert_eq!(row.0, "running");
        assert_eq!(row.1, "spawned");
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "pending");
        assert!(row.4.is_none());
        assert!(row.5.is_none());
    }
}
