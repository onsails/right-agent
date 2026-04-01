---
id: SEED-013
status: dormant
planted: 2026-04-01
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next UX milestone — any work on Telegram interaction quality, message handling, or responsiveness
scope: medium
---

# SEED-013: Improve debounce — detect user is typing and increase default

## Why This Matters

Current fixed 500ms debounce (`DEBOUNCE_MS = 500`) is too eager:
- CC gets invoked mid-typing when user pauses briefly between sentences
- Partial messages arrive as separate batches, breaking context
- 500ms feels too short for natural Telegram conversation cadence

Two improvements needed:
1. **Increase default debounce** — 500ms is too low; 1000–1500ms is a more natural pause threshold
2. **Typing-aware extension** — Telegram sends `chat_action: typing` events. When detected for the active session, extend (or restart) the debounce window so CC never fires while the user is visibly still typing

## When to Surface

**Trigger:** Next UX milestone — surface when working on Telegram interaction quality, message handling UX, or bot responsiveness

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Telegram bot UX improvements
- Message handling or batching changes
- Responsiveness / latency tuning
- Any work touching `worker.rs` or `handler.rs`

## Scope Estimate

**Medium** — A phase or two: bump default, add typing-action detection in handler, thread typing state into worker debounce loop, add tests for the extended-debounce path.

## Breadcrumbs

- `crates/bot/src/telegram/worker.rs:25` — `const DEBOUNCE_MS: u64 = 500;` — the value to increase
- `crates/bot/src/telegram/worker.rs:213–240` — debounce collection loop (inner `tokio::select!`) — where typing-aware extension would hook in
- `crates/bot/src/telegram/handler.rs:55` — `DebounceMsg` construction — typing events could be routed here too (or as a separate channel message variant)

## Notes

Telegram Bot API sends `ChatAction::Typing` when a user is composing. Teloxide exposes this as an `UpdateKind::ChatAction` (or similar). The worker could accept a `TypingPing` variant on its channel and reset/extend the debounce timer on receipt — elegant reuse of the existing channel.
