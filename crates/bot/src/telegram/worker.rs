//! Per-session worker task: debounce loop, CC subprocess invocation, reply tool parsing.
//!
//! Pure helpers are tested in isolation (TDD). `spawn_worker` and `invoke_cc` require
//! live infrastructure and are covered by code review pattern only.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

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

#[derive(serde::Deserialize)]
struct CcOutput {
    #[serde(default)]
    session_id: Option<String>,
    result: Option<serde_json::Value>,
    #[serde(default)]
    content: Vec<serde_json::Value>,
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
