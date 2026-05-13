# Cron Delivery Thread Idle Design

## Context

Cron result delivery currently waits on one agent-wide idle timestamp before
relaying a pending cron result to Telegram. That means activity in any allowed
chat or Telegram topic delays delivery everywhere. The delivery target is
already stored on `cron_runs` as `target_chat_id` and `target_thread_id`, and
delivery already sends to that target. The bug is only the idle gate: it is
not keyed by the target.

## Goals

- Gate cron delivery by the target Telegram chat/topic, not by the whole bot.
- Preserve the current 180 second politeness delay for the targeted chat/topic.
- Preserve process-local idle semantics. A bot restart should behave like a
  cold start, as it does today.
- Keep the implementation small and testable.

## Non-Goals

- No SQLite schema changes.
- No persisted idle timestamps.
- No changes to cron execution, cron target storage, or Telegram routing.
- No behavioral change for legacy cron rows without `target_chat_id`; they
  remain undeliverable and are marked with the existing no-target outcome.

## Chosen Approach

Replace the single shared `IdleTimestamp` with an in-memory `IdleTracker`
keyed by `(chat_id, effective_thread_id)`.

`effective_thread_id` keeps the existing Telegram normalization:

- no Telegram thread: `0`
- Telegram General topic: `0`
- real Telegram topic: the Telegram thread id

For a chat without topics, all activity naturally lands on `(chat_id, 0)`.
For a topic chat, each topic has an independent idle key. Activity in topic
`123` does not delay cron delivery targeted at topic `458`.

## API Shape

Put the tracker in `crates/bot/src/telegram/idle.rs` and expose it through the
Telegram module. The tracker should hide atomics and timestamp math behind a
small API:

```rust
// crates/bot/src/telegram/idle.rs
pub(crate) struct IdleKey {
    pub chat_id: i64,
    pub thread_id: i64,
}

pub(crate) struct IdleTracker { ... }

impl IdleTracker {
    pub fn touch(&self, key: IdleKey);
    pub fn idle_for_secs(&self, key: IdleKey, now: i64) -> i64;
}
```

Unknown keys should be treated as idle since tracker creation time. This
matches the current cold-start behavior: immediately after bot start, cron
delivery waits up to `IDLE_THRESHOLD_SECS` unless the target has activity.

## Data Flow

Inbound Telegram messages:

1. `handle_message` computes `effective_thread_id`.
2. It calls `idle_tracker.touch((chat_id, effective_thread_id))`.
3. The worker receives the same key and touches it again after sending the
   final reply. This preserves the existing behavior where the bot reply
   counts as interaction.

Cron delivery:

1. `fetch_pending` reads `target_chat_id` and `target_thread_id` from
   `cron_runs`.
2. After target classification passes, delivery builds
   `IdleKey { chat_id: target_chat_id, thread_id: target_thread_id.unwrap_or(0) }`.
3. It checks only that key against `IDLE_THRESHOLD_SECS`.
4. After successful delivery, it touches the same key. Multiple pending
   deliveries to the same target still space out by 180 seconds, but deliveries
   to other chats/topics are not blocked.

## Cleanup

The tracker is in-memory and process-local. To avoid unbounded growth on
long-lived agents, add a pruning method or periodic cleanup that removes keys
older than a conservative TTL such as 24 hours. Delivery correctness must not
depend on cleanup. If a key is pruned, `idle_for_secs` falls back to tracker
start time, which is acceptable and equivalent to cold-start semantics.

## Error Handling

There should be no fallible runtime path for ordinary touch/read operations.
The tracker should use lock-free or internally synchronized storage and expose
non-`Result` methods. If cleanup fails due to internal contention, skip that
cleanup tick; do not block delivery or message handling.

## Testing

Regression-first tests:

- `IdleTracker` unit test: touching `(chat, thread_a)` does not affect
  `idle_for_secs(chat, thread_b)`.
- `IdleTracker` unit test: `(chat, 0)` behaves as the shared key for a
  non-topic/general chat.
- `IdleTracker` unit test: unknown keys use tracker start time.
- `cron_delivery` pure-helper test: `target_thread_id = Some(458)` maps to
  `(chat, 458)`, and `None` maps to `(chat, 0)`.
- Wiring tests where practical: handler and worker call `touch` with the same
  `(chat_id, effective_thread_id)` key used for sessions.

Verification:

- `cargo test -p right-bot idle`
- `cargo test -p right-bot cron_delivery`
- `cargo build --workspace`
