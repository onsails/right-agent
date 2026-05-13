# Cron Delivery Thread Idle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make cron delivery idle gating per Telegram chat/topic instead of agent-wide.

**Architecture:** Add an in-memory `IdleTracker` keyed by `(chat_id, effective_thread_id)` and pass `Arc<IdleTracker>` through the existing bot/dispatcher/worker/cron-delivery wiring. Cron delivery builds the idle key from `cron_runs.target_chat_id` and `target_thread_id`, so only activity in the target chat/topic delays that delivery.

**Tech Stack:** Rust 2024, `right-bot`, `dashmap`, `chrono`, existing Telegram session/thread normalization.

---

## File Structure

- Create `crates/bot/src/telegram/idle.rs`
  - Owns `IdleKey` and `IdleTracker`.
  - Contains unit tests for per-thread isolation, root-thread behavior, unknown-key fallback, and pruning.
- Modify `crates/bot/src/telegram/mod.rs`
  - Adds `pub(crate) mod idle;`.
- Modify `crates/bot/src/telegram/handler.rs`
  - Removes `IdleTimestamp`.
  - Accepts `Arc<IdleTracker>`.
  - Touches `IdleKey { chat_id, thread_id }` after computing `effective_thread_id`.
  - Passes `Arc<IdleTracker>` to `WorkerContext`.
- Modify `crates/bot/src/telegram/worker.rs`
  - Changes `WorkerContext.idle_timestamp` to `Arc<IdleTracker>`.
  - Touches the worker's `(chat_id, effective_thread_id)` key after final reply handling.
- Modify `crates/bot/src/telegram/dispatch.rs`
  - Replaces all `IdleTimestamp` dependency injection with `IdleTracker`.
  - Updates dispatcher smoke test setup.
- Modify `crates/bot/src/lib.rs`
  - Creates one `Arc<IdleTracker>` at startup and passes it to cron delivery and Telegram.
- Modify `crates/bot/src/cron_delivery.rs`
  - Uses `IdleTracker` instead of one global atomic timestamp.
  - Adds a pure helper for deriving the target idle key after target classification.
  - Scans a bounded ordered batch of pending rows so a busy target does not
    create head-of-line blocking for idle targets.
  - Touches only the delivered target key after successful delivery.

Do not rewrite unrelated session, worker, or cron execution code. There is a known pre-existing unstaged change in `crates/bot/src/telegram/worker.rs`; inspect it before editing and preserve it.

### Task 1: Add IdleTracker

**Files:**
- Create: `crates/bot/src/telegram/idle.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write the failing IdleTracker tests**

Create `crates/bot/src/telegram/idle.rs` with only the public shape and tests:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IdleKey {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[derive(Debug)]
pub(crate) struct IdleTracker {
    start_ts: i64,
}

impl IdleTracker {
    pub(crate) fn new(start_ts: i64) -> Self {
        Self { start_ts }
    }

    pub(crate) fn touch(&self, _key: IdleKey, _now: i64) {
        unimplemented!("implemented in Step 3");
    }

    pub(crate) fn idle_for_secs(&self, _key: IdleKey, _now: i64) -> i64 {
        unimplemented!("implemented in Step 3");
    }

    pub(crate) fn prune_older_than(&self, _cutoff_ts: i64) {
        unimplemented!("implemented in Step 3");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn touch_is_isolated_by_thread() {
        let tracker = IdleTracker::new(1_000);
        let thread_a = IdleKey {
            chat_id: -100,
            thread_id: 111,
        };
        let thread_b = IdleKey {
            chat_id: -100,
            thread_id: 222,
        };

        tracker.touch(thread_a, 1_050);

        assert_eq!(tracker.idle_for_secs(thread_a, 1_080), 30);
        assert_eq!(tracker.idle_for_secs(thread_b, 1_080), 80);
    }

    #[test]
    fn thread_zero_is_root_chat_key() {
        let tracker = IdleTracker::new(2_000);
        let root = IdleKey {
            chat_id: -200,
            thread_id: 0,
        };

        tracker.touch(root, 2_010);

        assert_eq!(tracker.idle_for_secs(root, 2_050), 40);
    }

    #[test]
    fn unknown_key_uses_tracker_start_time() {
        let tracker = IdleTracker::new(3_000);
        let key = IdleKey {
            chat_id: -300,
            thread_id: 9,
        };

        assert_eq!(tracker.idle_for_secs(key, 3_090), 90);
    }

    #[test]
    fn prune_removes_old_keys_only() {
        let tracker = IdleTracker::new(4_000);
        let old_key = IdleKey {
            chat_id: -400,
            thread_id: 1,
        };
        let fresh_key = IdleKey {
            chat_id: -400,
            thread_id: 2,
        };

        tracker.touch(old_key, 4_010);
        tracker.touch(fresh_key, 4_100);
        tracker.prune_older_than(4_050);

        assert_eq!(tracker.idle_for_secs(old_key, 4_120), 120);
        assert_eq!(tracker.idle_for_secs(fresh_key, 4_120), 20);
    }

    #[test]
    fn tracker_is_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IdleTracker>();

        let tracker = Arc::new(IdleTracker::new(5_000));
        tracker.touch(
            IdleKey {
                chat_id: 5,
                thread_id: 0,
            },
            5_001,
        );
        assert_eq!(
            tracker.idle_for_secs(
                IdleKey {
                    chat_id: 5,
                    thread_id: 0,
                },
                5_011,
            ),
            10
        );
    }
}
```

- [ ] **Step 2: Run the new tests and verify they fail**

Run:

```bash
cargo test -p right-bot idle
```

Expected: FAIL. At least `touch_is_isolated_by_thread` should panic at `unimplemented!("implemented in Step 3")`.

- [ ] **Step 3: Implement IdleTracker**

Replace the top of `crates/bot/src/telegram/idle.rs` with:

```rust
use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IdleKey {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[derive(Debug)]
pub(crate) struct IdleTracker {
    start_ts: i64,
    last_seen: DashMap<IdleKey, i64>,
}

impl IdleTracker {
    pub(crate) fn new(start_ts: i64) -> Self {
        Self {
            start_ts,
            last_seen: DashMap::new(),
        }
    }

    pub(crate) fn touch(&self, key: IdleKey, now: i64) {
        self.last_seen.insert(key, now);
    }

    pub(crate) fn idle_for_secs(&self, key: IdleKey, now: i64) -> i64 {
        let last = self
            .last_seen
            .get(&key)
            .map(|entry| *entry.value())
            .unwrap_or(self.start_ts);
        now.saturating_sub(last)
    }

    pub(crate) fn prune_older_than(&self, cutoff_ts: i64) {
        self.last_seen.retain(|_, last_seen| *last_seen >= cutoff_ts);
    }
}
```

Keep the tests from Step 1 below this implementation.

- [ ] **Step 4: Export the module**

In `crates/bot/src/telegram/mod.rs`, add this near the other module declarations:

```rust
pub(crate) mod idle;
```

- [ ] **Step 5: Run the IdleTracker tests**

Run:

```bash
cargo test -p right-bot idle
```

Expected: PASS. The output should include the `telegram::idle::tests::*` tests.

- [ ] **Step 6: Commit Task 1**

Run:

```bash
git add crates/bot/src/telegram/idle.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add keyed idle tracker"
```

### Task 2: Add Cron Delivery Idle Helpers

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs`

- [ ] **Step 1: Write the failing helper tests**

In `crates/bot/src/cron_delivery.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn idle_key_for_target_maps_thread_to_idle_key() {
        let key = idle_key_for_target(-100, Some(458));

        assert_eq!(key.chat_id, -100);
        assert_eq!(key.thread_id, 458);
    }

    #[test]
    fn idle_key_for_target_maps_none_thread_to_root() {
        let key = idle_key_for_target(-100, None);

        assert_eq!(key.chat_id, -100);
        assert_eq!(key.thread_id, 0);
    }

    #[test]
    fn fetch_pending_candidates_returns_oldest_rows_up_to_limit() {
        let (_dir, conn) = setup_db();
        for (id, finished_at) in [
            ("a", "2026-01-01T00:01:00Z"),
            ("b", "2026-01-01T00:02:00Z"),
            ("c", "2026-01-01T00:03:00Z"),
        ] {
            conn.execute(
                "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, target_chat_id) \
                 VALUES (?1, ?1, '2026-01-01T00:00:00Z', ?2, 'success', '/log', 'sum', '{\"content\":\"x\"}', -100)",
                rusqlite::params![id, finished_at],
            )
            .unwrap();
        }

        let rows = fetch_pending_candidates(&conn, 2).unwrap();

        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }
```

- [ ] **Step 2: Run the helper tests and verify they fail**

Run:

```bash
cargo test -p right-bot idle_key_for_target
cargo test -p right-bot fetch_pending_candidates_returns_oldest_rows_up_to_limit
```

Expected: FAIL with missing `idle_key_for_target` and `fetch_pending_candidates`.

- [ ] **Step 3: Implement the helper**

In `crates/bot/src/cron_delivery.rs`, replace:

```rust
use crate::telegram::handler::IdleTimestamp;
```

with:

```rust
use crate::telegram::idle::{IdleKey, IdleTracker};
```

Then add this helper near `classify_pending_target`:

```rust
pub(crate) fn idle_key_for_target(chat_id: i64, thread_id: Option<i64>) -> IdleKey {
    IdleKey {
        chat_id,
        thread_id: thread_id.unwrap_or(0),
    }
}
```

Add this bounded batch query near `fetch_pending`:

```rust
pub(crate) fn fetch_pending_candidates(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, notify_json, summary, status, target_chat_id, target_thread_id \
         FROM cron_runs \
         WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at ASC LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            status: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            target_chat_id: row.get(5)?,
            target_thread_id: row.get(6)?,
        })
    })?;

    rows.collect()
}
```

- [ ] **Step 4: Run the helper tests**

Run:

```bash
cargo test -p right-bot idle_key_for_target
cargo test -p right-bot fetch_pending_candidates_returns_oldest_rows_up_to_limit
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(bot): derive cron delivery idle keys"
```

### Task 3: Wire IdleTracker Through Bot Runtime

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/cron_delivery.rs`
- Test: existing dispatcher smoke test and cron delivery tests

- [ ] **Step 1: Replace the handler timestamp type**

In `crates/bot/src/telegram/handler.rs`, delete:

```rust
/// Shared timestamp of last interaction (unix seconds).
/// Updated by handler on incoming messages and by worker after sending replies.
#[derive(Clone)]
pub struct IdleTimestamp(pub Arc<std::sync::atomic::AtomicI64>);
```

Add this import near the existing `use super::BotType;` block:

```rust
use super::idle::{IdleKey, IdleTracker};
```

Change the `handle_message` parameter from:

```rust
    idle_ts: Arc<IdleTimestamp>,
```

to:

```rust
    idle_tracker: Arc<IdleTracker>,
```

- [ ] **Step 2: Touch idle after computing the effective thread**

In `crates/bot/src/telegram/handler.rs`, remove the current leading store:

```rust
    idle_ts.0.store(
        chrono::Utc::now().timestamp(),
        std::sync::atomic::Ordering::Relaxed,
    );
```

After:

```rust
    let chat_id = msg.chat.id;
    let eff_thread_id = effective_thread_id(&msg);
```

add:

```rust
    idle_tracker.touch(
        IdleKey {
            chat_id: chat_id.0,
            thread_id: eff_thread_id,
        },
        chrono::Utc::now().timestamp(),
    );
```

- [ ] **Step 3: Pass IdleTracker into WorkerContext**

In `crates/bot/src/telegram/handler.rs`, change the `WorkerContext` field assignment from:

```rust
                    idle_timestamp: Arc::clone(&idle_ts.0),
```

to:

```rust
                    idle_tracker: Arc::clone(&idle_tracker),
```

- [ ] **Step 4: Update WorkerContext**

In `crates/bot/src/telegram/worker.rs`, add:

```rust
use super::idle::{IdleKey, IdleTracker};
```

Change the `WorkerContext` field from:

```rust
    /// Shared idle timestamp — worker updates after each reply sent.
    pub idle_timestamp: Arc<std::sync::atomic::AtomicI64>,
```

to:

```rust
    /// Shared idle tracker — worker updates the current chat/thread after each reply sent.
    pub idle_tracker: Arc<IdleTracker>,
```

Change the post-reply store from:

```rust
            ctx.idle_timestamp.store(
                chrono::Utc::now().timestamp(),
                std::sync::atomic::Ordering::Relaxed,
            );
```

to:

```rust
            ctx.idle_tracker.touch(
                IdleKey {
                    chat_id,
                    thread_id: eff_thread_id,
                },
                chrono::Utc::now().timestamp(),
            );
```

- [ ] **Step 5: Update dispatch imports and signatures**

In `crates/bot/src/telegram/dispatch.rs`, change the handler import list from:

```rust
    AgentDir, AgentSettings, IdleTimestamp, InterceptSlots, InternalApi, PendingTokenSlot,
```

to:

```rust
    AgentDir, AgentSettings, InterceptSlots, InternalApi, PendingTokenSlot,
```

Add:

```rust
use super::idle::IdleTracker;
```

Change both `run_telegram` and `build_dispatcher` parameters from:

```rust
    idle_ts: Arc<IdleTimestamp>,
```

to:

```rust
    idle_tracker: Arc<IdleTracker>,
```

Inside `run_telegram`, change:

```rust
        Arc::clone(&idle_ts),
```

to:

```rust
        Arc::clone(&idle_tracker),
```

Inside `build_dispatcher`, change the dependency from:

```rust
            idle_ts,
```

to:

```rust
            idle_tracker,
```

- [ ] **Step 6: Update dispatch smoke test**

In `crates/bot/src/telegram/dispatch.rs` tests, remove `AtomicI64` from:

```rust
    use std::sync::atomic::{AtomicBool, AtomicI64};
```

so it becomes:

```rust
    use std::sync::atomic::AtomicBool;
```

Replace:

```rust
        let idle_ts = Arc::new(IdleTimestamp(Arc::new(AtomicI64::new(0))));
```

with:

```rust
        let idle_tracker = Arc::new(IdleTracker::new(0));
```

And pass `idle_tracker` to `build_dispatcher`.

- [ ] **Step 7: Update bot startup wiring**

In `crates/bot/src/lib.rs`, replace:

```rust
    // Shared idle timestamp: tracks last handler/worker interaction for cron delivery gating.
    use crate::telegram::handler::IdleTimestamp;
    let idle_timestamp = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
        chrono::Utc::now().timestamp(),
    ))));
```

with:

```rust
    // Shared idle tracker: tracks last handler/worker interaction per chat/thread for cron delivery gating.
    let idle_tracker = Arc::new(crate::telegram::idle::IdleTracker::new(
        chrono::Utc::now().timestamp(),
    ));
```

Then replace all local clones of `idle_timestamp` with `idle_tracker`:

```rust
    let delivery_idle_tracker = Arc::clone(&idle_tracker);
```

Pass `delivery_idle_tracker` to `cron_delivery::run_delivery_loop`, and pass:

```rust
            Arc::clone(&idle_tracker),
```

to `telegram::run_telegram`.

- [ ] **Step 8: Update cron delivery idle gate**

In `crates/bot/src/cron_delivery.rs`, change `run_delivery_loop` parameter:

```rust
    idle_ts: Arc<IdleTimestamp>,
```

to:

```rust
    idle_tracker: Arc<IdleTracker>,
```

Near `POLL_INTERVAL_SECS`, add:

```rust
const PENDING_CANDIDATE_LIMIT: usize = 50;
```

Replace the single pending fetch:

```rust
        let pending = match fetch_pending(&conn) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("cron delivery: fetch_pending failed: {e:#}");
                continue;
            }
        };
```

with:

```rust
        let pending_candidates = match fetch_pending_candidates(&conn, PENDING_CANDIDATE_LIMIT) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("cron delivery: fetch_pending_candidates failed: {e:#}");
                continue;
            }
        };
        if pending_candidates.is_empty() {
            continue;
        }
```

Remove the global idle check block:

```rust
        let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
        let now = chrono::Utc::now().timestamp();
        let idle_for = now - last;
        if idle_for < IDLE_THRESHOLD_SECS {
            let wait = IDLE_THRESHOLD_SECS - idle_for;
            tracing::info!(
                job = %pending.job_name,
                run_id = %pending.id,
                idle_secs = idle_for,
                wait_secs = wait,
                "cron delivery: result pending, waiting for chat idle ({IDLE_THRESHOLD_SECS}s)"
            );
            continue;
        }
```

Then wrap the current block that begins with `let (to_deliver, skipped) =
match deduplicate_job(&conn, &pending.job_name)` and ends after the
`match deliver_through_session(...).await { ... }` in:

```rust
        for pending in pending_candidates {
```

and close the loop after the delivery `match` block with:

```rust
        }
```

Keep the existing `pending.job_name` deduplication input inside the loop. This
is the part that removes head-of-line blocking: a non-idle target uses
`continue`, so the loop can inspect another pending row for a different target
in the same poll tick.

Inside that loop, after target classification succeeds:

```rust
        let delivery_idle_key = idle_key_for_target(target_chat_id, target_thread_id);
        let now = chrono::Utc::now().timestamp();
        let idle_for = idle_tracker.idle_for_secs(delivery_idle_key, now);
        if idle_for < IDLE_THRESHOLD_SECS {
            let wait = IDLE_THRESHOLD_SECS - idle_for;
            tracing::info!(
                job = %to_deliver.job_name,
                run_id = %to_deliver.id,
                target_chat_id,
                ?target_thread_id,
                idle_secs = idle_for,
                wait_secs = wait,
                "cron delivery: result pending, waiting for target chat/thread idle ({IDLE_THRESHOLD_SECS}s)"
            );
            continue;
        }
```

Replace the successful delivery global store:

```rust
                idle_ts.0.store(
                    chrono::Utc::now().timestamp(),
                    std::sync::atomic::Ordering::Relaxed,
                );
```

with:

```rust
                idle_tracker.touch(
                    delivery_idle_key,
                    chrono::Utc::now().timestamp(),
                );
                break;
```

In the `Err(e)` delivery branch, add `break;` only after max attempts marks the
run delivered. If the target is merely not idle, use `continue;` and keep
scanning the remaining candidates.

- [ ] **Step 9: Add conservative idle cleanup**

In `crates/bot/src/cron_delivery.rs`, near `POLL_INTERVAL_SECS`, add:

```rust
const IDLE_TRACKER_PRUNE_AFTER_SECS: i64 = 24 * 60 * 60;
```

At the end of each successful or skipped poll iteration is too broad; keep cleanup simple and cheap by adding this immediately after the `tokio::select!` sleep in the loop:

```rust
        let prune_cutoff = chrono::Utc::now().timestamp() - IDLE_TRACKER_PRUNE_AFTER_SECS;
        idle_tracker.prune_older_than(prune_cutoff);
```

This cleanup is opportunistic and must not affect correctness; pruned keys fall back to tracker start time.

- [ ] **Step 10: Run focused tests**

Run:

```bash
cargo test -p right-bot idle
cargo test -p right-bot idle_key_for_target
cargo test -p right-bot fetch_pending_candidates_returns_oldest_rows_up_to_limit
cargo test -p right-bot dispatcher_builds_without_panic
```

Expected: all PASS.

- [ ] **Step 11: Commit Task 3**

Run:

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/cron_delivery.rs
git commit -m "feat(bot): gate cron delivery idle by chat thread"
```

### Task 4: Documentation And Final Verification

**Files:**
- Modify: `docs/architecture/sessions.md`
- Modify: `ARCHITECTURE.md` only if the prescriptive contract changes beyond this idle-gating detail

- [ ] **Step 1: Update session architecture docs**

In `docs/architecture/sessions.md`, in the "Per-session mutex on --resume" section, replace:

```markdown
`IDLE_THRESHOLD_SECS = 180` remains as UX politeness ("don't interrupt the
user mid-conversation"), but correctness now lives in the mutex.
```

with:

```markdown
`IDLE_THRESHOLD_SECS = 180` remains as UX politeness ("don't interrupt the
user mid-conversation"), but correctness now lives in the mutex. The idle
timestamp used by cron delivery is keyed by `(chat_id, effective_thread_id)`:
topic chats idle independently per Telegram topic, while non-topic and General
topic messages use thread id `0`.
```

- [ ] **Step 2: Run documentation diff check**

Run:

```bash
git diff -- docs/architecture/sessions.md ARCHITECTURE.md
```

Expected: only `docs/architecture/sessions.md` changes unless implementation revealed a prescriptive architecture change that belongs in `ARCHITECTURE.md`.

- [ ] **Step 3: Run full verification**

Run:

```bash
cargo test -p right-bot idle
cargo test -p right-bot cron_delivery
cargo build --workspace
```

Expected: all PASS.

- [ ] **Step 4: Commit docs**

Run:

```bash
git add docs/architecture/sessions.md ARCHITECTURE.md
git commit -m "docs: describe per-thread cron delivery idle"
```

If `ARCHITECTURE.md` has no changes, use:

```bash
git add docs/architecture/sessions.md
git commit -m "docs: describe per-thread cron delivery idle"
```

- [ ] **Step 5: Final status**

Run:

```bash
git status --short
```

Expected: no changes from this task remain unstaged or uncommitted. Pre-existing unrelated changes may still appear and must not be reverted.
