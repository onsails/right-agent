# Show Thinking Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Telegram inline `Show thinking` / `Hide thinking` control for the current running thinking message, with group chats collapsed by default and expandable on demand.

**Architecture:** Add process-local thinking visibility state next to the existing stop/background control maps. Callback handlers mutate only that in-memory state; the worker remains the sole owner of Telegram message sends/edits and observes visibility changes on a small UI tick while Claude Code runs.

**Tech Stack:** Rust 2024, `right-bot`, teloxide inline keyboards/callback queries, `DashMap`, tokio `select!`, existing CC `stream-json` formatter.

**Issue:** https://github.com/onsails/right-agent/issues/50

**Required implementation commit:** the main feature commit must use `Closes #50`, for example:

```bash
git commit -m "feat(bot): add show thinking toggle" -m "Closes #50"
```

---

## Scope

This plan implements the approved design in `docs/superpowers/specs/2026-05-12-show-thinking-toggle-design.md`.

It deliberately does not modify `agent.yaml`, does not hot-reload `show_thinking`, and does not change defaults for future turns. The toggle is per active run.

## File Structure

- Modify `crates/bot/src/telegram/mod.rs`
  - Add shared `ThinkingVisibility` state, `ThinkingVisibilityState`, `ThinkingToggleAction`, and pure helper functions.
  - Extend `WorkerControlDeps` so handlers and workers share the same state map.
- Modify `crates/bot/src/telegram/worker.rs`
  - Replace the fixed Stop/Background keyboard with mode-aware keyboard helpers.
  - Add `thinking_visibility` to `WorkerContext`.
  - Initialize/remove per-run visibility state in `invoke_cc`.
  - Render thinking messages from visibility state and apply callback changes on a periodic UI tick.
- Modify `crates/bot/src/telegram/handler.rs`
  - Add `handle_thinking_toggle_callback`.
  - Use shared parser/state helpers from `telegram/mod.rs`.
  - Pass `thinking_visibility` into `WorkerContext`.
- Modify `crates/bot/src/telegram/dispatch.rs`
  - Create the visibility map once per bot process.
  - Inject it through `WorkerControlDeps`.
  - Add the `think:` callback branch before background/stop handling.
- Modify `docs/architecture/sessions.md`
  - Document the new per-message toggle and group-chat default.

---

### Task 1: Shared Thinking Visibility State

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write failing tests for visibility helpers**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/bot/src/telegram/mod.rs`:

```rust
#[test]
fn initial_thinking_visibility_respects_context() {
    for (show_thinking, is_group, expected) in [
        (true, false, true),
        (false, false, false),
        (true, true, false),
        (false, true, false),
    ] {
        let state = initial_thinking_visibility(show_thinking, is_group);
        assert_eq!(state.expanded, expected);
        assert_eq!(state.version, 0);
    }
}

#[test]
fn parse_thinking_toggle_callback_accepts_valid_data() {
    assert_eq!(
        parse_thinking_toggle_callback("think:12345:678:show"),
        Some(((12345, 678), ThinkingToggleAction::Show))
    );
    assert_eq!(
        parse_thinking_toggle_callback("think:-100123:0:hide"),
        Some(((-100123, 0), ThinkingToggleAction::Hide))
    );
}

#[test]
fn parse_thinking_toggle_callback_rejects_malformed_data() {
    for bad in [
        "",
        "think",
        "think:1",
        "think:1:2",
        "think:1:2:toggle",
        "think:not-a-chat:2:show",
        "think:1:not-a-thread:show",
        "stop:1:2",
    ] {
        assert_eq!(parse_thinking_toggle_callback(bad), None, "bad={bad}");
    }
}

#[test]
fn set_thinking_visibility_updates_version_only_on_change() {
    let map: ThinkingVisibility = Arc::new(DashMap::new());
    let key = (12345_i64, 0_i64);
    map.insert(
        key,
        ThinkingVisibilityState {
            expanded: false,
            version: 0,
        },
    );

    assert!(set_thinking_visibility(&map, key, true));
    assert_eq!(
        *map.get(&key).unwrap().value(),
        ThinkingVisibilityState {
            expanded: true,
            version: 1
        }
    );

    assert!(set_thinking_visibility(&map, key, true));
    assert_eq!(map.get(&key).unwrap().version, 1, "same mode does not bump version");

    assert!(set_thinking_visibility(&map, key, false));
    assert_eq!(
        *map.get(&key).unwrap().value(),
        ThinkingVisibilityState {
            expanded: false,
            version: 2
        }
    );
    assert!(!set_thinking_visibility(&map, (999, 0), true));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_visibility --lib
```

Expected: FAIL with unresolved names such as `initial_thinking_visibility`, `ThinkingVisibility`, and `parse_thinking_toggle_callback`.

- [ ] **Step 3: Add visibility state and parser helpers**

In `crates/bot/src/telegram/mod.rs`, add the following after `pub(crate) type BgRequests = ...`:

```rust
/// Current thinking-preview visibility for an active CC invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThinkingVisibilityState {
    pub(crate) expanded: bool,
    pub(crate) version: u64,
}

/// User-requested thinking visibility action from an inline callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingToggleAction {
    Show,
    Hide,
}

impl ThinkingToggleAction {
    pub(crate) fn expanded(self) -> bool {
        matches!(self, Self::Show)
    }
}

/// Per-(chat, thread) thinking-preview visibility for active CC sessions.
///
/// Key: (chat_id, eff_thread_id). Value: current visibility and render version.
/// The worker inserts at run start and removes on run completion.
pub(crate) type ThinkingVisibility = Arc<DashMap<(i64, i64), ThinkingVisibilityState>>;

/// Initial thinking visibility for a run. Direct chats honor config; groups stay quiet.
pub(crate) fn initial_thinking_visibility(
    show_thinking: bool,
    is_group: bool,
) -> ThinkingVisibilityState {
    ThinkingVisibilityState {
        expanded: show_thinking && !is_group,
        version: 0,
    }
}

/// Parse `think:{chat_id}:{eff_thread_id}:{show|hide}` callback data.
pub(crate) fn parse_thinking_toggle_callback(
    data: &str,
) -> Option<((i64, i64), ThinkingToggleAction)> {
    let mut parts = data.splitn(4, ':');
    let prefix = parts.next()?;
    let chat_id = parts.next()?.parse::<i64>().ok()?;
    let thread_id = parts.next()?.parse::<i64>().ok()?;
    let action = match parts.next()? {
        "show" => ThinkingToggleAction::Show,
        "hide" => ThinkingToggleAction::Hide,
        _ => return None,
    };
    if prefix != "think" {
        return None;
    }
    Some(((chat_id, thread_id), action))
}

/// Update active visibility. Returns false when the run already finished.
pub(crate) fn set_thinking_visibility(
    map: &ThinkingVisibility,
    key: (i64, i64),
    expanded: bool,
) -> bool {
    let Some(mut entry) = map.get_mut(&key) else {
        return false;
    };
    let state = entry.value_mut();
    if state.expanded != expanded {
        state.expanded = expanded;
        state.version = state.version.saturating_add(1);
    }
    true
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_visibility --lib
devenv shell -- cargo test -p right-bot parse_thinking_toggle_callback --lib
```

Expected: PASS.

- [ ] **Step 5: Commit shared state**

Run:

```bash
git add crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add thinking visibility state"
```

---

### Task 2: Mode-Aware Thinking Keyboard

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Replace the existing keyboard test with failing mode tests**

In `crates/bot/src/telegram/worker.rs`, replace `working_keyboard_has_stop_and_background` with:

```rust
fn keyboard_row(kb: teloxide::types::InlineKeyboardMarkup) -> Vec<(String, String)> {
    kb.inline_keyboard
        .into_iter()
        .next()
        .unwrap()
        .into_iter()
        .map(|button| {
            let data = match button.kind {
                teloxide::types::InlineKeyboardButtonKind::CallbackData(data) => data,
                _ => panic!("button must use callback data"),
            };
            (button.text, data)
        })
        .collect()
}

#[test]
fn working_keyboard_modes_render_expected_buttons() {
    for (chat, thread, mode, expected) in [
        (
            12345,
            678,
            ThinkingKeyboardMode::Collapsed,
            vec![
                ("Show thinking", "think:12345:678:show"),
                ("\u{26d4} Stop", "stop:12345:678"),
                ("\u{1f319} Background", "bg:12345:678"),
            ],
        ),
        (
            12345,
            678,
            ThinkingKeyboardMode::ExpandedDirect,
            vec![
                ("Hide thinking", "think:12345:678:hide"),
                ("\u{26d4} Stop", "stop:12345:678"),
                ("\u{1f319} Background", "bg:12345:678"),
            ],
        ),
        (
            -100123,
            0,
            ThinkingKeyboardMode::ExpandedGroup,
            vec![
                ("\u{26d4} Stop", "stop:-100123:0"),
                ("\u{1f319} Background", "bg:-100123:0"),
            ],
        ),
    ] {
        let actual = keyboard_row(working_keyboard(chat, thread, mode));
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(text, data)| (text.to_string(), data.to_string()))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn thinking_keyboard_mode_maps_visibility_and_chat_type() {
    assert_eq!(
        thinking_keyboard_mode(false, false),
        ThinkingKeyboardMode::Collapsed
    );
    assert_eq!(
        thinking_keyboard_mode(false, true),
        ThinkingKeyboardMode::Collapsed
    );
    assert_eq!(
        thinking_keyboard_mode(true, false),
        ThinkingKeyboardMode::ExpandedDirect
    );
    assert_eq!(
        thinking_keyboard_mode(true, true),
        ThinkingKeyboardMode::ExpandedGroup
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot working_keyboard --lib
```

Expected: FAIL because `ThinkingKeyboardMode`, `thinking_keyboard_mode`, and the new `working_keyboard` signature do not exist.

- [ ] **Step 3: Implement keyboard modes**

In `crates/bot/src/telegram/worker.rs`, replace the current `working_keyboard` helper with:

```rust
/// Inline keyboard mode for the active thinking message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingKeyboardMode {
    Collapsed,
    ExpandedDirect,
    ExpandedGroup,
}

fn thinking_keyboard_mode(expanded: bool, is_group: bool) -> ThinkingKeyboardMode {
    match (expanded, is_group) {
        (false, _) => ThinkingKeyboardMode::Collapsed,
        (true, false) => ThinkingKeyboardMode::ExpandedDirect,
        (true, true) => ThinkingKeyboardMode::ExpandedGroup,
    }
}

/// Build the inline keyboard for thinking messages.
fn working_keyboard(
    chat_id: i64,
    eff_thread_id: i64,
    mode: ThinkingKeyboardMode,
) -> teloxide::types::InlineKeyboardMarkup {
    let mut row = Vec::new();

    match mode {
        ThinkingKeyboardMode::Collapsed => {
            row.push(teloxide::types::InlineKeyboardButton::callback(
                "Show thinking",
                format!("think:{chat_id}:{eff_thread_id}:show"),
            ));
        }
        ThinkingKeyboardMode::ExpandedDirect => {
            row.push(teloxide::types::InlineKeyboardButton::callback(
                "Hide thinking",
                format!("think:{chat_id}:{eff_thread_id}:hide"),
            ));
        }
        ThinkingKeyboardMode::ExpandedGroup => {}
    }

    row.push(teloxide::types::InlineKeyboardButton::callback(
        "\u{26d4} Stop",
        format!("stop:{chat_id}:{eff_thread_id}"),
    ));
    row.push(teloxide::types::InlineKeyboardButton::callback(
        "\u{1f319} Background",
        format!("bg:{chat_id}:{eff_thread_id}"),
    ));

    teloxide::types::InlineKeyboardMarkup::new(vec![row])
}
```

- [ ] **Step 4: Temporarily fix compile call sites**

The old `working_keyboard(chat_id, eff_thread_id)` call sites will not compile. Temporarily change each call in `invoke_cc` to:

```rust
let kb = working_keyboard(
    chat_id,
    eff_thread_id,
    thinking_keyboard_mode(ctx.show_thinking && !is_group, is_group),
);
```

This preserves current runtime behavior until Task 4 replaces it with active visibility state.

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot working_keyboard --lib
```

Expected: PASS.

- [ ] **Step 6: Commit keyboard helpers**

Run:

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): add thinking keyboard modes"
```

---

### Task 3: Callback Handler and Dependency Wiring

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write failing handler/dispatch tests**

Add these tests to the existing `#[cfg(test)] mod tests` in `crates/bot/src/telegram/handler.rs`:

```rust
#[test]
fn thinking_toggle_show_updates_active_visibility() {
    let map: super::ThinkingVisibility = Arc::new(DashMap::new());
    let key = (42_i64, 7_i64);
    map.insert(
        key,
        super::ThinkingVisibilityState {
            expanded: false,
            version: 0,
        },
    );

    let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
    assert_eq!(text, Some("Showing thinking..."));

    let state = *map.get(&key).unwrap().value();
    assert!(state.expanded);
    assert_eq!(state.version, 1);
}

#[test]
fn thinking_toggle_hide_updates_active_visibility() {
    let map: super::ThinkingVisibility = Arc::new(DashMap::new());
    let key = (42_i64, 7_i64);
    map.insert(
        key,
        super::ThinkingVisibilityState {
            expanded: true,
            version: 0,
        },
    );

    let text = apply_thinking_toggle_callback(&map, "think:42:7:hide");
    assert_eq!(text, Some("Hiding thinking..."));

    let state = *map.get(&key).unwrap().value();
    assert!(!state.expanded);
    assert_eq!(state.version, 1);
}

#[test]
fn thinking_toggle_after_finish_reports_already_finished() {
    let map: super::ThinkingVisibility = Arc::new(DashMap::new());

    let text = apply_thinking_toggle_callback(&map, "think:42:7:show");
    assert_eq!(text, Some("Already finished"));
}

#[test]
fn thinking_toggle_malformed_callback_returns_none() {
    let map: super::ThinkingVisibility = Arc::new(DashMap::new());

    assert_eq!(apply_thinking_toggle_callback(&map, "think:42:7"), None);
    assert_eq!(apply_thinking_toggle_callback(&map, "stop:42:7"), None);
}
```

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_toggle --lib
```

Expected: FAIL until the handler imports and shared state wiring compile.

- [ ] **Step 2: Add the callback handler**

In `crates/bot/src/telegram/handler.rs`, add this pure helper and handler near the existing Stop and Background callback handlers:

```rust
fn apply_thinking_toggle_callback(
    thinking_visibility: &super::ThinkingVisibility,
    data: &str,
) -> Option<&'static str> {
    let (key, action) = super::parse_thinking_toggle_callback(data)?;
    if super::set_thinking_visibility(thinking_visibility, key, action.expanded()) {
        Some(match action {
            super::ThinkingToggleAction::Show => "Showing thinking...",
            super::ThinkingToggleAction::Hide => "Hiding thinking...",
        })
    } else {
        Some("Already finished")
    }
}

/// Handle Show/Hide thinking callback queries from thinking messages.
///
/// Callback data format: `think:{chat_id}:{eff_thread_id}:{show|hide}`.
pub async fn handle_thinking_toggle_callback(
    bot: BotType,
    q: CallbackQuery,
    worker_ctl: super::WorkerControlDeps,
) -> ResponseResult<()> {
    let qid = q.id;
    let text = q.data.as_deref().and_then(|data| {
        apply_thinking_toggle_callback(&worker_ctl.thinking_visibility, data)
    });

    let mut answer = bot.answer_callback_query(qid);
    if let Some(t) = text {
        answer = answer.text(t);
    }
    answer.await?;

    Ok(())
}
```

- [ ] **Step 3: Wire callback dispatch**

In `crates/bot/src/telegram/dispatch.rs`, add `handle_thinking_toggle_callback` to the handler import list.

Then add this callback branch before the `bg:` branch:

```rust
.branch(
    dptree::filter(|q: CallbackQuery| {
        q.data.as_deref().is_some_and(|d| d.starts_with("think:"))
    })
    .endpoint(handle_thinking_toggle_callback),
)
```

- [ ] **Step 4: Wire shared visibility map through dispatcher dependencies**

In `crates/bot/src/telegram/mod.rs`, update `WorkerControlDeps` to include the new map:

```rust
#[derive(Clone)]
pub struct WorkerControlDeps {
    pub(crate) stop_tokens: StopTokens,
    pub(crate) session_locks: SessionLocks,
    pub(crate) bg_requests: BgRequests,
    pub(crate) thinking_visibility: ThinkingVisibility,
}
```

Update the `WorkerControlDeps` doc comment in the same file so it says four
maps instead of three and includes `thinking_visibility` in the bullet list:

```rust
/// All four maps share bot-process lifetime and are injected together:
/// - `stop_tokens`: per-(chat, thread) cancellation tokens for in-flight CC subprocesses.
/// - `session_locks`: per-main-session async mutex map (TOCTOU on session JSONL).
/// - `bg_requests`: per-(chat, thread) Background-button request flags.
/// - `thinking_visibility`: per-(chat, thread) Show/Hide thinking state for active runs.
```

In `run_telegram`, create the map beside `stop_tokens`:

```rust
let stop_tokens: super::StopTokens = Arc::new(DashMap::new());
let thinking_visibility: super::ThinkingVisibility = Arc::new(DashMap::new());
```

Pass `Arc::clone(&thinking_visibility)` to `build_dispatcher`.

Update the `build_dispatcher` signature:

```rust
thinking_visibility: super::ThinkingVisibility,
```

Update `WorkerControlDeps` construction:

```rust
let worker_ctl = super::WorkerControlDeps {
    stop_tokens,
    session_locks,
    bg_requests,
    thinking_visibility,
};
```

Update `dispatcher_builds_without_panic` in `dispatch.rs` to construct and pass the map:

```rust
let thinking_visibility: super::super::ThinkingVisibility = Arc::new(DashMap::new());
```

Then include it in the `build_dispatcher(...)` test call in the same argument position as production.

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_toggle --lib
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic --lib
```

Expected: PASS.

- [ ] **Step 6: Commit callback wiring**

Run:

```bash
git add crates/bot/src/telegram/mod.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): handle thinking toggle callbacks"
```

---

### Task 4: Worker Rendering and UI Tick

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Write failing worker rendering tests**

Add these tests to `crates/bot/src/telegram/worker.rs`:

```rust
#[test]
fn thinking_anchor_text_collapsed_is_static_working_message() {
    let events = VecDeque::new();
    let usage = crate::cc::stream::StreamUsage::default();

    assert_eq!(
        thinking_anchor_text(false, &events, &usage),
        "\u{23f3} Working..."
    );
}

#[test]
fn thinking_anchor_text_expanded_uses_stream_formatter() {
    let mut events = VecDeque::new();
    events.push_back(crate::cc::stream::StreamEvent::Thinking);
    let usage = crate::cc::stream::StreamUsage {
        num_turns: 1,
        cost_usd: 0.0,
    };

    let text = thinking_anchor_text(true, &events, &usage);
    assert!(text.contains("thinking..."));
    assert!(text.contains("Turn 1"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_anchor_text --lib
```

Expected: FAIL because `thinking_anchor_text` does not exist.

- [ ] **Step 3: Add worker rendering helper**

In `crates/bot/src/telegram/worker.rs`, add this helper near the keyboard helpers:

```rust
fn thinking_anchor_text(
    expanded: bool,
    events: &VecDeque<crate::cc::stream::StreamEvent>,
    usage: &crate::cc::stream::StreamUsage,
) -> String {
    if expanded {
        crate::cc::stream::format_thinking_message(events, usage)
    } else {
        "\u{23f3} Working...".to_string()
    }
}
```

- [ ] **Step 4: Add visibility state to worker context construction**

In `crates/bot/src/telegram/worker.rs`, add this field to `WorkerContext` after `bg_requests`:

```rust
/// Per-run thinking-preview visibility, mutated by Show/Hide thinking callbacks.
pub thinking_visibility: super::ThinkingVisibility,
```

In `crates/bot/src/telegram/handler.rs`, add this field where `WorkerContext` is constructed:

```rust
thinking_visibility: Arc::clone(&worker_ctl.thinking_visibility),
```

- [ ] **Step 5: Initialize active visibility in `invoke_cc`**

In `crates/bot/src/telegram/worker.rs`, immediately after inserting the stop token in `invoke_cc`, add:

```rust
let visibility_key = (chat_id, eff_thread_id);
let fallback_visibility = super::initial_thinking_visibility(ctx.show_thinking, is_group);
ctx.thinking_visibility
    .insert(visibility_key, fallback_visibility);
let mut last_rendered_visibility_version = fallback_visibility.version;
let read_visibility = || {
    ctx.thinking_visibility
        .get(&visibility_key)
        .map(|entry| *entry.value())
        .unwrap_or(fallback_visibility)
};
```

After `let mut last_edit = tokio::time::Instant::now();`, add:

```rust
let mut ui_tick = tokio::time::interval(Duration::from_millis(500));
ui_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
```

- [ ] **Step 6: Replace stream-event thinking rendering with visibility-aware send**

In the existing `if crate::cc::stream::format_event(&event).is_some()` thinking-message block, replace the keyboard/text selection with:

```rust
let visibility = read_visibility();
let kb = working_keyboard(
    chat_id,
    eff_thread_id,
    thinking_keyboard_mode(visibility.expanded, is_group),
);

if thinking_msg_id.is_none() {
    let text = thinking_anchor_text(visibility.expanded, ring_buffer.events(), &usage);
    let mut send = ctx
        .bot
        .send_message(tg_chat_id, &text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(kb);
    if eff_thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Ok(msg) = send.await {
        thinking_msg_id = Some(msg.id);
        last_rendered_visibility_version = visibility.version;
    }
    last_edit = tokio::time::Instant::now();
}
```

Delete the old `else if ctx.show_thinking && !is_group && last_edit.elapsed() >= ...` branch. The UI tick handles updates for both live refresh and Show/Hide changes.

- [ ] **Step 7: Add UI tick branch to the stream loop**

In the `tokio::select!` loop in `invoke_cc`, add this branch before the deadline branch:

```rust
_ = ui_tick.tick(), if thinking_msg_id.is_some() => {
    let visibility = read_visibility();
    let should_edit_for_toggle = visibility.version != last_rendered_visibility_version;
    let should_edit_for_live_refresh =
        visibility.expanded && last_edit.elapsed() >= Duration::from_secs(2);

    if should_edit_for_toggle || should_edit_for_live_refresh {
        let text = thinking_anchor_text(visibility.expanded, ring_buffer.events(), &usage);
        let kb = working_keyboard(
            chat_id,
            eff_thread_id,
            thinking_keyboard_mode(visibility.expanded, is_group),
        );

        if let Some(msg_id) = thinking_msg_id {
            let _ = ctx
                .bot
                .edit_message_text(tg_chat_id, msg_id, &text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .reply_markup(kb)
                .await;
            last_rendered_visibility_version = visibility.version;
            last_edit = tokio::time::Instant::now();
        }
    }
}
```

This keeps live updates at the existing two-second cadence, while Show/Hide changes apply within 500ms even if no new stream event arrives.

- [ ] **Step 8: Finalize and clean up visibility state**

Before removing the stop token, capture final visibility and remove both active maps:

```rust
let final_visibility = read_visibility();
ctx.stop_tokens.remove(&(chat_id, eff_thread_id));
ctx.thinking_visibility.remove(&visibility_key);
```

In the final thinking message update block, replace `ctx.show_thinking && !is_group` checks with `final_visibility.expanded`.

For stopped turns, use the active mode for final text:

```rust
let text = if final_visibility.expanded {
    let mut msg = crate::cc::stream::format_thinking_message(ring_buffer.events(), &usage);
    msg.push_str("\n\u{26d4} Stopped");
    msg
} else {
    "\u{23f3} Working...\n\u{26d4} Stopped".to_string()
};
```

For normal completion, keep the final live preview only when expanded; otherwise delete the collapsed anchor:

```rust
} else if !will_reflect && final_visibility.expanded {
    let text = crate::cc::stream::format_thinking_message(ring_buffer.events(), &usage);
    let _ = ctx.bot.edit_message_text(tg_chat_id, msg_id, &text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(teloxide::types::InlineKeyboardMarkup::default())
        .await;
} else if !will_reflect {
    let _ = ctx.bot.delete_message(tg_chat_id, msg_id).await;
}
```

- [ ] **Step 9: Run tests to verify they pass**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_anchor_text --lib
devenv shell -- cargo test -p right-bot working_keyboard --lib
devenv shell -- cargo test -p right-bot thinking_toggle --lib
```

Expected: PASS.

- [ ] **Step 10: Commit worker integration with issue-closing footer**

Run:

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): add show thinking toggle" -m "Closes #50"
```

---

### Task 5: Architecture Documentation

**Files:**
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update the thinking-message paragraph**

In `docs/architecture/sessions.md`, replace the current paragraph that starts with `When show_thinking: true` with:

```markdown
Thinking messages in Telegram are per-run UI anchors with Stop and Background
buttons. In direct chats, `show_thinking: true` starts expanded and shows the
last 5 displayable stream events (tool calls, thinking, text) with turn counter
and cost; `show_thinking: false` starts collapsed as `Working...`. Users can
toggle the active run with `Show thinking` / `Hide thinking` without changing
`agent.yaml`.

Group chats always start collapsed as `Working...` to keep shared rooms quiet.
They include `Show thinking`; after expansion the run shows the same live event
preview, but no `Hide thinking` button is shown in groups. Live expanded
messages refresh every 2s via `editMessageText`. Collapsed messages stay static
until completion, stop, timeout, reflection, or background handoff.
```

- [ ] **Step 2: Inspect the rendered doc context**

Run:

```bash
devenv shell -- sed -n '1,45p' docs/architecture/sessions.md
```

Expected: The stream logging section describes the new toggle and still mentions CC stream-json invocation and execution limits.

- [ ] **Step 3: Commit docs**

Run:

```bash
git add docs/architecture/sessions.md
git commit -m "docs: describe thinking toggle behavior"
```

---

### Task 6: Final Verification

**Files:**
- Modify if needed: files touched by fixes from verification

- [ ] **Step 1: Run focused tests**

Run:

```bash
devenv shell -- cargo test -p right-bot thinking_visibility --lib
devenv shell -- cargo test -p right-bot parse_thinking_toggle_callback --lib
devenv shell -- cargo test -p right-bot thinking_toggle --lib
devenv shell -- cargo test -p right-bot working_keyboard --lib
devenv shell -- cargo test -p right-bot thinking_anchor_text --lib
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic --lib
```

Expected: PASS.

- [ ] **Step 2: Run package tests**

Run:

```bash
devenv shell -- cargo test -p right-bot --lib
```

Expected: PASS.

- [ ] **Step 3: Run final workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4: Inspect working tree**

Run:

```bash
devenv shell -- git status --short
```

Expected: only intended files are modified, or the tree is clean if all task commits were made.

- [ ] **Step 5: Verify the issue-closing footer exists on the feature commit**

Run:

```bash
devenv shell -- git log --format=%B -5 | rg "Closes #50"
```

Expected: output includes exactly one `Closes #50` line from the Task 4 feature commit.
