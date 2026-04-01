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
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::session::{create_session, get_session, touch_session};

/// Session key: `(chat_id, effective_thread_id)`.
type SessionKey = (i64, i64);

/// Fixed 500ms debounce window (D-01).
const DEBOUNCE_MS: u64 = 500;

/// Reply tool definition injected on every CC invocation (D-03).
///
/// CC must call the `reply` tool — plain text responses fail `parse_reply_tool`.
/// `--append-system-prompt` (inline string) works alongside `--system-prompt-file`.
const REPLY_TOOL_JSON: &str = r#"{"name":"reply","description":"Send a reply to the user or stay silent","input_schema":{"type":"object","properties":{"content":{"type":["string","null"],"description":"Message text. null = silent (no Telegram reply)"},"reply_to_message_id":{"type":["integer","null"],"description":"Telegram message_id to reply to. null = reply to thread only"},"media_paths":{"type":["array","null"],"items":{"type":"string"},"description":"STUB: Phase 25 logs warning, does not send"}},"required":["content"]}}"#;

/// A single Telegram message queued into the debounce channel.
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
    pub bot: super::BotType,
    /// agent_dir — passed separately so worker opens its own Connection
    pub db_path: PathBuf,
}

/// Parsed output from the `reply` tool call in CC JSON response.
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

/// Parse the `reply` tool call from CC JSON output (D-04, D-05).
///
/// Returns `Ok((ReplyOutput, Option<session_id>))` if the reply tool was called.
/// Returns `Err(String)` if no tool call found (triggers error reply per D-05).
/// Returns `Ok((ReplyOutput { content: None, .. }, _))` if content=null (silent response).
pub fn parse_reply_tool(raw_json: &str) -> Result<(ReplyOutput, Option<String>), String> {
    // Log raw output at DEBUG level for format verification (Open Question #1)
    tracing::debug!("CC raw JSON output: {}", raw_json);

    let parsed: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("JSON parse error: {e}"))?;

    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Search for reply tool_use block in both result array and content array
    let tool_input = find_reply_tool_input(&parsed)
        .ok_or_else(|| "CC did not call the reply tool".to_string())?;

    let content = tool_input.get("content").and_then(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().map(|s| s.to_string())
        }
    });

    let reply_to_message_id = tool_input
        .get("reply_to_message_id")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32);

    let media_paths = tool_input
        .get("media_paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        });

    if let Some(ref paths) = media_paths {
        if !paths.is_empty() {
            tracing::warn!("media_paths returned but not yet implemented — skipping");
        }
    }

    Ok((
        ReplyOutput {
            content,
            reply_to_message_id,
            media_paths,
        },
        session_id,
    ))
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
            // Wait for first message in this debounce cycle
            let Some(first) = rx.recv().await else {
                tracing::debug!(?key, "worker channel closed — exiting");
                break;
            };
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
                    // Should not happen (parse_reply_tool returns Err when no tool call)
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

    let system_prompt_append = format!(
        "You MUST respond exclusively by calling the `reply` tool. NEVER output plain text.\nTool definition:\n{}",
        REPLY_TOOL_JSON
    );

    // Build command (DIS-01, DIS-02, DIS-03, D-03, D-13, D-14)
    let system_prompt_path = ctx.agent_dir.join(".claude").join("system-prompt.txt");
    let mut cmd = tokio::process::Command::new(&cc_bin);
    cmd.arg("-p");
    for arg in &cmd_args {
        cmd.arg(arg);
    }
    cmd.arg("--output-format").arg("json");

    // --system-prompt-file only on first call (Phase 24 decision)
    if is_first_call && system_prompt_path.exists() {
        cmd.arg("--system-prompt-file").arg(&system_prompt_path);
    }

    // D-03: inject reply tool definition on every call (first and resume)
    cmd.arg("--append-system-prompt").arg(&system_prompt_append);

    cmd.arg("--").arg(xml);
    cmd.env("HOME", &ctx.agent_dir);
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
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format_error_reply(-1, &format!("wait failed: {:#}", e)))?;

    // DIS-06: non-zero exit or non-empty stderr → error reply
    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format_error_reply(exit_code, &stderr));
    }

    let raw = String::from_utf8_lossy(&output.stdout);

    // DIS-04: parse session_id for debug verification (D-15: mismatch only warns)
    match parse_reply_tool(&raw) {
        Ok((reply_output, session_id_from_cc)) => {
            // D-15: verify session_id at debug level only
            if let (Some(cc_sid), true) = (session_id_from_cc, is_first_call) {
                if let Ok(Some(stored)) = get_session(&conn, chat_id, eff_thread_id) {
                    if cc_sid != stored {
                        tracing::warn!(
                            ?chat_id,
                            cc_session_id = %cc_sid,
                            stored_session_id = %stored,
                            "session_id mismatch between CC and stored — not blocking"
                        );
                    }
                }
            }
            // Update last_used_at (non-fatal: log error but do not fail the reply)
            touch_session(&conn, chat_id, eff_thread_id)
                .map_err(|e| tracing::error!(?chat_id, "touch_session failed: {:#}", e))
                .ok();
            Ok(Some(reply_output))
        }
        Err(reason) => {
            // D-05: no reply tool call → error reply
            tracing::warn!(?chat_id, reason, "CC did not call reply tool");
            Err(format!(
                "⚠️ Agent error: {reason}\nRaw output (truncated): {}",
                &raw.chars().take(200).collect::<String>()
            ))
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn find_reply_tool_input(v: &serde_json::Value) -> Option<&serde_json::Value> {
    // Search in `result` array (CC --output-format json format)
    if let Some(arr) = v.get("result").and_then(|r| r.as_array()) {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && item.get("name").and_then(|n| n.as_str()) == Some("reply")
            {
                return item.get("input");
            }
        }
    }
    // Also check top-level content array (alternate CC output format)
    if let Some(arr) = v.get("content").and_then(|r| r.as_array()) {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && item.get("name").and_then(|n| n.as_str()) == Some("reply")
            {
                return item.get("input");
            }
        }
    }
    None
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

    // parse_reply_tool tests
    #[test]
    fn parse_reply_content_string() {
        let json = r#"{"session_id":"abc","result":[{"type":"tool_use","name":"reply","input":{"content":"hello","reply_to_message_id":null,"media_paths":null}}]}"#;
        let (output, session_id) = parse_reply_tool(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_reply_content_null() {
        let json =
            r#"{"result":[{"type":"tool_use","name":"reply","input":{"content":null}}]}"#;
        let (output, _) = parse_reply_tool(json).unwrap();
        assert!(output.content.is_none());
    }

    #[test]
    fn parse_no_tool_call_returns_error() {
        let json = r#"{"result":[{"type":"text","text":"plain response"}]}"#;
        let result = parse_reply_tool(json);
        assert!(result.is_err());
    }
}
