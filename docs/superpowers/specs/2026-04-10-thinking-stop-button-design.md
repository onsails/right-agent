# Design: Stop Button on Thinking Messages

## Problem

Users have no way to cancel a running CC session from Telegram. Long-running or
misdirected agent turns waste budget and time with no escape hatch.

## Solution

Add an inline "Stop" button to every thinking message. The button kills the CC
process. When `show_thinking` is disabled, a static anchor message is sent solely
to host the button, then cleaned up after completion.

## Behavior Matrix

| `show_thinking` | Thinking message content | Updates | On normal finish | On stop |
|-----------------|------------------------|---------|-----------------|---------|
| `true` | Live events + status footer | Every 2s | Final cost/turns, remove keyboard | Append "Stopped" + final cost/turns, remove keyboard |
| `false` | Static "Working..." | None | Delete message | Edit to "Working...\nStopped", remove keyboard |

## Components

### 1. Shared Stop State

New type alias:

```rust
type StopTokens = Arc<DashMap<(ChatId, i32), CancellationToken>>;
```

Key: `(chat_id, eff_thread_id)` -- same key the worker uses. Injected via
`dptree::deps!` alongside existing shared state in `dispatch.rs`.

### 2. Worker Changes (`worker.rs`)

#### Token lifecycle

- Before `invoke_cc`: create `CancellationToken`, insert into `StopTokens`.
- After CC exits (any reason): remove entry from `StopTokens`.

#### Thinking message -- always sent

Regardless of `show_thinking`, send a thinking message with an
`InlineKeyboardMarkup` containing one button:

```
[Stop]  callback_data = "stop:{chat_id}:{eff_thread_id}"
```

- `show_thinking: true` -- current behavior (2s-throttled event updates), every
  `edit_message_text` preserves `reply_markup` with the Stop button.
- `show_thinking: false` -- static text `"Working..."`, no edits until
  completion.

#### Stream loop -- stop branch

Add a `tokio::select!` branch in the existing stream-reading loop:

```rust
_ = stop_token.cancelled() => {
    child.kill().await.ok();
    break;
}
```

#### Post-completion edits

After CC exits and process is waited:

| Condition | Action |
|-----------|--------|
| stopped + `show_thinking: true` | Edit: current events + "Stopped" footer with cost/turns, remove keyboard |
| stopped + `show_thinking: false` | Edit: "Working...\nStopped", remove keyboard |
| normal + `show_thinking: true` | Edit: final cost/turns (current behavior), remove keyboard |
| normal + `show_thinking: false` | Delete thinking message |

"Remove keyboard" = `edit_message_reply_markup` with empty `InlineKeyboardMarkup`,
or pass empty markup in `edit_message_text`.

### 3. Callback Query Handler

New handler branch in `dispatch.rs` for `Update::CallbackQuery`:

```rust
fn handle_stop_callback(q: CallbackQuery, stop_tokens: StopTokens, bot: Bot)
```

- Parse `data` matching `stop:{chat_id}:{eff_thread_id}`.
- Look up in `StopTokens`:
  - Found: call `token.cancel()`, answer `"Stopping..."`.
  - Not found (already finished): answer `"Already finished"`.
- Always call `answer_callback_query` (Telegram requires it).

### 4. `WorkerContext` Change

Add `stop_tokens: StopTokens` field to `WorkerContext` so the worker can
insert/remove tokens.

## Data Flow

```
User taps Stop
  -> Telegram sends CallbackQuery with data "stop:{chat_id}:{thread_id}"
  -> handler parses, looks up StopTokens DashMap
  -> token.cancel()
  -> answer_callback_query("Stopping...")

Worker stream loop:
  -> stop_token.cancelled() fires in tokio::select!
  -> child.kill()
  -> loop breaks
  -> child.wait() collects exit status
  -> edit thinking message (append "Stopped", remove keyboard)
  -> remove token from DashMap
```

## Edge Cases

- **Double tap**: `CancellationToken::cancel()` is idempotent. Both callback
  queries get answered.
- **Race (stop arrives after CC finished)**: DashMap entry already removed.
  Handler answers "Already finished".
- **Multiple messages in flight**: Impossible -- one worker per
  `(chat_id, eff_thread_id)` key, sequential processing.
- **Bot restart while CC running**: `kill_on_drop(true)` on child process
  handles cleanup. DashMap is in-memory, rebuilt empty on restart.

## Files to Modify

| File | Change |
|------|--------|
| `crates/bot/src/telegram/dispatch.rs` | Inject `StopTokens`, add callback query handler branch |
| `crates/bot/src/telegram/handler.rs` | Add `handle_stop_callback` function |
| `crates/bot/src/telegram/worker.rs` | Token lifecycle, always-send thinking msg with keyboard, select! stop branch, post-completion edits |
| `crates/bot/src/telegram/stream.rs` | No changes needed |
| `crates/bot/src/telegram/mod.rs` | Export `StopTokens` type alias |

## Dependencies

- `dashmap` -- already in workspace deps.
- `tokio_util::sync::CancellationToken` -- already used in worker (typing indicator).
- `teloxide::types::InlineKeyboardButton`, `InlineKeyboardMarkup` -- from teloxide, already a dependency.
