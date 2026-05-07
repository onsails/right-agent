//! Stream event parsing, formatting, and ring buffer for CC stream-json output.

use std::collections::VecDeque;

use right_agent::usage::UsageBreakdown;

/// A parsed stream event from CC's stream-json output.
#[derive(Debug, Clone)]
pub(crate) enum StreamEvent {
    /// Model text output
    Text(String),
    /// Model thinking
    Thinking,
    /// Tool use: tool name + truncated input
    ToolUse { tool: String, input_summary: String },
    /// Final result line (raw JSON)
    Result(String),
    /// System init or other (ignored for display)
    Other,
}

/// Usage info extracted from stream events.
#[derive(Debug, Default, Clone)]
pub(crate) struct StreamUsage {
    pub num_turns: u32,
    pub cost_usd: f64,
}

/// Parse a single NDJSON line from CC stream-json output.
pub(crate) fn parse_stream_event(line: &str) -> StreamEvent {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return StreamEvent::Other;
    };

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "result" => StreamEvent::Result(line.to_string()),
        "assistant" => {
            let content = v.pointer("/message/content").and_then(|c| c.as_array());
            if let Some(blocks) = content {
                for block in blocks {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                return StreamEvent::Text(text.to_string());
                            }
                        }
                        "thinking" => return StreamEvent::Thinking,
                        "tool_use" => {
                            let tool = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                            let input = block.get("input").unwrap_or(&serde_json::Value::Null);
                            let summary = summarize_tool_input(tool, input);
                            return StreamEvent::ToolUse {
                                tool: tool.to_string(),
                                input_summary: summary,
                            };
                        }
                        _ => {}
                    }
                }
            }
            StreamEvent::Other
        }
        _ => StreamEvent::Other,
    }
}

/// Extract usage info from a result event JSON.
pub(crate) fn parse_usage(result_json: &str) -> StreamUsage {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(result_json) else {
        return StreamUsage::default();
    };
    StreamUsage {
        num_turns: v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        cost_usd: v
            .get("total_cost_usd")
            .and_then(|n| n.as_f64())
            .unwrap_or(0.0),
    }
}

/// Parse the full `result` event JSON into `UsageBreakdown`. Returns `None` if
/// required fields (`total_cost_usd`, `num_turns`, `session_id`) are missing or
/// the JSON is malformed. The `modelUsage` object is preserved as a JSON string
/// for per-model reduction at read time.
pub(crate) fn parse_usage_full(result_json: &str) -> Option<UsageBreakdown> {
    let v: serde_json::Value = serde_json::from_str(result_json).ok()?;

    let total_cost_usd = v.get("total_cost_usd")?.as_f64()?;
    let num_turns = u32::try_from(v.get("num_turns")?.as_u64()?).ok()?;
    let session_uuid = v.get("session_id")?.as_str()?.to_string();

    let get_u64 = |ptr: &str| -> u64 { v.pointer(ptr).and_then(|n| n.as_u64()).unwrap_or(0) };

    let model_usage_json = v
        .get("modelUsage")
        .map(|m| m.to_string())
        .unwrap_or_else(|| "{}".to_string());

    Some(UsageBreakdown {
        session_uuid,
        total_cost_usd,
        num_turns,
        input_tokens: get_u64("/usage/input_tokens"),
        output_tokens: get_u64("/usage/output_tokens"),
        cache_creation_tokens: get_u64("/usage/cache_creation_input_tokens"),
        cache_read_tokens: get_u64("/usage/cache_read_input_tokens"),
        web_search_requests: get_u64("/usage/server_tool_use/web_search_requests"),
        web_fetch_requests: get_u64("/usage/server_tool_use/web_fetch_requests"),
        model_usage_json,
        api_key_source: "none".into(),
    })
}

/// Parse `apiKeySource` from the CC `system/init` NDJSON line.
///
/// Returns `None` when:
/// - line is not valid JSON
/// - `type` is not `"system"` or `subtype` is not `"init"`
/// - `apiKeySource` key is absent
///
/// Callers fall back to `"none"` (subscription) if `None` is returned —
/// matching the column default in the `usage_events` table.
pub(crate) fn parse_api_key_source(init_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(init_json).ok()?;
    if v.get("type")?.as_str()? != "system" {
        return None;
    }
    if v.get("subtype")?.as_str()? != "init" {
        return None;
    }
    v.get("apiKeySource")?.as_str().map(|s| s.to_string())
}

/// Format a single event for Telegram display (HTML mode).
///
/// All dynamic content is HTML-escaped for safe use with ParseMode::Html.
pub(crate) fn format_event(event: &StreamEvent) -> Option<String> {
    match event {
        StreamEvent::Text(t) => {
            // Truncate long text — thinking indicator is a preview, not the full reply.
            let preview = truncate_str(t, 150);
            let escaped = crate::cc::markdown_utils::html_escape(&preview);
            Some(format!("\u{1f4dd} \"{escaped}\""))
        }
        StreamEvent::Thinking => Some("\u{1f4ad} thinking...".to_string()),
        StreamEvent::ToolUse {
            tool,
            input_summary,
        } => {
            // StructuredOutput is the final reply JSON — it will be sent as a
            // separate Telegram message, so showing it in the thinking indicator
            // is redundant noise (and the payload is huge).
            if tool == "StructuredOutput" {
                return None;
            }
            let icon = match tool.as_str() {
                "Bash" => "\u{1f527}",
                "Read" => "\u{1f4d6}",
                "Write" | "Edit" => "\u{270f}\u{fe0f}",
                "Grep" | "Glob" => "\u{1f50d}",
                _ => "\u{1f527}",
            };
            let truncated = truncate_str(input_summary, 120);
            let escaped = crate::cc::markdown_utils::html_escape(&truncated);
            Some(format!("{icon} {tool} <code>{escaped}</code>"))
        }
        StreamEvent::Result(_) | StreamEvent::Other => None,
    }
}

/// Format the full thinking message: events on top, status footer at bottom.
pub(crate) fn format_thinking_message(events: &VecDeque<StreamEvent>, usage: &StreamUsage) -> String {
    let mut lines: Vec<String> = Vec::new();

    for event in events {
        if let Some(formatted) = format_event(event) {
            lines.push(formatted);
        }
    }

    if lines.is_empty() {
        lines.push("\u{23f3} starting...".to_string());
    }

    // Status footer — always at the bottom so it's visible when scrolling.
    let cost_str = if usage.cost_usd > 0.0 {
        format!(" | ${:.2}", usage.cost_usd)
    } else {
        String::new()
    };
    lines.push(format!(
        "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n\u{23f3} Turn {}{}",
        usage.num_turns, cost_str
    ));

    let msg = lines.join("\n");
    // Telegram message limit is 4096 chars. Truncate if needed.
    if msg.chars().count() > 4000 {
        let truncated: String = msg.chars().take(4000).collect();
        format!("{truncated}\n...")
    } else {
        msg
    }
}

/// Ring buffer of recent displayable events.
pub(crate) struct EventRingBuffer {
    events: VecDeque<StreamEvent>,
    capacity: usize,
}

impl EventRingBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push an event. Only displayable events (Text, Thinking, ToolUse) are kept.
    pub(crate) fn push(&mut self, event: &StreamEvent) {
        if format_event(event).is_some() {
            if self.events.len() == self.capacity {
                self.events.pop_front();
            }
            self.events.push_back(event.clone());
        }
    }

    pub(crate) fn events(&self) -> &VecDeque<StreamEvent> {
        &self.events
    }
}

/// Truncate a string to at most `max_chars` characters, appending "…" if cut.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}…")
}

fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "Bash" => input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        "Read" => input
            .get("file_path")
            .and_then(|p| p.as_str())
            .unwrap_or("?")
            .to_string(),
        "Write" | "Edit" => input
            .get("file_path")
            .and_then(|p| p.as_str())
            .unwrap_or("?")
            .to_string(),
        "Grep" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        "Glob" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        "Skill" => input
            .get("skill")
            .and_then(|s| s.as_str())
            .map(|s| format!("/{s}"))
            .unwrap_or_default(),
        "Agent" => input
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("…")
            .to_string(),
        _ => {
            let s = input.to_string();
            truncate_str(&s, 80)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_result_event() {
        let line = r#"{"type":"result","subtype":"success","num_turns":3,"total_cost_usd":0.05,"result":"hello"}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Result(_)));
    }

    #[test]
    fn parse_text_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world"}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::Text(t) => assert_eq!(t, "Hello world"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::ToolUse {
                tool,
                input_summary,
            } => {
                assert_eq!(tool, "Bash");
                assert_eq!(input_summary, "ls -la");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Thinking));
    }

    #[test]
    fn parse_unknown_type() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Other));
    }

    #[test]
    fn parse_invalid_json() {
        assert!(matches!(parse_stream_event("not json"), StreamEvent::Other));
    }

    #[test]
    fn parse_usage_from_result() {
        let line = r#"{"type":"result","num_turns":5,"total_cost_usd":0.123}"#;
        let usage = parse_usage(line);
        assert_eq!(usage.num_turns, 5);
        assert!((usage.cost_usd - 0.123).abs() < 0.001);
    }

    #[test]
    fn ring_buffer_capacity() {
        let mut buf = EventRingBuffer::new(3);
        for i in 0..5 {
            buf.push(&StreamEvent::Text(format!("msg {i}")));
        }
        assert_eq!(buf.events().len(), 3);
        match &buf.events()[0] {
            StreamEvent::Text(t) => assert_eq!(t, "msg 2"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn ring_buffer_skips_non_displayable() {
        let mut buf = EventRingBuffer::new(5);
        buf.push(&StreamEvent::Other);
        buf.push(&StreamEvent::Result("{}".into()));
        buf.push(&StreamEvent::Text("hello".into()));
        assert_eq!(buf.events().len(), 1);
    }

    #[test]
    fn format_thinking_message_with_events() {
        let mut events = VecDeque::new();
        events.push_back(StreamEvent::ToolUse {
            tool: "Bash".into(),
            input_summary: "ls -la".into(),
        });
        events.push_back(StreamEvent::Text("checking files".into()));
        let usage = StreamUsage {
            num_turns: 2,
            cost_usd: 0.05,
        };
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.contains("Turn 2"));
        assert!(msg.contains("$0.05"));
        assert!(msg.contains("Bash <code>ls -la</code>"));
        assert!(msg.contains("\"checking files\""));
    }

    #[test]
    fn format_thinking_message_empty() {
        let events = VecDeque::new();
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.contains("starting..."));
    }

    #[test]
    fn structured_output_excluded_from_thinking() {
        let mut buf = EventRingBuffer::new(5);
        buf.push(&StreamEvent::ToolUse {
            tool: "Bash".into(),
            input_summary: "ls".into(),
        });
        buf.push(&StreamEvent::ToolUse {
            tool: "StructuredOutput".into(),
            input_summary: r#"{"content":"big payload"}"#.into(),
        });
        // StructuredOutput should be filtered out by format_event → not stored in ring buffer
        assert_eq!(buf.events().len(), 1);
        let usage = StreamUsage {
            num_turns: 3,
            cost_usd: 0.10,
        };
        let msg = format_thinking_message(buf.events(), &usage);
        assert!(!msg.contains("StructuredOutput"));
        assert!(msg.contains("Bash"));
    }

    #[test]
    fn format_thinking_message_truncates_long_content() {
        let mut events = VecDeque::new();
        // Add a very long text event
        events.push_back(StreamEvent::Text("x".repeat(5000)));
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.chars().count() <= 4010); // 4000 + "...\n"
    }

    #[test]
    fn tool_use_input_summary_truncated() {
        let long_cmd = "a".repeat(200);
        let formatted = format_event(&StreamEvent::ToolUse {
            tool: "Bash".into(),
            input_summary: long_cmd,
        })
        .unwrap();
        // 120 chars + "…" + icon + " Bash <code></code>" overhead
        assert!(formatted.chars().count() < 160, "got: {formatted}");
        assert!(formatted.contains('…'));
    }

    #[test]
    fn skill_tool_shows_skill_name() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"rightcron","args":"big prompt..."}}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::ToolUse {
                tool,
                input_summary,
            } => {
                assert_eq!(tool, "Skill");
                assert_eq!(input_summary, "/rightcron");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn agent_tool_shows_description() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Agent","input":{"description":"Build workspace","prompt":"long prompt..."}}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::ToolUse {
                tool,
                input_summary,
            } => {
                assert_eq!(tool, "Agent");
                assert_eq!(input_summary, "Build workspace");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn unknown_tool_input_truncated() {
        let long_json = serde_json::json!({"data": "x".repeat(200)});
        let summary = summarize_tool_input("UnknownTool", &long_json);
        assert!(summary.chars().count() <= 81); // 80 + "…"
        assert!(summary.contains('…'));
    }

    #[test]
    fn parse_api_key_source_happy_path() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"s","tools":[],"mcp_servers":[],"model":"claude-sonnet-4-6","permissionMode":"bypassPermissions","slash_commands":[],"apiKeySource":"none"}"#;
        assert_eq!(parse_api_key_source(line).as_deref(), Some("none"));
    }

    #[test]
    fn parse_api_key_source_api_key_mode() {
        let line = r#"{"type":"system","subtype":"init","apiKeySource":"ANTHROPIC_API_KEY"}"#;
        assert_eq!(
            parse_api_key_source(line).as_deref(),
            Some("ANTHROPIC_API_KEY")
        );
    }

    #[test]
    fn parse_api_key_source_wrong_type_returns_none() {
        // Result event has apiKeySource-adjacent fields but different type.
        let line = r#"{"type":"result","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_wrong_subtype_returns_none() {
        let line = r#"{"type":"system","subtype":"other","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_missing_field_returns_none() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[test]
    fn parse_api_key_source_malformed_json_returns_none() {
        assert!(parse_api_key_source("not json").is_none());
    }

    #[test]
    fn parse_usage_full_happy_path() {
        let line = r#"{
            "type":"result","subtype":"success","is_error":false,
            "session_id":"abc-123",
            "total_cost_usd":0.24,"num_turns":5,
            "usage":{
                "input_tokens":10,"output_tokens":200,
                "cache_creation_input_tokens":500,"cache_read_input_tokens":1500,
                "server_tool_use":{"web_search_requests":2,"web_fetch_requests":3}
            },
            "modelUsage":{
                "claude-sonnet-4-6":{
                    "inputTokens":10,"outputTokens":200,
                    "cacheReadInputTokens":1500,"cacheCreationInputTokens":500,
                    "costUSD":0.24,"contextWindow":200000,"maxOutputTokens":32000
                }
            }
        }"#;
        let breakdown = parse_usage_full(line).expect("happy path must parse");
        assert_eq!(breakdown.session_uuid, "abc-123");
        assert!((breakdown.total_cost_usd - 0.24).abs() < 1e-9);
        assert_eq!(breakdown.num_turns, 5);
        assert_eq!(breakdown.input_tokens, 10);
        assert_eq!(breakdown.output_tokens, 200);
        assert_eq!(breakdown.cache_creation_tokens, 500);
        assert_eq!(breakdown.cache_read_tokens, 1500);
        assert_eq!(breakdown.web_search_requests, 2);
        assert_eq!(breakdown.web_fetch_requests, 3);
        assert!(breakdown.model_usage_json.contains("claude-sonnet-4-6"));
    }

    #[test]
    fn parse_usage_full_missing_cost_returns_none() {
        let line = r#"{"type":"result","session_id":"x","num_turns":1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[test]
    fn parse_usage_full_missing_turns_returns_none() {
        let line = r#"{"type":"result","session_id":"x","total_cost_usd":0.1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[test]
    fn parse_usage_full_missing_session_id_returns_none() {
        let line = r#"{"type":"result","total_cost_usd":0.1,"num_turns":1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[test]
    fn parse_usage_full_missing_model_usage_uses_empty_object() {
        let line = r#"{
            "type":"result","session_id":"x",
            "total_cost_usd":0.1,"num_turns":1,
            "usage":{"input_tokens":5,"output_tokens":7}
        }"#;
        let b = parse_usage_full(line).expect("must parse");
        assert_eq!(b.model_usage_json, "{}");
        assert_eq!(b.input_tokens, 5);
        assert_eq!(b.output_tokens, 7);
        assert_eq!(b.cache_creation_tokens, 0);
        assert_eq!(b.web_search_requests, 0);
    }

    #[test]
    fn parse_usage_full_invalid_json_returns_none() {
        assert!(parse_usage_full("not json").is_none());
    }
}
