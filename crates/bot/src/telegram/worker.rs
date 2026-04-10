//! Per-session worker task: debounce loop, CC subprocess invocation, reply tool parsing.
//!
//! Pure helpers are tested in isolation (TDD). `spawn_worker` and `invoke_cc` require
//! live infrastructure and are covered by code review pattern only.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, MessageId, ReplyParameters, ThreadId};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::session::{create_session, delete_session, get_session, touch_session};

/// Session key: `(chat_id, effective_thread_id)`.
pub type SessionKey = (i64, i64);

/// Fixed 500ms debounce window (D-01).
const DEBOUNCE_MS: u64 = 500;

/// Maximum time to wait for a CC subprocess to complete.
const CC_TIMEOUT_SECS: u64 = 600;

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
    /// Max CC turns per invocation (passed as --max-turns).
    pub max_turns: u32,
    /// Max dollar spend per CC invocation (passed as --max-budget-usd).
    pub max_budget_usd: f64,
    /// Show live thinking indicator in Telegram.
    pub show_thinking: bool,
}

/// Parsed output from CC structured JSON response (`result` field per D-03).
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
    /// Bootstrap mode: `true` signals agent claims onboarding is complete.
    /// Server-side file check (`should_accept_bootstrap`) gates actual completion.
    pub bootstrap_complete: Option<bool>,
}

/// Required identity files that must exist for bootstrap to be accepted as complete.
const BOOTSTRAP_REQUIRED_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "USER.md"];

/// Check whether bootstrap completion should be accepted.
///
/// Returns `true` only when all required identity files exist in `agent_dir`.
/// If any are missing, the agent didn't actually complete the onboarding flow
/// and bootstrap mode should continue.
fn should_accept_bootstrap(agent_dir: &Path) -> bool {
    BOOTSTRAP_REQUIRED_FILES
        .iter()
        .all(|f| agent_dir.join(f).exists())
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Strip HTML tags for plain-text fallback when Telegram rejects HTML.
/// Also decodes common entities back to their characters.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
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
            bootstrap_complete: None,
        }
    } else {
        serde_json::from_value(result_val.clone())
            .map_err(|e| format!("failed to deserialize result: {e}"))?
    };

    Ok((output, session_id))
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
            let mut skip_batch = false;
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
                            let _ = send_tg(&ctx.bot, tg_chat_id, eff_thread_id, &format!("⚠️ Failed to download attachments: {e:#}\nYour message was not forwarded.")).await;
                            skip_batch = true;
                            break;
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
            if skip_batch {
                continue;
            }

            let Some(input) = super::attachments::format_cc_input(&input_messages) else {
                tracing::warn!(?key, "empty input after formatting -- skipping CC invocation");
                continue;
            };

            // Typing indicator: always active until reply is sent (D-10).
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
                    action.await.ok();
                    tokio::select! {
                        _ = cancel_clone.cancelled() => break,
                        _ = sleep(Duration::from_secs(4)) => {}
                    }
                }
            });

            // Invoke claude -p (D-13, D-14)
            let reply_result = invoke_cc(&input, chat_id, eff_thread_id, &ctx).await;

            // Reverse sync .md changes from sandbox.
            // Bootstrap mode: BLOCK so files are on host for completion check.
            // Normal mode: fire-and-forget, don't delay reply.
            let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
            if ctx.ssh_config_path.is_some() {
                let sandbox = rightclaw::openshell::sandbox_name(&ctx.agent_name);
                if bootstrap_mode {
                    if let Err(e) =
                        crate::sync::reverse_sync_md(&ctx.agent_dir, &sandbox).await
                    {
                        tracing::warn!(
                            agent = %ctx.agent_name,
                            "bootstrap reverse sync failed: {e:#}"
                        );
                    }
                } else {
                    let agent_dir = ctx.agent_dir.clone();
                    let agent_name = ctx.agent_name.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            crate::sync::reverse_sync_md(&agent_dir, &sandbox).await
                        {
                            tracing::warn!(agent = %agent_name, "reverse sync failed: {e:#}");
                        }
                    });
                }
            }

            // Bootstrap completion: check if identity files are now on host after sync.
            // MCP tool bootstrap_done may have already deleted BOOTSTRAP.md, but
            // we also check here as a safety net (handles no-sandbox mode too).
            let bootstrap_signaled = matches!(
                &reply_result,
                Ok(Some(output)) if output.bootstrap_complete == Some(true)
            );
            if bootstrap_mode && bootstrap_signaled && should_accept_bootstrap(&ctx.agent_dir) {
                tracing::info!(
                    key = ?key,
                    "bootstrap complete — identity files present after sync"
                );
                // Open a short-lived connection to delete the session.
                if let Ok(conn) = rightclaw::memory::open_connection(&ctx.agent_dir) {
                    delete_session(&conn, chat_id, eff_thread_id)
                        .map_err(|e| {
                            tracing::error!(
                                key = ?key,
                                "delete_session after bootstrap: {:#}",
                                e
                            )
                        })
                        .ok();
                }
                // BOOTSTRAP.md may already be deleted by MCP tool; ensure cleanup.
                let bp = ctx.agent_dir.join("BOOTSTRAP.md");
                if bp.exists()
                    && let Err(e) = std::fs::remove_file(&bp)
                {
                    tracing::warn!(key = ?key, "failed to delete BOOTSTRAP.md: {e:#}");
                }
            }

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
                        let html = super::markdown::md_to_telegram_html(&content);
                        let parts = super::markdown::split_html_message(&html);
                        tracing::info!(
                            ?key,
                            content_len = content.len(),
                            html_len = html.len(),
                            parts = parts.len(),
                            ?reply_to,
                            "sending reply to Telegram"
                        );
                        for part in &parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, part);
                            send = send.parse_mode(teloxide::types::ParseMode::Html);
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
                                tracing::warn!(?key, "HTML send failed, retrying plain text: {:#}", e);
                                let plain = strip_html_tags(part);
                                let mut fallback = ctx.bot.send_message(tg_chat_id, &plain);
                                if eff_thread_id != 0 {
                                    fallback = fallback.message_thread_id(ThreadId(MessageId(
                                        eff_thread_id as i32,
                                    )));
                                }
                                if let Some(ref_id) = reply_to {
                                    fallback = fallback.reply_parameters(ReplyParameters {
                                        message_id: MessageId(ref_id),
                                        ..Default::default()
                                    });
                                }
                                if let Err(e2) = fallback.await {
                                    tracing::error!(?key, "plain text fallback also failed: {:#}", e2);
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            ?key,
                            "CC returned content: null -- no text reply sent"
                        );
                    }

                    // Send outbound attachments
                    #[allow(clippy::collapsible_if)]
                    if let Some(ref atts) = output.attachments
                        && !atts.is_empty()
                    {
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

/// Generate a shell script that assembles a composite system prompt from sandbox files.
///
/// The script concatenates base identity + framed content files into a temp file,
/// then runs `claude -p` with `--system-prompt-file` pointing to it.
/// This runs as a single SSH command — no extra roundtrips, always fresh files.
fn build_sandbox_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
    claude_args: &[String],
) -> String {
    let escaped_base = base_prompt.replace('\'', "'\\''");
    let escaped_args: Vec<String> = claude_args.iter().map(|a| shell_escape(a)).collect();
    let claude_cmd = escaped_args.join(" ");

    let file_sections = if bootstrap_mode {
        r#"
if [ -f /sandbox/.claude/agents/BOOTSTRAP.md ]; then
  printf '\n## Bootstrap Instructions\n'
  cat /sandbox/.claude/agents/BOOTSTRAP.md
  printf '\n'
fi"#
    } else {
        r#"
if [ -f /sandbox/IDENTITY.md ]; then
  printf '\n## Your Identity\n'
  cat /sandbox/IDENTITY.md
  printf '\n'
fi
if [ -f /sandbox/SOUL.md ]; then
  printf '\n## Your Personality and Values\n'
  cat /sandbox/SOUL.md
  printf '\n'
fi
if [ -f /sandbox/USER.md ]; then
  printf '\n## Your User\n'
  cat /sandbox/USER.md
  printf '\n'
fi
if [ -f /sandbox/.claude/agents/AGENTS.md ]; then
  printf '\n## Operating Instructions\n'
  cat /sandbox/.claude/agents/AGENTS.md
  printf '\n'
fi
if [ -f /sandbox/.claude/agents/TOOLS.md ]; then
  printf '\n## Environment and Tools\n'
  cat /sandbox/.claude/agents/TOOLS.md
  printf '\n'
fi"#
    };

    format!(
        "{{ printf '{escaped_base}'\n{file_sections}\n}} > /tmp/rightclaw-system-prompt.md\ncd /sandbox && {claude_cmd} --system-prompt-file /tmp/rightclaw-system-prompt.md"
    )
}

/// Assemble a composite system prompt from host-side files.
///
/// Used in no-sandbox mode where files are directly accessible.
fn assemble_host_system_prompt(
    base_prompt: &str,
    bootstrap_mode: bool,
    agent_dir: &Path,
) -> String {
    let mut prompt = base_prompt.to_string();

    if bootstrap_mode {
        if let Ok(content) = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")) {
            prompt.push_str("\n## Bootstrap Instructions\n");
            prompt.push_str(&content);
            prompt.push('\n');
        }
    } else {
        let sections: &[(&str, &str)] = &[
            ("IDENTITY.md", "## Your Identity"),
            ("SOUL.md", "## Your Personality and Values"),
            ("USER.md", "## Your User"),
        ];
        for (file, header) in sections {
            if let Ok(content) = std::fs::read_to_string(agent_dir.join(file)) {
                prompt.push_str(&format!("\n{header}\n"));
                prompt.push_str(&content);
                prompt.push('\n');
            }
        }
        // AGENTS.md and TOOLS.md are in .claude/agents/
        let agents_subdir: &[(&str, &str)] = &[
            ("AGENTS.md", "## Operating Instructions"),
            ("TOOLS.md", "## Environment and Tools"),
        ];
        for (file, header) in agents_subdir {
            let path = agent_dir.join(".claude").join("agents").join(file);
            if let Ok(content) = std::fs::read_to_string(path) {
                prompt.push_str(&format!("\n{header}\n"));
                prompt.push_str(&content);
                prompt.push('\n');
            }
        }
    }

    prompt
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

    // Bootstrap mode detection: check if BOOTSTRAP.md exists in agent dir.
    let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
    if bootstrap_mode {
        tracing::info!(?chat_id, "bootstrap mode: BOOTSTRAP.md present");
    }

    // Build claude -p args for execution inside OpenShell sandbox
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

    for arg in &cmd_args {
        claude_args.push(arg.clone());
    }
    claude_args.push("--verbose".into());
    claude_args.push("--output-format".into());
    claude_args.push("stream-json".into());
    claude_args.push("--max-turns".into());
    claude_args.push(ctx.max_turns.to_string());
    claude_args.push("--max-budget-usd".into());
    claude_args.push(format!("{:.2}", ctx.max_budget_usd));

    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4).
    // Bootstrap mode uses bootstrap-schema (adds bootstrap_complete field).
    let schema_filename = if bootstrap_mode {
        "bootstrap-schema.json"
    } else {
        "reply-schema.json"
    };
    let reply_schema_path = ctx.agent_dir.join(".claude").join(schema_filename);
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("{schema_filename} read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);

    // Generate base system prompt (identity-neutral — no agent name to avoid
    // contradicting IDENTITY.md which the agent may have customized).
    let base_prompt = rightclaw::codegen::generate_system_prompt(
        &ctx.agent_name,
        &if ctx.ssh_config_path.is_some() {
            rightclaw::agent::types::SandboxMode::Openshell
        } else {
            rightclaw::agent::types::SandboxMode::None
        },
    );

    let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
        // OpenShell sandbox: composite system prompt assembled IN the sandbox
        // from fresh files — single SSH command, no extra roundtrips.
        let ssh_host = rightclaw::openshell::ssh_host(&ctx.agent_name);
        let assembly_script =
            build_sandbox_prompt_assembly_script(&base_prompt, bootstrap_mode, &claude_args);
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        // Direct exec (no sandbox): assemble prompt on host from local files.
        let composite = assemble_host_system_prompt(
            &base_prompt,
            bootstrap_mode,
            &ctx.agent_dir,
        );
        // Write composite prompt to temp file in agent dir.
        let prompt_path = ctx.agent_dir.join(".claude").join("composite-system-prompt.md");
        std::fs::write(&prompt_path, &composite).map_err(|e| {
            format_error_reply(-1, &format!("failed to write composite system prompt: {e:#}"))
        })?;

        let cc_bin = which::which("claude")
            .or_else(|_| which::which("claude-bun"))
            .map_err(|_| "⚠️ Agent error: claude binary not found in PATH".to_string())?;
        let mut c = tokio::process::Command::new(&cc_bin);
        // Skip "claude" (first element in claude_args) — it's the binary name for SSH mode.
        for arg in &claude_args[1..] {
            c.arg(arg);
        }
        c.arg("--system-prompt-file");
        c.arg(&prompt_path);
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
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format_error_reply(-1, &format!("stdin write failed: {:#}", e)))?;
    }

    // Stream stdout line-by-line: log to file, parse events, update thinking message.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format_error_reply(-1, "no stdout handle"))?;

    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stdout).lines();

    // Per-session stream log file.
    let stream_log_dir = ctx
        .agent_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&ctx.agent_dir)
        .join("logs")
        .join("streams");
    std::fs::create_dir_all(&stream_log_dir).ok();
    let session_id_for_log = cmd_args
        .first()
        .filter(|a| a.contains('-') && a.len() > 30)
        .or(cmd_args.get(1))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let stream_log_path = stream_log_dir.join(format!("{session_id_for_log}.ndjson"));
    let mut stream_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stream_log_path)
        .ok();

    let mut ring_buffer = super::stream::EventRingBuffer::new(5);
    let mut usage = super::stream::StreamUsage::default();
    let mut result_line: Option<String> = None;
    let mut thinking_msg_id: Option<teloxide::types::MessageId> = None;
    let mut last_edit = tokio::time::Instant::now();
    let mut total_assistant_events: u32 = 0;
    let tg_chat_id = ctx.chat_id;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(CC_TIMEOUT_SECS);
    let mut timed_out = false;

    loop {
        tokio::select! {
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        // Write to stream log file.
                        if let Some(ref mut log) = stream_log {
                            use std::io::Write;
                            let _ = writeln!(log, "{line}");
                        }

                        let event = super::stream::parse_stream_event(&line);

                        match &event {
                            super::stream::StreamEvent::Result(json) => {
                                usage = super::stream::parse_usage(json);
                                result_line = Some(json.clone());
                                // Final update of thinking message with real cost.
                                if let Some(msg_id) = thinking_msg_id {
                                    let text = super::stream::format_thinking_message(
                                        ring_buffer.events(),
                                        &usage,
                                        ctx.max_turns,
                                    );
                                    let _ = ctx
                                        .bot
                                        .edit_message_text(tg_chat_id, msg_id, &text)
                                        .parse_mode(teloxide::types::ParseMode::Html)
                                        .await;
                                }
                            }
                            _ => {
                                if let Some(formatted) = super::stream::format_event(&event) {
                                    total_assistant_events += 1;
                                    tracing::info!(?chat_id, turn = total_assistant_events, "{formatted}");
                                }
                                ring_buffer.push(&event);
                                // Update turn count from assistant events.
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                                    && v.pointer("/message/usage/output_tokens").is_some()
                                {
                                    usage.num_turns = usage.num_turns.max(1);
                                }
                            }
                        }

                        // Update thinking message (throttled to 2s).
                        // Only show after 2+ displayable events — single-turn responses
                        // don't need a thinking indicator.
                        if ctx.show_thinking
                            && total_assistant_events >= 2
                            && super::stream::format_event(&event).is_some()
                            && last_edit.elapsed() >= Duration::from_secs(2)
                        {
                            let text = super::stream::format_thinking_message(
                                ring_buffer.events(),
                                &usage,
                                ctx.max_turns,
                            );
                            if let Some(msg_id) = thinking_msg_id {
                                let _ = ctx
                                    .bot
                                    .edit_message_text(tg_chat_id, msg_id, &text)
                                    .parse_mode(teloxide::types::ParseMode::Html)
                                    .await;
                            } else {
                                let mut send = ctx.bot.send_message(tg_chat_id, &text)
                                    .parse_mode(teloxide::types::ParseMode::Html);
                                if eff_thread_id != 0 {
                                    send = send.message_thread_id(
                                        teloxide::types::ThreadId(
                                            teloxide::types::MessageId(eff_thread_id as i32),
                                        ),
                                    );
                                }
                                if let Ok(msg) = send.await {
                                    thinking_msg_id = Some(msg.id);
                                }
                            }
                            last_edit = tokio::time::Instant::now();
                        }
                    }
                    Ok(None) => break, // stdout closed — process exited
                    Err(e) => {
                        tracing::warn!(?chat_id, "stream read error: {e:#}");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                child.kill().await.ok();
                break;
            }
        }
    }

    // If thinking message was sent but only 1 turn happened, delete it
    // (single-turn responses don't need visible thinking).
    if let Some(msg_id) = thinking_msg_id
        && usage.num_turns <= 1
    {
        let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
    }

    // Wait for process exit.
    let exit_status = child.wait().await.ok();
    let exit_code = exit_status.and_then(|s| s.code()).unwrap_or(-1);

    // Read any remaining stderr.
    let stderr_str = if let Some(mut stderr) = child.stderr.take() {
        let mut buf = String::new();
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    } else {
        String::new()
    };

    tracing::info!(
        ?chat_id,
        exit_code,
        timed_out,
        stream_log = %stream_log_path.display(),
        sandboxed,
        "claude -p finished"
    );

    if !stderr_str.is_empty() {
        tracing::warn!(?chat_id, stderr = %stderr_str, "CC stderr");
    }

    // Handle timeout.
    if timed_out {
        let mut timeout_msg = format!(
            "⚠️ Agent timed out ({CC_TIMEOUT_SECS}s safety limit). Last activity:\n─────────────\n"
        );
        for event in ring_buffer.events() {
            if let Some(formatted) = super::stream::format_event(event) {
                timeout_msg.push_str(&formatted);
                timeout_msg.push('\n');
            }
        }
        timeout_msg.push_str(&format!("\nStream log: {}", stream_log_path.display()));
        return Err(timeout_msg);
    }

    let stdout_str = result_line.unwrap_or_default();

    // DIS-06: non-zero exit → error reply
    if exit_code != 0 {
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
            // Delete the session created before invoke_cc — it's from a failed auth
            // attempt and must not be resumed. Next message will start fresh.
            delete_session(&conn, chat_id, eff_thread_id).ok();
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

            // Bootstrap completion is now detected by file presence after
            // reverse_sync in spawn_worker — no bootstrap_complete field needed.

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


    // bootstrap mode tests
    #[test]
    fn parse_reply_output_bootstrap_complete_true() {
        let json = r#"{"type":"result","result":{"content":"Done!","bootstrap_complete":true},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Done!"));
        assert_eq!(output.bootstrap_complete, Some(true));
    }

    #[test]
    fn parse_reply_output_bootstrap_complete_false() {
        let json = r#"{"type":"result","result":{"content":"What's your name?","bootstrap_complete":false},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, Some(false));
    }

    #[test]
    fn parse_reply_output_no_bootstrap_field() {
        let json = r#"{"type":"result","result":{"content":"Hello!"},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, None);
    }

    #[test]
    fn should_accept_bootstrap_all_files_present() {
        let dir = tempfile::tempdir().unwrap();
        for f in BOOTSTRAP_REQUIRED_FILES {
            std::fs::write(dir.path().join(f), "# test").unwrap();
        }
        assert!(should_accept_bootstrap(dir.path()));
    }

    #[test]
    fn should_accept_bootstrap_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // No identity files created
        assert!(!should_accept_bootstrap(dir.path()));
    }

    #[test]
    fn should_accept_bootstrap_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        // Only IDENTITY.md exists
        std::fs::write(dir.path().join("IDENTITY.md"), "# test").unwrap();
        assert!(!should_accept_bootstrap(dir.path()));
    }

    // ── Prompt assembly tests ────────────────────────────────────────────────

    #[test]
    fn sandbox_script_bootstrap_includes_bootstrap_md() {
        let script = build_sandbox_prompt_assembly_script(
            "Base prompt",
            true,
            &["claude".into(), "-p".into()],
        );
        assert!(script.contains("BOOTSTRAP.md"), "must reference BOOTSTRAP.md");
        assert!(!script.contains("IDENTITY.md"), "bootstrap must not include IDENTITY.md");
        assert!(!script.contains("SOUL.md"), "bootstrap must not include SOUL.md");
        assert!(script.contains("claude"), "must contain claude command");
        assert!(script.contains("--system-prompt-file"), "must pass --system-prompt-file");
    }

    #[test]
    fn sandbox_script_normal_includes_all_identity_files() {
        let script = build_sandbox_prompt_assembly_script(
            "Base prompt",
            false,
            &["claude".into(), "-p".into()],
        );
        assert!(script.contains("IDENTITY.md"));
        assert!(script.contains("SOUL.md"));
        assert!(script.contains("USER.md"));
        assert!(script.contains("AGENTS.md"));
        assert!(script.contains("TOOLS.md"));
        assert!(!script.contains("BOOTSTRAP.md"), "normal must not include BOOTSTRAP.md");
    }

    #[test]
    fn sandbox_script_escapes_single_quotes_in_base() {
        let script = build_sandbox_prompt_assembly_script(
            "It's a test",
            true,
            &["claude".into()],
        );
        // Single quote must be escaped for shell: ' → '\''
        assert!(!script.contains("It's"), "raw single quote must be escaped");
        assert!(script.contains("It"), "content must still be present");
    }

    #[test]
    fn sandbox_script_shell_escapes_claude_args() {
        let script = build_sandbox_prompt_assembly_script(
            "Base",
            false,
            &["claude".into(), "-p".into(), "--json-schema".into(), r#"{"type":"object"}"#.into()],
        );
        // JSON with braces and quotes must be shell-escaped
        assert!(script.contains("--json-schema"));
        assert!(script.contains("type"));
    }

    #[test]
    fn sandbox_script_writes_to_tmp_and_uses_system_prompt_file() {
        let script = build_sandbox_prompt_assembly_script("X", false, &["claude".into()]);
        assert!(script.contains("/tmp/rightclaw-system-prompt.md"));
        assert!(script.contains("--system-prompt-file /tmp/rightclaw-system-prompt.md"));
    }

    #[test]
    fn host_prompt_bootstrap_includes_bootstrap_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("BOOTSTRAP.md"), "# Onboarding").unwrap();

        let result = assemble_host_system_prompt("Base\n", true, dir.path());
        assert!(result.contains("Base"));
        assert!(result.contains("## Bootstrap Instructions"));
        assert!(result.contains("# Onboarding"));
    }

    #[test]
    fn host_prompt_normal_includes_identity_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IDENTITY.md"), "I am Spark").unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "Snarky").unwrap();
        std::fs::write(dir.path().join("USER.md"), "Andrey").unwrap();
        let agents_dir = dir.path().join(".claude").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("AGENTS.md"), "Procedures").unwrap();
        std::fs::write(agents_dir.join("TOOLS.md"), "outbox: /sandbox/outbox/").unwrap();

        let result = assemble_host_system_prompt("Base\n", false, dir.path());
        assert!(result.contains("## Your Identity"));
        assert!(result.contains("I am Spark"));
        assert!(result.contains("## Your Personality and Values"));
        assert!(result.contains("Snarky"));
        assert!(result.contains("## Your User"));
        assert!(result.contains("Andrey"));
        assert!(result.contains("## Operating Instructions"));
        assert!(result.contains("Procedures"));
        assert!(result.contains("## Environment and Tools"));
        assert!(result.contains("outbox: /sandbox/outbox/"));
    }

    #[test]
    fn host_prompt_normal_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // No identity files — only AGENTS.md
        let agents_dir = dir.path().join(".claude").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("AGENTS.md"), "Procedures").unwrap();

        let result = assemble_host_system_prompt("Base\n", false, dir.path());
        assert!(result.contains("Base"));
        assert!(result.contains("Procedures"));
        assert!(!result.contains("## Your Identity"), "missing file must be skipped");
        assert!(!result.contains("## Your User"), "missing file must be skipped");
    }

    #[test]
    fn host_prompt_bootstrap_skips_missing_bootstrap() {
        let dir = tempfile::tempdir().unwrap();
        // No BOOTSTRAP.md
        let result = assemble_host_system_prompt("Base\n", true, dir.path());
        assert_eq!(result, "Base\n");
    }
}
