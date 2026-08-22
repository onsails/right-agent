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
    /// Streaming system-level progress (e.g. `system/thinking_tokens`): the
    /// API call is live and producing output. Not displayed, but counts as
    /// API progress for the foreground auth-heuristic watchdog.
    SystemProgress,
    /// System init or other (ignored for display)
    Other,
}

/// Typed stream event persisted for later learning episode selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistedStreamEventKind {
    AssistantText,
    Thinking,
    ToolCall,
    ToolResult,
    ToolError,
    InvocationResult,
}

/// Parsed evidence from one CC stream-json line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PersistedStreamEvent {
    pub(crate) kind: PersistedStreamEventKind,
    pub(crate) tool_name: Option<String>,
    pub(crate) content_json: serde_json::Value,
    pub(crate) content_text: String,
}

/// Usage info extracted from stream events.
#[derive(Debug, Default, Clone)]
pub(crate) struct StreamUsage {
    pub num_turns: u32,
    pub cost_usd: f64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResultTiming {
    pub(crate) duration_ms: Option<u64>,
    pub(crate) duration_api_ms: Option<u64>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cache_creation_input_tokens: Option<u64>,
    pub(crate) cache_read_input_tokens: Option<u64>,
    pub(crate) cache_miss_reason: Option<String>,
}

/// Parse a single NDJSON line from CC stream-json output.
pub(crate) fn parse_stream_event(line: &str) -> StreamEvent {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return StreamEvent::Other;
    };

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "result" => StreamEvent::Result(line.to_string()),
        "system" => {
            let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            match subtype {
                // Heartbeat subtypes that prove the API call is streaming.
                "thinking_tokens" | "status" => StreamEvent::SystemProgress,
                _ => StreamEvent::Other,
            }
        }
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

/// Parse a single CC stream-json line into a typed event suitable for durable
/// execution evidence. Untyped lines return `None`.
#[allow(dead_code)]
pub(crate) fn parse_persisted_stream_event(line: &str) -> Option<PersistedStreamEvent> {
    parse_persisted_stream_events(line).into_iter().next()
}

/// Parse all typed events from a single CC stream-json line in block order.
pub(crate) fn parse_persisted_stream_events(line: &str) -> Vec<PersistedStreamEvent> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "assistant" => parse_assistant_persisted_event(v),
        "user" => parse_user_persisted_event(v),
        "result" => vec![PersistedStreamEvent {
            kind: PersistedStreamEventKind::InvocationResult,
            tool_name: None,
            content_text: value_text(v.get("result").unwrap_or(&serde_json::Value::Null)),
            content_json: v,
        }],
        _ => Vec::new(),
    }
}

/// The substring CC's structured-output validator emits when the model's
/// StructuredOutput tool call does not satisfy `--json-schema`.
pub(crate) const SCHEMA_REJECTION_MARKER: &str = "does not match required schema";

/// Single-parse classification of a CC stream line for the schema-rejection guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaLineClass {
    /// A structured-output schema-validation rejection (tool_result is_error + marker).
    Rejection,
    /// A successful tool_result (resets the consecutive-rejection run).
    SuccessfulToolResult,
    /// Anything else (assistant text/thinking/tool_use, result line, etc.).
    Other,
}

/// Classify a raw stream line in ONE parse. Rejection takes precedence so a
/// line carrying both an error rejection and a success cannot reset the run.
pub(crate) fn classify_schema_line(line: &str) -> SchemaLineClass {
    let mut saw_success = false;
    for e in parse_persisted_stream_events(line) {
        if e.kind == PersistedStreamEventKind::ToolError
            && e.content_text.contains(SCHEMA_REJECTION_MARKER)
        {
            return SchemaLineClass::Rejection;
        }
        if e.kind == PersistedStreamEventKind::ToolResult {
            saw_success = true;
        }
    }
    if saw_success {
        SchemaLineClass::SuccessfulToolResult
    } else {
        SchemaLineClass::Other
    }
}

/// True when this stream line is a `tool_result` error reporting a
/// structured-output schema violation. Reuses `classify_schema_line` so the
/// matching rules stay in one place. Retained as a tested boolean view of the
/// classifier; runtime callers use `classify_schema_line` directly.
#[allow(dead_code)]
pub(crate) fn is_structured_output_rejection(line: &str) -> bool {
    classify_schema_line(line) == SchemaLineClass::Rejection
}

/// True when this line is a successful `tool_result` (resets the rejection run).
/// Retained as a tested boolean view of the classifier; runtime callers use
/// `classify_schema_line` directly.
#[allow(dead_code)]
pub(crate) fn is_successful_tool_result(line: &str) -> bool {
    classify_schema_line(line) == SchemaLineClass::SuccessfulToolResult
}

/// Extract owned `/message/content` array from a stream-json event JSON.
/// Returns `None` when the path is missing or not an array.
fn take_message_content_blocks(mut v: serde_json::Value) -> Option<Vec<serde_json::Value>> {
    let content = v
        .get_mut("message")
        .and_then(|message| message.get_mut("content"))?;
    match std::mem::take(content) {
        serde_json::Value::Array(blocks) => Some(blocks),
        _ => None,
    }
}

fn parse_assistant_persisted_event(v: serde_json::Value) -> Vec<PersistedStreamEvent> {
    let Some(blocks) = take_message_content_blocks(v) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for block in blocks {
        let block_type = block
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_owned();
        match block_type.as_str() {
            "text" => {
                let text = block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !text.is_empty() {
                    events.push(PersistedStreamEvent {
                        kind: PersistedStreamEventKind::AssistantText,
                        tool_name: None,
                        content_text: text,
                        content_json: block,
                    });
                }
            }
            "thinking" => {
                let thinking = block
                    .get("thinking")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !thinking.is_empty() {
                    events.push(PersistedStreamEvent {
                        kind: PersistedStreamEventKind::Thinking,
                        tool_name: None,
                        content_text: thinking,
                        content_json: block,
                    });
                }
            }
            "tool_use" => {
                let tool = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_owned();
                let content_text = summarize_tool_input(
                    &tool,
                    block.get("input").unwrap_or(&serde_json::Value::Null),
                );
                events.push(PersistedStreamEvent {
                    kind: PersistedStreamEventKind::ToolCall,
                    tool_name: Some(tool),
                    content_text,
                    content_json: block,
                });
            }
            _ => {}
        }
    }
    events
}

fn parse_user_persisted_event(v: serde_json::Value) -> Vec<PersistedStreamEvent> {
    let Some(blocks) = take_message_content_blocks(v) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }
        let is_error = block
            .get("is_error")
            .and_then(|is_error| is_error.as_bool())
            .unwrap_or(false);
        let content_text = value_text(block.get("content").unwrap_or(&serde_json::Value::Null));
        events.push(PersistedStreamEvent {
            kind: if is_error {
                PersistedStreamEventKind::ToolError
            } else {
                PersistedStreamEventKind::ToolResult
            },
            tool_name: None,
            content_text,
            content_json: block,
        });
    }
    events
}

pub(crate) fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.as_str().map(str::to_owned).or_else(|| {
                    item.get("text")
                        .and_then(|text| text.as_str())
                        .map(str::to_owned)
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// Extract usage info from a result event JSON.
pub(crate) fn parse_usage(result_json: &str) -> StreamUsage {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(result_json) else {
        return StreamUsage::default();
    };
    let get_u64 = |ptr: &str| -> u64 { v.pointer(ptr).and_then(|n| n.as_u64()).unwrap_or(0) };
    StreamUsage {
        num_turns: v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        cost_usd: v
            .get("total_cost_usd")
            .and_then(|n| n.as_f64())
            .unwrap_or(0.0),
        cache_creation_tokens: get_u64("/usage/cache_creation_input_tokens"),
        cache_read_tokens: get_u64("/usage/cache_read_input_tokens"),
    }
}

/// Return the last `{"type":"result", …}` NDJSON line from a stream-json
/// stdout, if any.
pub(crate) fn last_result_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rfind(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| {
                    v.get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| t == "result")
                })
                .unwrap_or(false)
        })
        .map(ToOwned::to_owned)
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
        wall_elapsed_ms: None,
    })
}

pub(crate) fn parse_result_timing(result_json: &str) -> Option<ResultTiming> {
    let v: serde_json::Value = serde_json::from_str(result_json).ok()?;
    if v.get("type")?.as_str()? != "result" {
        return None;
    }

    Some(ResultTiming {
        duration_ms: optional_u64(&v, "/duration_ms"),
        duration_api_ms: optional_u64(&v, "/duration_api_ms"),
        ttft_ms: optional_u64(&v, "/ttft_ms"),
        input_tokens: optional_u64(&v, "/usage/input_tokens"),
        output_tokens: optional_u64(&v, "/usage/output_tokens"),
        cache_creation_input_tokens: optional_u64(&v, "/usage/cache_creation_input_tokens"),
        cache_read_input_tokens: optional_u64(&v, "/usage/cache_read_input_tokens"),
        cache_miss_reason: cache_miss_reason_from_event_diagnostics(&v),
    })
}

pub(crate) fn parse_cache_miss_reason(line: &str) -> Option<String> {
    if !has_cache_diagnostic_key(line) {
        return None;
    }

    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }

    cache_miss_reason_from_event_diagnostics(&v)
}

fn has_cache_diagnostic_key(line: &str) -> bool {
    [
        "cache_miss_reason",
        "cacheMissReason",
        "cache_miss",
        "cacheMiss",
        "prompt_cache",
        "promptCache",
        "cache_diagnostics",
        "cacheDiagnostics",
    ]
    .into_iter()
    .any(|key| line.contains(key))
}

fn optional_u64(v: &serde_json::Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(|n| n.as_u64())
}

fn cache_miss_reason_from_event_diagnostics(v: &serde_json::Value) -> Option<String> {
    v.get("diagnostics")
        .and_then(cache_miss_reason_from_value)
        .or_else(|| {
            v.pointer("/message/diagnostics")
                .and_then(cache_miss_reason_from_value)
        })
        .or_else(|| v.get("prompt_cache").and_then(cache_miss_reason_from_value))
        .or_else(|| v.get("promptCache").and_then(cache_miss_reason_from_value))
        .or_else(|| {
            v.get("cache_diagnostics")
                .and_then(cache_miss_reason_from_value)
        })
        .or_else(|| {
            v.get("cacheDiagnostics")
                .and_then(cache_miss_reason_from_value)
        })
}

fn cache_miss_reason_from_value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Bool(_)
        | serde_json::Value::Null => None,
        serde_json::Value::Array(_) => None,
        serde_json::Value::Object(map) => {
            for key in ["cache_miss_reason", "cacheMissReason"] {
                if let Some(reason) = map.get(key).and_then(cache_miss_reason_leaf) {
                    return Some(reason);
                }
            }

            for key in [
                "cache_miss",
                "cacheMiss",
                "prompt_cache",
                "promptCache",
                "cache_diagnostics",
                "cacheDiagnostics",
            ] {
                if let Some(cache_info) = map.get(key)
                    && let Some(reason) = cache_miss_reason_from_value(cache_info)
                {
                    return Some(reason);
                }
            }

            None
        }
    }
}

fn cache_miss_reason_leaf(v: &serde_json::Value) -> Option<String> {
    if let Some(reason) = v.as_str() {
        let reason = reason.trim();
        return (!reason.is_empty()).then(|| reason.to_owned());
    }

    let map = v.as_object()?;
    for key in ["type", "reason", "message"] {
        if let Some(reason) = map.get(key).and_then(|value| value.as_str()) {
            let reason = reason.trim();
            if !reason.is_empty() {
                return Some(reason.to_owned());
            }
        }
    }
    None
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

/// Agent-facing status of the built-in `right` MCP server from Claude Code's
/// `system/init` stream event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RightMcpInitStatus {
    Connected,
    /// `status = None` means the init event did not list `right` at all.
    Unhealthy {
        status: Option<String>,
    },
}

/// Parse the built-in `right` MCP server status from a Claude Code
/// `system/init` NDJSON line.
///
/// Returns `None` for non-init lines and malformed JSON. Returns
/// `Unhealthy { status: None }` when the line is an init event but `right`
/// is absent, because the agent-facing MCP registry is missing the platform
/// server.
pub(crate) fn parse_right_mcp_init_status(init_json: &str) -> Option<RightMcpInitStatus> {
    let v: serde_json::Value = serde_json::from_str(init_json).ok()?;
    if v.get("type")?.as_str()? != "system" {
        return None;
    }
    if v.get("subtype")?.as_str()? != "init" {
        return None;
    }

    if let Some(servers) = v.get("mcp_servers").and_then(|s| s.as_array()) {
        for server in servers {
            if server.get("name").and_then(|n| n.as_str()) == Some("right") {
                let status = server
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                return Some(if status == "connected" {
                    RightMcpInitStatus::Connected
                } else {
                    RightMcpInitStatus::Unhealthy {
                        status: Some(status.to_owned()),
                    }
                });
            }
        }
    }

    Some(RightMcpInitStatus::Unhealthy { status: None })
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
        StreamEvent::Result(_) | StreamEvent::SystemProgress | StreamEvent::Other => None,
    }
}

/// Format the full thinking message: events on top, status footer at bottom.
pub(crate) fn format_thinking_message(
    events: &VecDeque<StreamEvent>,
    usage: &StreamUsage,
) -> String {
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
    use PersistedStreamEventKind::*;

    #[tokio::test]
    async fn parse_result_event() {
        let line = r#"{"type":"result","subtype":"success","num_turns":3,"total_cost_usd":0.05,"result":"hello"}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Result(_)));
    }

    #[tokio::test]
    async fn parse_text_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world"}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::Text(t) => assert_eq!(t, "Hello world"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_tool_use_event() {
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

    #[tokio::test]
    async fn parse_thinking_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Thinking));
    }

    #[tokio::test]
    async fn persisted_event_parses_thinking_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Need check Notion first"}]}}"#;
        let event = parse_persisted_stream_event(line).unwrap();
        assert_eq!(event.kind, Thinking);
        assert_eq!(event.content_text, "Need check Notion first");
    }

    #[tokio::test]
    async fn persisted_event_parses_tool_result_error() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":true,"content":"permission denied"}]}}"#;
        let event = parse_persisted_stream_event(line).unwrap();
        assert_eq!(event.kind, ToolError);
    }

    #[tokio::test]
    async fn persisted_events_parse_all_assistant_blocks_in_order() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First"},{"type":"thinking","thinking":"Then think"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}"#;
        let events = parse_persisted_stream_events(line);
        let kinds = events.iter().map(|event| event.kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec![AssistantText, Thinking, ToolCall]);
    }

    #[tokio::test]
    async fn persisted_events_parse_all_user_tool_results_in_order() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"},{"type":"tool_result","tool_use_id":"toolu_2","is_error":true,"content":"denied"}]}}"#;
        let events = parse_persisted_stream_events(line);
        let kinds = events.iter().map(|event| event.kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec![ToolResult, ToolError]);
    }

    #[tokio::test]
    async fn persisted_events_parse_invocation_result() {
        let line = r#"{"type":"result","result":"done"}"#;
        let events = parse_persisted_stream_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, InvocationResult);
        assert_eq!(events[0].content_text, "done");
    }

    #[tokio::test]
    async fn parse_unknown_type() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert!(matches!(parse_stream_event(line), StreamEvent::Other));
    }

    #[tokio::test]
    async fn parse_invalid_json() {
        assert!(matches!(parse_stream_event("not json"), StreamEvent::Other));
    }

    #[tokio::test]
    async fn parse_system_thinking_tokens_is_progress() {
        let line = r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":106}"#;
        assert!(matches!(
            parse_stream_event(line),
            StreamEvent::SystemProgress
        ));
    }

    #[tokio::test]
    async fn parse_system_status_is_progress() {
        let line = r#"{"type":"system","subtype":"status"}"#;
        assert!(matches!(
            parse_stream_event(line),
            StreamEvent::SystemProgress
        ));
    }

    #[tokio::test]
    async fn parse_usage_from_result() {
        let line = r#"{"type":"result","num_turns":5,"total_cost_usd":0.123}"#;
        let usage = parse_usage(line);
        assert_eq!(usage.num_turns, 5);
        assert!((usage.cost_usd - 0.123).abs() < 0.001);
    }

    #[tokio::test]
    async fn ring_buffer_capacity() {
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

    #[tokio::test]
    async fn ring_buffer_skips_non_displayable() {
        let mut buf = EventRingBuffer::new(5);
        buf.push(&StreamEvent::Other);
        buf.push(&StreamEvent::Result("{}".into()));
        buf.push(&StreamEvent::Text("hello".into()));
        assert_eq!(buf.events().len(), 1);
    }

    #[tokio::test]
    async fn format_thinking_message_with_events() {
        let mut events = VecDeque::new();
        events.push_back(StreamEvent::ToolUse {
            tool: "Bash".into(),
            input_summary: "ls -la".into(),
        });
        events.push_back(StreamEvent::Text("checking files".into()));
        let usage = StreamUsage {
            num_turns: 2,
            cost_usd: 0.05,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.contains("Turn 2"));
        assert!(msg.contains("$0.05"));
        assert!(msg.contains("Bash <code>ls -la</code>"));
        assert!(msg.contains("\"checking files\""));
    }

    #[tokio::test]
    async fn format_thinking_message_empty() {
        let events = VecDeque::new();
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.contains("starting..."));
    }

    #[tokio::test]
    async fn structured_output_excluded_from_thinking() {
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
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let msg = format_thinking_message(buf.events(), &usage);
        assert!(!msg.contains("StructuredOutput"));
        assert!(msg.contains("Bash"));
    }

    #[tokio::test]
    async fn format_thinking_message_truncates_long_content() {
        let mut events = VecDeque::new();
        // Add a very long text event
        events.push_back(StreamEvent::Text("x".repeat(5000)));
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage);
        assert!(msg.chars().count() <= 4010); // 4000 + "...\n"
    }

    #[tokio::test]
    async fn tool_use_input_summary_truncated() {
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

    #[tokio::test]
    async fn skill_tool_shows_skill_name() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"right-cron","args":"big prompt..."}}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::ToolUse {
                tool,
                input_summary,
            } => {
                assert_eq!(tool, "Skill");
                assert_eq!(input_summary, "/right-cron");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_tool_shows_description() {
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

    #[tokio::test]
    async fn unknown_tool_input_truncated() {
        let long_json = serde_json::json!({"data": "x".repeat(200)});
        let summary = summarize_tool_input("UnknownTool", &long_json);
        assert!(summary.chars().count() <= 81); // 80 + "…"
        assert!(summary.contains('…'));
    }

    #[tokio::test]
    async fn parse_api_key_source_happy_path() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"s","tools":[],"mcp_servers":[],"model":"claude-sonnet-4-6","permissionMode":"bypassPermissions","slash_commands":[],"apiKeySource":"none"}"#;
        assert_eq!(parse_api_key_source(line).as_deref(), Some("none"));
    }

    #[tokio::test]
    async fn parse_api_key_source_api_key_mode() {
        let line = r#"{"type":"system","subtype":"init","apiKeySource":"ANTHROPIC_API_KEY"}"#;
        assert_eq!(
            parse_api_key_source(line).as_deref(),
            Some("ANTHROPIC_API_KEY")
        );
    }

    #[tokio::test]
    async fn parse_api_key_source_wrong_type_returns_none() {
        // Result event has apiKeySource-adjacent fields but different type.
        let line = r#"{"type":"result","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[tokio::test]
    async fn parse_api_key_source_wrong_subtype_returns_none() {
        let line = r#"{"type":"system","subtype":"other","apiKeySource":"none"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[tokio::test]
    async fn parse_api_key_source_missing_field_returns_none() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert!(parse_api_key_source(line).is_none());
    }

    #[tokio::test]
    async fn parse_api_key_source_malformed_json_returns_none() {
        assert!(parse_api_key_source("not json").is_none());
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_connected() {
        let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[
            {"name":"right","status":"connected"},
            {"name":"composio","status":"connected"}
        ]
    }"#;

        assert_eq!(
            parse_right_mcp_init_status(line),
            Some(RightMcpInitStatus::Connected)
        );
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_needs_auth() {
        let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[{"name":"right","status":"needs-auth"}]
    }"#;

        assert_eq!(
            parse_right_mcp_init_status(line),
            Some(RightMcpInitStatus::Unhealthy {
                status: Some("needs-auth".to_owned())
            })
        );
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_missing_right_is_unhealthy() {
        let line = r#"{
        "type":"system",
        "subtype":"init",
        "mcp_servers":[{"name":"composio","status":"connected"}]
    }"#;

        assert_eq!(
            parse_right_mcp_init_status(line),
            Some(RightMcpInitStatus::Unhealthy { status: None })
        );
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_missing_servers_is_unhealthy() {
        let line = r#"{"type":"system","subtype":"init"}"#;

        assert_eq!(
            parse_right_mcp_init_status(line),
            Some(RightMcpInitStatus::Unhealthy { status: None })
        );
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_ignores_non_init_lines() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;

        assert_eq!(parse_right_mcp_init_status(line), None);
    }

    #[tokio::test]
    async fn parse_right_mcp_init_status_ignores_malformed_json() {
        assert_eq!(parse_right_mcp_init_status("not json"), None);
    }

    #[tokio::test]
    async fn parse_usage_full_happy_path() {
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

    #[tokio::test]
    async fn parse_usage_full_missing_cost_returns_none() {
        let line = r#"{"type":"result","session_id":"x","num_turns":1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[tokio::test]
    async fn parse_usage_full_missing_turns_returns_none() {
        let line = r#"{"type":"result","session_id":"x","total_cost_usd":0.1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[tokio::test]
    async fn parse_usage_full_missing_session_id_returns_none() {
        let line = r#"{"type":"result","total_cost_usd":0.1,"num_turns":1}"#;
        assert!(parse_usage_full(line).is_none());
    }

    #[tokio::test]
    async fn parse_usage_full_missing_model_usage_uses_empty_object() {
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

    #[tokio::test]
    async fn parse_usage_full_invalid_json_returns_none() {
        assert!(parse_usage_full("not json").is_none());
    }

    #[tokio::test]
    async fn parse_result_timing_extracts_optional_fields() {
        let line = r#"{
            "type":"result",
            "duration_ms":1234,
            "duration_api_ms":987,
            "ttft_ms":321,
            "usage":{
                "input_tokens":10,
                "output_tokens":20,
                "cache_creation_input_tokens":30,
                "cache_read_input_tokens":40
            },
            "diagnostics":{
                "cache_miss_reason":{
                    "type":"previous_message_not_found"
                }
            }
        }"#;

        assert_eq!(
            parse_result_timing(line),
            Some(ResultTiming {
                duration_ms: Some(1234),
                duration_api_ms: Some(987),
                ttft_ms: Some(321),
                input_tokens: Some(10),
                output_tokens: Some(20),
                cache_creation_input_tokens: Some(30),
                cache_read_input_tokens: Some(40),
                cache_miss_reason: Some("previous_message_not_found".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn parse_result_timing_ignores_non_result_lines() {
        let assistant_line =
            r#"{"type":"assistant","duration_ms":1234,"usage":{"input_tokens":10}}"#;
        let malformed_line = "not json";

        assert_eq!(parse_result_timing(assistant_line), None);
        assert_eq!(parse_result_timing(malformed_line), None);
    }

    #[tokio::test]
    async fn parse_result_timing_ignores_unrelated_cache_miss_reason() {
        let line = r#"{
            "type":"result",
            "duration_ms":1234,
            "result":{
                "content":"{\"cache_miss_reason\":\"not a diagnostic\"}"
            },
            "metadata":{
                "nested":{
                    "cache_miss_reason":"unrelated metadata"
                }
            }
        }"#;

        let timing = parse_result_timing(line).expect("result timing should parse");
        assert_eq!(timing.duration_ms, Some(1234));
        assert_eq!(timing.cache_miss_reason, None);
    }

    #[tokio::test]
    async fn parse_cache_miss_reason_extracts_from_assistant_diagnostics() {
        let line = r#"{
            "type":"assistant",
            "message":{
                "content":[{"type":"text","text":"Working..."}],
                "diagnostics":{
                    "cache_miss_reason":{
                        "type":"previous_message_not_found"
                    }
                }
            }
        }"#;

        assert_eq!(
            parse_cache_miss_reason(line).as_deref(),
            Some("previous_message_not_found")
        );
    }

    #[tokio::test]
    async fn parse_cache_miss_reason_ignores_unexpected_diagnostic_shapes() {
        let line = r#"{
            "type":"assistant",
            "diagnostics":{
                "prompt_cache":{
                    "message":"generic cache message",
                    "reason":"generic cache reason"
                }
            }
        }"#;

        assert_eq!(parse_cache_miss_reason(line), None);
    }

    #[tokio::test]
    async fn cache_diagnostic_key_prefilter_covers_supported_shapes() {
        for key in [
            "cache_miss_reason",
            "cacheMissReason",
            "cache_miss",
            "cacheMiss",
            "prompt_cache",
            "promptCache",
            "cache_diagnostics",
            "cacheDiagnostics",
        ] {
            let line = format!(r#"{{"type":"assistant","diagnostics":{{"{key}":{{}}}}}}"#);
            assert!(
                has_cache_diagnostic_key(&line),
                "{key} should pass prefilter"
            );
        }

        assert!(!has_cache_diagnostic_key(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#
        ));
    }

    #[test]
    fn parse_usage_captures_cache_tokens() {
        let json = r#"{"type":"result","num_turns":2,"total_cost_usd":0.1,
            "usage":{"cache_creation_input_tokens":30,"cache_read_input_tokens":40}}"#;
        let u = parse_usage(json);
        assert_eq!(u.cache_creation_tokens, 30);
        assert_eq!(u.cache_read_tokens, 40);
    }

    #[test]
    fn detects_structured_output_schema_rejection() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"Output does not match required schema: root: must have required property 'content'","is_error":true,"tool_use_id":"x"}]}}"#;
        assert!(is_structured_output_rejection(line));
    }

    #[test]
    fn non_error_tool_result_is_not_rejection() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok","is_error":false,"tool_use_id":"x"}]}}"#;
        assert!(!is_structured_output_rejection(line));
    }

    #[test]
    fn assistant_and_result_lines_are_not_rejection() {
        assert!(!is_structured_output_rejection(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#
        ));
        assert!(!is_structured_output_rejection(
            r#"{"type":"result","is_error":true,"result":"boom"}"#
        ));
    }

    #[test]
    fn successful_tool_result_detected() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok","is_error":false}]}}"#;
        assert!(is_successful_tool_result(line));
        let rej = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"Output does not match required schema","is_error":true}]}}"#;
        assert!(!is_successful_tool_result(rej));
    }

    #[test]
    fn classify_schema_line_precedence_and_cases() {
        assert_eq!(
            classify_schema_line(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"Output does not match required schema","is_error":true}]}}"#
            ),
            SchemaLineClass::Rejection
        );
        assert_eq!(
            classify_schema_line(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok","is_error":false}]}}"#
            ),
            SchemaLineClass::SuccessfulToolResult
        );
        assert_eq!(
            classify_schema_line(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"StructuredOutput","input":{}}]}}"#
            ),
            SchemaLineClass::Other
        );
    }
}
