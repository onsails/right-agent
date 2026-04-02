//! Per-session worker task: debounce loop, CC subprocess invocation, reply tool parsing.
//!
//! Pure helpers are tested in isolation (TDD). `spawn_worker` and `invoke_cc` require
//! live infrastructure and are covered by code review pattern only.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

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
    pub text: String,
    pub timestamp: DateTime<Utc>,
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
}

/// Parsed output from CC structured JSON response (`result` field per D-03).
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    /// STUB: Phase 25 warns and skips
    pub media_paths: Option<Vec<String>>,
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Format a batch of messages as XML per D-02.
///
/// Output:
/// ```xml
/// <messages>
/// <msg id="123" ts="2026-03-31T12:00:00Z" from="user">text</msg>
/// </messages>
/// ```
pub fn format_batch_xml(msgs: &[DebounceMsg]) -> String {
    let mut out = String::from("<messages>\n");
    for m in msgs {
        let escaped = m
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        out.push_str(&format!(
            "<msg id=\"{}\" ts=\"{}\" from=\"user\">{}</msg>\n",
            m.message_id,
            m.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
            escaped,
        ));
    }
    out.push_str("</messages>");
    out
}

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

    let output: ReplyOutput = serde_json::from_value(result_val.clone())
        .map_err(|e| format!("failed to deserialize result: {e}"))?;

    if let Some(ref paths) = output.media_paths
        && !paths.is_empty()
    {
        tracing::warn!("media_paths returned but not yet implemented -- skipping");
    }

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

            // Build XML batch (D-02)
            let xml = format_batch_xml(&batch);

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
            let reply_result = invoke_cc(&xml, chat_id, eff_thread_id, &ctx).await;

            // Cancel typing indicator
            cancel_token.cancel();
            typing_task.await.ok();

            // Send reply (D-04, D-05, DIS-05, DIS-06)
            match reply_result {
                Ok(Some(output)) => {
                    if let Some(content) = output.content {
                        // Split long responses (DIS-05)
                        let parts = split_message(&content);
                        for part in parts {
                            let mut send = ctx.bot.send_message(tg_chat_id, &part);
                            if eff_thread_id != 0 {
                                send = send.message_thread_id(ThreadId(MessageId(
                                    eff_thread_id as i32,
                                )));
                            }
                            if let Some(ref_id) = output.reply_to_message_id {
                                send = send.reply_parameters(ReplyParameters {
                                    message_id: MessageId(ref_id),
                                    ..Default::default()
                                });
                            }
                            // No parse_mode — send as plain text (Pitfall 6)
                            if let Err(e) = send.await {
                                tracing::error!(
                                    ?key,
                                    "failed to send Telegram reply: {:#}",
                                    e
                                );
                            }
                        }
                    }
                    // content: None → silent (D-05)
                }
                Ok(None) => {
                    // Should not happen (parse_reply_output returns Err on missing result)
                    tracing::warn!(?key, "unexpected Ok(None) from invoke_cc");
                }
                Err(err_msg) => {
                    // DIS-06: error reply
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

/// Invoke `claude -p` and parse the reply tool call from its JSON output.
///
/// Returns `Ok(Some(ReplyOutput))` on success,
/// `Err(error_message_for_telegram)` on subprocess failure or missing reply tool.
async fn invoke_cc(
    xml: &str,
    chat_id: i64,
    eff_thread_id: i64,
    ctx: &WorkerContext,
) -> Result<Option<ReplyOutput>, String> {
    // Resolve CC binary (D-12)
    let cc_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| "⚠️ Agent error: claude binary not found in PATH".to_string())?;

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

    // Build command (AGDEF-02, AGDEF-03, D-01, D-13, D-14)
    let reply_schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let mut cmd = tokio::process::Command::new(&cc_bin);
    cmd.arg("-p");
    cmd.arg("--dangerously-skip-permissions");
    if ctx.debug {
        cmd.arg("--verbose");
    }
    for arg in &cmd_args {
        cmd.arg(arg);
    }
    cmd.arg("--output-format").arg("json");

    // --agent only on first call (AGDEF-02); resume inherits from session (AGDEF-03)
    if is_first_call {
        cmd.arg("--agent").arg(&ctx.agent_name);
    }

    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4)
    // CC expects inline JSON string, NOT a file path — read and inline the content
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("reply-schema.json read failed: {:#}", e)))?;
    cmd.arg("--json-schema").arg(&reply_schema);

    cmd.arg("--").arg(xml);
    cmd.env("HOME", &ctx.agent_dir);
    // Use system rg instead of CC's bundled vendor binary (nix store rg lacks execute bit).
    cmd.env("USE_BUILTIN_RIPGREP", "1");
    cmd.current_dir(&ctx.agent_dir);
    cmd.stdin(Stdio::null()); // DIS-02: prevent pipe deadlock
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true); // BOT-04: killed on SIGTERM

    tracing::info!(
        ?chat_id,
        ?eff_thread_id,
        is_first_call,
        "invoking claude -p"
    );

    let child = cmd
        .spawn()
        .map_err(|e| format_error_reply(-1, &format!("spawn failed: {:#}", e)))?;

    // DIS-02: always wait_with_output, never .wait()
    let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;

    // DIS-06: non-zero exit or non-empty stderr → error reply
    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format_error_reply(exit_code, &stderr));
    }

    if ctx.debug {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if !stderr_str.is_empty() {
            tracing::info!(?chat_id, stderr = %stderr_str, "CC stderr");
        }
    }

    let raw = String::from_utf8_lossy(&output.stdout);

    // DIS-04: parse session_id for debug verification (D-15: mismatch only warns)
    match parse_reply_output(&raw) {
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
                &raw.chars().take(200).collect::<String>()
            ))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_msg(id: i32, text: &str) -> DebounceMsg {
        DebounceMsg {
            message_id: id,
            text: text.to_string(),
            timestamp: Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0).unwrap(),
        }
    }

    // format_batch_xml tests
    #[test]
    fn batch_xml_single_message() {
        let msgs = [make_msg(100, "hello")];
        let xml = format_batch_xml(&msgs);
        assert!(xml.contains("<messages>"));
        assert!(xml.contains(r#"id="100""#));
        assert!(xml.contains("hello"));
        assert!(xml.contains("</messages>"));
    }

    #[test]
    fn batch_xml_multi_message() {
        let msgs = [make_msg(100, "first"), make_msg(101, "second")];
        let xml = format_batch_xml(&msgs);
        assert!(xml.contains(r#"id="100""#));
        assert!(xml.contains(r#"id="101""#));
        // order preserved
        let pos100 = xml.find(r#"id="100""#).unwrap();
        let pos101 = xml.find(r#"id="101""#).unwrap();
        assert!(pos100 < pos101);
    }

    #[test]
    fn batch_xml_escapes_special_chars() {
        let msgs = [make_msg(1, "<b> & 'test'")];
        let xml = format_batch_xml(&msgs);
        assert!(xml.contains("&lt;b&gt;"));
        assert!(xml.contains("&amp;"));
        assert!(!xml.contains("<b>"));
    }

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
        let json = r#"{"session_id":"abc","result":{"content":"hello","reply_to_message_id":null,"media_paths":null}}"#;
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
        let json = r#"{"result":{"content":"hi","reply_to_message_id":42,"media_paths":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.reply_to_message_id, Some(42));
    }

    #[test]
    fn parse_reply_output_media_paths() {
        let json = r#"{"result":{"content":"hi","reply_to_message_id":null,"media_paths":["a.png"]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        let paths = output.media_paths.unwrap();
        assert_eq!(paths, vec!["a.png".to_string()]);
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
        let json = r#"{"session_id":"abc","result":"","structured_output":{"content":"Hello from structured!","reply_to_message_id":null,"media_paths":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Hello from structured!"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_output_falls_back_to_result_when_no_structured_output() {
        // When structured_output is absent, fall back to result field
        let json = r#"{"session_id":"xyz","result":{"content":"Fallback result","reply_to_message_id":null,"media_paths":null}}"#;
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
