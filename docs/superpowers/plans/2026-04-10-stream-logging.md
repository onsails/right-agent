# Stream Logging + Live Thinking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Switch CC invocation to stream-json, show live thinking indicator in Telegram, log stream to per-session files, use CC-native execution controls.

**Architecture:** Worker reads CC stdout line-by-line via `AsyncBufReadExt`. Each line is written to a per-session NDJSON file and parsed for display. A "thinking" Telegram message shows last 5 events, updated every 2s. CC-native `--max-turns` and `--max-budget-usd` replace our process timeout. Process timeout stays as 600s safety net.

**Tech Stack:** Rust, tokio (AsyncBufReadExt, select!), teloxide (editMessageText), serde_json

---

### Task 1: Add execution control fields to AgentConfig

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`

- [ ] **Step 1: Add new fields to AgentConfig**

In `crates/rightclaw/src/agent/types.rs`, add three fields to the `AgentConfig` struct, after the `attachments` field (line ~152):

```rust
    /// Maximum number of CC turns per invocation.
    /// CC stops gracefully with `terminal_reason: "max_turns"`.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Maximum dollar spend per CC invocation.
    /// CC stops gracefully with `terminal_reason: "max_budget"`.
    #[serde(default = "default_max_budget_usd")]
    pub max_budget_usd: f64,

    /// Show live thinking indicator in Telegram during CC execution.
    #[serde(default = "default_show_thinking")]
    pub show_thinking: bool,
```

Add the default functions near the existing defaults (`default_max_restarts`, `default_backoff_seconds`):

```rust
fn default_max_turns() -> u32 {
    30
}

fn default_max_budget_usd() -> f64 {
    1.0
}

fn default_show_thinking() -> bool {
    true
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw -- agent::types`
Expected: PASS — existing agent.yaml fixtures don't have these fields, but defaults handle them.

- [ ] **Step 3: Verify existing agent.yaml parses**

Run: `cargo test -p rightclaw -- agent`
Expected: PASS — `deny_unknown_fields` isn't violated because these are new serde fields with defaults.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "feat: add max_turns, max_budget_usd, show_thinking to AgentConfig"
```

---

### Task 2: Add execution control fields to WorkerContext and wire through

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/lib.rs` (where WorkerContext is created)

- [ ] **Step 1: Add fields to WorkerContext**

In `crates/bot/src/telegram/worker.rs`, add to the `WorkerContext` struct (after `auth_code_tx` field, line ~59):

```rust
    /// Max CC turns per invocation (passed as --max-turns).
    pub max_turns: u32,
    /// Max dollar spend per CC invocation (passed as --max-budget-usd).
    pub max_budget_usd: f64,
    /// Show live thinking indicator in Telegram.
    pub show_thinking: bool,
```

- [ ] **Step 2: Wire fields from AgentConfig to WorkerContext**

Find where `WorkerContext` is constructed in `crates/bot/src/lib.rs` (search for `WorkerContext {`). Add the three new fields, reading from the agent config. If the config has `agent.config.max_turns` etc., use those. Example:

```rust
    max_turns: agent_config.max_turns,
    max_budget_usd: agent_config.max_budget_usd,
    show_thinking: agent_config.show_thinking,
```

- [ ] **Step 3: Build and test**

Run: `cargo build -p rightclaw-bot`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/lib.rs
git commit -m "feat: wire max_turns, max_budget_usd, show_thinking to WorkerContext"
```

---

### Task 3: Stream event types and ring buffer

**Files:**
- Create: `crates/bot/src/telegram/stream.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Create stream.rs with event types, formatter, and ring buffer**

Create `crates/bot/src/telegram/stream.rs`:

```rust
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
    /// Tool result (truncated)
    ToolResult(String),
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
            let content = v
                .pointer("/message/content")
                .and_then(|c| c.as_array());
            if let Some(blocks) = content {
                for block in blocks {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                return StreamEvent::Text(truncate(text, 60));
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

/// Format a single event for Telegram display.
pub fn format_event(event: &StreamEvent) -> Option<String> {
    match event {
        StreamEvent::Text(t) => Some(format!("📝 \"{t}\"")),
        StreamEvent::Thinking => Some("💭 thinking...".to_string()),
        StreamEvent::ToolUse { tool, input_summary } => {
            let icon = match tool.as_str() {
                "Bash" => "🔧",
                "Read" => "📖",
                "Write" | "Edit" => "✏️",
                "Grep" | "Glob" => "🔍",
                _ => "🔧",
            };
            Some(format!("{icon} {tool}: {input_summary}"))
        }
        StreamEvent::ToolResult(_) => None, // don't show tool results in thinking
        StreamEvent::Result(_) | StreamEvent::Other => None,
    }
}

/// Format the full thinking message: header + last N events.
pub fn format_thinking_message(
    events: &VecDeque<StreamEvent>,
    usage: &StreamUsage,
    max_turns: u32,
) -> String {
    let mut lines = vec![format!(
        "⏳ Turn {}/{} | ${:.2}\n─────────────",
        usage.num_turns, max_turns, usage.cost_usd
    )];

    for event in events {
        if let Some(formatted) = format_event(event) {
            lines.push(formatted);
        }
    }

    if lines.len() == 1 {
        lines.push("⏳ starting...".to_string());
    }

    lines.join("\n")
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}...")
    }
}

fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "Bash" => input
            .get("command")
            .and_then(|c| c.as_str())
            .map(|c| truncate(c, 50))
            .unwrap_or_default(),
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
            .map(|p| truncate(p, 40))
            .unwrap_or_default(),
        "Glob" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(|p| truncate(p, 40))
            .unwrap_or_default(),
        _ => truncate(&input.to_string(), 50),
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
        // Should have messages 2, 3, 4
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
        assert_eq!(buf.events().len(), 1); // only Text is displayable
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
        assert!(msg.contains("🔧 Bash: ls -la"));
        assert!(msg.contains("📝 \"checking files\""));
    }

    #[test]
    fn format_thinking_message_empty() {
        let events = VecDeque::new();
        let usage = StreamUsage::default();
        let msg = format_thinking_message(&events, &usage, 30);
        assert!(msg.contains("starting..."));
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 14); // 10 chars + "..."
    }
}
```

- [ ] **Step 2: Register module**

In `crates/bot/src/telegram/mod.rs`, add:

```rust
pub mod stream;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw-bot -- telegram::stream`
Expected: PASS — all 12 stream module tests

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/stream.rs crates/bot/src/telegram/mod.rs
git commit -m "feat: stream event parser, formatter, and ring buffer"
```

---

### Task 4: Refactor invoke_cc to stream stdout

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

This is the core refactor. Replace `wait_with_timeout(child)` with a streaming readline loop.

- [ ] **Step 1: Add constants and imports**

At the top of worker.rs, change `CC_TIMEOUT_SECS`:

Find:
```rust
const CC_TIMEOUT_SECS: u64 = 120;
```

Replace:
```rust
/// Safety-net process timeout. CC should stop itself via --max-turns / --max-budget-usd.
/// This only fires for truly hung processes.
const CC_TIMEOUT_SECS: u64 = 600;
```

- [ ] **Step 2: Add --verbose, --output-format stream-json, --max-turns, --max-budget-usd to claude args**

In the claude args building section of `invoke_cc`, find:

```rust
    claude_args.push("--output-format".into());
    claude_args.push("json".into());
```

Replace with:

```rust
    claude_args.push("--verbose".into());
    claude_args.push("--output-format".into());
    claude_args.push("stream-json".into());
    claude_args.push("--max-turns".into());
    claude_args.push(ctx.max_turns.to_string());
    claude_args.push("--max-budget-usd".into());
    claude_args.push(format!("{:.2}", ctx.max_budget_usd));
```

- [ ] **Step 3: Replace wait_with_timeout with stream reading loop**

Find the block from `let mut child = cmd.spawn()` through `let output = wait_with_timeout(child, CC_TIMEOUT_SECS).await?;` and the stdout/stderr reading. Replace the entire section (from `let mut child = cmd.spawn()` through `let stdout_str = ...` and `let stderr_str = ...`) with:

```rust
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
    let session_uuid = cmd_args
        .iter()
        .find(|a| a.contains('-') && a.len() > 30)
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let stream_log_dir = ctx
        .agent_dir
        .parent()
        .unwrap_or(&ctx.agent_dir)
        .parent()
        .unwrap_or(&ctx.agent_dir)
        .join("logs")
        .join("streams");
    std::fs::create_dir_all(&stream_log_dir).ok();
    let stream_log_path = stream_log_dir.join(format!("{session_uuid}.ndjson"));
    let mut stream_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stream_log_path)
        .ok();

    let mut ring_buffer = super::stream::EventRingBuffer::new(10);
    let mut usage = super::stream::StreamUsage::default();
    let mut result_line: Option<String> = None;
    let mut thinking_msg_id: Option<teloxide::types::MessageId> = None;
    let mut last_edit = tokio::time::Instant::now();
    let tg_chat_id = ctx.chat_id;
    let eff_tid = ctx.effective_thread_id;

    let timeout_duration = Duration::from_secs(CC_TIMEOUT_SECS);
    let deadline = tokio::time::Instant::now() + timeout_duration;
    let mut timed_out = false;

    loop {
        tokio::select! {
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        // Write to stream log file.
                        if let Some(ref mut log) = stream_log {
                            use std::io::Write;
                            writeln!(log, "{line}").ok();
                        }

                        let event = super::stream::parse_stream_event(&line);

                        // Update usage from assistant events.
                        if let serde_json::Value::Object(ref map) = serde_json::from_str::<serde_json::Value>(&line).unwrap_or_default() {
                            if let Some(msg) = map.get("message") {
                                if let Some(u) = msg.pointer("/usage/output_tokens") {
                                    // Rough turn counting from assistant messages
                                    usage.num_turns = usage.num_turns.saturating_add(1);
                                }
                            }
                            if let Some(cost) = map.get("total_cost_usd").and_then(|c| c.as_f64()) {
                                usage.cost_usd = cost;
                            }
                        }

                        match &event {
                            super::stream::StreamEvent::Result(json) => {
                                // Extract final usage from result.
                                let final_usage = super::stream::parse_usage(json);
                                usage = final_usage;
                                result_line = Some(json.clone());
                            }
                            _ => {
                                ring_buffer.push(&event);
                            }
                        }

                        // Update thinking message (throttled to 2s).
                        if ctx.show_thinking
                            && super::stream::format_event(&event).is_some()
                            && last_edit.elapsed() >= Duration::from_secs(2)
                        {
                            let text = super::stream::format_thinking_message(
                                ring_buffer.events(),
                                &usage,
                                ctx.max_turns,
                            );
                            if let Some(msg_id) = thinking_msg_id {
                                // Edit existing thinking message.
                                let _ = ctx.bot
                                    .edit_message_text(tg_chat_id, msg_id, &text)
                                    .await;
                            } else {
                                // Send first thinking message.
                                let mut send = ctx.bot.send_message(tg_chat_id, &text);
                                if eff_tid != 0 {
                                    send = send.message_thread_id(
                                        teloxide::types::ThreadId(
                                            teloxide::types::MessageId(eff_tid as i32)
                                        )
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
                // Kill the process.
                child.kill().await.ok();
                break;
            }
        }
    }

    // Wait for process exit.
    let exit_status = child.wait().await.ok();
    let exit_code = exit_status
        .and_then(|s| s.code())
        .unwrap_or(-1);

    // Read any remaining stderr.
    let stderr_str = if let Some(mut stderr) = child.stderr.take() {
        let mut buf = String::new();
        use tokio::io::AsyncReadExt;
        stderr.read_to_string(&mut buf).await.ok();
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
        timeout_msg.push_str(&format!(
            "\nStream log: {}",
            stream_log_path.display()
        ));
        return Err(timeout_msg);
    }

    // Build stdout_str from result_line for parse_reply_output.
    let stdout_str = result_line.unwrap_or_default();
```

- [ ] **Step 4: Remove wait_with_timeout function**

The `wait_with_timeout` function (lines 235-245) is no longer used. Remove it.

- [ ] **Step 5: Update remaining stdout/stderr handling**

The code after this point uses `stdout_str` and `stderr_str` — both are still available. The `exit_code` variable is also available. The existing error handling, `parse_reply_output`, and response handling code should work unchanged since `stdout_str` now contains the result JSON line (same format as before).

Find and remove the old stdout/stderr extraction:
```rust
    let exit_code = output.status.code().unwrap_or(-1);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let stdout_str = String::from_utf8_lossy(&output.stdout);
```

These are now produced by the stream loop above.

- [ ] **Step 6: Replace typing indicator with thinking message**

In `spawn_worker`, find the typing indicator block:

```rust
            // Typing indicator: spawn task, cancel after subprocess completes (D-10)
            let cancel_token = CancellationToken::new();
            // ... (typing task)
```

The typing indicator is no longer needed when `show_thinking` is true — the thinking message replaces it. But when `show_thinking` is false, we still want the typing indicator.

Replace the typing indicator section:

```rust
            // Typing indicator: only when show_thinking is false (thinking message replaces it).
            let cancel_token = CancellationToken::new();
            let cancel_clone = cancel_token.clone();
            let bot_clone = ctx.bot.clone();
            let show_thinking = ctx.show_thinking;
            let typing_task = tokio::spawn(async move {
                if show_thinking {
                    // Thinking message handles the indicator — just wait for cancel.
                    cancel_clone.cancelled().await;
                    return;
                }
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
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p rightclaw-bot --lib`
Expected: PASS — existing tests still work (parse_reply_output gets same format input).

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat: stream stdout line-by-line, live thinking message, 600s safety timeout"
```

---

### Task 5: Update agent init wizard

**Files:**
- Modify: `crates/rightclaw/src/init.rs`

- [ ] **Step 1: Find the agent init wizard**

Search for where agent.yaml fields are set during `rightclaw agent init`. Add prompts for `max_turns`, `max_budget_usd`, `show_thinking` with defaults.

Read the file first, find the pattern for existing prompts (e.g. model selection, network_policy), and add similar prompts for the new fields. The new fields should use defaults without prompting in `-y` (non-interactive) mode.

In the section where `AgentConfig` is assembled for writing to agent.yaml, add:

```rust
    max_turns: 30,
    max_budget_usd: 1.0,
    show_thinking: true,
```

- [ ] **Step 2: Build**

Run: `cargo build -p rightclaw`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat: add max_turns, max_budget_usd, show_thinking to agent init"
```

---

### Task 6: Update documentation

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update ARCHITECTURE.md**

Add after the "Prompting Architecture" section a new section:

```markdown
### Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line. Each event is written to a per-session NDJSON log file at
`~/.rightclaw/logs/streams/<session-uuid>.ndjson`.

When `show_thinking: true` (default), a live "thinking" message in Telegram shows
the last 5 events (tool calls, text) with turn counter and cost. Updated every 2s
via `editMessageText`. The thinking message stays in chat after completion.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 1.0)
from agent.yaml. Process-level timeout (600s) is a safety net only.
```

- [ ] **Step 2: Update PROMPT_SYSTEM.md**

In PROMPT_SYSTEM.md, find the "CLI Flags Change" or invocation section and update to reflect new flags:

```
claude -p --verbose --output-format stream-json --max-turns 30 --max-budget-usd 1.00 \
  --system-prompt-file /tmp/rightclaw-system-prompt.md \
  --json-schema <schema> \
  --dangerously-skip-permissions --mcp-config ... --strict-mcp-config
```

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md
git commit -m "docs: stream logging, thinking message, CC execution controls"
```

---

### Task 7: Build and full test

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: PASS, 0 errors, 0 warnings

- [ ] **Step 2: Full test suite**

Run: `cargo test -p rightclaw && cargo test -p rightclaw-bot --lib`
Expected: All tests pass including new stream module tests.

- [ ] **Step 3: Manual test**

Rebuild bot, restart, send message via Telegram. Expected:
1. Thinking message appears with "⏳ Turn 0/30 | $0.00 / starting..."
2. Thinking message updates as CC works (tool calls, text)
3. Final reply arrives as separate message
4. Thinking message stays in chat
5. Stream log file exists at `~/.rightclaw/logs/streams/<uuid>.ndjson`
