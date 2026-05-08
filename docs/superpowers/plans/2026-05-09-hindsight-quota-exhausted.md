# Hindsight Quota Exhausted Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface Hindsight Cloud `HTTP 402` (insufficient credits) to the user via the agent's normal reply path, stop silently growing `pending_retains`, and auto-recover on top-up.

**Architecture:** New `ErrorKind::Quota` for 402 (no breaker tick, like `Client`); new sticky `MemoryStatus::QuotaExhausted` set in the resilient wrapper's error arm and cleared on any 2xx; agent learns about it through an explicit `<memory-status>` marker that asks the user to top up. No Telegram broadcast — the agent informs the user.

**Tech Stack:** Rust 2024, `thiserror`, `tokio::sync::watch`, `reqwest` mocked via in-test TCP server.

**Spec:** `docs/superpowers/specs/2026-05-09-hindsight-quota-exhausted-design.md`

---

## File map

- Modify `crates/right-memory/src/classify.rs` — add `ErrorKind::Quota`, route 402.
- Modify `crates/right-memory/src/circuit.rs` — `Quota` joins `Client` no-tick branch.
- Modify `crates/right-memory/src/status.rs` — add `QuotaExhausted` variant + severity slot.
- Modify `crates/right-memory/src/resilient.rs` — set on 402, clear on 2xx, no enqueue, refresh_status preservation.
- Modify `crates/bot/src/telegram/worker.rs` — `build_memory_marker` arm.

No new files. No schema/migrations. No CLI surface change.

---

## Task 1: Classify HTTP 402 as `ErrorKind::Quota`

**Files:**
- Modify: `crates/right-memory/src/classify.rs`

- [ ] **Step 1: Add the failing test**

Append to the `tests` mod in `crates/right-memory/src/classify.rs` (just before the existing `classify_db_transient` test):

```rust
    #[test]
    fn classify_402_quota() {
        assert_eq!(h(402).classify(), ErrorKind::Quota);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-memory classify::tests::classify_402_quota`
Expected: compile error — `ErrorKind::Quota` does not exist.

- [ ] **Step 3: Add `Quota` variant**

In `crates/right-memory/src/classify.rs`, change the `ErrorKind` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Transient,   // 5xx, timeout, connect error
    RateLimited, // 429
    Auth,        // 401, 403
    Client,      // 400, 404, 422 (caller bug or upstream API drift)
    Malformed,   // response body parse error
    Quota,       // 402 — Hindsight insufficient credits (recoverable on top-up)
}
```

- [ ] **Step 4: Route 402 to `Quota` in `classify`**

In the same file, update the match arms in `MemoryError::classify`:

```rust
            MemoryError::Hindsight { status, .. } => match *status {
                401 | 403 => ErrorKind::Auth,
                402 => ErrorKind::Quota,
                429 => ErrorKind::RateLimited,
                400 | 404 | 422 => ErrorKind::Client,
                _ => ErrorKind::Transient,
            },
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p right-memory classify::tests::classify_402_quota`
Expected: PASS.

- [ ] **Step 6: Run the full classify test suite**

Run: `cargo test -p right-memory --lib classify::tests`
Expected: all tests PASS (existing tests unchanged).

- [ ] **Step 7: Commit**

```bash
git add crates/right-memory/src/classify.rs
git commit -m "feat(right-memory): classify HTTP 402 as ErrorKind::Quota

402 (Insufficient credits) was falling through the catch-all to
Transient. Add a dedicated Quota variant so callers can react
specifically without mixing with 5xx/timeout backoff."
```

---

## Task 2: Circuit breaker leaves `Quota` untouched

**Files:**
- Modify: `crates/right-memory/src/circuit.rs`

- [ ] **Step 1: Add the failing test**

Append to the `tests` mod in `crates/right-memory/src/circuit.rs`, after `client_does_not_tick`:

```rust
    #[tokio::test(start_paused = true)]
    async fn quota_does_not_tick() {
        let mut b = Breaker::new();
        for _ in 0..(TRIP_THRESHOLD * 2) {
            fail(&mut b, ErrorKind::Quota);
        }
        assert_eq!(b.state(), CircuitState::Closed);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-memory circuit::tests::quota_does_not_tick`
Expected: FAIL — breaker opens after `TRIP_THRESHOLD` Quota failures because Quota lands in the catch-all `Failure(_)` branch that pushes failures.

- [ ] **Step 3: Add `Quota` to the no-tick branch**

In `crates/right-memory/src/circuit.rs::record`, change the `Client` arm to also accept `Quota`:

```rust
            Outcome::Failure(ErrorKind::Client | ErrorKind::Quota) => {
                // Client and Quota errors do not tick the breaker.
                // Quota (402) is a stable known state — every turn should
                // retry; the first 2xx after top-up clears the status.
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p right-memory circuit::tests::quota_does_not_tick`
Expected: PASS.

- [ ] **Step 5: Run the full circuit test suite**

Run: `cargo test -p right-memory --lib circuit::tests`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-memory/src/circuit.rs
git commit -m "feat(right-memory): exempt ErrorKind::Quota from breaker ticks

402 is a stable server-side state, not a transient hiccup. Letting
it tick the breaker would gate recovery behind a backoff window,
even though every retry costs nothing on the API side and the first
2xx is the natural recovery signal."
```

---

## Task 3: Add `MemoryStatus::QuotaExhausted` variant

**Files:**
- Modify: `crates/right-memory/src/status.rs`

- [ ] **Step 1: Update the existing severity-ordering test**

Replace the `severity_ordering` test in `crates/right-memory/src/status.rs` with a version that includes the new variant:

```rust
    #[test]
    fn severity_ordering() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded {
            since: Instant::now(),
        };
        let q = MemoryStatus::QuotaExhausted {
            since: Instant::now(),
        };
        let a = MemoryStatus::AuthFailed {
            since: Instant::now(),
        };
        assert!(h < d);
        assert!(d < q);
        assert!(q < a);
    }
```

- [ ] **Step 2: Update `max_merges_by_severity` for the new variant**

Replace the existing `max_merges_by_severity` test:

```rust
    #[test]
    fn max_merges_by_severity() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded {
            since: Instant::now(),
        };
        let q = MemoryStatus::QuotaExhausted {
            since: Instant::now(),
        };
        let a = MemoryStatus::AuthFailed {
            since: Instant::now(),
        };
        assert_eq!(h.max(d).severity(), d.severity());
        assert_eq!(d.max(q).severity(), q.severity());
        assert_eq!(q.max(a).severity(), a.severity());
        assert_eq!(h.max(a).severity(), a.severity());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p right-memory --lib status::tests`
Expected: compile error — `MemoryStatus::QuotaExhausted` does not exist.

- [ ] **Step 4: Add the variant + severity slot**

In `crates/right-memory/src/status.rs`, update both `MemoryStatus` and `severity`:

```rust
#[derive(Debug, Clone, Copy)]
pub enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    QuotaExhausted { since: Instant },
    AuthFailed { since: Instant },
}

impl MemoryStatus {
    fn severity(&self) -> u8 {
        match self {
            MemoryStatus::Healthy => 0,
            MemoryStatus::Degraded { .. } => 1,
            MemoryStatus::QuotaExhausted { .. } => 2,
            MemoryStatus::AuthFailed { .. } => 3,
        }
    }
}
```

- [ ] **Step 5: Run the status test suite**

Run: `cargo test -p right-memory --lib status::tests`
Expected: all PASS.

- [ ] **Step 6: Workspace compile check**

Run: `cargo check --workspace`
Expected: clean — `MemoryStatus` is `#[derive(Clone, Copy)]` and existing match sites in `worker.rs::build_memory_marker` are non-exhaustive (use generic arms). If a `match` site somewhere errors with "non-exhaustive patterns", note the file/line — Task 6 will cover `build_memory_marker`; any other site must be addressed in this task before commit.

- [ ] **Step 7: Commit**

```bash
git add crates/right-memory/src/status.rs
git commit -m "feat(right-memory): add MemoryStatus::QuotaExhausted variant

Severity sits between Degraded and AuthFailed: more actionable than
a generic degradation, but auth failure (bad key) still wins."
```

---

## Task 4: Wrapper sets `QuotaExhausted` on 402 and skips enqueue

**Files:**
- Modify: `crates/right-memory/src/resilient.rs`

- [ ] **Step 1: Add the failing test for status flip + no-enqueue**

Append to the `tests` mod in `crates/right-memory/src/resilient.rs`, after `retain_does_not_enqueue_on_client_error`:

```rust
    #[tokio::test]
    async fn retain_402_sets_quota_status_no_enqueue() {
        let (_h, url) = mock(
            r#"{"detail":"Insufficient credits. Balance: $-0.01"}"#,
            402,
        )
        .await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };
        let err = w
            .retain("ignored content", None, None, None, None, policy)
            .await
            .unwrap_err();
        assert!(matches!(err, ResilientError::Upstream(_)));
        assert!(
            matches!(w.status(), MemoryStatus::QuotaExhausted { .. }),
            "expected QuotaExhausted, got {:?}",
            w.status()
        );
        let conn = open_connection(w.agent_db_path(), false).unwrap();
        let cnt = crate::retain_queue::count(&conn).unwrap();
        assert_eq!(cnt, 0, "402 must not enqueue (will never drain)");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p right-memory resilient::tests::retain_402_sets_quota_status_no_enqueue`
Expected: FAIL — currently 402 → Transient → status stays Healthy until breaker trips, AND retain enqueues 1 row.

- [ ] **Step 3: Add the `Quota` arm in `call_with_policy`**

In `crates/right-memory/src/resilient.rs::call_with_policy`, insert a new branch immediately after the `Auth` branch (around line 229):

```rust
                    if matches!(kind, ErrorKind::Quota) {
                        // Quota is sticky against itself; AuthFailed (higher
                        // severity) wins. Cleared on any 2xx — see success arm.
                        self.status_tx.send_if_modified(|cur| {
                            if matches!(
                                *cur,
                                MemoryStatus::QuotaExhausted { .. }
                                    | MemoryStatus::AuthFailed { .. }
                            ) {
                                false
                            } else {
                                *cur = MemoryStatus::QuotaExhausted {
                                    since: std::time::Instant::now(),
                                };
                                true
                            }
                        });
                        return Err(ResilientError::Upstream(e));
                    }
```

- [ ] **Step 4: Add `Quota` to the no-enqueue branch in `retain`**

In `crates/right-memory/src/resilient.rs::retain`, change the existing `Auth` no-enqueue arm:

```rust
                    ErrorKind::Auth | ErrorKind::Quota => {
                        // Don't enqueue; will not drain until the user fixes
                        // the root cause (rotate key / top up credits).
                    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p right-memory resilient::tests::retain_402_sets_quota_status_no_enqueue`
Expected: PASS.

- [ ] **Step 6: Run the full resilient test suite**

Run: `cargo test -p right-memory --lib resilient::tests`
Expected: all PASS — existing Auth/Client/Transient cases unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/right-memory/src/resilient.rs
git commit -m "feat(right-memory): set QuotaExhausted status and skip enqueue on 402

Mirror the Auth path: stick the watch-channel status, return upstream
error early, and skip the pending_retains enqueue (queue would never
drain until the user tops up)."
```

---

## Task 5: Wrapper clears `QuotaExhausted` on any 2xx and refresh_status preserves it

**Files:**
- Modify: `crates/right-memory/src/resilient.rs`

- [ ] **Step 1: Add the failing recovery test**

Append to the same `tests` mod, after the test from Task 4:

```rust
    /// Mock that returns N responses with given (status, body) pairs in order.
    /// After exhausting the list, every further connection returns the last entry.
    async fn mock_seq(seq: Vec<(u16, &'static str)>) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let handle = tokio::spawn(async move {
            let mut idx = 0usize;
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let (status, body) = seq[idx.min(seq.len() - 1)];
                idx += 1;
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }

    #[tokio::test]
    async fn recall_402_then_200_clears_quota_status() {
        let (_h, url) = mock_seq(vec![
            (402, r#"{"detail":"Insufficient credits"}"#),
            (200, r#"{"results":[]}"#),
        ])
        .await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };

        // First call: 402 → status becomes QuotaExhausted.
        let err = w.recall("q", None, None, policy).await.unwrap_err();
        assert!(matches!(err, ResilientError::Upstream(_)));
        assert!(matches!(w.status(), MemoryStatus::QuotaExhausted { .. }));

        // Second call: 200 → status returns to Healthy.
        let _ok = w.recall("q", None, None, policy).await.unwrap();
        assert!(
            matches!(w.status(), MemoryStatus::Healthy),
            "expected Healthy after 2xx, got {:?}",
            w.status()
        );
    }

    #[tokio::test]
    async fn auth_wins_over_quota() {
        let (_h, url) = mock_seq(vec![
            (402, r#"{"detail":"Insufficient credits"}"#),
            (401, r#"{"error":"unauthorized"}"#),
        ])
        .await;
        let w = wrap(&url);
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(2),
            attempts: 0,
        };

        let _ = w.recall("q", None, None, policy).await.unwrap_err();
        assert!(matches!(w.status(), MemoryStatus::QuotaExhausted { .. }));

        let _ = w.recall("q", None, None, policy).await.unwrap_err();
        assert!(
            matches!(w.status(), MemoryStatus::AuthFailed { .. }),
            "401 must override Quota"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p right-memory resilient::tests::recall_402_then_200_clears_quota_status resilient::tests::auth_wins_over_quota`
Expected: FAIL — first test: status flips back to Healthy via `refresh_status` after the breaker is Closed (no preservation), but on a system where `refresh_status` runs, it might already pass; the more reliable failure mode here is that the explicit-clear logic in the success arm is missing, leaving status stale at QuotaExhausted between the 402 and a future change. The test asserts the `Healthy` post-2xx state. If it passes by accident (because `refresh_status` overwrites), Step 3-4 must still be applied to make the behavior intentional and survive Task 4's ordering.

- [ ] **Step 3: Add `QuotaExhausted` to `refresh_status` preservation**

In `crates/right-memory/src/resilient.rs::refresh_status`, change the early-return matcher:

```rust
        self.status_tx.send_if_modified(|cur| {
            if matches!(
                *cur,
                MemoryStatus::AuthFailed { .. }
                    | MemoryStatus::QuotaExhausted { .. }
            ) {
                return false;
            }
            let new = match st {
                crate::circuit::CircuitState::Closed => MemoryStatus::Healthy,
                crate::circuit::CircuitState::Open { .. }
                | crate::circuit::CircuitState::HalfOpen => MemoryStatus::Degraded {
                    since: std::time::Instant::now(),
                },
            };
            if *cur != new {
                *cur = new;
                true
            } else {
                false
            }
        });
```

- [ ] **Step 4: Add the explicit clear in `call_with_policy`'s success arm**

In `crates/right-memory/src/resilient.rs::call_with_policy`, modify the `Ok(val)` branch (around lines 202-208):

```rust
                Ok(val) => {
                    {
                        let mut b = self.breaker.lock().await;
                        b.record(Outcome::Success);
                    }
                    // Clear QuotaExhausted on any 2xx — recovery signal after
                    // the user tops up credits.
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

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p right-memory resilient::tests::recall_402_then_200_clears_quota_status resilient::tests::auth_wins_over_quota`
Expected: both PASS.

- [ ] **Step 6: Run the full resilient test suite**

Run: `cargo test -p right-memory --lib resilient::tests`
Expected: all PASS — existing Auth stickiness, Client drop, Transient enqueue cases unchanged.

- [ ] **Step 7: Commit**

```bash
git add crates/right-memory/src/resilient.rs
git commit -m "feat(right-memory): clear QuotaExhausted on any 2xx, preserve on refresh

Two coordinated changes: refresh_status now preserves QuotaExhausted
(parallel paths must not silently flip it back to Healthy when the
breaker is Closed), and the success arm explicitly clears it on any
2xx so the first call after top-up restores normal operation."
```

---

## Task 6: Agent-facing marker for `QuotaExhausted`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add the failing test**

Append to the `#[cfg(test)] mod tests` in `crates/bot/src/telegram/worker.rs` (the one starting at line 2468). Place it after the existing `is_auth_error_*` tests:

```rust
    // build_memory_marker tests

    #[test]
    fn marker_quota_exhausted_includes_topup_instruction() {
        let status = right_memory::MemoryStatus::QuotaExhausted {
            since: std::time::Instant::now(),
        };
        let marker = build_memory_marker(status, 0).expect("marker required");
        assert!(
            marker.contains("out of credits"),
            "marker must explain the failure mode: {marker}"
        );
        assert!(
            marker.contains("https://hindsight.vectorize.io"),
            "marker must include top-up URL: {marker}"
        );
        assert!(
            marker.contains("tell the user"),
            "marker must instruct the agent to inform the user: {marker}"
        );
    }

    #[test]
    fn marker_healthy_no_drops_returns_none() {
        let status = right_memory::MemoryStatus::Healthy;
        assert!(build_memory_marker(status, 0).is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p right-bot --lib telegram::worker::tests::marker_quota_exhausted_includes_topup_instruction telegram::worker::tests::marker_healthy_no_drops_returns_none`
Expected: first test FAILs — `build_memory_marker` does not handle `QuotaExhausted`. Second test passes (already covered by existing logic), kept for regression safety. If `build_memory_marker` is private with no test access, see Step 3 — it lives in the same file as the `tests` module so `super::*` already exposes it.

- [ ] **Step 3: Add the `QuotaExhausted` arm to `build_memory_marker`**

In `crates/bot/src/telegram/worker.rs::build_memory_marker` (line 464), update the match:

```rust
fn build_memory_marker(
    status: right_memory::MemoryStatus,
    client_drops_24h: usize,
) -> Option<String> {
    use right_memory::MemoryStatus as S;
    match status {
        S::AuthFailed { .. } => Some(
            "<memory-status>unavailable — memory provider authentication failed, \
             memory ops will error until the user rotates the API key</memory-status>"
                .into(),
        ),
        S::QuotaExhausted { .. } => Some(
            "<memory-status>unavailable — Hindsight Cloud account is out of credits. \
             Memory ops will fail until the user tops up. \
             IMPORTANT: tell the user clearly that they need to add credits at \
             https://hindsight.vectorize.io to restore memory.</memory-status>"
                .into(),
        ),
        S::Degraded { .. } => Some(
            "<memory-status>degraded — recall may be incomplete or stale, \
             retain may be queued</memory-status>"
                .into(),
        ),
        S::Healthy => {
            if client_drops_24h > 0 {
                Some(format!(
                    "<memory-status>retain-errors: {client_drops_24h} records dropped \
                     in last 24h due to bad payload — check logs</memory-status>"
                ))
            } else {
                None
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p right-bot --lib telegram::worker::tests::marker_quota_exhausted_includes_topup_instruction telegram::worker::tests::marker_healthy_no_drops_returns_none`
Expected: both PASS.

- [ ] **Step 5: Workspace build**

Run: `cargo build --workspace`
Expected: clean build, no warnings introduced.

- [ ] **Step 6: Workspace tests**

Run: `cargo test --workspace`
Expected: all PASS. (Existing tests for `Degraded`/`AuthFailed` markers — if any — must still pass; the new arm is additive and doesn't change their behavior.)

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): inject actionable marker for QuotaExhausted memory status

The agent gets a system-prompt marker that names the cause (out of
credits), the action (top up at hindsight.vectorize.io), and an
explicit IMPORTANT instruction to tell the user. No Telegram
broadcast — the agent informs the user via its normal reply path."
```

---

## Final verification

- [ ] **Step 1: Workspace test sweep**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 2: Clippy on touched crates**

Run: `cargo clippy -p right-memory -p right-bot --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Manual smoke (optional, only if Hindsight key with empty balance is available)**

In an agent's `agent.yaml` set `memory.api_key` to a key with zero balance and restart. Send a message. Observe:
- Bot log shows `WARN ... HTTP 402` once.
- The agent's reply mentions running out of credits and links to `https://hindsight.vectorize.io`.
- `pending_retains` row count in the agent's `data.db` does not increase.
- After topping up and sending another message, the agent works normally and stops mentioning credits.

- [ ] **Step 4: Update `MEMORY.md` only if the user asks**

No automatic memory writes — the spec is the durable record.
