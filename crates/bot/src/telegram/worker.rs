//! Per-session worker task: debounce loop, CC subprocess invocation, reply tool parsing.
//!
//! Pure helpers are tested in isolation (TDD). `spawn_worker` and `invoke_cc` require
//! live infrastructure and are covered by code review pattern only.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, MessageId, ReplyParameters, ThreadId};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::session::{create_session, get_session, touch_session};

/// Session key: `(chat_id, effective_thread_id)`.
pub type SessionKey = (i64, i64);

/// Fixed 500ms debounce window (D-01).
const DEBOUNCE_MS: u64 = 500;

/// Maximum time to wait for a CC subprocess to complete.
const CC_TIMEOUT_SECS: u64 = 120;

/// A single Telegram message queued into the debounce channel.
#[derive(Clone)]
pub struct DebounceMsg {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<super::attachments::InboundAttachment>,
}

/// Context passed to each worker task when it is spawned.
#[derive(Clone)]
pub struct WorkerContext {
    pub chat_id: teloxide::types::ChatId,
    pub effective_thread_id: i64,
    pub agent_dir: PathBuf,
    /// Agent name for --agent flag on first CC invocation (AGDEF-02).
    pub agent_name: String,
    pub bot: super::BotType,
    /// agent_dir — passed separately so worker opens its own Connection
    pub db_path: PathBuf,
    /// When true, pass --verbose to CC subprocess and log CC stderr at debug level.
    pub debug: bool,
    /// Path to the SSH config file for this agent's OpenShell sandbox (None when --no-sandbox).
    pub ssh_config_path: Option<PathBuf>,
    /// Guard: true when an auth watcher task is active for this agent. Prevents duplicates.
    pub auth_watcher_active: Arc<AtomicBool>,
    /// Slot for auth code sender — when login flow is waiting for a code from Telegram,
    /// the oneshot::Sender is stored here. Message handler checks this before routing to worker.
    pub auth_code_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
}

/// Parsed output from CC structured JSON response (`result` field per D-03).
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

const TELEGRAM_LIMIT: usize = 4096;

/// Split a message at the 4096-char Telegram limit (D-17).
///
/// Splits at the last `\n` in the final 200 chars before the boundary.
/// Hard-cuts at 4096 if no `\n` found there.
pub fn split_message(text: &str) -> Vec<String> {
    if text.len() <= TELEGRAM_LIMIT {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut remaining = text;
    while remaining.len() > TELEGRAM_LIMIT {
        let cut = &remaining[..TELEGRAM_LIMIT];
        let window_start = TELEGRAM_LIMIT.saturating_sub(200);
        let split_pos = cut[window_start..]
            .rfind('\n')
            .map(|p| window_start + p + 1)
            .unwrap_or(TELEGRAM_LIMIT);
        parts.push(remaining[..split_pos].to_string());
        remaining = &remaining[split_pos..];
    }
    if !remaining.is_empty() {
        parts.push(remaining.to_string());
    }
    parts
}

/// Format a CC subprocess error as a Telegram message (D-16).
///
/// Output: `⚠️ Agent error (exit N):\n```\n<stderr>\n````
pub fn format_error_reply(exit_code: i32, stderr: &str) -> String {
    let truncated = if stderr.len() > 300 {
        &stderr[..300]
    } else {
        stderr
    };
    format!("⚠️ Agent error (exit {exit_code}):\n```\n{truncated}\n```")
}

/// Check whether CC stdout JSON indicates an authentication failure (403/401).
///
/// Returns true when the JSON has `is_error: true` and the `result` string
/// contains known auth-failure patterns. Returns false for non-JSON input,
/// parse errors, or non-auth errors.
pub fn is_auth_error(stdout: &str) -> bool {
    let parsed: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let is_error = parsed
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_error {
        return false;
    }

    let result = match parsed.get("result").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };

    const AUTH_PATTERNS: &[&str] = &[
        "API Error: 403",
        "API Error: 401",
        "Failed to authenticate",
        "Not logged in",
        "Please run /login",
    ];

    AUTH_PATTERNS.iter().any(|pattern| result.contains(pattern))
}

/// Extract an OAuth URL from process log lines.
///
/// Scans for `https://` URLs containing OAuth-specific path segments
/// (`/oauth/` or `/authorize`) on Anthropic/Claude domains.
/// Returns the first matching URL, trimmed of surrounding text.
pub fn extract_auth_url(lines: &[String]) -> Option<String> {
    for line in lines {
        let Some(start) = line.find("https://") else {
            continue;
        };
        let url_part = &line[start..];
        let end = url_part
            .find(|c: char| c.is_whitespace())
            .unwrap_or(url_part.len());
        let url = &url_part[..end];

        // Match OAuth-specific URLs on Anthropic/Claude domains.
        let is_auth_domain = url.contains("anthropic") || url.contains("claude.ai") || url.contains("claude.com");
        let is_auth_path = url.contains("/oauth/") || url.contains("/authorize");
        if is_auth_domain && is_auth_path {
            return Some(url.to_string());
        }
    }
    None
}

/// Parse CC structured JSON output (D-03, D-04).
///
/// Returns `Ok((ReplyOutput, Option<session_id>))` on success.
/// Returns `Err(String)` if JSON is malformed or the `result` field is missing.
/// Returns `Ok((ReplyOutput { content: None, .. }, _))` if content=null (silent response per D-04).
pub fn parse_reply_output(raw_json: &str) -> Result<(ReplyOutput, Option<String>), String> {
    tracing::debug!("CC raw JSON output: {}", raw_json);

    let parsed: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("JSON parse error: {e}"))?;

    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let result_val = parsed
        .get("structured_output")
        .filter(|v| !v.is_null())
        .or_else(|| parsed.get("result"))
        .ok_or_else(|| "CC response missing both 'structured_output' and 'result' fields".to_string())?;

    // CC sometimes returns result as a plain string (e.g. after multi-turn MCP tool use)
    // instead of complying with --json-schema. Wrap it as ReplyOutput so the message is delivered.
    let output: ReplyOutput = if let Some(text) = result_val.as_str() {
        ReplyOutput {
            content: if text.is_empty() { None } else { Some(text.to_string()) },
            reply_to_message_id: None,
            attachments: None,
        }
    } else {
        serde_json::from_value(result_val.clone())
            .map_err(|e| format!("failed to deserialize result: {e}"))?
    };

    Ok((output, session_id))
}

/// Wait for a child process to complete, killing it if `timeout_secs` elapses.
///
/// On timeout, `tokio::time::timeout` cancels the inner `wait_with_output` future.
/// When that future is dropped it drops the `Child`, and `kill_on_drop(true)` issues
/// the OS-level kill.  Returns a formatted error string on both timeout and wait
/// failure so callers can forward it straight to Telegram.
async fn wait_with_timeout(
    child: tokio::process::Child,
    timeout_secs: u64,
) -> Result<std::process::Output, String> {
    timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| {
            format_error_reply(-1, &format!("CC subprocess timed out after {timeout_secs}s"))
        })?
        .map_err(|e| format_error_reply(-1, &format!("wait failed: {:#}", e)))
}

// ── Async worker ─────────────────────────────────────────────────────────────

/// Spawn a per-session worker task.
///
/// Called by the message handler when no sender exists for the session key.
/// Returns the `Sender` to store in the DashMap. The worker task:
///   1. Waits for the first message.
///   2. Collects additional messages within the 500ms debounce window (D-01).
///   3. Batches them as XML (D-02).
///   4. Invokes `claude -p` (D-13, D-14).
///   5. Parses the `reply` tool call (D-03, D-04, D-05).
///   6. Sends the Telegram reply.
///   7. Loops back to step 1.
///
/// On channel close (DashMap entry removed on `/reset`), the task exits.
/// On worker task panic, Sender in DashMap becomes stale; handler detects
/// `SendError` and removes the entry + respawns (Pitfall 7 mitigation).
pub fn spawn_worker(
    key: SessionKey,
    ctx: WorkerContext,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
) -> mpsc::Sender<DebounceMsg> {
    let (tx, mut rx) = mpsc::channel::<DebounceMsg>(32); // bounded — safe for debounce

    let tx_for_map = tx.clone();
    tokio::spawn(async move {
        let window = Duration::from_millis(DEBOUNCE_MS);
        let (chat_id, eff_thread_id) = key;
        let tg_chat_id = ctx.chat_id;

        loop {
            tracing::info!(?key, "worker waiting for message");
            // Wait for first message in this debounce cycle
            let Some(first) = rx.recv().await else {
                tracing::info!(?key, "worker channel closed — exiting");
                break;
            };
            tracing::info!(?key, batch_size = 1, "worker received message, starting debounce");
            let mut batch = vec![first];

            // Collect additional messages within debounce window (D-01)
            loop {
                tokio::select! {
                    biased;
                    msg = rx.recv() => {
                        match msg {
                            Some(m) => batch.push(m),
                            None => break,
                        }
                    }
                    _ = sleep(window) => break,
                }
            }

            // Download attachments for all messages in batch
            let mut input_messages = Vec::with_capacity(batch.len());
            for msg in &batch {
                let resolved = if msg.attachments.is_empty() {
                    vec![]
                } else {
                    match super::attachments::download_attachments(
                        &msg.attachments,
                        msg.message_id,
                        &ctx.bot,
                        &ctx.agent_dir,
                        ctx.ssh_config_path.as_deref(),
                        &ctx.agent_name,
                        tg_chat_id,
                        eff_thread_id,
                    ).await {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(?key, "attachment download failed: {:#}", e);
                            let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("Failed to download attachments: {e}")).await;
                            vec![]
                        }
                    }
                };
                input_messages.push(super::attachments::InputMessage {
                    message_id: msg.message_id,
                    text: msg.text.clone(),
                    timestamp: msg.timestamp,
                    attachments: resolved,
                });
            }

            let Some(input) = super::attachments::format_cc_input(&input_messages) else {
                tracing::warn!(?key, "empty input after formatting -- skipping CC invocation");
                continue;
            };

            // Typing indicator: spawn task, cancel after subprocess completes (D-10)
            let cancel_token = CancellationToken::new();
            let cancel_clone = cancel_token.clone();
            let bot_clone = ctx.bot.clone();
            let typing_task = tokio::spawn(async move {
                loop {
                    let mut action =
                        bot_clone.send_chat_action(tg_chat_id, ChatAction::Typing);
                    if eff_thread_id != 0 {
                        action =
                            action.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                    }
                    action.await.ok(); // best-effort; ignore errors
                    tokio::select! {
                        _ = cancel_clone.cancelled() => break,
                        _ = sleep(Duration::from_secs(4)) => {}
                    }
                }
            });

            // Invoke claude -p (D-13, D-14)
            let reply_result = invoke_cc(&input, chat_id, eff_thread_id, &ctx).await;

            // Cancel typing indicator
            cancel_token.cancel();
            typing_task.await.ok();

            // Send reply (D-04, D-05, DIS-05, DIS-06)
            match reply_result {
                Ok(Some(output)) => {
                    let reply_to = if batch.len() == 1 {
                        Some(batch[0].message_id)
                    } else {
                        output.reply_to_message_id
                    };

                    if let Some(content) = output.content {
                        let parts = split_message(&content);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );
                        for part in parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, &part);
                            if eff_thread_id != 0 {
                                send = send.message_thread_id(ThreadId(MessageId(
                                    eff_thread_id as i32,
                                )));
                            }
                            if let Some(ref_id) = reply_to {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            if let Err(e) = send.await {
                                tracing::error!(?key, "failed to send Telegram reply: {:#}", e);
                            }
                        }
                    } else {
                        tracing::warn!(
                            ?key,
                            "CC returned content: null -- no text reply sent"
                        );
                    }

                    // Send outbound attachments
                    if let Some(ref atts) = output.attachments {
                        if !atts.is_empty() {
                            if let Err(e) = super::attachments::send_attachments(
                                atts,
                                &ctx.bot,
                                tg_chat_id,
                                eff_thread_id,
                                &ctx.agent_dir,
                                ctx.ssh_config_path.as_deref(),
                                &ctx.agent_name,
                            ).await {
                                tracing::error!(?key, "failed to send attachments: {:#}", e);
                                let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("Failed to send attachments: {e}")).await;
                            }
                        }
                    }
                }
                Ok(None) => {
                    tracing::warn!(?key, "unexpected Ok(None) from invoke_cc — no reply sent");
                }
                Err(err_msg) => {
                    tracing::info!(?key, "sending error reply to Telegram");
                    let mut send = ctx.bot.send_message(tg_chat_id, &err_msg);
                    if eff_thread_id != 0 {
                        send = send
                            .message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
                    }
                    if let Err(e) = send.await {
                        tracing::error!(?key, "failed to send error reply: {:#}", e);
                    }
                }
            }
        }

        // Worker exiting — remove DashMap entry to prevent stale sender (Pitfall 3)
        worker_map.remove(&key);
        tracing::debug!(?key, "worker task exited, DashMap entry removed");
    });

    tx_for_map
}

/// Shell-escape a string for safe inclusion in an SSH remote command.
fn shell_escape(s: &str) -> String {
    shlex::try_quote(s).expect("shlex::try_quote cannot fail for valid UTF-8").into_owned()
}


/// Send a Telegram message, optionally in a thread.
pub(crate) async fn send_tg(
    bot: &super::BotType,
    chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    let mut send = bot.send_message(chat_id, text);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    send.await?;
    Ok(())
}

/// Spawn a background task that drives the Claude login flow via PTY.
///
/// 1. Spawns `claude --dangerously-skip-permissions -- /login` via SSH with PTY.
/// 2. Drives through login method menu (sends Enter).
/// 3. Extracts OAuth URL and sends to Telegram.
/// 4. Waits for auth code from Telegram user.
/// 5. Sends code to CC PTY, waits for success.
fn spawn_auth_watcher(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    eff_thread_id: i64,
) {
    let agent_name = ctx.agent_name.clone();
    let bot = ctx.bot.clone();
    let ssh_config_path = ctx.ssh_config_path.clone();
    let active_flag = Arc::clone(&ctx.auth_watcher_active);
    let auth_code_tx_slot = Arc::clone(&ctx.auth_code_tx);

    tokio::spawn(async move {
        let ssh_config = match ssh_config_path {
            Some(ref p) => p.clone(),
            None => {
                tracing::error!(agent = %agent_name, "auth watcher: no SSH config");
                active_flag.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Create channels for PTY ↔ async communication
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<crate::login::LoginEvent>(8);
        let (code_tx, code_rx) = tokio::sync::oneshot::channel::<String>();

        // Store the code sender so Telegram handler can forward the auth code
        auth_code_tx_slot.lock().await.replace(code_tx);

        // Spawn PTY login in blocking thread
        let agent_for_pty = agent_name.clone();
        tokio::task::spawn_blocking(move || {
            crate::login::run_login_pty(&ssh_config, &agent_for_pty, event_tx, code_rx);
        });

        // Process events from the PTY task
        let timeout = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(crate::login::LoginEvent::Url(url)) => {
                            let msg = format!("Open this link to authenticate:\n{url}");
                            if let Err(e) = send_tg(&bot, tg_chat_id, eff_thread_id, &msg).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                        }
                        Some(crate::login::LoginEvent::WaitingForCode) => {
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                "After authenticating in the browser, send me the code shown on the page.",
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                        }
                        Some(crate::login::LoginEvent::Done) => {
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                "Logged in successfully. You can continue chatting.",
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                            break;
                        }
                        Some(crate::login::LoginEvent::Error(msg)) => {
                            tracing::error!(agent = %agent_name, "auth watcher: login error: {msg}");
                            if let Err(e) = send_tg(
                                &bot, tg_chat_id, eff_thread_id,
                                &format!("Login failed: {msg}"),
                            ).await {
                                tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                            }
                            break;
                        }
                        None => {
                            tracing::info!(agent = %agent_name, "auth watcher: PTY task exited");
                            break;
                        }
                    }
                }
                _ = &mut timeout => {
                    tracing::warn!(agent = %agent_name, "auth watcher: login timed out after 5 min");
                    if let Err(e) = send_tg(
                        &bot, tg_chat_id, eff_thread_id,
                        "Login timed out after 5 minutes. Send another message to retry.",
                    ).await {
                        tracing::warn!(agent = %agent_name, "auth watcher: Telegram send failed: {e:#}");
                    }
                    break;
                }
            }
        }

        // Cleanup
        auth_code_tx_slot.lock().await.take();
        active_flag.store(false, Ordering::SeqCst);
    });
}

/// Invoke `claude -p` and parse the reply tool call from its JSON output.
///
/// Returns `Ok(Some(ReplyOutput))` on success,
/// `Err(error_message_for_telegram)` on subprocess failure or missing reply tool.
async fn invoke_cc(
    input: &str,
    chat_id: i64,
    eff_thread_id: i64,
    ctx: &WorkerContext,
) -> Result<Option<ReplyOutput>, String> {
    // Open per-worker DB connection (rusqlite is !Send — each worker opens its own)
    let conn = rightclaw::memory::open_connection(&ctx.agent_dir)
        .map_err(|e| format!("⚠️ Agent error: DB open failed: {:#}", e))?;

    // Session lookup / create (SES-02, SES-03)
    let (cmd_args, is_first_call) = match get_session(&conn, chat_id, eff_thread_id) {
        Ok(Some(root_id)) => {
            // Resume: --resume <root_session_id>
            (vec!["--resume".to_string(), root_id], false)
        }
        Ok(None) => {
            // First message: generate UUID, --session-id <uuid>
            let new_uuid = Uuid::new_v4().to_string();
            create_session(&conn, chat_id, eff_thread_id, &new_uuid)
                .map_err(|e| format!("⚠️ Agent error: session create failed: {:#}", e))?;
            (vec!["--session-id".to_string(), new_uuid], true)
        }
        Err(e) => {
            return Err(format!("⚠️ Agent error: session lookup failed: {:#}", e));
        }
    };

    // Build claude -p args for execution inside OpenShell sandbox
    let reply_schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
    ];

    // MCP isolation: only use servers from our mcp.json, block cloud MCPs.
    // Path differs by execution mode: /sandbox/ inside container, agent_dir on host.
    let mcp_config_path = if ctx.ssh_config_path.is_some() {
        rightclaw::openshell::SANDBOX_MCP_JSON_PATH.to_string()
    } else {
        ctx.agent_dir.join("mcp.json").to_string_lossy().into_owned()
    };
    claude_args.push("--mcp-config".into());
    claude_args.push(mcp_config_path);
    claude_args.push("--strict-mcp-config".into());

    // NOTE: --verbose is intentionally NOT passed even in debug mode.
    // --verbose combined with --output-format json switches CC to stream-json array output,
    // breaking parse_reply_output which expects a single JSON object.
    // CC stderr is already captured and logged at debug level below.
    for arg in &cmd_args {
        claude_args.push(arg.clone());
    }
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    // --agent only on first call (AGDEF-02); resume inherits from session (AGDEF-03)
    if is_first_call {
        claude_args.push("--agent".into());
        claude_args.push(ctx.agent_name.clone());
    }

    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4)
    // CC expects inline JSON string, NOT a file path — read and inline the content
    // Schema file lives on HOST; bot reads it before exec into sandbox.
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("reply-schema.json read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);

    let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
        // OpenShell sandbox: exec via SSH into the container.
        // SSH concatenates remote args into a single string and passes to `sh -c`.
        // Args containing JSON ({, }, ") must be shell-escaped to survive this.
        let ssh_host = rightclaw::openshell::ssh_host(&ctx.agent_name);
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        // Build a single shell-escaped command string for the remote shell.
        let escaped: Vec<String> = claude_args.iter().map(|a| shell_escape(a)).collect();
        c.arg(escaped.join(" "));
        c
    } else {
        // Direct exec (no sandbox).
        let cc_bin = which::which("claude")
            .or_else(|_| which::which("claude-bun"))
            .map_err(|_| "⚠️ Agent error: claude binary not found in PATH".to_string())?;
        let mut c = tokio::process::Command::new(&cc_bin);
        // Skip "claude" (first element in claude_args) — it's the binary name for SSH mode.
        for arg in &claude_args[1..] {
            c.arg(arg);
        }
        c.env("HOME", &ctx.agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        c.current_dir(&ctx.agent_dir);
        c
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true); // BOT-04: killed on SIGTERM

    let sandboxed = ctx.ssh_config_path.is_some();
    tracing::info!(
        ?chat_id,
        ?eff_thread_id,
        is_first_call,
        sandboxed,
        "invoking claude -p"
    );

    let mut child = cmd
        .spawn()
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;

    // Write input to stdin, then drop to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input.as_bytes()).await
            .map_err(|e| format_error_reply(-1, &format!("stdin write failed: {:#}", e)))?;
    }

    let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let stdout_str = String::from_utf8_lossy(&output.stdout);

    // Always log outcome — debuggability over brevity.
    tracing::info!(
        ?chat_id,
        exit_code,
        stdout_len = stdout_str.len(),
        stderr_len = stderr_str.len(),
        sandboxed,
        "claude -p finished"
    );
    if !stderr_str.is_empty() {
        tracing::warn!(?chat_id, stderr = %stderr_str, "CC stderr");
    }

    // DIS-06: non-zero exit → error reply
    if !output.status.success() {
        // Log full output on failure for debuggability.
        tracing::error!(
            ?chat_id,
            exit_code,
            stdout = %stdout_str.chars().take(1000).collect::<String>(),
            stderr = %stderr_str,
            "claude -p failed"
        );

        // Check for auth error — trigger login flow if sandboxed.
        if is_auth_error(&stdout_str) {
            tracing::warn!(?chat_id, "detected auth error from CC");
            if ctx.ssh_config_path.is_some() {
                // Sandbox mode: spawn auth watcher if not already active.
                if !ctx.auth_watcher_active.swap(true, Ordering::SeqCst) {
                    let tg_chat_id = ctx.chat_id;
                    if let Err(e) = send_tg(
                        &ctx.bot,
                        tg_chat_id,
                        ctx.effective_thread_id,
                        "Claude needs to log in. A login link will be sent shortly...",
                    )
                    .await
                    {
                        tracing::warn!(?chat_id, "failed to send auth error notification: {e:#}");
                    }
                    spawn_auth_watcher(ctx, tg_chat_id, ctx.effective_thread_id);
                    // Return Ok(None) — the initial message above is sufficient,
                    // don't send a second error message before the URL arrives.
                    return Ok(None);
                } else {
                    // Watcher already running — silent, don't spam.
                    return Ok(None);
                }
            } else {
                return Err(
                    "Claude needs to log in. Run `claude` in your terminal to authenticate."
                        .to_string(),
                );
            }
        }

        // Non-auth error: generic error reply.
        let error_detail = if stderr_str.trim().is_empty() && !stdout_str.trim().is_empty() {
            format!(
                "(stderr empty, stdout): {}",
                stdout_str.chars().take(500).collect::<String>()
            )
        } else {
            stderr_str.to_string()
        };
        return Err(format_error_reply(exit_code, &error_detail));
    }

    // DIS-04: parse session_id for debug verification (D-15: mismatch only warns)
    match parse_reply_output(&stdout_str) {
        Ok((reply_output, session_id_from_cc)) => {
            // D-15: verify session_id at debug level only
            if let (Some(cc_sid), true) = (session_id_from_cc, is_first_call)
                && let Ok(Some(stored)) = get_session(&conn, chat_id, eff_thread_id)
                && cc_sid != stored
            {
                tracing::warn!(
                    ?chat_id,
                    cc_session_id = %cc_sid,
                    stored_session_id = %stored,
                    "session_id mismatch between CC and stored — not blocking"
                );
            }
            // Update last_used_at (non-fatal: log error but do not fail the reply)
            touch_session(&conn, chat_id, eff_thread_id)
                .map_err(|e| tracing::error!(?chat_id, "touch_session failed: {:#}", e))
                .ok();
            Ok(Some(reply_output))
        }
        Err(reason) => {
            // D-05: parse failure → error reply
            tracing::warn!(?chat_id, reason, "CC response parse failed");
            Err(format!(
                "⚠️ Agent error: {reason}\nRaw output (truncated): {}",
                &stdout_str.chars().take(200).collect::<String>()
            ))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // split_message tests
    #[test]
    fn split_short_message() {
        let text = "hello world";
        let parts = split_message(text);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], text);
    }

    #[test]
    fn split_at_newline() {
        // Build a 4100-char string with \n at position 4090 (within last 200 chars)
        let mut text = "a".repeat(4090);
        text.push('\n');
        text.push_str(&"b".repeat(9));
        assert!(text.len() > 4096);
        let parts = split_message(&text);
        assert_eq!(parts.len(), 2);
        // First part ends with \n (split at newline boundary)
        assert!(parts[0].ends_with('\n'));
    }

    #[test]
    fn split_hard_cut() {
        // 4200 chars of 'a' — no newlines
        let text = "a".repeat(4200);
        let parts = split_message(&text);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 4096);
        assert_eq!(parts[1].len(), 104);
    }

    // format_error_reply tests
    #[test]
    fn error_reply_contains_exit_code_and_stderr() {
        let reply = format_error_reply(1, "something failed");
        assert!(reply.contains("⚠️ Agent error (exit 1):"));
        assert!(reply.contains("something failed"));
    }

    #[test]
    fn error_reply_truncates_long_stderr() {
        let long_stderr = "y".repeat(500); // use 'y' — no collision with "exit" containing 'x'
        let reply = format_error_reply(2, &long_stderr);
        // The y-block in the reply should not exceed 300 chars of stderr
        let y_block: String = reply.chars().filter(|&c| c == 'y').collect();
        assert_eq!(y_block.len(), 300);
    }

    // parse_reply_output tests (new structured output format per D-03)
    #[test]
    fn parse_reply_output_content_string() {
        let json = r#"{"session_id":"abc","result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_content_null() {
        let json = r#"{"result":{"content":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[test]
    fn parse_reply_output_missing_result_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing both"));
    }

    #[test]
    fn parse_reply_output_reply_to_message_id() {
        let json = r#"{"result":{"content":"hi","reply_to_message_id":42,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.reply_to_message_id, Some(42));
    }

    #[test]
    fn parse_reply_output_plain_string_result_wrapped_as_content() {
        // CC sometimes returns "result": "plain text" after MCP tool use instead of complying
        // with --json-schema. Must deliver the message rather than show an error.
        let json = r#"{"session_id":"abc","result":"hello from plain result"}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello from plain result"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_empty_string_result_is_silent() {
        let json = r#"{"result":""}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[test]
    fn parse_reply_output_array_result_returns_error() {
        // Array instead of object should fail deserialization
        let json = r#"{"result":[{"type":"text"}]}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_reply_output_structured_output_field() {
        // When structured_output is present, it should be used instead of result
        let json = r#"{"session_id":"abc","result":"","structured_output":{"content":"Hello from structured!","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Hello from structured!"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_falls_back_to_result_when_no_structured_output() {
        // When structured_output is absent, fall back to result field
        let json = r#"{"session_id":"xyz","result":{"content":"Fallback result","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Fallback result"));
        assert_eq!(session_id.as_deref(), Some("xyz"));
    }

    #[test]
    fn parse_reply_output_missing_result_and_structured_output_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("missing both"), "error should mention both fields: {err}");
    }

    #[test]
    fn parse_reply_output_with_attachments() {
        let json = r#"{"session_id":"abc","result":{"content":"Here you go","attachments":[{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Exported data"}]}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Here you go"));
        assert_eq!(session_id.as_deref(), Some("abc"));
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].path, "/sandbox/outbox/data.csv");
        assert_eq!(atts[0].filename.as_deref(), Some("results.csv"));
    }

    #[test]
    fn parse_reply_output_text_only() {
        let json = r#"{"result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert!(output.attachments.is_none());
    }

    #[test]
    fn parse_reply_output_plain_string_fallback() {
        let json = r#"{"result":"plain text fallback"}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("plain text fallback"));
        assert!(output.attachments.is_none());
    }

    // wait_with_timeout tests
    #[tokio::test]
    async fn wait_with_timeout_fires_before_slow_process_exits() {
        let child = tokio::process::Command::new("sleep")
            .arg("999")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("sleep should be available");

        let err = wait_with_timeout(child, 1).await.unwrap_err();
        assert!(
            err.contains("timed out after 1s"),
            "expected timeout message, got: {err}"
        );
    }

    // is_auth_error tests
    #[test]
    fn is_auth_error_detects_403() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 403 status code (no body)"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_401() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 401 Unauthorized"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_not_logged_in() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_detects_please_run_login() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Please run /login · API Error: 403"}"#;
        assert!(is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_normal_error() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"result":"Tool execution failed: timeout"}"#;
        assert!(!is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_success() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,"result":{"content":"hello"}}"#;
        assert!(!is_auth_error(stdout));
    }

    #[test]
    fn is_auth_error_false_for_non_json() {
        assert!(!is_auth_error("Not logged in. Run claude auth login."));
    }

    #[test]
    fn is_auth_error_false_for_empty() {
        assert!(!is_auth_error(""));
    }

    // extract_auth_url tests
    #[test]
    fn extract_auth_url_finds_anthropic_url() {
        let lines = vec![
            "Initializing...".to_string(),
            "Open this URL to authenticate: https://console.anthropic.com/oauth/authorize?client_id=abc".to_string(),
            "Waiting for callback...".to_string(),
        ];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("console.anthropic.com"));
    }

    #[test]
    fn extract_auth_url_finds_claude_ai_url() {
        let lines = vec![
            "Please visit: https://claude.ai/oauth/login?token=xyz".to_string(),
        ];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("claude.ai"));
    }

    #[test]
    fn extract_auth_url_finds_claude_com_url() {
        // Real URL from `claude auth login --claudeai` inside sandbox.
        let lines = vec![
            "Opening browser to sign in…\r".to_string(),
            "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=abc".to_string(),
        ];
        let url = extract_auth_url(&lines);
        assert!(url.is_some());
        assert!(url.unwrap().contains("claude.com/cai/oauth/"));
    }

    #[test]
    fn extract_auth_url_returns_none_when_no_url() {
        let lines = vec![
            "Starting up...".to_string(),
            "Checking credentials...".to_string(),
        ];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_ignores_non_auth_urls() {
        let lines = vec![
            "Connecting to https://api.example.com/v1".to_string(),
        ];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_handles_empty() {
        let lines: Vec<String> = vec![];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_ignores_non_oauth_anthropic_url() {
        // The "supported countries" link from error messages must not be picked up.
        let lines = vec![
            "Check supported countries at https://anthropic.com/supported-countries".to_string(),
        ];
        assert!(extract_auth_url(&lines).is_none());
    }

    #[test]
    fn extract_auth_url_extracts_just_url_from_line() {
        let lines = vec![
            "Go to https://console.anthropic.com/oauth/authorize?foo=bar to continue".to_string(),
        ];
        let url = extract_auth_url(&lines).unwrap();
        assert!(url.starts_with("https://"));
        assert!(!url.contains(" to continue"));
    }

    #[tokio::test]
    async fn wait_with_timeout_returns_output_for_fast_process() {
        let child = tokio::process::Command::new("true")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("true should be available");

        let output = wait_with_timeout(child, 5).await.expect("should succeed");
        assert!(output.status.success());
    }
}
