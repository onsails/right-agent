# Hindsight Quota Exhausted — Silent-Failure Fix

## Problem

When the Hindsight Cloud account runs out of credits, the API returns
`HTTP 402 Payment Required` with body
`{"detail":"Insufficient credits. Balance: $-0.01..."}`.

Today this is misclassified and silently degrades the agent:

- `right-memory/src/classify.rs:21-26` maps 402 to `ErrorKind::Transient`
  via the catch-all `_` arm.
- `resilient.rs::retain()` enqueues every Transient retain into
  `pending_retains`, which can never drain.
- The circuit breaker eventually opens; `MemoryStatus` flips to `Degraded`.
- `memory_alerts.rs::handle_status_change` only fires Telegram alerts on
  `AuthFailed` — `Degraded` has no user-facing branch.
- `worker.rs::build_memory_marker` injects a generic
  `<memory-status>degraded — recall may be incomplete or stale</memory-status>`
  marker that the LLM reads as a transient hiccup, not a billing problem.

Net effect: WARN-level lines in `~/.right/logs/<agent>.log` are the only
signal. The user sees an agent that has quietly forgotten everything;
the agent has no actionable context to surface the cause.

## Goals

1. Classify 402 distinctly from other 4xx/5xx.
2. Tell the user — through the agent itself — that credits are exhausted
   and they need to top up.
3. Stop growing `pending_retains` with entries that will never drain.
4. Auto-recover on top-up: the next successful 2xx clears the status.

Non-goals: separate Telegram broadcast (the agent informs the user via
its normal reply path), auth-flow changes, reflection changes,
parsing the Hindsight error body for sub-reasons.

## Design

### 1. New error kind

`crates/right-memory/src/classify.rs` — add `Quota` and route 402 to it:

```rust
pub enum ErrorKind {
    Transient,
    RateLimited,
    Auth,
    Client,
    Malformed,
    Quota,        // NEW — 402 Payment Required (out of credits)
}

// In MemoryError::classify()
MemoryError::Hindsight { status, .. } => match *status {
    401 | 403 => ErrorKind::Auth,
    402       => ErrorKind::Quota,        // NEW
    429       => ErrorKind::RateLimited,
    400 | 404 | 422 => ErrorKind::Client,
    _ => ErrorKind::Transient,
},
```

### 2. Breaker leaves Quota untouched

`crates/right-memory/src/circuit.rs::record` — `Quota` joins `Client` in
the no-tick branch:

```rust
Outcome::Failure(ErrorKind::Client | ErrorKind::Quota) => {
    // Quota and Client errors do not tick the breaker.
    // 402 is a stable known state — every turn should retry; the
    // first 2xx after top-up clears the status.
}
```

This means `recall`/`retain` keep flowing on 402 (no half-open backoff).
402 responses are server-side fast and not billable, so this is cheap.

### 3. New status variant

`crates/right-memory/src/status.rs`:

```rust
pub enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    QuotaExhausted { since: Instant },   // NEW
    AuthFailed { since: Instant },
}

fn severity(&self) -> u8 {
    match self {
        MemoryStatus::Healthy => 0,
        MemoryStatus::Degraded { .. } => 1,
        MemoryStatus::QuotaExhausted { .. } => 2,
        MemoryStatus::AuthFailed { .. } => 3,
    }
}
```

`QuotaExhausted` sits between `Degraded` and `AuthFailed` in severity:
strictly more actionable than a generic degradation, but auth failure
takes precedence (a bad key is a hard stop; quota is recoverable by
top-up).

### 4. Resilient wrapper handles Quota like Auth

`crates/right-memory/src/resilient.rs::call_with_policy`:

```rust
if matches!(kind, ErrorKind::Auth) {
    self.status_tx.send_if_modified(|cur| {
        if matches!(*cur, MemoryStatus::AuthFailed { .. }) { return false; }
        *cur = MemoryStatus::AuthFailed { since: Instant::now() };
        true
    });
    return Err(ResilientError::Upstream(e));
}
if matches!(kind, ErrorKind::Quota) {                      // NEW
    self.status_tx.send_if_modified(|cur| {
        // QuotaExhausted is sticky against itself, but allow upgrade
        // from Degraded/Healthy. AuthFailed wins (higher severity).
        if matches!(*cur, MemoryStatus::QuotaExhausted { .. }
                        | MemoryStatus::AuthFailed { .. }) {
            return false;
        }
        *cur = MemoryStatus::QuotaExhausted { since: Instant::now() };
        true
    });
    return Err(ResilientError::Upstream(e));
}
```

`retain()` enqueue path: `Quota` joins `Auth` in the do-not-enqueue arm
(retain returns `Err(ResilientError::Upstream(e))` and disappears — no
`pending_retains` row).

```rust
ErrorKind::Auth | ErrorKind::Quota => {
    // Don't enqueue; will not drain until user fixes the root cause.
}
```

### 5. Recovery clears Quota on any 2xx

Two changes work together:

**5a. Explicit clear in the success arm.** Before the existing
`refresh_status()` call, flip the watch channel to `Healthy` if the
current status is `QuotaExhausted`:

```rust
Ok(val) => {
    {
        let mut b = self.breaker.lock().await;
        b.record(Outcome::Success);
    }
    // NEW: clear QuotaExhausted on any successful call.
    self.status_tx.send_if_modified(|cur| {
        if matches!(*cur, MemoryStatus::QuotaExhausted { .. }) {
            *cur = MemoryStatus::Healthy;
            true
        } else {
            false
        }
    });
    self.refresh_status().await;
    return Ok(val);
}
```

**5b. `refresh_status` must preserve `QuotaExhausted` stickiness.**
Without this, a concurrent task that hits `refresh_status` while the
breaker is `Closed` (Quota doesn't tick it) would silently flip
`QuotaExhausted` → `Healthy` even though no 2xx ever happened:

```rust
self.status_tx.send_if_modified(|cur| {
    if matches!(*cur, MemoryStatus::AuthFailed { .. }
                    | MemoryStatus::QuotaExhausted { .. }) {  // NEW
        return false;
    }
    let new = match st {
        crate::circuit::CircuitState::Closed => MemoryStatus::Healthy,
        crate::circuit::CircuitState::Open { .. }
        | crate::circuit::CircuitState::HalfOpen => MemoryStatus::Degraded {
            since: std::time::Instant::now(),
        },
    };
    if *cur != new { *cur = new; true } else { false }
});
```

The clear in (5a) is the only path that flips `QuotaExhausted` →
`Healthy`. Symmetric with the existing `AuthFailed` recovery in
`get_or_create_bank` (resilient.rs:377-379).

### 6. Agent-facing marker

`crates/bot/src/telegram/worker.rs::build_memory_marker` — add the
`QuotaExhausted` arm:

```rust
S::QuotaExhausted { .. } => Some(
    "<memory-status>unavailable — Hindsight Cloud account is out of credits. \
     Memory ops will fail until the user tops up. \
     IMPORTANT: tell the user clearly that they need to add credits at \
     https://hindsight.vectorize.io to restore memory.</memory-status>"
        .into(),
),
```

This injects into the composite system prompt via the existing
`<memory-status>` channel. Every `claude -p` invocation (foreground,
background continuation, cron) carries it until the next 2xx clears
the status.

### 7. No Telegram broadcast

`memory_alerts.rs` is **not** modified. The user is informed via the
agent's reply path: the marker tells the LLM to surface the issue in
plain language. This avoids the dual-message UX (alert + reply arriving
simultaneously) and reuses the channel the user already reads.

Trade-off accepted: deduplication is the LLM's responsibility — Claude
typically mentions the issue once per conversation and not in every
reply. If it repeats, that's mild noise, not a regression. The marker
is gone the moment status flips back to `Healthy`.

## Edge cases

- **Bot startup with empty quota.** `get_or_create_bank` probe in
  `bot::lib.rs` returns 402 → status is `QuotaExhausted` from boot.
  No user notification fires until the user sends a message. Acceptable:
  memory ops aren't being used without activity.
- **Cron jobs.** Cron `claude -p` invocations skip memory injection
  entirely (`memory_mode: None` in `cron.rs`), so the marker does NOT
  reach cron-spawned turns. A cron job running on a quota-exhausted
  account surfaces the failure indirectly: any MCP retain/recall call
  inside the job returns `upstream_quota` from the aggregator, and the
  agent observes the error mid-execution. This is a pre-existing
  limitation shared by `Degraded` and `AuthFailed`; not addressed by
  this spec.
- **Explicit MCP tool calls** (`mcp__right__memory_retain`,
  `memory_recall`, `memory_reflect`). The aggregator returns the
  upstream error to the agent on 402, which the agent already surfaces
  in its reply. The marker reinforces this on the next turn.
- **Concurrent transitions.** `send_if_modified` ensures a single
  atomic read-modify-write on the watch channel — no race between
  observed status and emitted update.
- **Status downgrade ordering.** If a single call returns 402, status
  becomes `QuotaExhausted`. If a *subsequent* call returns 401, status
  upgrades to `AuthFailed`. If a 401 is followed by a 402, the 402
  branch checks for `AuthFailed` first and skips — auth wins, as
  intended (a bad key shadows a billing issue; once the key is fixed,
  the next call surfaces the real billing state).

## What we're not doing

- No `pending_retains` cleanup migration. Existing rows from before
  this fix will drain or stay; on this codepath they are no longer
  added. Their volume is small (10-minute window in the user's logs
  before the breaker opened).
- No body parsing of the 402 detail. Only the status code is used.
  If Vectorize introduces a different 402 cause (e.g., per-tier
  rate-limiting), the marker text is still actionable: top-up is the
  universal fix.
- No CLI/host-log alert. WARN-level Hindsight logs are unchanged.
- No changes to `AuthFailed` UX — that path stays as-is. Eliminating
  the redundant Telegram broadcast for auth is a separate decision
  out of scope here.

## Testing

Unit tests, all in-tree, no live Hindsight needed.

- `right-memory/src/classify.rs::tests::classify_402_quota` —
  `assert_eq!(h(402).classify(), ErrorKind::Quota)`.
- `right-memory/src/circuit.rs::tests::quota_does_not_tick` —
  feed `TRIP_THRESHOLD * 2` Quota failures, assert `Closed`.
- `right-memory/src/resilient.rs::tests::retain_402_sets_quota_no_enqueue` —
  mock 402, assert status `QuotaExhausted` and `pending_retains` count = 0.
- `right-memory/src/resilient.rs::tests::recall_402_then_200_clears_quota` —
  mock 402 then 200, assert status flips `Healthy` after the 2xx.
- `right-memory/src/resilient.rs::tests::quota_does_not_override_auth` —
  mock 401 then 402, assert status remains `AuthFailed`.
- `right-memory/src/status.rs::tests::quota_severity_between_degraded_and_auth` —
  ordering invariant.
- `bot/src/telegram/worker.rs` — extend existing `build_memory_marker`
  test coverage with a `QuotaExhausted` case asserting the marker
  contains both the URL and "out of credits".

## Migration / upgrade

Zero on-disk changes. No schema changes. No `pending_retains` drop.
Per `ARCHITECTURE.md` upgrade rules, this is a code-only change that
takes effect on `right restart <agent>` (or natural process-compose
restart). Already-deployed agents need no manual steps.

## Files touched

- `crates/right-memory/src/classify.rs` — new variant + tests.
- `crates/right-memory/src/circuit.rs` — no-tick branch + test.
- `crates/right-memory/src/status.rs` — new variant + severity + tests.
- `crates/right-memory/src/resilient.rs` — set on 402, clear on 2xx,
  do-not-enqueue arm + tests.
- `crates/bot/src/telegram/worker.rs` — `build_memory_marker` arm + test.
