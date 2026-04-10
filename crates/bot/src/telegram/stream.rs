//! Stream event parsing, formatting, and ring buffer for CC stream-json output.

use std::collections::VecDeque;

/// A parsed stream event from CC's stream-json output.
#[derive(Debug, Clone)]
pub enum StreamEvent {
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
pub struct StreamUsage {
    pub num_turns: u32,
    pub cost_usd: f64,
}

/// Parse a single NDJSON line from CC stream-json output.
pub fn parse_stream_event(line: &str) -> StreamEvent {
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
pub fn parse_usage(result_json: &str) -> StreamUsage {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(result_json) else {
        return StreamUsage::default();
    };
    StreamUsage {
        num_turns: v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        cost_usd: v.get("total_cost_usd").and_then(|n| n.as_f64()).unwrap_or(0.0),
    }
}

/// Format a single event for Telegram display (HTML mode).
///
/// All dynamic content is HTML-escaped for safe use with ParseMode::Html.
pub fn format_event(event: &StreamEvent) -> Option<String> {
    match event {
        StreamEvent::Text(t) => {
            // Truncate long text — thinking indicator is a preview, not the full reply.
            let preview = if t.len() > 150 {
                let cut = &t[..t.char_indices().take_while(|&(i, _)| i < 150).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(150)];
                format!("{}…", super::markdown::html_escape(cut))
            } else {
                super::markdown::html_escape(t)
            };
            Some(format!("\u{1f4dd} \"{preview}\""))
        }
        StreamEvent::Thinking => Some("\u{1f4ad} thinking...".to_string()),
        StreamEvent::ToolUse { tool, input_summary } => {
            let icon = match tool.as_str() {
                "Bash" => "\u{1f527}",
                "Read" => "\u{1f4d6}",
                "Write" | "Edit" => "\u{270f}\u{fe0f}",
                "Grep" | "Glob" => "\u{1f50d}",
                _ => "\u{1f527}",
            };
            let escaped = super::markdown::html_escape(input_summary);
            Some(format!("{icon} {tool} <code>{escaped}</code>"))
        }
        StreamEvent::Result(_) | StreamEvent::Other => None,
    }
}

/// Format the full thinking message: events on top, status footer at bottom.
pub fn format_thinking_message(
    events: &VecDeque<StreamEvent>,
    usage: &StreamUsage,
    max_turns: u32,
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
        "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n\u{23f3} Turn {}/{}{}",
        usage.num_turns, max_turns, cost_str
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
pub struct EventRingBuffer {
    events: VecDeque<StreamEvent>,
    capacity: usize,
}

impl EventRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push an event. Only displayable events (Text, Thinking, ToolUse) are kept.
    pub fn push(&mut self, event: &StreamEvent) {
        if format_event(event).is_some() {
            if self.events.len() == self.capacity {
                self.events.pop_front();
            }
            self.events.push_back(event.clone());
        }
    }

    pub fn events(&self) -> &VecDeque<StreamEvent> {
        &self.events
    }
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
        _ => input.to_string(),
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
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world"}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::Text(t) => assert_eq!(t, "Hello world"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        match parse_stream_event(line) {
            StreamEvent::ToolUse { tool, input_summary } => {
                assert_eq!(tool, "Bash");
                assert_eq!(input_summary, "ls -la");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
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
        let usage = StreamUsage { num_turns: 2, cost_usd: 0.05 };
        let msg = format_thinking_message(&events, &usage, 30);
        assert!(msg.contains("Turn 2/30"));
        assert!(msg.contains("$0.05"));
        assert!(msg.contains("Bash <code>ls -la</code>"));
        assert!(msg.contains("\"checking files\""));
    }

    #[test]
    fn format_thinking_message_empty() {
        let events = VecDeque::new();
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage, 30);
        assert!(msg.contains("starting..."));
    }

    #[test]
    fn format_thinking_message_truncates_long_content() {
        let mut events = VecDeque::new();
        // Add a very long text event
        events.push_back(StreamEvent::Text("x".repeat(5000)));
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage, 30);
        assert!(msg.chars().count() <= 4010); // 4000 + "...\n"
    }
}
