# Immediate Working Anchor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a Telegram `Working...` anchor with Stop and Background controls as soon as a real foreground Claude invocation starts, before the first Claude stream event.

**Architecture:** Keep the change inside the existing Telegram worker and stream parser. Add a pure render helper plus a small async send helper for the initial anchor, reuse the existing per-run visibility state and keyboard logic, and add stream-result timing/cache diagnostics parsed from CC NDJSON.

**Tech Stack:** Rust 2024, `right-bot`, `teloxide`, `tokio`, existing `tracing`, existing `devenv shell -- cargo ...` verification commands.

---

## File Structure

- Modify `crates/bot/src/telegram/worker.rs`
  - Add a pure initial-anchor render helper near `thinking_anchor_text`.
  - Add an async `send_thinking_anchor` helper near the render helper.
  - Call the helper after subprocess start, stdin write, stop token insert, and visibility initialization.
  - Keep first-stream-event anchor creation only as fallback when the immediate send failed.
  - Add a `log_result_timing` helper and wire parsed timing/cache diagnostics into the result-event branch.
- Modify `crates/bot/src/cc/stream.rs`
  - Add `ResultTiming` plus parsers for result timing/cache fields and cache-miss reasons.
  - Add focused parser tests.
- Modify `docs/architecture/sessions.md`
  - Document that foreground workers create the thinking anchor immediately after a real Claude subprocess starts.

## Task 1: Baseline

**Files:**
- Read: `crates/bot/src/telegram/worker.rs`
- Read: `crates/bot/src/cc/stream.rs`

- [ ] **Step 1: Check worktree state**

Run: `devenv shell -- git status --short`

Expected: no unexpected untracked or modified files except files intentionally touched for this implementation.

- [ ] **Step 2: Run baseline worker helper tests**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor_text`

Expected: PASS. Existing tests `thinking_anchor_text_collapsed_is_static_working_message` and `thinking_anchor_text_expanded_uses_stream_formatter` pass.

- [ ] **Step 3: Run baseline stream usage parser tests**

Run: `devenv shell -- cargo test -p right-bot parse_usage_full`

Expected: PASS. Existing `parse_usage_full_*` tests pass.

## Task 2: Stream Timing and Cache Parsers

**Files:**
- Modify: `crates/bot/src/cc/stream.rs`

- [ ] **Step 1: Add failing parser tests**

In `crates/bot/src/cc/stream.rs`, inside `#[cfg(test)] mod tests`, add these tests before `parse_usage_full_happy_path`:

```rust
#[test]
fn parse_result_timing_extracts_optional_fields() {
    let line = r#"{
        "type":"result",
        "duration_ms":52014,
        "duration_api_ms":51997,
        "ttft_ms":40033,
        "usage":{
            "input_tokens":5,
            "output_tokens":3445,
            "cache_creation_input_tokens":190890,
            "cache_read_input_tokens":0
        },
        "diagnostics":{"cache_miss_reason":{"type":"previous_message_not_found"}}
    }"#;

    let timing = parse_result_timing(line).expect("result timing must parse");

    assert_eq!(timing.duration_ms, Some(52014));
    assert_eq!(timing.duration_api_ms, Some(51997));
    assert_eq!(timing.ttft_ms, Some(40033));
    assert_eq!(timing.input_tokens, Some(5));
    assert_eq!(timing.output_tokens, Some(3445));
    assert_eq!(timing.cache_creation_input_tokens, Some(190890));
    assert_eq!(timing.cache_read_input_tokens, Some(0));
    assert_eq!(
        timing.cache_miss_reason.as_deref(),
        Some("previous_message_not_found")
    );
}

#[test]
fn parse_result_timing_ignores_non_result_lines() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;

    assert_eq!(parse_result_timing(line), None);
}

#[test]
fn parse_cache_miss_reason_extracts_from_assistant_diagnostics() {
    let line = r#"{
        "type":"assistant",
        "message":{
            "content":[{"type":"thinking","thinking":""}],
            "diagnostics":{"cache_miss_reason":{"type":"system_changed"}}
        }
    }"#;

    assert_eq!(
        parse_cache_miss_reason(line).as_deref(),
        Some("system_changed")
    );
}
```

- [ ] **Step 2: Run parser tests and verify failure**

Run: `devenv shell -- cargo test -p right-bot parse_result_timing`

Expected: FAIL because `parse_result_timing` and `ResultTiming` are not defined yet.

Run: `devenv shell -- cargo test -p right-bot parse_cache_miss_reason`

Expected: FAIL because `parse_cache_miss_reason` is not defined yet.

- [ ] **Step 3: Add parser implementation**

In `crates/bot/src/cc/stream.rs`, after `parse_usage_full`, add:

```rust
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
        cache_miss_reason: cache_miss_reason_from_value(&v),
    })
}

pub(crate) fn parse_cache_miss_reason(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    cache_miss_reason_from_value(&v)
}

fn optional_u64(v: &serde_json::Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(serde_json::Value::as_u64)
}

fn cache_miss_reason_from_value(v: &serde_json::Value) -> Option<String> {
    v.pointer("/message/diagnostics/cache_miss_reason/type")
        .or_else(|| v.pointer("/diagnostics/cache_miss_reason/type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}
```

- [ ] **Step 4: Run parser tests and verify pass**

Run: `devenv shell -- cargo test -p right-bot parse_result_timing`

Expected: PASS.

Run: `devenv shell -- cargo test -p right-bot parse_cache_miss_reason`

Expected: PASS.

- [ ] **Step 5: Commit parser changes**

```bash
git add crates/bot/src/cc/stream.rs
git commit -m "feat(bot): parse claude result timing"
```

## Task 3: Initial Anchor Render and Send Helpers

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add failing render-helper tests**

In `crates/bot/src/telegram/worker.rs`, inside `#[cfg(test)] mod tests`, add these tests after `thinking_anchor_text_expanded_uses_stream_formatter`:

```rust
#[test]
fn thinking_anchor_render_collapsed_uses_working_text_and_keyboard() {
    let events = VecDeque::new();
    let usage = crate::cc::stream::StreamUsage::default();

    let render = build_thinking_anchor_render(12345, 678, false, false, &events, &usage);

    assert_eq!(render.text, "\u{23f3} Working...");
    assert_eq!(
        keyboard_row(render.keyboard),
        vec![
            (
                "\u{1f4ad} Show thinking".to_string(),
                "think:12345:678:show".to_string()
            ),
            ("\u{1f6d1} Stop".to_string(), "stop:12345:678".to_string()),
            (
                "\u{2699}\u{fe0f} Background it".to_string(),
                "bg:12345:678".to_string()
            ),
        ]
    );
}

#[test]
fn thinking_anchor_render_expanded_group_uses_preview_and_group_keyboard() {
    let mut events = VecDeque::new();
    events.push_back(crate::cc::stream::StreamEvent::Thinking);
    let usage = crate::cc::stream::StreamUsage {
        num_turns: 1,
        cost_usd: 0.0,
    };

    let render = build_thinking_anchor_render(-100123, 0, true, true, &events, &usage);

    assert!(render.text.contains("thinking..."));
    assert_eq!(
        keyboard_row(render.keyboard),
        vec![
            ("\u{1f6d1} Stop".to_string(), "stop:-100123:0".to_string()),
            (
                "\u{2699}\u{fe0f} Background it".to_string(),
                "bg:-100123:0".to_string()
            ),
        ]
    );
}
```

- [ ] **Step 2: Run render-helper tests and verify failure**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor_render`

Expected: FAIL because `build_thinking_anchor_render` is not defined.

- [ ] **Step 3: Add render and send helpers**

In `crates/bot/src/telegram/worker.rs`, after `thinking_anchor_text`, add:

```rust
struct ThinkingAnchorRender {
    text: String,
    keyboard: teloxide::types::InlineKeyboardMarkup,
}

fn build_thinking_anchor_render(
    chat_id: i64,
    eff_thread_id: i64,
    expanded: bool,
    is_group: bool,
    events: &VecDeque<crate::cc::stream::StreamEvent>,
    usage: &crate::cc::stream::StreamUsage,
) -> ThinkingAnchorRender {
    ThinkingAnchorRender {
        text: thinking_anchor_text(expanded, events, usage),
        keyboard: working_keyboard(
            chat_id,
            eff_thread_id,
            thinking_keyboard_mode(expanded, is_group),
        ),
    }
}

async fn send_thinking_anchor(
    ctx: &WorkerContext,
    tg_chat_id: teloxide::types::ChatId,
    chat_id: i64,
    eff_thread_id: i64,
    expanded: bool,
    is_group: bool,
    events: &VecDeque<crate::cc::stream::StreamEvent>,
    usage: &crate::cc::stream::StreamUsage,
) -> Option<teloxide::types::MessageId> {
    let render =
        build_thinking_anchor_render(chat_id, eff_thread_id, expanded, is_group, events, usage);
    let mut send = ctx
        .bot
        .send_message(tg_chat_id, &render.text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(render.keyboard);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }

    match send.await {
        Ok(msg) => Some(msg.id),
        Err(e) => {
            tracing::warn!(
                chat_id,
                eff_thread_id,
                key = ?(chat_id, eff_thread_id),
                "send thinking anchor failed: {e:#}"
            );
            None
        }
    }
}
```

- [ ] **Step 4: Run render-helper tests and verify pass**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor_render`

Expected: PASS.

- [ ] **Step 5: Commit helper changes**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): add thinking anchor helper"
```

## Task 4: Send Anchor Immediately After Invocation Starts

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add flow-focused helper test for fallback semantics**

In `crates/bot/src/telegram/worker.rs`, inside `#[cfg(test)] mod tests`, add this pure helper near the other thinking tests:

```rust
#[test]
fn thinking_anchor_render_empty_expanded_starts_without_stream_event() {
    let events = VecDeque::new();
    let usage = crate::cc::stream::StreamUsage::default();

    let render = build_thinking_anchor_render(12345, 678, true, false, &events, &usage);

    assert!(render.text.contains("starting..."));
    assert!(render.text.contains("Turn 0"));
    assert_eq!(
        keyboard_row(render.keyboard)[0],
        (
            "\u{1f4ad} Hide thinking".to_string(),
            "think:12345:678:hide".to_string()
        )
    );
}
```

- [ ] **Step 2: Run focused worker thinking tests**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor`

Expected: PASS before flow edits. This proves the helper can render an anchor without any stream event.

- [ ] **Step 3: Move initial anchor creation before the stream loop**

In `crates/bot/src/telegram/worker.rs`, inside `invoke_cc`, replace the initial `thinking_msg_id`, `last_edit`, and `last_rendered_event_count` setup block with:

```rust
let mut ring_buffer = crate::cc::stream::EventRingBuffer::new(5);
let mut usage = crate::cc::stream::StreamUsage::default();
let mut result_line: Option<String> = None;
let mut api_key_source: Option<String> = None;
let tg_chat_id = ctx.chat_id;

let initial_expanded = read_expanded();
let mut thinking_msg_id = send_thinking_anchor(
    ctx,
    tg_chat_id,
    chat_id,
    eff_thread_id,
    initial_expanded,
    is_group,
    ring_buffer.events(),
    &usage,
)
.await;
let mut last_edit = tokio::time::Instant::now();
let mut last_rendered_expanded = initial_expanded;
let mut last_rendered_event_count: u32 = 0;
let mut ui_tick = tokio::time::interval(Duration::from_millis(500));
ui_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
let mut total_assistant_events: u32 = 0;
```

Remove the older duplicated initialization lines for these same variables. Keep the existing `learning_invocation_id`, `execution_event_scope`, `deadline`, `timed_out`, and `stopped` setup after this block.

- [ ] **Step 4: Replace first-stream-event send block with fallback helper call**

In the stream loop branch that currently starts with `if crate::cc::stream::format_event(&event).is_some()`, replace only the inner `if thinking_msg_id.is_none()` send-message body with:

```rust
if thinking_msg_id.is_none() {
    let text_expanded = read_expanded();
    thinking_msg_id = send_thinking_anchor(
        ctx,
        tg_chat_id,
        chat_id,
        eff_thread_id,
        text_expanded,
        is_group,
        ring_buffer.events(),
        &usage,
    )
    .await;
    if thinking_msg_id.is_some() {
        last_rendered_expanded = text_expanded;
        last_rendered_event_count = total_assistant_events;
    }
    last_edit = tokio::time::Instant::now();
}
```

The surrounding `if format_event(...).is_some()` condition stays in place. This keeps retry behavior when the immediate anchor send fails.

- [ ] **Step 5: Run focused worker thinking tests**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor`

Expected: PASS.

- [ ] **Step 6: Run broader worker tests**

Run: `devenv shell -- cargo test -p right-bot telegram::worker::tests::`

Expected: PASS.

- [ ] **Step 7: Commit flow changes**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): show working anchor after invoke start"
```

## Task 5: Log Result Timing and Cache Diagnostics

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add logging helper**

In `crates/bot/src/telegram/worker.rs`, after `log_claude_finished`, add:

```rust
fn log_result_timing(ctx: &InvocationLogContext, timing: &crate::cc::stream::ResultTiming) {
    tracing::info!(
        chat_id = ctx.chat_id,
        eff_thread_id = ctx.eff_thread_id,
        key = ?ctx.key(),
        session_uuid = %ctx.session_uuid,
        turn_id = ctx.turn_id,
        duration_ms = ?timing.duration_ms,
        duration_api_ms = ?timing.duration_api_ms,
        ttft_ms = ?timing.ttft_ms,
        input_tokens = ?timing.input_tokens,
        output_tokens = ?timing.output_tokens,
        cache_creation_input_tokens = ?timing.cache_creation_input_tokens,
        cache_read_input_tokens = ?timing.cache_read_input_tokens,
        cache_miss_reason = ?timing.cache_miss_reason.as_deref(),
        "claude result timing"
    );
}
```

- [ ] **Step 2: Capture cache miss reason across stream lines**

In `invoke_cc`, near `let mut api_key_source: Option<String> = None;`, add:

```rust
let mut cache_miss_reason: Option<String> = None;
```

Inside the `Ok(Some(line))` branch, immediately after the `api_key_source` extraction block, add:

```rust
if cache_miss_reason.is_none() {
    cache_miss_reason = crate::cc::stream::parse_cache_miss_reason(&line);
}
```

- [ ] **Step 3: Log timing in result-event branch**

In the `StreamEvent::Result(json)` branch, immediately after `result_line = Some(json.clone());`, add:

```rust
if let Some(mut timing) = crate::cc::stream::parse_result_timing(json) {
    if timing.cache_miss_reason.is_none() {
        timing.cache_miss_reason = cache_miss_reason.clone();
    }
    log_result_timing(&log_ctx, &timing);
}
```

- [ ] **Step 4: Run timing and worker log tests**

Run: `devenv shell -- cargo test -p right-bot parse_result_timing`

Expected: PASS.

Run: `devenv shell -- cargo test -p right-bot capture_worker_log`

Expected: PASS. Existing log-capture tests continue compiling with the new helper.

- [ ] **Step 5: Commit diagnostics logging**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): log claude result timing"
```

## Task 6: Update Sessions Architecture Doc

**Files:**
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update Stream Logging section**

In `docs/architecture/sessions.md`, in the paragraph beginning `Thinking messages in Telegram are per-run UI anchors`, replace it with:

```markdown
Thinking messages in Telegram are per-run UI anchors with Stop and Background
buttons. Foreground workers create the anchor after a real Claude subprocess has
started, stdin has been written, and the per-run stop/visibility state is
registered; they do not wait for Claude's first stream event. In direct chats,
`show_thinking: true` starts expanded and shows the last 5 displayable stream
events (tool calls, thinking, text) with turn counter and cost;
`show_thinking: false` starts collapsed as `Working...`. Users can toggle the
active run with `Show thinking` / `Hide thinking` without changing `agent.yaml`.
```

- [ ] **Step 2: Check doc diff**

Run: `devenv shell -- git diff -- docs/architecture/sessions.md`

Expected: diff only documents immediate foreground anchor timing.

- [ ] **Step 3: Commit doc update**

```bash
git add docs/architecture/sessions.md
git commit -m "docs(sessions): document immediate working anchor"
```

## Task 7: Final Verification

**Files:**
- Verify: all modified files

- [ ] **Step 1: Run targeted stream parser tests**

Run: `devenv shell -- cargo test -p right-bot parse_result_timing`

Expected: PASS.

Run: `devenv shell -- cargo test -p right-bot parse_cache_miss_reason`

Expected: PASS.

- [ ] **Step 2: Run targeted worker thinking tests**

Run: `devenv shell -- cargo test -p right-bot thinking_anchor`

Expected: PASS.

- [ ] **Step 3: Run right-bot package tests**

Run: `devenv shell -- cargo test -p right-bot`

Expected: PASS.

- [ ] **Step 4: Run mandatory full workspace tests**

Run: `devenv shell -- cargo test --workspace`

Expected: PASS.

- [ ] **Step 5: Inspect final history and status**

Run: `devenv shell -- git status --short`

Expected: clean worktree.

Run: `devenv shell -- git log --oneline -5`

Expected: shows the implementation commits for parser, helper, flow, diagnostics, and docs.
