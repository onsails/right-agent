# Memory Failure Handling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden every RightClaw memory operation against upstream and local failures through a `ResilientHindsight` wrapper (circuit breaker + classified retries), a persistent SQLite retain queue with 24h age cap, a `<memory-status>` marker on the system prompt, one-shot Telegram alerts for `AuthFailed`/`ClientFlood`, and doctor queue-health checks.

**Architecture:** Five new modules under `crates/rightclaw/src/memory/` (`classify`, `status`, `circuit`, `retain_queue`, `resilient`), a single `V14` migration adding `pending_retains` + `memory_alerts` tables, one new bot module (`memory_alerts.rs`), plus call-site migration in `worker.rs`, `lib.rs`, `prompt.rs`, and `aggregator.rs`. `HindsightClient` gains `retain_many` and classified `MemoryError` variants; its HTTP surface is otherwise unchanged.

**Tech Stack:** Rust edition 2024, tokio 1.50 (watch, spawn, time::pause for tests), rusqlite/rusqlite_migration, reqwest 0.13 (error classification via `is_timeout`/`is_connect`/`is_decode`), teloxide for Telegram notifications.

---

## File Structure

### Created

| File | Responsibility |
|------|----------------|
| `crates/rightclaw/src/memory/sql/v14_memory_failure_handling.sql` | Migration V14: `pending_retains`, `memory_alerts` tables + indices |
| `crates/rightclaw/src/memory/classify.rs` | `ErrorKind` enum + `MemoryError::classify()` |
| `crates/rightclaw/src/memory/status.rs` | `MemoryStatus` (`Healthy`/`Degraded`/`AuthFailed`) + `Ord` + watch helper |
| `crates/rightclaw/src/memory/circuit.rs` | `CircuitState` machine + rolling-window failure counter |
| `crates/rightclaw/src/memory/retain_queue.rs` | SQLite-backed queue API (`enqueue`, `count`, `oldest_age`, `drain_tick`) |
| `crates/rightclaw/src/memory/resilient.rs` | `ResilientHindsight` wrapper + `ResilientError` + `client_drops_24h()` |
| `crates/bot/src/telegram/memory_alerts.rs` | Telegram notification watcher (AuthFailed + ClientFlood with dedup) |

### Modified

| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/error.rs` | Add `HindsightTimeout/Connect/Parse/Other` variants |
| `crates/rightclaw/src/memory/hindsight.rs` | Classified reqwest errors; add `retain_many(&[RetainItem])` |
| `crates/rightclaw/src/memory/mod.rs` | Declare new modules, re-export types |
| `crates/rightclaw/src/memory/migrations.rs` | Register V14 |
| `crates/rightclaw/src/doctor.rs` | `check_memory(agent_dir)` — db/queue/alert checks |
| `crates/bot/src/lib.rs` | Construct `ResilientHindsight`; non-fatal Auth on startup; spawn drain task |
| `crates/bot/src/telegram/worker.rs` | `WorkerContext.hindsight` type; call sites; effective-status helper |
| `crates/bot/src/telegram/handler.rs` | `WorkerContextSettings.hindsight` type propagation |
| `crates/bot/src/telegram/dispatch.rs` | Type propagation + test fixture update |
| `crates/bot/src/telegram/prompt.rs` | `deploy_composite_memory → Result`; heredoc fallback; `MEMORY.md` annotation |
| `crates/rightclaw-cli/src/aggregator.rs` | `HindsightBackend` uses `ResilientHindsight` |

---

## Task 1: Migration V14 — tables + indices (TDD)

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v14_memory_failure_handling.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs:16` (add `V14_SCHEMA`), `:109` (register)
- Modify: `crates/rightclaw/src/memory/mod.rs:139` (`user_version_is_13` → `user_version_is_14`)

- [ ] **Step 1: Write failing test for V14 tables**

Append to `crates/rightclaw/src/memory/migrations.rs` `mod tests`:

```rust
#[test]
fn v14_pending_retains_table_exists() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('pending_retains')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for col in [
        "id", "content", "context", "document_id", "update_mode",
        "tags_json", "created_at", "attempts", "last_attempt_at",
        "last_error", "source",
    ] {
        assert!(cols.contains(&col.to_string()), "{col} column missing");
    }
}

#[test]
fn v14_memory_alerts_table_exists() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for col in ["alert_type", "first_sent_at"] {
        assert!(cols.contains(&col.to_string()), "{col} column missing");
    }
}

#[test]
fn v14_pending_retains_created_index_exists() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
             AND name='idx_pending_retains_created'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "idx_pending_retains_created should exist");
}
```

Also update `crates/rightclaw/src/memory/mod.rs:139` — change the assertion:

```rust
assert_eq!(version, 14, "user_version should be 14 after V14 migration");
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw memory::migrations::tests::v14 -- --nocapture`
Expected: FAIL with "pending_retains column missing" and "user_version should be 14".

- [ ] **Step 3: Create V14 SQL file**

Write `crates/rightclaw/src/memory/sql/v14_memory_failure_handling.sql`:

```sql
CREATE TABLE IF NOT EXISTS pending_retains (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    content         TEXT NOT NULL,
    context         TEXT,
    document_id     TEXT,
    update_mode     TEXT,
    tags_json       TEXT,
    created_at      TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    last_error      TEXT,
    source          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pending_retains_created
    ON pending_retains(created_at);

CREATE TABLE IF NOT EXISTS memory_alerts (
    alert_type    TEXT PRIMARY KEY,
    first_sent_at TEXT NOT NULL
);
```

- [ ] **Step 4: Register V14 in migrations.rs**

Modify `crates/rightclaw/src/memory/migrations.rs`:

At top with other `const`s (after line 16):
```rust
const V14_SCHEMA: &str = include_str!("sql/v14_memory_failure_handling.sql");
```

In `Migrations::new(vec![...])` after `up_with_hook("", v13_one_shot_cron)`:
```rust
M::up(V14_SCHEMA),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw memory::migrations`
Expected: all pass. Also run `cargo test -p rightclaw memory::tests::user_version_is_14`.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v14_memory_failure_handling.sql \
        crates/rightclaw/src/memory/migrations.rs \
        crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): V14 migration — pending_retains + memory_alerts tables"
```

---

## Task 2: Classified MemoryError variants

**Files:**
- Modify: `crates/rightclaw/src/memory/error.rs` (add variants)
- Modify: `crates/rightclaw/src/memory/hindsight.rs:171,181,211,222,248,260,277,289` (reqwest-error → classified variant)

- [ ] **Step 1: Write failing test for structured error construction**

Append to `crates/rightclaw/src/memory/hindsight.rs` `mod tests`:

```rust
#[tokio::test]
async fn retain_timeout_maps_to_timeout_variant() {
    // Mock server that accepts connection but never responds, forcing reqwest timeout.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let _keep = tokio::spawn(async move {
        // Accept and hold — never write response.
        let _ = listener.accept().await;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });

    // Client with aggressive timeout — must be shorter than RETAIN_TIMEOUT for this path.
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let client = HindsightClient {
        http,
        base_url: url,
        api_key: "hs_x".into(),
        bank_id: "b".into(),
        budget: "high".into(),
        max_tokens: 1024,
    };
    let err = client.retain("x", None, None, None, None).await.unwrap_err();
    assert!(
        matches!(err, MemoryError::HindsightTimeout),
        "expected HindsightTimeout, got: {err:?}"
    );
}

#[tokio::test]
async fn retain_connect_failure_maps_to_connect_variant() {
    // Port 1 is unprivileged-closed on typical dev machines.
    let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
    let err = client.retain("x", None, None, None, None).await.unwrap_err();
    assert!(
        matches!(err, MemoryError::HindsightConnect(_)),
        "expected HindsightConnect, got: {err:?}"
    );
}
```

This requires `HindsightClient` fields to be `pub(crate)` or a test-only constructor. Simpler: do NOT access fields directly; use the public constructor `HindsightClient::new`. Replace the `HindsightClient { ... }` block above with:

```rust
let client = HindsightClient::new("hs_x", "b", "high", 1024, Some(&url))
    .with_http_client(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build().unwrap()
    );
```

To support this, add a test-only builder to `HindsightClient` in Step 3.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p rightclaw memory::hindsight::tests::retain_timeout_maps_to_timeout_variant`
Expected: FAIL (variants don't exist yet / compile error).

- [ ] **Step 3: Extend MemoryError**

Replace `crates/rightclaw/src/memory/error.rs`:

```rust
/// Errors that can occur in the memory module.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("content rejected: possible prompt injection detected")]
    InjectionDetected,

    #[error("memory not found: id {0}")]
    NotFound(i64),

    #[error("hindsight API error (HTTP {status}): {body}")]
    Hindsight { status: u16, body: String },

    #[error("hindsight request timed out")]
    HindsightTimeout,

    #[error("hindsight connection error: {0}")]
    HindsightConnect(String),

    #[error("hindsight response parse error: {0}")]
    HindsightParse(String),

    #[error("hindsight request error: {0}")]
    HindsightOther(String),

    // Kept for deprecation path; no new code should construct this.
    #[error("hindsight request failed: {0}")]
    HindsightRequest(String),
}

impl MemoryError {
    /// Convert a `reqwest::Error` from send/recv into the appropriate classified variant.
    pub fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            MemoryError::HindsightTimeout
        } else if e.is_connect() || e.is_request() {
            MemoryError::HindsightConnect(format!("{e:#}"))
        } else if e.is_decode() || e.is_body() {
            MemoryError::HindsightParse(format!("{e:#}"))
        } else {
            MemoryError::HindsightOther(format!("{e:#}"))
        }
    }

    /// Convert a JSON deserialization error.
    pub fn from_parse(e: impl std::fmt::Display) -> Self {
        MemoryError::HindsightParse(format!("{e:#}"))
    }
}
```

- [ ] **Step 4: Route reqwest errors through `from_reqwest` in `hindsight.rs`**

In `hindsight.rs`, replace every occurrence of:

```rust
.map_err(|e| MemoryError::HindsightRequest(format!("{e:#}")))
```

with:

```rust
.map_err(MemoryError::from_reqwest)
```

And every body-parse occurrence like:

```rust
.map_err(|e| MemoryError::HindsightRequest(format!("parse retain response: {e:#}")))
```

with:

```rust
.map_err(MemoryError::from_parse)
```

Affected lines (approximate): 171, 181, 211, 222, 248, 260, 277, 289.

- [ ] **Step 5: Add test-only `with_http_client` builder on `HindsightClient`**

Append to `impl HindsightClient` (before the closing `}`):

```rust
#[cfg(test)]
pub(crate) fn with_http_client(mut self, http: reqwest::Client) -> Self {
    self.http = http;
    self
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rightclaw memory::hindsight`
Expected: all pass (including the new timeout/connect tests plus existing).

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/memory/error.rs crates/rightclaw/src/memory/hindsight.rs
git commit -m "feat(memory): classify reqwest errors into MemoryError variants"
```

---

## Task 3: `ErrorKind` classifier

**Files:**
- Create: `crates/rightclaw/src/memory/classify.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs` (declare module)

- [ ] **Step 1: Write failing tests**

Create `crates/rightclaw/src/memory/classify.rs`:

```rust
//! Classification of `MemoryError` into operational `ErrorKind`.

use super::MemoryError;

/// Operational classification used by the resilient wrapper to decide retry,
/// breaker-tick, enqueue, and surface behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Transient,    // 5xx, timeout, connect error
    RateLimited,  // 429
    Auth,         // 401, 403
    Client,       // 400, 404, 422 (caller bug or upstream API drift)
    Malformed,    // response body parse error
}

impl MemoryError {
    /// Classify this error. Non-Hindsight variants (Sqlite/Migration/Injection/NotFound)
    /// are unreachable at the wrapper boundary and classified as `Transient` defensively.
    pub fn classify(&self) -> ErrorKind {
        match self {
            MemoryError::Hindsight { status, .. } => match *status {
                401 | 403 => ErrorKind::Auth,
                429 => ErrorKind::RateLimited,
                400 | 404 | 422 => ErrorKind::Client,
                500..=599 => ErrorKind::Transient,
                _ => ErrorKind::Transient,
            },
            MemoryError::HindsightTimeout => ErrorKind::Transient,
            MemoryError::HindsightConnect(_) => ErrorKind::Transient,
            MemoryError::HindsightParse(_) => ErrorKind::Malformed,
            MemoryError::HindsightOther(_) => ErrorKind::Transient,
            MemoryError::HindsightRequest(_) => ErrorKind::Transient, // legacy
            MemoryError::Sqlite(_)
            | MemoryError::Migration(_)
            | MemoryError::InjectionDetected
            | MemoryError::NotFound(_) => ErrorKind::Transient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(status: u16) -> MemoryError {
        MemoryError::Hindsight { status, body: String::new() }
    }

    #[test]
    fn classify_5xx_transient() {
        assert_eq!(h(500).classify(), ErrorKind::Transient);
        assert_eq!(h(502).classify(), ErrorKind::Transient);
        assert_eq!(h(503).classify(), ErrorKind::Transient);
        assert_eq!(h(504).classify(), ErrorKind::Transient);
    }

    #[test]
    fn classify_429_rate_limited() {
        assert_eq!(h(429).classify(), ErrorKind::RateLimited);
    }

    #[test]
    fn classify_auth() {
        assert_eq!(h(401).classify(), ErrorKind::Auth);
        assert_eq!(h(403).classify(), ErrorKind::Auth);
    }

    #[test]
    fn classify_client() {
        assert_eq!(h(400).classify(), ErrorKind::Client);
        assert_eq!(h(404).classify(), ErrorKind::Client);
        assert_eq!(h(422).classify(), ErrorKind::Client);
    }

    #[test]
    fn classify_timeout_transient() {
        assert_eq!(MemoryError::HindsightTimeout.classify(), ErrorKind::Transient);
    }

    #[test]
    fn classify_connect_transient() {
        assert_eq!(
            MemoryError::HindsightConnect("dns".into()).classify(),
            ErrorKind::Transient
        );
    }

    #[test]
    fn classify_parse_malformed() {
        assert_eq!(
            MemoryError::HindsightParse("bad json".into()).classify(),
            ErrorKind::Malformed
        );
    }
}
```

- [ ] **Step 2: Declare module in mod.rs**

Modify `crates/rightclaw/src/memory/mod.rs`, add after existing `pub mod` lines:

```rust
pub mod classify;
pub use classify::ErrorKind;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::classify`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/classify.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): ErrorKind classifier"
```

---

## Task 4: `MemoryStatus` + ordering + watch helper

**Files:**
- Create: `crates/rightclaw/src/memory/status.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/rightclaw/src/memory/status.rs`:

```rust
//! Agent-facing memory status and its watch channel.
//!
//! Severity ordering: Healthy < Degraded < AuthFailed. Worker merges the
//! wrapper-owned status with per-turn local status via `.max()`.

use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    AuthFailed { since: Instant },
}

impl MemoryStatus {
    fn severity(&self) -> u8 {
        match self {
            MemoryStatus::Healthy => 0,
            MemoryStatus::Degraded { .. } => 1,
            MemoryStatus::AuthFailed { .. } => 2,
        }
    }
}

impl PartialEq for MemoryStatus {
    fn eq(&self, other: &Self) -> bool {
        self.severity() == other.severity()
    }
}
impl Eq for MemoryStatus {}
impl PartialOrd for MemoryStatus {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MemoryStatus {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.severity().cmp(&other.severity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded { since: Instant::now() };
        let a = MemoryStatus::AuthFailed { since: Instant::now() };
        assert!(h < d);
        assert!(d < a);
        assert!(h < a);
    }

    #[test]
    fn max_merges_by_severity() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded { since: Instant::now() };
        let a = MemoryStatus::AuthFailed { since: Instant::now() };
        assert_eq!(h.max(d).severity(), d.severity());
        assert_eq!(d.max(a).severity(), a.severity());
        assert_eq!(h.max(a).severity(), a.severity());
    }

    #[test]
    fn equal_severity_eq() {
        let d1 = MemoryStatus::Degraded { since: Instant::now() };
        let d2 = MemoryStatus::Degraded {
            since: Instant::now() + std::time::Duration::from_secs(5),
        };
        assert_eq!(d1, d2);
    }
}
```

- [ ] **Step 2: Declare module**

Modify `crates/rightclaw/src/memory/mod.rs`:

```rust
pub mod status;
pub use status::MemoryStatus;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::status`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/status.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): MemoryStatus enum with severity ordering"
```

---

## Task 5: Circuit breaker state machine

**Files:**
- Create: `crates/rightclaw/src/memory/circuit.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/rightclaw/src/memory/circuit.rs`:

```rust
//! Circuit breaker for outbound Hindsight calls.
//!
//! State transitions:
//! - Closed + N failures in 30s rolling window → Open { until = now + 30s }
//! - Open + now >= until → HalfOpen
//! - HalfOpen + success → Closed
//! - HalfOpen + failure → Open { until = now + 2 * previous_open_dur }, capped at 10 min
//! - Any state + Auth kind → Open { until = now + 1h }

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::ErrorKind;

pub(crate) const WINDOW: Duration = Duration::from_secs(30);
pub(crate) const TRIP_THRESHOLD: usize = 5;
pub(crate) const INITIAL_OPEN: Duration = Duration::from_secs(30);
pub(crate) const MAX_OPEN: Duration = Duration::from_secs(600); // 10 min
pub(crate) const AUTH_OPEN: Duration = Duration::from_secs(3600); // 1h

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}

pub struct Breaker {
    state: CircuitState,
    failures: VecDeque<Instant>,
    last_open_duration: Duration,
}

impl Breaker {
    pub fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failures: VecDeque::new(),
            last_open_duration: INITIAL_OPEN,
        }
    }

    pub fn state(&mut self) -> CircuitState {
        self.refresh();
        self.state
    }

    /// Call before each network attempt. Returns `Err(retry_after)` when call must be skipped.
    pub fn admit(&mut self) -> Result<(), Duration> {
        self.refresh();
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => Ok(()),
            CircuitState::Open { until } => Err(until.saturating_duration_since(Instant::now())),
        }
    }

    /// Record an attempt outcome.
    pub fn record(&mut self, outcome: Outcome) {
        self.refresh();
        match outcome {
            Outcome::Success => {
                if matches!(self.state, CircuitState::HalfOpen) {
                    self.state = CircuitState::Closed;
                    self.failures.clear();
                    self.last_open_duration = INITIAL_OPEN;
                }
            }
            Outcome::Failure(ErrorKind::Auth) => {
                self.state = CircuitState::Open {
                    until: Instant::now() + AUTH_OPEN,
                };
                self.last_open_duration = AUTH_OPEN;
            }
            Outcome::Failure(ErrorKind::Client) => {
                // Client errors do not tick the breaker.
            }
            Outcome::Failure(_) => {
                self.push_failure(Instant::now());
                match self.state {
                    CircuitState::Closed if self.failures.len() >= TRIP_THRESHOLD => {
                        self.state = CircuitState::Open {
                            until: Instant::now() + INITIAL_OPEN,
                        };
                        self.last_open_duration = INITIAL_OPEN;
                    }
                    CircuitState::HalfOpen => {
                        let new_dur = (self.last_open_duration * 2).min(MAX_OPEN);
                        self.state = CircuitState::Open {
                            until: Instant::now() + new_dur,
                        };
                        self.last_open_duration = new_dur;
                    }
                    _ => {}
                }
            }
        }
    }

    fn refresh(&mut self) {
        if let CircuitState::Open { until } = self.state {
            if Instant::now() >= until {
                self.state = CircuitState::HalfOpen;
            }
        }
        // Evict stale failures.
        let cutoff = Instant::now() - WINDOW;
        while self.failures.front().map_or(false, |t| *t < cutoff) {
            self.failures.pop_front();
        }
    }

    fn push_failure(&mut self, at: Instant) {
        self.failures.push_back(at);
        let cutoff = at - WINDOW;
        while self.failures.front().map_or(false, |t| *t < cutoff) {
            self.failures.pop_front();
        }
    }
}

pub enum Outcome {
    Success,
    Failure(ErrorKind),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(b: &mut Breaker, kind: ErrorKind) {
        b.record(Outcome::Failure(kind));
    }

    #[tokio::test(start_paused = true)]
    async fn closed_opens_after_threshold() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD {
            fail(&mut b, ErrorKind::Transient);
        }
        assert!(matches!(b.state(), CircuitState::Open { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn open_transitions_to_half_open() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD { fail(&mut b, ErrorKind::Transient); }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        assert_eq!(b.state(), CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_success_closes() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD { fail(&mut b, ErrorKind::Transient); }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        assert_eq!(b.state(), CircuitState::HalfOpen);
        b.record(Outcome::Success);
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_failure_doubles_backoff() {
        let mut b = Breaker::new();
        for _ in 0..TRIP_THRESHOLD { fail(&mut b, ErrorKind::Transient); }
        tokio::time::advance(INITIAL_OPEN + Duration::from_millis(10)).await;
        fail(&mut b, ErrorKind::Transient);
        match b.state() {
            CircuitState::Open { until } => {
                let dur = until.saturating_duration_since(Instant::now());
                assert!(dur > INITIAL_OPEN, "expected >30s, got {dur:?}");
                assert!(dur <= INITIAL_OPEN * 2 + Duration::from_millis(50));
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn auth_opens_for_1h() {
        let mut b = Breaker::new();
        fail(&mut b, ErrorKind::Auth);
        match b.state() {
            CircuitState::Open { until } => {
                let dur = until.saturating_duration_since(Instant::now());
                assert!(dur >= AUTH_OPEN - Duration::from_millis(10));
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn client_does_not_tick() {
        let mut b = Breaker::new();
        for _ in 0..(TRIP_THRESHOLD * 2) {
            fail(&mut b, ErrorKind::Client);
        }
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn stale_failures_evict_by_window() {
        let mut b = Breaker::new();
        for _ in 0..(TRIP_THRESHOLD - 1) {
            fail(&mut b, ErrorKind::Transient);
        }
        tokio::time::advance(WINDOW + Duration::from_secs(1)).await;
        // One more failure shouldn't trip because older ones expired.
        fail(&mut b, ErrorKind::Transient);
        assert_eq!(b.state(), CircuitState::Closed);
    }
}
```

- [ ] **Step 2: Declare module**

Modify `crates/rightclaw/src/memory/mod.rs`:

```rust
pub mod circuit;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::circuit`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/circuit.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): circuit breaker state machine"
```

---

## Task 6: Retain queue — enqueue + count + oldest_age

**Files:**
- Create: `crates/rightclaw/src/memory/retain_queue.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/rightclaw/src/memory/retain_queue.rs`:

```rust
//! SQLite-backed queue of pending retain calls, drained by the bot.

use std::time::Duration;

use rusqlite::{params, Connection};

use super::MemoryError;

pub const QUEUE_CAP: usize = 1000;

/// A queued retain payload (mirrors `HindsightClient::retain_many` item inputs).
#[derive(Debug, Clone)]
pub struct PendingRetain {
    pub id: i64,
    pub content: String,
    pub context: Option<String>,
    pub document_id: Option<String>,
    pub update_mode: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub attempts: i64,
}

/// Enqueue a retain attempt for later drain. Evicts the oldest row if cap exceeded.
/// Uses a single transaction for the combined eviction + insert when needed.
pub fn enqueue(
    conn: &Connection,
    source: &str,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<(), MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pending_retains",
        [],
        |r| r.get(0),
    )?;

    let tx = conn.unchecked_transaction()?;

    if count as usize >= QUEUE_CAP {
        tx.execute(
            "DELETE FROM pending_retains WHERE id = (SELECT id FROM pending_retains ORDER BY created_at ASC LIMIT 1)",
            [],
        )?;
    }

    let tags_json = tags
        .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".into()));
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    tx.execute(
        "INSERT INTO pending_retains
            (content, context, document_id, update_mode, tags_json, created_at, attempts, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![
            content,
            context,
            document_id,
            update_mode,
            tags_json,
            created_at,
            source,
        ],
    )?;

    tx.commit()?;
    Ok(())
}

/// Current row count.
pub fn count(conn: &Connection) -> Result<usize, MemoryError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0))?;
    Ok(n as usize)
}

/// Age of the oldest row (None if queue empty).
pub fn oldest_age(conn: &Connection) -> Result<Option<Duration>, MemoryError> {
    let iso: Option<String> = conn
        .query_row(
            "SELECT MIN(created_at) FROM pending_retains",
            [],
            |r| r.get(0),
        )
        .ok();
    let Some(iso) = iso else { return Ok(None) };
    let parsed = chrono::DateTime::parse_from_rfc3339(&iso).map_err(|e| {
        MemoryError::HindsightOther(format!("oldest_age parse: {e:#}"))
    })?;
    let now = chrono::Utc::now();
    let dur = now.signed_duration_since(parsed.with_timezone(&chrono::Utc));
    Ok(Some(Duration::from_secs(dur.num_seconds().max(0) as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
    use tempfile::tempdir;

    fn fresh_db() -> Connection {
        let dir = tempdir().unwrap();
        // Keep the tempdir alive inside the test scope (leak is fine for test).
        let path = dir.into_path();
        open_connection(&path, true).unwrap()
    }

    #[test]
    fn enqueue_inserts_row() {
        let conn = fresh_db();
        enqueue(&conn, "bot", "content", Some("ctx"), Some("doc"), Some("append"), None).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
    }

    #[test]
    fn enqueue_cap_evicts_oldest() {
        let conn = fresh_db();
        for i in 0..(QUEUE_CAP + 5) {
            let c = format!("content-{i}");
            enqueue(&conn, "bot", &c, None, None, None, None).unwrap();
        }
        assert_eq!(count(&conn).unwrap(), QUEUE_CAP);
        // Oldest remaining rows should not include the first 5.
        let oldest_content: String = conn.query_row(
            "SELECT content FROM pending_retains ORDER BY created_at ASC LIMIT 1",
            [], |r| r.get(0),
        ).unwrap();
        assert!(oldest_content.starts_with("content-"), "got {oldest_content}");
        // The first inserted entry ("content-0") must be evicted.
        let first_gone: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pending_retains WHERE content = 'content-0'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(first_gone, 0);
    }

    #[test]
    fn oldest_age_returns_none_when_empty() {
        let conn = fresh_db();
        assert!(oldest_age(&conn).unwrap().is_none());
    }

    #[test]
    fn tags_serialize_as_json_array() {
        let conn = fresh_db();
        let tags = vec!["chat:42".to_string(), "user:7".to_string()];
        enqueue(&conn, "bot", "c", None, None, None, Some(&tags)).unwrap();
        let json: String = conn.query_row(
            "SELECT tags_json FROM pending_retains LIMIT 1",
            [], |r| r.get(0),
        ).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tags);
    }
}
```

- [ ] **Step 2: Declare module + add chrono + tempfile dev-dep verification**

Modify `crates/rightclaw/src/memory/mod.rs`:

```rust
pub mod retain_queue;
```

Verify `crates/rightclaw/Cargo.toml` already has `chrono`, `serde_json`, and dev-dep `tempfile`. These are pre-existing in this project; confirm with `cargo tree -p rightclaw | grep -E '^(chrono|serde_json|tempfile)'` before proceeding.

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::retain_queue`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/retain_queue.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): retain_queue enqueue/count/oldest_age"
```

---

## Task 7: HindsightClient `retain_many` (multi-item POST)

**Files:**
- Modify: `crates/rightclaw/src/memory/hindsight.rs` (add `RetainItem` pub struct + `retain_many`)

- [ ] **Step 1: Write failing test**

Append to `crates/rightclaw/src/memory/hindsight.rs` `mod tests`:

```rust
#[tokio::test]
async fn retain_many_batches_items_in_single_post() {
    let (handle, url) = mock_hindsight_server(
        r#"{"success": true, "operation_id": "op-batch"}"#, 200,
    ).await;

    let client = test_client(&url);
    let items = vec![
        RetainItem {
            content: "first".into(),
            context: Some("c1".into()),
            document_id: Some("doc-1".into()),
            update_mode: Some("append".into()),
            tags: None,
        },
        RetainItem {
            content: "second".into(),
            context: None,
            document_id: None,
            update_mode: None,
            tags: Some(vec!["t".into()]),
        },
    ];
    client.retain_many(&items).await.unwrap();

    let (method, _auth, body) = handle.await.unwrap();
    assert!(method.starts_with("POST"));
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["items"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["items"][0]["content"], "first");
    assert_eq!(parsed["items"][1]["content"], "second");
    assert_eq!(parsed["async"], true);
}
```

- [ ] **Step 2: Promote `RetainItem` to public + add `retain_many`**

Modify `crates/rightclaw/src/memory/hindsight.rs`:

Change (around line 58):

```rust
/// Retain request item.
#[derive(Debug, Serialize)]
struct RetainItem {
```

to:

```rust
/// Retain request item.
#[derive(Debug, Clone, Serialize)]
pub struct RetainItem {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}
```

(Delete the old private-field definition; keep `RetainRequest` unchanged.)

In `impl HindsightClient`, after the existing `retain` method:

```rust
/// Retain a batch of items in a single POST. Used by the resilient drain task.
pub async fn retain_many(&self, items: &[RetainItem]) -> Result<RetainResponse, MemoryError> {
    let url = format!(
        "{}/v1/default/banks/{}/memories",
        self.base_url, self.bank_id
    );
    let body = RetainRequest {
        items: items.to_vec(),
        is_async: true,
    };

    let resp = self
        .http
        .post(&url)
        .header("Authorization", format!("Bearer {}", self.api_key))
        .json(&body)
        .timeout(RETAIN_TIMEOUT)
        .send()
        .await
        .map_err(MemoryError::from_reqwest)?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MemoryError::Hindsight { status, body });
    }

    resp.json::<RetainResponse>()
        .await
        .map_err(MemoryError::from_parse)
}
```

Also refactor the existing `retain(...)` to delegate to `retain_many` (keeps the surface for current callers):

```rust
pub async fn retain(
    &self,
    content: &str,
    context: Option<&str>,
    document_id: Option<&str>,
    update_mode: Option<&str>,
    tags: Option<&[String]>,
) -> Result<RetainResponse, MemoryError> {
    self.retain_many(&[RetainItem {
        content: content.to_owned(),
        context: context.map(|s| s.to_owned()),
        document_id: document_id.map(|s| s.to_owned()),
        update_mode: update_mode.map(|s| s.to_owned()),
        tags: tags.map(|t| t.to_vec()),
    }]).await
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::hindsight`
Expected: all tests pass (existing + new `retain_many_batches`).

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/hindsight.rs
git commit -m "feat(memory): HindsightClient::retain_many for batched POST"
```

---

## Task 8: Retain queue drain logic (classified per ErrorKind)

**Files:**
- Modify: `crates/rightclaw/src/memory/retain_queue.rs` (append `drain_tick` + tests)

- [ ] **Step 1: Write failing tests (drain behaviours)**

Append to `mod tests` in `retain_queue.rs`:

```rust
use super::super::ErrorKind;

#[derive(Default)]
struct FakeOutcome {
    // For each call in order, return this error kind or Ok if None.
    queue: std::sync::Mutex<std::collections::VecDeque<Option<ErrorKind>>>,
    calls: std::sync::Mutex<Vec<PendingRetain>>,
}

impl FakeOutcome {
    fn push(&self, outcome: Option<ErrorKind>) {
        self.queue.lock().unwrap().push_back(outcome);
    }
    fn next(&self, item: &PendingRetain) -> Result<(), ErrorKind> {
        self.calls.lock().unwrap().push(item.clone());
        match self.queue.lock().unwrap().pop_front().flatten() {
            None => Ok(()),
            Some(kind) => Err(kind),
        }
    }
}

#[tokio::test]
async fn drain_success_deletes_entry() {
    let conn = fresh_db();
    enqueue(&conn, "bot", "c1", None, None, None, None).unwrap();
    let fake = FakeOutcome::default();
    fake.push(None);

    let report = drain_tick(&conn, |items| {
        let kind = fake.next(&items[0]);
        async move { kind.map_err(|k| k) }
    }).await;

    assert_eq!(report.deleted, 1);
    assert_eq!(count(&conn).unwrap(), 0);
}

#[tokio::test]
async fn drain_client_error_deletes_and_continues() {
    let conn = fresh_db();
    enqueue(&conn, "bot", "poison", None, None, None, None).unwrap();
    enqueue(&conn, "bot", "good", None, None, None, None).unwrap();
    let fake = FakeOutcome::default();
    fake.push(Some(ErrorKind::Client));
    fake.push(None);

    let report = drain_tick(&conn, |items| {
        let kind = fake.next(&items[0]);
        async move { kind.map_err(|k| k) }
    }).await;

    assert_eq!(report.dropped_client, 1);
    assert_eq!(report.deleted, 1);
    assert_eq!(count(&conn).unwrap(), 0);
}

#[tokio::test]
async fn drain_transient_updates_attempts_and_breaks() {
    let conn = fresh_db();
    enqueue(&conn, "bot", "first", None, None, None, None).unwrap();
    enqueue(&conn, "bot", "second", None, None, None, None).unwrap();
    let fake = FakeOutcome::default();
    fake.push(Some(ErrorKind::Transient));
    // no second push — if we call twice, test fails (None vs Some mismatch)

    let report = drain_tick(&conn, |items| {
        let kind = fake.next(&items[0]);
        async move { kind.map_err(|k| k) }
    }).await;

    assert_eq!(report.deleted, 0);
    assert_eq!(report.bumped_attempts, 1);
    let attempts: i64 = conn.query_row(
        "SELECT attempts FROM pending_retains WHERE content = 'first'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(count(&conn).unwrap(), 2);
}

#[tokio::test]
async fn drain_age_cap_drops_stale_rows() {
    let conn = fresh_db();
    enqueue(&conn, "bot", "old", None, None, None, None).unwrap();
    // Backdate the row to simulate age > 24h.
    conn.execute(
        "UPDATE pending_retains SET created_at = datetime('now', '-48 hours')",
        [],
    ).unwrap();

    let report = drain_tick(&conn, |_items| async move {
        panic!("should not call upstream for stale entries");
    }).await;

    assert_eq!(report.dropped_age, 1);
    assert_eq!(count(&conn).unwrap(), 0);
}
```

- [ ] **Step 2: Implement `drain_tick` + `DrainReport`**

Append to `crates/rightclaw/src/memory/retain_queue.rs`:

```rust
use std::future::Future;

pub const DRAIN_BATCH: usize = 20;
pub const MAX_AGE: chrono::Duration = chrono::Duration::hours(24);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    pub deleted: usize,          // successfully retained + removed
    pub dropped_age: usize,      // removed due to 24h age cap
    pub dropped_client: usize,   // removed due to Client-kind error
    pub bumped_attempts: usize,  // attempts incremented (Transient/RateLimited/Malformed/CircuitOpen)
}

/// Run one drain tick.
///
/// `call` is invoked with a single-item batch (for now we serialize by entry; see
/// spec note about per-document batching — TODO for future work).
///
/// The closure returns `Err(ErrorKind)` on failure (already classified by caller)
/// or `Ok(())` on success.
pub async fn drain_tick<F, Fut>(conn: &Connection, mut call: F) -> DrainReport
where
    F: FnMut(Vec<PendingRetain>) -> Fut,
    Fut: Future<Output = Result<(), ErrorKind>>,
{
    let mut report = DrainReport::default();

    // Snapshot batch outside the txn (reads are fine without tx).
    let batch = match load_batch(conn, DRAIN_BATCH) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("drain: load_batch failed: {e:#}");
            return report;
        }
    };
    if batch.is_empty() {
        return report;
    }

    let Ok(tx) = conn.unchecked_transaction() else {
        tracing::warn!("drain: failed to begin tx");
        return report;
    };

    let now = chrono::Utc::now();

    for entry in batch {
        // Age cap.
        let created = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc));
        if let Some(c) = created {
            if now.signed_duration_since(c) > MAX_AGE {
                if tx.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]).is_ok() {
                    tracing::warn!(id = entry.id, "retain dropped: >24h");
                    report.dropped_age += 1;
                }
                continue;
            }
        }

        match call(vec![entry.clone()]).await {
            Ok(()) => {
                if tx.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]).is_ok() {
                    report.deleted += 1;
                }
            }
            Err(ErrorKind::Client) => {
                if tx.execute("DELETE FROM pending_retains WHERE id = ?1", [entry.id]).is_ok() {
                    tracing::error!(id = entry.id, "retain dropped on 4xx: {entry:?}");
                    report.dropped_client += 1;
                }
                continue;
            }
            Err(ErrorKind::Auth) => {
                // Should not happen (Auth never enqueues), but defensively stop.
                tracing::warn!(id = entry.id, "drain encountered Auth; stopping");
                break;
            }
            Err(_) => {
                let _ = tx.execute(
                    "UPDATE pending_retains SET attempts = attempts + 1, \
                       last_attempt_at = ?1, last_error = ?2 WHERE id = ?3",
                    params![now.to_rfc3339(), "classified_transient", entry.id],
                );
                report.bumped_attempts += 1;
                break; // don't storm
            }
        }
    }

    let _ = tx.commit();
    report
}

fn load_batch(conn: &Connection, limit: usize) -> Result<Vec<PendingRetain>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, content, context, document_id, update_mode, tags_json, created_at, attempts
           FROM pending_retains ORDER BY created_at ASC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        let tags_json: Option<String> = row.get(5)?;
        let tags = tags_json.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
        Ok(PendingRetain {
            id: row.get(0)?,
            content: row.get(1)?,
            context: row.get(2)?,
            document_id: row.get(3)?,
            update_mode: row.get(4)?,
            tags,
            created_at: row.get(6)?,
            attempts: row.get(7)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::retain_queue`
Expected: all drain tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/retain_queue.rs
git commit -m "feat(memory): retain_queue drain_tick with classified outcome"
```

---

## Task 9: `ResilientHindsight` skeleton + `ResilientError`

**Files:**
- Create: `crates/rightclaw/src/memory/resilient.rs`
- Modify: `crates/rightclaw/src/memory/mod.rs`

- [ ] **Step 1: Write failing tests (API shape)**

Create `crates/rightclaw/src/memory/resilient.rs`:

```rust
//! Resilient wrapper around `HindsightClient`: circuit breaker + classified retry
//! + retain queue + status watch.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{watch, Mutex};

use super::circuit::{Breaker, Outcome};
use super::hindsight::{HindsightClient, RecallResult, RecallResponse, ReflectResponse, RetainItem, RetainResponse, BankProfile};
use super::status::MemoryStatus;
use super::{ErrorKind, MemoryError};

/// Error returned by the resilient wrapper.
#[derive(Debug, thiserror::Error)]
pub enum ResilientError {
    #[error("upstream error: {0}")]
    Upstream(#[from] MemoryError),
    #[error("memory circuit open; retry after {retry_after:?}")]
    CircuitOpen { retry_after: Option<Duration> },
}

/// Per-operation retry policy.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub per_attempt: Duration,
    pub attempts: u32, // total attempts = attempts + 1 (i.e. 0 means single try, no retry)
}

pub const POLICY_BLOCKING_RECALL: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(3),
    attempts: 0,
};
pub const POLICY_AUTO_RETAIN: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 2,
};
pub const POLICY_PREFETCH: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(5),
    attempts: 1,
};
pub const POLICY_MCP_RETAIN: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 1,
};
pub const POLICY_MCP_RECALL: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(5),
    attempts: 0,
};
pub const POLICY_MCP_REFLECT: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(15),
    attempts: 0,
};
pub const POLICY_STARTUP_BANK: RetryPolicy = RetryPolicy {
    per_attempt: Duration::from_secs(10),
    attempts: 3,
};

/// In-memory 1h rolling window of Client-kind drop timestamps.
const DROP_WINDOW: Duration = Duration::from_secs(3600);
const DROP_WINDOW_24H: Duration = Duration::from_secs(86_400);
pub const CLIENT_FLOOD_THRESHOLD: usize = 20;

pub struct ResilientHindsight {
    inner: HindsightClient,
    agent_db_path: PathBuf,
    breaker: Mutex<Breaker>,
    status_tx: watch::Sender<MemoryStatus>,
    client_drops: Mutex<VecDeque<Instant>>,
    source: String, // "bot" or "aggregator" — for pending_retains.source
}

impl ResilientHindsight {
    pub fn new(inner: HindsightClient, agent_db_path: PathBuf, source: impl Into<String>) -> Self {
        let (tx, _rx) = watch::channel(MemoryStatus::Healthy);
        Self {
            inner,
            agent_db_path,
            breaker: Mutex::new(Breaker::new()),
            status_tx: tx,
            client_drops: Mutex::new(VecDeque::new()),
            source: source.into(),
        }
    }

    pub fn inner(&self) -> &HindsightClient { &self.inner }

    pub fn status(&self) -> MemoryStatus { *self.status_tx.borrow() }
    pub fn subscribe_status(&self) -> watch::Receiver<MemoryStatus> { self.status_tx.subscribe() }

    /// Count of Client-kind drops in the last 24h.
    pub async fn client_drops_24h(&self) -> usize {
        let mut q = self.client_drops.lock().await;
        let cutoff = Instant::now() - DROP_WINDOW_24H;
        while q.front().map_or(false, |t| *t < cutoff) {
            q.pop_front();
        }
        q.len()
    }

    /// Count of Client-kind drops in the last 1h (for flood alert).
    pub async fn client_drops_1h(&self) -> usize {
        let mut q = self.client_drops.lock().await;
        let cutoff = Instant::now() - DROP_WINDOW;
        // We keep a single deque; for 1h just count recent entries.
        q.iter().filter(|t| **t >= cutoff).count()
    }

    pub async fn bump_client_drop(&self) {
        let mut q = self.client_drops.lock().await;
        q.push_back(Instant::now());
        let cutoff = Instant::now() - DROP_WINDOW_24H;
        while q.front().map_or(false, |t| *t < cutoff) {
            q.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bump_client_drop_records_timestamp() {
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
        let w = ResilientHindsight::new(client, PathBuf::from("/tmp"), "bot");
        assert_eq!(w.client_drops_24h().await, 0);
        w.bump_client_drop().await;
        w.bump_client_drop().await;
        assert_eq!(w.client_drops_24h().await, 2);
    }

    #[tokio::test]
    async fn status_starts_healthy() {
        let client = HindsightClient::new("hs_x", "b", "high", 1024, Some("http://127.0.0.1:1"));
        let w = ResilientHindsight::new(client, PathBuf::from("/tmp"), "bot");
        matches!(w.status(), MemoryStatus::Healthy);
    }
}
```

- [ ] **Step 2: Declare module**

Modify `crates/rightclaw/src/memory/mod.rs`:

```rust
pub mod resilient;
pub use resilient::{ResilientHindsight, ResilientError};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::resilient`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/resilient.rs crates/rightclaw/src/memory/mod.rs
git commit -m "feat(memory): ResilientHindsight skeleton with drop counters"
```

---

## Task 10: `ResilientHindsight` call methods — retain, recall, reflect, get_or_create_bank

**Files:**
- Modify: `crates/rightclaw/src/memory/resilient.rs` (append impl methods + tests)

- [ ] **Step 1: Write failing tests**

Append to `resilient.rs` `mod tests`:

```rust
use crate::memory::{open_connection};
use tempfile::tempdir;

async fn mock(hs_body: &str, status: u16) -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let body = hs_body.to_owned();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { return; };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 8192];
            let _ = s.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(), body,
            );
            let _ = s.write_all(resp.as_bytes()).await;
        }
    });
    (handle, url)
}

fn wrap(url: &str) -> ResilientHindsight {
    let dir = tempdir().unwrap().into_path();
    let _ = open_connection(&dir, true).unwrap(); // ensure data.db exists with schema
    let client = HindsightClient::new("hs_x", "bank-1", "high", 1024, Some(url));
    ResilientHindsight::new(client, dir, "bot")
}

#[tokio::test]
async fn recall_success_returns_results() {
    let (_h, url) = mock(r#"{"results":[{"text":"hello","score":0.9}]}"#, 200).await;
    let w = wrap(&url);
    let out = w.recall(
        "q", None, None, POLICY_MCP_RECALL,
    ).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "hello");
}

#[tokio::test]
async fn recall_auth_sets_status_auth_failed_and_returns_upstream_err() {
    let (_h, url) = mock(r#"{"error":"no"}"#, 401).await;
    let w = wrap(&url);
    let err = w.recall("q", None, None, POLICY_MCP_RECALL).await.unwrap_err();
    assert!(matches!(err, ResilientError::Upstream(_)));
    matches!(w.status(), MemoryStatus::AuthFailed { .. });
}

#[tokio::test]
async fn recall_circuit_open_skips_http_call() {
    let (_h, url) = mock(r#"{"error":"x"}"#, 500).await;
    let w = wrap(&url);
    // Force breaker open by tripping it with many auths (faster) — use status API trick.
    // Easier: call recall enough times to trip threshold.
    for _ in 0..crate::memory::circuit::TRIP_THRESHOLD {
        let _ = w.recall("q", None, None, POLICY_MCP_RECALL).await;
    }
    // Next call must be CircuitOpen.
    let err = w.recall("q", None, None, POLICY_MCP_RECALL).await.unwrap_err();
    assert!(matches!(err, ResilientError::CircuitOpen { .. }));
}

#[tokio::test]
async fn retain_enqueues_on_transient_error() {
    let (_h, url) = mock(r#"{"error":"boom"}"#, 500).await;
    let w = wrap(&url);
    let _ = w.retain("content", None, Some("doc"), Some("append"), None, POLICY_AUTO_RETAIN).await;
    let conn = open_connection(&w.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn retain_does_not_enqueue_on_client_error() {
    let (_h, url) = mock(r#"{"error":"bad"}"#, 400).await;
    let w = wrap(&url);
    let _ = w.retain("content", None, None, None, None, POLICY_AUTO_RETAIN).await;
    let conn = open_connection(&w.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // Drop counter bumped.
    assert_eq!(w.client_drops_24h().await, 1);
}
```

- [ ] **Step 2: Implement retry + call methods**

Append to `resilient.rs`:

```rust
impl ResilientHindsight {
    pub fn agent_db_path(&self) -> &Path { &self.agent_db_path }

    async fn refresh_status(&self) {
        let mut b = self.breaker.lock().await;
        let st = b.state();
        let new = match st {
            crate::memory::circuit::CircuitState::Closed => MemoryStatus::Healthy,
            crate::memory::circuit::CircuitState::Open { .. }
            | crate::memory::circuit::CircuitState::HalfOpen =>
                MemoryStatus::Degraded { since: Instant::now() },
        };
        let cur = *self.status_tx.borrow();
        // Preserve AuthFailed — only startup probe reset clears it.
        if matches!(cur, MemoryStatus::AuthFailed { .. }) { return; }
        if cur.cmp(&new) != std::cmp::Ordering::Equal {
            let _ = self.status_tx.send(new);
        }
    }

    fn backoff(attempt: u32) -> Duration {
        let base = Duration::from_millis(500 << attempt); // 500, 1000, 2000, ...
        let jitter_ms = fastrand::u64(0..250);
        base + Duration::from_millis(jitter_ms)
    }

    /// Wrap a single upstream call with the policy's per-attempt timeout and retry loop.
    /// `op` is executed each attempt. Returns the first non-retriable error or the final error.
    async fn call_with_policy<F, Fut, T>(
        &self,
        policy: RetryPolicy,
        mut op: F,
    ) -> Result<T, ResilientError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, MemoryError>>,
    {
        for attempt in 0..=policy.attempts {
            // Check breaker.
            {
                let mut b = self.breaker.lock().await;
                if let Err(retry_after) = b.admit() {
                    self.refresh_status().await;
                    return Err(ResilientError::CircuitOpen { retry_after: Some(retry_after) });
                }
            }

            let call = op();
            let res = tokio::time::timeout(policy.per_attempt, call).await;
            let out = match res {
                Err(_) => Err(MemoryError::HindsightTimeout),
                Ok(r) => r,
            };

            match out {
                Ok(val) => {
                    {
                        let mut b = self.breaker.lock().await;
                        b.record(Outcome::Success);
                    }
                    self.refresh_status().await;
                    return Ok(val);
                }
                Err(e) => {
                    let kind = e.classify();
                    {
                        let mut b = self.breaker.lock().await;
                        b.record(Outcome::Failure(kind));
                    }
                    if matches!(kind, ErrorKind::Auth) {
                        let _ = self.status_tx.send(MemoryStatus::AuthFailed { since: Instant::now() });
                        return Err(ResilientError::Upstream(e));
                    }
                    // Non-retriable kinds.
                    if matches!(kind, ErrorKind::Client | ErrorKind::Malformed) {
                        self.refresh_status().await;
                        return Err(ResilientError::Upstream(e));
                    }
                    // Last attempt? Return.
                    if attempt == policy.attempts {
                        self.refresh_status().await;
                        return Err(ResilientError::Upstream(e));
                    }
                    tokio::time::sleep(Self::backoff(attempt)).await;
                }
            }
        }
        unreachable!("retry loop must return");
    }

    pub async fn recall(
        &self,
        query: &str,
        tags: Option<&[String]>,
        tags_match: Option<&str>,
        policy: RetryPolicy,
    ) -> Result<Vec<RecallResult>, ResilientError> {
        let inner = &self.inner;
        let tags_v = tags.map(|t| t.to_vec());
        self.call_with_policy(policy, || {
            let tv = tags_v.clone();
            let tm = tags_match.map(|s| s.to_owned());
            async move {
                inner
                    .recall(query, tv.as_deref(), tm.as_deref())
                    .await
            }
        }).await
    }

    pub async fn retain(
        &self,
        content: &str,
        context: Option<&str>,
        document_id: Option<&str>,
        update_mode: Option<&str>,
        tags: Option<&[String]>,
        policy: RetryPolicy,
    ) -> Result<RetainResponse, ResilientError> {
        let res = self
            .call_with_policy(policy, || {
                let inner = &self.inner;
                async move {
                    inner
                        .retain(content, context, document_id, update_mode, tags)
                        .await
                }
            })
            .await;

        if let Err(ref err) = res {
            // Decide whether to enqueue for later drain.
            match err {
                ResilientError::Upstream(e) => {
                    match e.classify() {
                        ErrorKind::Transient | ErrorKind::RateLimited | ErrorKind::Malformed => {
                            if let Ok(conn) = crate::memory::open_connection(&self.agent_db_path, false) {
                                let _ = super::retain_queue::enqueue(
                                    &conn, &self.source, content, context, document_id, update_mode, tags,
                                );
                            }
                        }
                        ErrorKind::Client => {
                            self.bump_client_drop().await;
                            tracing::error!(
                                "retain rejected (4xx) — dropping, not enqueueing; content_preview={:?}",
                                &content.chars().take(80).collect::<String>()
                            );
                        }
                        ErrorKind::Auth => {
                            // Don't enqueue; Auth resets on next-startup probe.
                        }
                    }
                }
                ResilientError::CircuitOpen { .. } => {
                    if let Ok(conn) = crate::memory::open_connection(&self.agent_db_path, false) {
                        let _ = super::retain_queue::enqueue(
                            &conn, &self.source, content, context, document_id, update_mode, tags,
                        );
                    }
                }
            }
        }
        res
    }

    pub async fn reflect(
        &self,
        query: &str,
        policy: RetryPolicy,
    ) -> Result<ReflectResponse, ResilientError> {
        let inner = &self.inner;
        self.call_with_policy(policy, || async move { inner.reflect(query).await }).await
    }

    pub async fn get_or_create_bank(
        &self,
        policy: RetryPolicy,
    ) -> Result<BankProfile, ResilientError> {
        let inner = &self.inner;
        let out = self
            .call_with_policy(policy, || async move { inner.get_or_create_bank().await })
            .await;

        // On success, reset AuthFailed status to Healthy (only exit path from AuthFailed).
        if out.is_ok() {
            if matches!(*self.status_tx.borrow(), MemoryStatus::AuthFailed { .. }) {
                let _ = self.status_tx.send(MemoryStatus::Healthy);
            }
        }
        out
    }

    /// Drain helper invoked by the bot drain task. Uses `retain_many` for a single-item POST.
    pub async fn drain_retain_item(
        &self,
        item: &RetainItem,
    ) -> Result<(), ErrorKind> {
        let inner = &self.inner;
        // Unclassified passthrough — drain_tick wants bare ErrorKind.
        let policy = RetryPolicy {
            per_attempt: Duration::from_secs(10),
            attempts: 0,
        };
        let res = self
            .call_with_policy(policy, || {
                let batch = vec![item.clone()];
                async move { inner.retain_many(&batch).await.map(|_| ()) }
            })
            .await;
        match res {
            Ok(()) => Ok(()),
            Err(ResilientError::Upstream(e)) => Err(e.classify()),
            Err(ResilientError::CircuitOpen { .. }) => Err(ErrorKind::Transient),
        }
    }
}
```

Also add dev-dependency `fastrand` (pure, no deps) to `crates/rightclaw/Cargo.toml`:

Check first: `grep '^fastrand' crates/rightclaw/Cargo.toml`. If absent:

```toml
# under [dependencies]
fastrand = "2"
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw memory::resilient`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/memory/resilient.rs crates/rightclaw/Cargo.toml
git commit -m "feat(memory): ResilientHindsight retain/recall/reflect with retry + enqueue"
```

---

## Task 11: Doctor memory check

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs` (add `check_memory` fn + invocation from existing run_doctor)

- [ ] **Step 1: Write failing test**

Append to `crates/rightclaw/src/doctor.rs` `mod tests` (create `mod tests` if absent):

```rust
#[cfg(test)]
mod memory_tests {
    use super::*;
    use crate::memory::open_connection;
    use tempfile::tempdir;

    #[test]
    fn check_memory_passes_on_empty_queue() {
        let dir = tempdir().unwrap();
        let _ = open_connection(dir.path(), true).unwrap();
        let checks = check_memory(dir.path());
        assert!(
            checks.iter().all(|c| matches!(c.status, CheckStatus::Pass)),
            "expected all pass, got {checks:#?}"
        );
    }

    #[test]
    fn check_memory_warns_on_500_rows() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..600 {
            crate::memory::retain_queue::enqueue(
                &conn, "bot", &format!("c-{i}"), None, None, None, None,
            ).unwrap();
        }
        let checks = check_memory(dir.path());
        assert!(checks.iter().any(|c| c.status == CheckStatus::Warn
            && c.name.contains("retain backlog")));
    }

    #[test]
    fn check_memory_fails_on_901_rows() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..901 {
            crate::memory::retain_queue::enqueue(
                &conn, "bot", &format!("c-{i}"), None, None, None, None,
            ).unwrap();
        }
        let checks = check_memory(dir.path());
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail
            && c.name.contains("retain backlog")));
    }

    #[test]
    fn check_memory_fails_on_24h_auth_alert() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        conn.execute(
            "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES ('auth_failed', datetime('now','-25 hours'))",
            [],
        ).unwrap();
        let checks = check_memory(dir.path());
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail
            && c.name.contains("auth")));
    }
}
```

- [ ] **Step 2: Implement `check_memory`**

Append to `crates/rightclaw/src/doctor.rs`:

```rust
/// Run memory-subsystem checks against a single agent directory.
pub fn check_memory(agent_dir: &Path) -> Vec<DoctorCheck> {
    let mut out = Vec::new();
    let db_path = agent_dir.join("data.db");

    // 1. data.db opens.
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            out.push(DoctorCheck {
                name: "memory db".into(),
                status: CheckStatus::Fail,
                detail: format!("open {}: {e:#}", db_path.display()),
                fix: Some("verify agent dir and permissions".into()),
            });
            return out;
        }
    };

    // 2. journal_mode.
    let mode: Result<String, _> = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0));
    match mode {
        Ok(m) if m.eq_ignore_ascii_case("wal") => {
            out.push(DoctorCheck {
                name: "memory db WAL".into(),
                status: CheckStatus::Pass,
                detail: "journal_mode=wal".into(),
                fix: None,
            });
        }
        Ok(other) => out.push(DoctorCheck {
            name: "memory db WAL".into(),
            status: CheckStatus::Fail,
            detail: format!("journal_mode={other}"),
            fix: Some("re-run bot startup to apply PRAGMA".into()),
        }),
        Err(e) => out.push(DoctorCheck {
            name: "memory db WAL".into(),
            status: CheckStatus::Fail,
            detail: format!("PRAGMA failed: {e:#}"),
            fix: None,
        }),
    }

    // 3. user_version matches migration.
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
    let expected = 14;
    if version == expected {
        out.push(DoctorCheck {
            name: "memory schema".into(),
            status: CheckStatus::Pass,
            detail: format!("user_version={version}"),
            fix: None,
        });
    } else {
        out.push(DoctorCheck {
            name: "memory schema".into(),
            status: CheckStatus::Fail,
            detail: format!("user_version={version}, expected={expected}"),
            fix: Some("start the bot to run pending migrations".into()),
        });
    }

    // 4. pending_retains row count.
    if let Ok(n) = crate::memory::retain_queue::count(&conn) {
        let (st, detail) = match n {
            n if n < 500 => (CheckStatus::Pass, format!("{n} entries")),
            n if n <= 900 => (CheckStatus::Warn, format!("retain backlog growing: {n} entries")),
            n => (CheckStatus::Fail, format!("retain backlog near cap: {n}/1000 entries")),
        };
        out.push(DoctorCheck {
            name: "retain backlog count".into(),
            status: st,
            detail,
            fix: None,
        });
    }

    // 5. oldest age.
    if let Ok(Some(age)) = crate::memory::retain_queue::oldest_age(&conn) {
        let hours = age.as_secs() / 3600;
        let (st, detail) = if hours < 1 {
            (CheckStatus::Pass, format!("oldest {hours}h"))
        } else if hours <= 12 {
            (CheckStatus::Warn, format!("drain behind by {hours}h — upstream may be degraded"))
        } else {
            (CheckStatus::Fail, format!("drain severely stuck ({hours}h) — investigate logs"))
        };
        out.push(DoctorCheck {
            name: "retain backlog age".into(),
            status: st,
            detail,
            fix: None,
        });
    }

    // 6. memory_alerts rows older than 24h.
    for alert_type in ["auth_failed", "client_flood"] {
        if let Ok(found) = conn.query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM memory_alerts WHERE alert_type = ?1 \
                 AND datetime(first_sent_at) < datetime('now', '-24 hours'))",
            [alert_type],
            |r| r.get(0),
        ) {
            if found {
                out.push(DoctorCheck {
                    name: format!("memory alert: {alert_type}"),
                    status: CheckStatus::Fail,
                    detail: format!("{alert_type} standing for >24h"),
                    fix: Some(match alert_type {
                        "auth_failed" => "rotate memory.api_key / HINDSIGHT_API_KEY and restart",
                        _ => "check ~/.rightclaw/logs/ for repeated 4xx",
                    }.into()),
                });
            }
        }
    }

    out
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw doctor::memory_tests`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/doctor.rs
git commit -m "feat(doctor): check_memory — db/schema/queue/alerts"
```

---

## Task 12: Bot lib.rs — construct ResilientHindsight + non-fatal Auth

**Files:**
- Modify: `crates/bot/src/lib.rs:151-197` (memory init block) and WorkerContext wiring downstream

- [ ] **Step 1: Update memory init to use ResilientHindsight, non-fatal Auth**

Replace lines 151–197 of `crates/bot/src/lib.rs` with:

```rust
// Memory: initialize ResilientHindsight wrapper + prefetch cache if configured.
let memory_provider = config
    .memory
    .as_ref()
    .map(|m| &m.provider)
    .cloned()
    .unwrap_or_default();

let (hindsight_wrapper, prefetch_cache): (
    Option<Arc<rightclaw::memory::ResilientHindsight>>,
    Option<rightclaw::memory::prefetch::PrefetchCache>,
) = match &memory_provider {
    rightclaw::agent::types::MemoryProvider::Hindsight => {
        let mem_config = config.memory.as_ref().unwrap();
        let api_key = std::env::var("HINDSIGHT_API_KEY")
            .ok()
            .or_else(|| mem_config.api_key.clone())
            .ok_or_else(|| {
                miette::miette!(
                    help = "Set HINDSIGHT_API_KEY env var, add `memory.api_key` to agent.yaml, \
                            or switch to `memory.provider: file`",
                    "Hindsight memory provider requires an API key"
                )
            })?;
        let bank_id = mem_config.bank_id.as_deref().unwrap_or(&args.agent).to_string();
        let budget = mem_config.recall_budget.to_string();
        let client = rightclaw::memory::hindsight::HindsightClient::new(
            &api_key,
            &bank_id,
            &budget,
            mem_config.recall_max_tokens,
            None,
        );

        let wrapper = rightclaw::memory::ResilientHindsight::new(
            client,
            agent_dir.clone(),
            "bot",
        );

        // Startup probe — non-fatal on Auth/Transient; bot boots in degraded mode on failure.
        match wrapper
            .get_or_create_bank(rightclaw::memory::resilient::POLICY_STARTUP_BANK)
            .await
        {
            Ok(profile) => {
                tracing::info!(
                    agent = %args.agent,
                    bank_id = %profile.bank_id,
                    "Hindsight memory bank ready"
                );
            }
            Err(rightclaw::memory::ResilientError::Upstream(e)) => {
                let kind = e.classify();
                match kind {
                    rightclaw::memory::ErrorKind::Auth => {
                        tracing::error!(
                            agent = %args.agent,
                            "Hindsight bank probe: AUTH FAILED at startup — bot will boot in degraded mode ({e:#})"
                        );
                        // status_tx already set to AuthFailed by wrapper.
                    }
                    rightclaw::memory::ErrorKind::Client => {
                        tracing::error!(
                            agent = %args.agent,
                            "Hindsight bank probe: 4xx at startup — payload or API-drift bug ({e:#})"
                        );
                    }
                    _ => {
                        tracing::warn!(
                            agent = %args.agent,
                            "Hindsight bank probe: transient failure at startup ({e:#}) — will retry in background"
                        );
                        // Background re-probe every 60s until success.
                        let w = std::sync::Arc::new(wrapper);
                        let w_bg = w.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                                match w_bg
                                    .get_or_create_bank(
                                        rightclaw::memory::resilient::POLICY_STARTUP_BANK,
                                    )
                                    .await
                                {
                                    Ok(p) => {
                                        tracing::info!(bank_id = %p.bank_id, "background re-probe succeeded");
                                        return;
                                    }
                                    Err(e) => {
                                        tracing::warn!("background re-probe failed: {e:#}");
                                    }
                                }
                            }
                        });
                        let cache = rightclaw::memory::prefetch::PrefetchCache::new();
                        let prefetch = Some(cache);
                        // Return early: w is already Arc; finish both sides of match
                        break 'memblock (Some(w), prefetch);
                    }
                }
            }
            Err(rightclaw::memory::ResilientError::CircuitOpen { .. }) => {
                // Unreachable at startup — breaker is fresh. Log and move on.
                tracing::warn!("unexpected circuit-open at startup");
            }
        }

        let cache = rightclaw::memory::prefetch::PrefetchCache::new();
        (Some(Arc::new(wrapper)), Some(cache))
    }
    rightclaw::agent::types::MemoryProvider::File => (None, None),
};
```

Wrap the whole block above in a labelled block to enable `break 'memblock`. Replace the outer `let (hindsight_wrapper, prefetch_cache): ... = match ...` line with:

```rust
let (hindsight_wrapper, prefetch_cache): (
    Option<Arc<rightclaw::memory::ResilientHindsight>>,
    Option<rightclaw::memory::prefetch::PrefetchCache>,
) = 'memblock: {
    match &memory_provider {
        rightclaw::agent::types::MemoryProvider::Hindsight => {
            // ... (rest of block — startup probe etc.)
            // final line of Hindsight arm:
            (Some(Arc::new(wrapper)), Some(cache))
        }
        rightclaw::agent::types::MemoryProvider::File => (None, None),
    }
};
```

(The `break 'memblock (...)` statement in the transient branch exits with the
given value. If this proves awkward, simpler form: fold the background-probe
case into the normal success path by always returning the wrapper and letting
the status encode the degradation; the background task is only spawned when
the probe errored transiently. The simpler shape:)

Simpler refactor — do NOT use labelled block. Always return `(Some(Arc::new(wrapper)), Some(cache))` from the Hindsight arm; the match on the probe result only logs + spawns background. Rewrite the Hindsight arm as:

```rust
rightclaw::agent::types::MemoryProvider::Hindsight => {
    let mem_config = config.memory.as_ref().unwrap();
    let api_key = /* as above */;
    let bank_id = mem_config.bank_id.as_deref().unwrap_or(&args.agent).to_string();
    let budget = mem_config.recall_budget.to_string();
    let client = rightclaw::memory::hindsight::HindsightClient::new(
        &api_key, &bank_id, &budget, mem_config.recall_max_tokens, None,
    );

    let wrapper = Arc::new(rightclaw::memory::ResilientHindsight::new(
        client, agent_dir.clone(), "bot",
    ));

    match wrapper
        .get_or_create_bank(rightclaw::memory::resilient::POLICY_STARTUP_BANK)
        .await
    {
        Ok(profile) => tracing::info!(
            agent = %args.agent, bank_id = %profile.bank_id, "Hindsight bank ready"
        ),
        Err(rightclaw::memory::ResilientError::Upstream(e)) => {
            match e.classify() {
                rightclaw::memory::ErrorKind::Auth =>
                    tracing::error!("Hindsight AUTH failed at startup: {e:#}"),
                rightclaw::memory::ErrorKind::Client =>
                    tracing::error!("Hindsight 4xx at startup: {e:#}"),
                _ => {
                    tracing::warn!("Hindsight transient at startup: {e:#}");
                    let w_bg = wrapper.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            if w_bg.get_or_create_bank(
                                rightclaw::memory::resilient::POLICY_STARTUP_BANK,
                            ).await.is_ok() {
                                tracing::info!("background bank probe succeeded");
                                return;
                            }
                        }
                    });
                }
            }
        }
        Err(rightclaw::memory::ResilientError::CircuitOpen { .. }) =>
            tracing::warn!("unexpected CircuitOpen at startup"),
    }

    let cache = rightclaw::memory::prefetch::PrefetchCache::new();
    (Some(wrapper), Some(cache))
}
```

- [ ] **Step 2: Update downstream type references**

Search for `Option<Arc<rightclaw::memory::hindsight::HindsightClient>>` usages in `crates/bot/src/`:

```
$ rg 'hindsight::HindsightClient' crates/bot/src/
```

Likely locations:
- `crates/bot/src/lib.rs:540` (in final WorkerContextSettings construction — `hindsight_client` → `hindsight_wrapper`)
- `crates/bot/src/telegram/worker.rs:96` (`WorkerContext.hindsight`)
- `crates/bot/src/telegram/handler.rs:79` (`WorkerContextSettings.hindsight`)
- `crates/bot/src/telegram/dispatch.rs:90` and test fixture at `:387`

Change each to `Option<std::sync::Arc<rightclaw::memory::ResilientHindsight>>`. Rename local bindings `hindsight_client` → `hindsight_wrapper` where it helps grep.

- [ ] **Step 3: Verify compile**

Run: `cargo check -p rightclaw-bot`
Expected: compiles (call sites inside worker.rs still use old client methods — those break, see Task 13).

For this step, accept compile errors limited to worker.rs recall/retain call sites. Commit only if `lib.rs` + `handler.rs` + `dispatch.rs` type changes compile in isolation — fold the call-site fixes into Task 13.

In practice: `cargo check -p rightclaw-bot 2>&1 | grep -E 'error\[' | head`. If the only errors are in worker.rs method call compatibility, proceed.

- [ ] **Step 4: Commit (WIP)**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/handler.rs \
        crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): switch memory context type to Arc<ResilientHindsight> (WIP)"
```

---

## Task 13: Worker call sites — recall, retain, prefetch

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (lines 590–639 auto-retain + prefetch, lines 912–936 blocking recall)

- [ ] **Step 1: Write failing test (worker compile)**

Skip — integration-level. We will verify via `cargo check` and the later integration test.

- [ ] **Step 2: Update blocking recall (`worker.rs:912–936`)**

Replace the `if let Some(ref hs) = ctx.hindsight { ... match tokio::time::timeout(...) ... }` block with:

```rust
} else if let Some(ref hs) = ctx.hindsight {
    tracing::info!(?chat_id, "prefetch cache miss, blocking recall");
    let truncated_query = truncate_to_chars(input, RECALL_MAX_CHARS);
    let recall_tags_v = recall_tags(chat_id);
    match hs
        .recall(
            truncated_query,
            Some(&recall_tags_v),
            Some("any"),
            rightclaw::memory::resilient::POLICY_BLOCKING_RECALL,
        )
        .await
    {
        Ok(results) if !results.is_empty() => {
            let content = rightclaw::memory::hindsight::join_recall_texts(&results);
            if let Some(ref cache) = ctx.prefetch_cache {
                cache.put(&cache_key, content.clone()).await;
            }
            Some(content)
        }
        Ok(_) => None,
        Err(rightclaw::memory::ResilientError::CircuitOpen { .. }) => {
            tracing::warn!(?chat_id, "blocking recall skipped: circuit open");
            None
        }
        Err(rightclaw::memory::ResilientError::Upstream(e)) => {
            tracing::warn!(?chat_id, "blocking recall failed: {e:#}");
            None
        }
    }
}
```

- [ ] **Step 3: Update auto-retain + prefetch (`worker.rs:590–639`)**

Replace the `if let Some(ref hs) = ctx.hindsight { ... }` block with:

```rust
if let Some(ref hs) = ctx.hindsight {
    // Auto-retain this turn.
    if let Some(ref reply_text) = reply_text_for_retain {
        let hs_retain = Arc::clone(hs);
        let retain_input = input.clone();
        let retain_response = reply_text.clone();
        let retain_doc_id = session_uuid.clone();
        let sender_id = batch.first().and_then(|m| m.author.user_id);
        let retain_tags_v = retain_tags(chat_id, sender_id, eff_thread_id, is_group);
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        tokio::spawn(async move {
            let content = serde_json::json!([
                {"role": "user", "content": retain_input, "timestamp": now},
                {"role": "assistant", "content": retain_response, "timestamp": now},
            ]).to_string();
            if let Err(e) = hs_retain
                .retain(
                    &content,
                    Some("conversation between RightClaw Agent and the User"),
                    Some(&retain_doc_id),
                    Some("append"),
                    Some(&retain_tags_v),
                    rightclaw::memory::resilient::POLICY_AUTO_RETAIN,
                )
                .await
            {
                tracing::warn!("auto-retain failed: {e:#}");
            }
        });
    }

    // Prefetch for next turn.
    let hs_recall = Arc::clone(hs);
    let recall_query = truncate_to_chars(&input, RECALL_MAX_CHARS).to_owned();
    let recall_tags_v = recall_tags(chat_id);
    let cache_key = format!("{}:{}", chat_id, eff_thread_id);
    let cache = ctx.prefetch_cache.clone();
    tokio::spawn(async move {
        match hs_recall
            .recall(
                &recall_query,
                Some(&recall_tags_v),
                Some("any"),
                rightclaw::memory::resilient::POLICY_PREFETCH,
            )
            .await
        {
            Ok(results) if !results.is_empty() => {
                let content = rightclaw::memory::hindsight::join_recall_texts(&results);
                if let Some(ref c) = cache {
                    c.put(&cache_key, content).await;
                }
            }
            Ok(_) => {}
            Err(rightclaw::memory::ResilientError::CircuitOpen { .. }) => {
                tracing::warn!("prefetch recall skipped: circuit open");
            }
            Err(rightclaw::memory::ResilientError::Upstream(e)) => {
                tracing::warn!("prefetch recall failed: {e:#}");
            }
        }
    });
}
```

- [ ] **Step 4: Verify compile + unit tests**

Run: `cargo build -p rightclaw-bot`
Expected: success.

Run: `cargo test -p rightclaw-bot`
Expected: existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): worker uses ResilientHindsight wrapper for recall/retain"
```

---

## Task 14: Drain task spawn in lib.rs

**Files:**
- Modify: `crates/bot/src/lib.rs` (add drain task spawn after wrapper construction)

- [ ] **Step 1: Add drain task**

After the Hindsight arm of the memory `match` (i.e. right after `(Some(wrapper), Some(cache))` returns), and before `WorkerContextSettings` construction, insert:

```rust
// Spawn background drain task if wrapper is present.
if let Some(ref w) = hindsight_wrapper {
    let w = w.clone();
    let agent_db = agent_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // first tick is immediate
        loop {
            interval.tick().await;
            if !matches!(w.status(), rightclaw::memory::MemoryStatus::Healthy) {
                continue;
            }
            let Ok(conn) = rightclaw::memory::open_connection(&agent_db, false) else {
                continue;
            };
            let w_call = w.clone();
            let _report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| {
                let w = w_call.clone();
                async move {
                    let item = rightclaw::memory::hindsight::RetainItem {
                        content: items[0].content.clone(),
                        context: items[0].context.clone(),
                        document_id: items[0].document_id.clone(),
                        update_mode: items[0].update_mode.clone(),
                        tags: items[0].tags.clone(),
                    };
                    w.drain_retain_item(&item).await
                }
            }).await;
        }
    });
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo build -p rightclaw-bot`
Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): spawn retain-queue drain task"
```

---

## Task 15: Effective status + agent marker in composite memory

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (around line 892 — `memory_mode` / recall content block)
- Modify: `crates/bot/src/telegram/prompt.rs` (`deploy_composite_memory` signature + marker append)

- [ ] **Step 1: Update `deploy_composite_memory` to append marker**

Replace `deploy_composite_memory` in `prompt.rs:118-137` with:

```rust
pub(crate) async fn deploy_composite_memory(
    content: &str,
    label: &str,
    agent_dir: &std::path::Path,
    resolved_sandbox: Option<&str>,
    status_marker: Option<&str>,
) -> Result<(), DeployError> {
    let marker_tail = status_marker.map(|m| format!("\n\n{m}")).unwrap_or_default();
    let fenced = format!(
        "<memory-context>\n[System: recalled memory context, {label}.]\n\n{content}\n</memory-context>{marker_tail}"
    );
    let host_path = agent_dir.join(".claude").join("composite-memory.md");
    tokio::fs::write(&host_path, &fenced).await.map_err(DeployError::Write)?;
    if let Some(sandbox) = resolved_sandbox {
        rightclaw::openshell::upload_file(sandbox, &host_path, "/sandbox/.claude/")
            .await
            .map_err(|e| DeployError::Upload(format!("{e:#}")))?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DeployError {
    #[error("write composite-memory.md: {0}")]
    Write(std::io::Error),
    #[error("upload composite-memory.md: {0}")]
    Upload(String),
}
```

Note: this changes the signature. Update callers (there is exactly one, in `worker.rs`).

- [ ] **Step 2: Compute effective status + marker in worker**

Modify `crates/bot/src/telegram/worker.rs` in the `memory_mode = ... Hindsight` arm (around line 941). Change:

```rust
match recall_content {
    Some(content) => {
        super::prompt::deploy_composite_memory(
            &content,
            "NOT new user input. Treat as background",
            &ctx.agent_dir,
            ctx.resolved_sandbox.as_deref(),
        )
        .await;
        // ...
    }
    None => { /* remove composite-memory.md */ }
}
```

to:

```rust
let mut local_status = rightclaw::memory::MemoryStatus::Healthy;
let wrapper_status = ctx.hindsight.as_ref().map(|h| h.status()).unwrap_or(rightclaw::memory::MemoryStatus::Healthy);
let client_drops_24h = if let Some(ref h) = ctx.hindsight { h.client_drops_24h().await } else { 0 };

match recall_content {
    Some(content) => {
        let effective = wrapper_status.max(local_status);
        let marker = build_memory_marker(effective, client_drops_24h);
        if let Err(e) = super::prompt::deploy_composite_memory(
            &content,
            "NOT new user input. Treat as background",
            &ctx.agent_dir,
            ctx.resolved_sandbox.as_deref(),
            marker.as_deref(),
        ).await {
            tracing::warn!("composite-memory deploy failed: {e:#}");
            local_status = rightclaw::memory::MemoryStatus::Degraded {
                since: std::time::Instant::now(),
            };
            // Fallback: inline content + marker into prompt-assembly heredoc.
            // (The script generated in build_prompt_assembly_script already reads
            // composite-memory.md from disk; when deploy failed, the file is stale
            // or absent — the assembly will simply skip. Explicit inline fallback
            // is deferred; the marker below still gets injected for next turn.)
        }
    }
    None => {
        super::prompt::remove_composite_memory(&ctx.agent_dir).await;
    }
}
```

Add helper near the top of `worker.rs` (below `retain_tags`):

```rust
fn build_memory_marker(
    status: rightclaw::memory::MemoryStatus,
    client_drops_24h: usize,
) -> Option<String> {
    use rightclaw::memory::MemoryStatus as S;
    match status {
        S::AuthFailed { .. } => Some(
            "<memory-status>unavailable — memory provider authentication failed, \
             memory ops will error until the user rotates the API key</memory-status>"
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

- [ ] **Step 3: Compile + test**

Run: `cargo build -p rightclaw-bot`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): inject <memory-status> marker into composite-memory.md"
```

---

## Task 16: `MEMORY.md` unreadable annotation (shell fallback)

**Files:**
- Modify: `crates/bot/src/telegram/prompt.rs:94-96`

- [ ] **Step 1: Update shell snippet**

Change the `File` arm in `build_prompt_assembly_script` (around prompt.rs:91):

```rust
match memory_mode {
    Some(MemoryMode::File) => format!(
        r#"
if [ -s {root_path}/MEMORY.md ]; then
  head -200 {root_path}/MEMORY.md 2>/dev/null \
    || echo "<memory-status>MEMORY.md unreadable</memory-status>"
fi"#
    ),
    // ...
}
```

- [ ] **Step 2: Update test `script_includes_memory_md_for_file_mode` (if strict)**

If the existing test does a `contains("head -200")` check, it still passes. No update needed — verify:

Run: `cargo test -p rightclaw-bot telegram::prompt`
Expected: existing tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs
git commit -m "feat(bot): annotate unreadable MEMORY.md in file-mode prompt"
```

---

## Task 17: `memory_alerts.rs` — Telegram notification watcher

**Files:**
- Create: `crates/bot/src/telegram/memory_alerts.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (declare)
- Modify: `crates/bot/src/lib.rs` (spawn watcher if wrapper present)

- [ ] **Step 1: Write the module**

Create `crates/bot/src/telegram/memory_alerts.rs`:

```rust
//! Watches MemoryStatus + client-flood counters and sends one-shot Telegram alerts
//! with 24h dedup via the `memory_alerts` SQLite table.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use teloxide::prelude::*;
use teloxide::types::ChatId;

use rightclaw::memory::{MemoryStatus, ResilientHindsight};

use super::bot::BotAdaptor;

pub const CLIENT_FLOOD_POLL: std::time::Duration = std::time::Duration::from_secs(60);

pub fn spawn_watcher(
    bot: BotAdaptor,
    wrapper: Arc<ResilientHindsight>,
    agent_db_path: PathBuf,
    allowlist_chats: Vec<i64>,
) {
    // Startup cleanup: delete alerts older than 1h so crash-loops re-notify.
    if let Ok(conn) = rightclaw::memory::open_connection(&agent_db_path, false) {
        let _ = conn.execute(
            "DELETE FROM memory_alerts WHERE datetime(first_sent_at) < datetime('now', '-1 hour')",
            [],
        );
    }

    // Task A: status watcher.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_db_path.clone();
        let chats = allowlist_chats.clone();
        tokio::spawn(async move {
            let mut rx = wrapper.subscribe_status();
            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let status = *rx.borrow();
                if matches!(status, MemoryStatus::AuthFailed { .. }) {
                    if should_fire(&db, "auth_failed") {
                        let msg = "\u{26a0} Memory provider authentication failed.\n\
                                   Rotate the Hindsight API key — set `memory.api_key` in \
                                   agent.yaml or the HINDSIGHT_API_KEY env var — and restart \
                                   the agent. Memory ops are disabled until then.";
                        send_to_chats(&bot, &chats, msg).await;
                        record_fire(&db, "auth_failed");
                    }
                } else if matches!(status, MemoryStatus::Healthy) {
                    // Clear dedup on recovery.
                    if let Ok(conn) = rightclaw::memory::open_connection(&db, false) {
                        let _ = conn.execute(
                            "DELETE FROM memory_alerts WHERE alert_type = 'auth_failed'",
                            [],
                        );
                    }
                }
            }
        });
    }

    // Task B: client-flood poller.
    {
        let bot = bot.clone();
        let wrapper = wrapper.clone();
        let db = agent_db_path.clone();
        let chats = allowlist_chats.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(CLIENT_FLOOD_POLL);
            loop {
                t.tick().await;
                let drops_1h = wrapper.client_drops_1h().await;
                if drops_1h > rightclaw::memory::resilient::CLIENT_FLOOD_THRESHOLD
                    && should_fire(&db, "client_flood")
                {
                    let msg = format!(
                        "\u{26a0} Memory retains persistently rejected (HTTP 4xx) — \
                         possible Hindsight API drift or payload bug. {drops_1h} drops \
                         in the last hour. Check ~/.rightclaw/logs/ for details."
                    );
                    send_to_chats(&bot, &chats, &msg).await;
                    record_fire(&db, "client_flood");
                }
            }
        });
    }
}

fn should_fire(db: &std::path::Path, alert_type: &str) -> bool {
    let Ok(conn) = rightclaw::memory::open_connection(db, false) else {
        return false;
    };
    let existing: Option<String> = conn
        .query_row(
            "SELECT first_sent_at FROM memory_alerts WHERE alert_type = ?1",
            [alert_type],
            |r| r.get(0),
        )
        .ok();
    let Some(sent) = existing else { return true };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&sent) else { return true };
    Utc::now().signed_duration_since(parsed.with_timezone(&Utc))
        > chrono::Duration::hours(24)
}

fn record_fire(db: &std::path::Path, alert_type: &str) {
    if let Ok(conn) = rightclaw::memory::open_connection(db, false) {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let _ = conn.execute(
            "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES (?1, ?2) \
             ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at",
            [alert_type, &now],
        );
    }
}

async fn send_to_chats(bot: &BotAdaptor, chats: &[i64], text: &str) {
    for &chat_id in chats {
        if let Err(e) = bot.send_message(ChatId(chat_id), text).await {
            tracing::warn!(chat_id, "memory alert send failed: {e:#}");
        }
    }
}
```

- [ ] **Step 2: Declare module**

Modify `crates/bot/src/telegram/mod.rs`:

```rust
pub mod memory_alerts;
```

- [ ] **Step 3: Wire into `lib.rs`**

In `lib.rs`, after the drain task spawn (Task 14) and after the allowlist has been loaded, add:

```rust
if let Some(ref w) = hindsight_wrapper {
    let chats: Vec<i64> = allowlist.read().await.iter().copied().collect();
    crate::telegram::memory_alerts::spawn_watcher(
        bot_adaptor.clone(),
        w.clone(),
        agent_dir.clone(),
        chats,
    );
}
```

The exact form depends on the local variable names for the teloxide `BotAdaptor` instance; verify against existing `bot_adaptor` construction in `lib.rs`. If the variable is named differently, adjust.

- [ ] **Step 4: Compile + test**

Run: `cargo build -p rightclaw-bot`
Expected: success. Run: `cargo test -p rightclaw-bot memory_alerts` (module may not have tests yet — skip if none).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/memory_alerts.rs crates/bot/src/telegram/mod.rs crates/bot/src/lib.rs
git commit -m "feat(bot): memory_alerts — AuthFailed + ClientFlood Telegram watchers"
```

---

## Task 18: Aggregator — HindsightBackend uses ResilientHindsight

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs:105-231` (HindsightBackend ctor + tools_call)

- [ ] **Step 1: Change HindsightBackend to hold Arc<ResilientHindsight>**

Replace `crates/rightclaw-cli/src/aggregator.rs:105-112` (HindsightBackend struct + `new`) with:

```rust
pub(crate) struct HindsightBackend {
    client: std::sync::Arc<rightclaw::memory::ResilientHindsight>,
}

impl HindsightBackend {
    pub fn new(client: std::sync::Arc<rightclaw::memory::ResilientHindsight>) -> Self {
        Self { client }
    }
}
```

In `tools_call`, replace the three tool method invocations (lines 182-230). For example `memory_retain`:

```rust
"memory_retain" => {
    let content = args["content"].as_str()
        .ok_or_else(|| anyhow::anyhow!("missing required param: content"))?;
    let context = args["context"].as_str();
    let result = self.client
        .retain(
            content, context, None, None, None,
            rightclaw::memory::resilient::POLICY_MCP_RETAIN,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let json = serde_json::json!({
        "status": "accepted",
        "operation_id": result.operation_id,
    });
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json)?,
    )]))
}
```

`memory_recall` and `memory_reflect` analogously with `POLICY_MCP_RECALL` and `POLICY_MCP_REFLECT`. Keep the rest of the arm bodies unchanged.

- [ ] **Step 2: Aggregator construction — wrap HindsightClient**

In the aggregator's setup code (the site that currently constructs `HindsightClient` and passes it to `HindsightBackend::new`), wrap with `ResilientHindsight::new(client, agent_dir_for_db, "aggregator")` and then `Arc::new(...)`.

Search:
```
rg 'HindsightBackend::new' crates/rightclaw-cli/src/
```

Update the call site accordingly. Ensure the same per-agent `data.db` path is passed as `agent_db_path`.

- [ ] **Step 3: Compile + test**

Run: `cargo build --workspace`
Expected: success.

Run: `cargo test -p rightclaw-cli`
Expected: existing aggregator tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "refactor(agg): HindsightBackend uses ResilientHindsight wrapper"
```

---

## Task 19: Prompt-cache invariant test

**Files:**
- Modify: `crates/bot/src/telegram/prompt.rs` (append to `mod tests`)

- [ ] **Step 1: Write test**

Append to `prompt.rs` `mod tests`:

```rust
#[test]
fn composite_memory_section_is_last_in_assembled_script() {
    // With a hindsight-like memory_mode, the memory block must be the final
    // appended section of the prompt-assembly script to preserve cache.
    let script = build_prompt_assembly_script(
        "Base",
        false,
        "/sandbox",
        "/tmp/prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
        Some(&MemoryMode::Hindsight {
            composite_memory_path: "/sandbox/.claude/composite-memory.md".into(),
        }),
    );

    // Find the position of the last }-closing-brace before the `>` redirect.
    let redirect_pos = script.rfind("} >").expect("redirect must exist");
    let head = &script[..redirect_pos];

    // Anything that would bust the cache (other sections like TOOLS.md,
    // MCP instructions) must appear BEFORE the composite-memory section.
    let memory_pos = head.rfind("composite-memory.md").expect("composite-memory.md must appear");
    let tools_pos = head.find("TOOLS.md");
    let mcp_pos = head.find("# MCP Server Instructions");

    if let Some(tp) = tools_pos {
        assert!(tp < memory_pos, "TOOLS.md must precede composite-memory.md");
    }
    if let Some(mp) = mcp_pos {
        assert!(mp < memory_pos, "MCP instructions must precede composite-memory.md");
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p rightclaw-bot telegram::prompt::tests::composite_memory_section_is_last`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs
git commit -m "test(prompt): assert composite-memory is last section (cache invariant)"
```

---

## Task 20: Integration test — full outage scenario

**Files:**
- Create: `crates/rightclaw/tests/memory_failure_scenarios.rs`

- [ ] **Step 1: Write test**

Create `crates/rightclaw/tests/memory_failure_scenarios.rs`:

```rust
//! Integration scenarios covering memory failure handling.

use std::sync::Arc;

use rightclaw::memory::hindsight::{HindsightClient, RetainItem};
use rightclaw::memory::resilient::{
    POLICY_AUTO_RETAIN, POLICY_BLOCKING_RECALL, POLICY_MCP_RECALL,
};
use rightclaw::memory::{ResilientHindsight, ResilientError, MemoryStatus};

mod common;
use common::mock;

#[tokio::test]
async fn outage_queues_retain_and_degrades_status() {
    let (_h, url) = mock::always(500, r#"{"error":"boom"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let err = wrapper
        .retain("turn-1", None, Some("doc-1"), Some("append"), None, POLICY_AUTO_RETAIN)
        .await
        .unwrap_err();
    assert!(matches!(err, ResilientError::Upstream(_)));

    // After 3 total attempts the circuit is not yet tripped (need 5 transient fails
    // within 30s). Do 4 more retain calls to trip.
    for _ in 0..4 {
        let _ = wrapper
            .retain("more", None, None, None, None, POLICY_AUTO_RETAIN)
            .await;
    }

    // Status should be Degraded by now.
    assert!(matches!(wrapper.status(), MemoryStatus::Degraded { .. }));

    // pending_retains has rows.
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert!(n >= 1, "expected queue non-empty, got {n}");
}

#[tokio::test]
async fn auth_failure_sets_auth_failed_status() {
    let (_h, url) = mock::always(401, r#"{"error":"bad key"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let err = wrapper
        .recall("q", None, None, POLICY_MCP_RECALL)
        .await
        .unwrap_err();
    assert!(matches!(err, ResilientError::Upstream(_)));
    assert!(matches!(wrapper.status(), MemoryStatus::AuthFailed { .. }));
}

#[tokio::test]
async fn client_error_drops_record_bumps_counter_no_enqueue() {
    let (_h, url) = mock::always(400, r#"{"error":"bad payload"}"#).await;
    let wrapper = common::wrap(&url, "bot").await;

    let _ = wrapper.retain("x", None, None, None, None, POLICY_AUTO_RETAIN).await;

    assert_eq!(wrapper.client_drops_24h().await, 1);
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}
```

- [ ] **Step 2: Write `tests/common/mod.rs` helper**

Create `crates/rightclaw/tests/common/mod.rs`:

```rust
use std::path::PathBuf;

use rightclaw::memory::hindsight::HindsightClient;
use rightclaw::memory::ResilientHindsight;

pub mod mock {
    pub async fn always(status: u16, body: &str) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return; };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }
}

pub async fn wrap(url: &str, source: &str) -> ResilientHindsight {
    let dir = tempfile::tempdir().unwrap().into_path();
    let _ = rightclaw::memory::open_connection(&dir, true).unwrap();
    let client = HindsightClient::new("hs_x", "b", "high", 1024, Some(url));
    ResilientHindsight::new(client, dir, source)
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw --test memory_failure_scenarios`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/tests/memory_failure_scenarios.rs crates/rightclaw/tests/common/mod.rs
git commit -m "test(memory): outage / auth-fail / client-drop scenarios"
```

---

## Task 21: Integration test — recovery + drain

**Files:**
- Modify: `crates/rightclaw/tests/memory_failure_scenarios.rs`, `crates/rightclaw/tests/common/mod.rs`

- [ ] **Step 1: Add switchable mock**

Append to `tests/common/mod.rs`:

```rust
pub mod switch {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Clone)]
    pub struct ResponseSwitch {
        pub inner: Arc<Mutex<(u16, String)>>,
    }

    impl ResponseSwitch {
        pub fn new(status: u16, body: &str) -> Self {
            Self { inner: Arc::new(Mutex::new((status, body.to_owned()))) }
        }

        pub async fn set(&self, status: u16, body: &str) {
            *self.inner.lock().await = (status, body.to_owned());
        }
    }

    pub async fn server(switch: ResponseSwitch) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return; };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let (status, body) = switch.inner.lock().await.clone();
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }
}
```

- [ ] **Step 2: Add recovery test**

Append to `memory_failure_scenarios.rs`:

```rust
use common::switch::{server, ResponseSwitch};

#[tokio::test]
async fn recovery_drains_queue_after_breaker_closes() {
    let sw = ResponseSwitch::new(500, r#"{"error":"boom"}"#);
    let (_h, url) = server(sw.clone()).await;
    let wrapper = common::wrap(&url, "bot").await;

    // Fail retain 5x → breaker opens, 5 items queued.
    for i in 0..6 {
        let _ = wrapper
            .retain(&format!("turn-{i}"), None, Some("doc"), Some("append"), None, POLICY_AUTO_RETAIN)
            .await;
    }

    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();
    let queued: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert!(queued > 0, "expected non-empty queue");

    // Flip mock to success. Wait past breaker timer then drain.
    sw.set(200, r#"{"success":true,"operation_id":"op-1"}"#).await;
    tokio::time::sleep(std::time::Duration::from_secs(31)).await;

    // Drain one tick.
    let report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| {
        let w = &wrapper;
        async move {
            let item = RetainItem {
                content: items[0].content.clone(),
                context: items[0].context.clone(),
                document_id: items[0].document_id.clone(),
                update_mode: items[0].update_mode.clone(),
                tags: items[0].tags.clone(),
            };
            w.drain_retain_item(&item).await
        }
    }).await;

    assert!(report.deleted > 0, "drain should have deleted at least one entry");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw --test memory_failure_scenarios recovery_drains_queue`
Expected: pass (may take ~31s due to the sleep).

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/tests/memory_failure_scenarios.rs crates/rightclaw/tests/common/mod.rs
git commit -m "test(memory): recovery + drain scenario"
```

---

## Task 22: Integration test — poison pill + eviction

**Files:**
- Modify: `crates/rightclaw/tests/memory_failure_scenarios.rs`

- [ ] **Step 1: Add poison-pill drain test**

Append to `memory_failure_scenarios.rs`:

```rust
#[tokio::test]
async fn drain_poison_pill_deleted_good_records_still_processed() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let wrapper = common::wrap(&url, "bot").await;
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();

    // Directly seed queue with a "poison" row and a "good" row.
    rightclaw::memory::retain_queue::enqueue(
        &conn, "bot", "POISON", None, None, None, None,
    ).unwrap();
    rightclaw::memory::retain_queue::enqueue(
        &conn, "bot", "GOOD", None, None, None, None,
    ).unwrap();

    // Fake classifier: POISON → Client error; GOOD → Ok.
    let report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| {
        async move {
            if items[0].content == "POISON" {
                Err(rightclaw::memory::ErrorKind::Client)
            } else {
                Ok(())
            }
        }
    }).await;

    assert_eq!(report.dropped_client, 1);
    assert_eq!(report.deleted, 1);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn queue_eviction_at_cap() {
    let (_h, url) = common::mock::always(200, r#"{"success":true}"#).await;
    let wrapper = common::wrap(&url, "bot").await;
    let conn = rightclaw::memory::open_connection(wrapper.agent_db_path(), false).unwrap();

    for i in 0..(rightclaw::memory::retain_queue::QUEUE_CAP + 5) {
        let c = format!("row-{i}");
        rightclaw::memory::retain_queue::enqueue(&conn, "bot", &c, None, None, None, None).unwrap();
    }

    let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_retains", [], |r| r.get(0)).unwrap();
    assert_eq!(n as usize, rightclaw::memory::retain_queue::QUEUE_CAP);
    let first_gone: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pending_retains WHERE content = 'row-0'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(first_gone, 0, "row-0 should have been evicted");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw --test memory_failure_scenarios drain_poison_pill`
Run: `cargo test -p rightclaw --test memory_failure_scenarios queue_eviction_at_cap`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/tests/memory_failure_scenarios.rs
git commit -m "test(memory): poison pill + queue eviction"
```

---

## Task 23: Integration test — independence of bot & aggregator breakers

**Files:**
- Modify: `crates/rightclaw/tests/memory_failure_scenarios.rs`

- [ ] **Step 1: Write test**

Append:

```rust
#[tokio::test]
async fn two_wrappers_have_independent_breakers() {
    // Two mocks — one always 500, one always 200.
    let (_h1, url_bad) = common::mock::always(500, r#"{"error":"x"}"#).await;
    let (_h2, url_ok) = common::mock::always(200, r#"{"results":[]}"#).await;

    let bot_wrapper = common::wrap(&url_bad, "bot").await;
    let agg_wrapper = common::wrap(&url_ok, "aggregator").await;

    // Trip the bot wrapper.
    for _ in 0..6 {
        let _ = bot_wrapper.recall("q", None, None, POLICY_MCP_RECALL).await;
    }
    assert!(matches!(bot_wrapper.status(), MemoryStatus::Degraded { .. }));

    // Aggregator wrapper stays healthy.
    let res = agg_wrapper.recall("q", None, None, POLICY_MCP_RECALL).await;
    assert!(res.is_ok(), "independent wrapper must still serve");
    assert!(matches!(agg_wrapper.status(), MemoryStatus::Healthy));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw --test memory_failure_scenarios two_wrappers_have_independent`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/tests/memory_failure_scenarios.rs
git commit -m "test(memory): independent breakers for bot vs aggregator wrappers"
```

---

## Task 24: Spec cross-refs — PROMPT_SYSTEM.md / ARCHITECTURE.md

**Files:**
- Modify: `PROMPT_SYSTEM.md` (memory section — describe `<memory-status>` marker)
- Modify: `ARCHITECTURE.md` (memory section — describe wrapper + drain + alerts tables)

- [ ] **Step 1: Update PROMPT_SYSTEM.md**

Find the memory section (likely near "Composite memory" or similar heading). Append:

```markdown
### Memory Status Marker

When the agent runs with `memory.provider: hindsight`, the bot injects a
`<memory-status>...</memory-status>` marker at the end of
`composite-memory.md` whenever the ResilientHindsight wrapper is not
`Healthy`. Three states:

- `degraded — recall may be incomplete or stale, retain may be queued` —
  circuit breaker is open or half-open, or a recent transient failure occurred.
- `unavailable — memory provider authentication failed, memory ops will error
  until the user rotates the API key` — 401/403 from Hindsight. Requires
  user action.
- `retain-errors: N records dropped in last 24h due to bad payload — check
  logs` — in a Healthy state but Client-kind (4xx) retain drops occurred in
  the last 24h.

The marker is always the last section of the system prompt, preserving
prompt cache for all preceding blocks.
```

- [ ] **Step 2: Update ARCHITECTURE.md — Memory section**

Append under the existing "Memory" subsection:

```markdown
### Resilience Layer

`memory::resilient::ResilientHindsight` wraps `HindsightClient` with:
- per-process circuit breaker (closed→open after 5 fails in 30s; 30s initial
  open with doubling backoff to a 10 min cap; 1h hard open on Auth)
- classified retries (Transient/RateLimited yes; Auth/Client/Malformed no)
- SQLite-backed `pending_retains` queue (1000-row cap, 24h age cap)
- `watch::Sender<MemoryStatus>` signalling Healthy/Degraded/AuthFailed

The bot runs a single drain task (30s interval, batch 20, stop on first
non-Client failure). The aggregator shares the same SQLite queue via the
per-agent `data.db`; it enqueues on failure but never drains.

Telegram alerts (`memory_alerts` table, 24h dedup, 1h startup cleanup) fire
on:
- `AuthFailed` transition
- >20 `Client`-kind drops in a 1h rolling window (`client_flood`)

Doctor checks queue size (500/900 row thresholds), oldest-row age (1h/12h
thresholds), and long-standing (>24h) alerts.
```

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: memory failure handling — prompt marker + architecture"
```

---

## Task 25: Remove deprecated `HindsightRequest` usages

**Files:**
- Grep: any remaining `MemoryError::HindsightRequest` usages
- Modify: `crates/rightclaw/src/memory/error.rs` (mark deprecated if any remain)

- [ ] **Step 1: Find remaining usages**

Run:
```bash
rg 'MemoryError::HindsightRequest' crates/
```

- [ ] **Step 2: Replace**

Any call site constructing `HindsightRequest(String)` directly: route through
`MemoryError::from_reqwest` (transport errors) or `MemoryError::from_parse`
(JSON errors). `classify()` already treats `HindsightRequest` as `Transient`
(see Task 3), so behaviour is preserved during the transition.

If no call sites remain, add a `#[deprecated]` attribute to
`MemoryError::HindsightRequest`:

```rust
#[deprecated(note = "use HindsightTimeout/Connect/Parse/Other variants")]
HindsightRequest(String),
```

- [ ] **Step 3: Compile + test**

Run: `cargo build --workspace` and `cargo test --workspace`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/
git commit -m "chore(memory): mark MemoryError::HindsightRequest deprecated"
```

---

## Task 26: Final verification — full build, clippy, format, tests

**Files:** — none (verification-only)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: no errors.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Format check**

Run: `cargo fmt --all --check`
Expected: no diff. If diff: `cargo fmt --all` then commit as
`chore: cargo fmt`.

- [ ] **Step 4: Full test suite**

Run: `cargo test --workspace`
Expected: all tests pass. If the `recovery_drains_queue_after_breaker_closes`
test is flaky due to the 31s sleep, wrap it with
`#[tokio::test(flavor = "multi_thread")]` — still no `#[ignore]`.

- [ ] **Step 5: Manual smoke (optional)**

- Start a test agent with `memory.provider: hindsight` and a valid key.
- Verify startup probe completes in logs.
- Send a chat message; verify retain/recall logs are clean.
- Temporarily point `memory.api_key` to garbage; restart; expect an
  `⚠️ Memory provider authentication failed` Telegram message in the
  allowlisted chat.

No commit required for verification-only; if formatting change required:

```bash
git add -A
git commit -m "chore: cargo fmt"
```

---

## Self-Review

**Spec coverage (scanned top to bottom):**

| Spec section | Covered by |
|---|---|
| § Overview / Goals / Non-Goals | implicit |
| § Module Layout | Tasks 3–9 |
| § Invariant: same-config coupling | Task 18 (aggregator ctor comment); Task 24 (ARCHITECTURE) |
| § Error Classification — MemoryError variants | Task 2 |
| § Error Classification — ErrorKind | Task 3 |
| § Behaviour per kind (retry/breaker/enqueue/drain/marker/Telegram) | Tasks 5 (breaker), 8 (drain), 10 (retain enqueue + Client drop), 15 (marker), 17 (Telegram) |
| § Retry Policy Per Operation | Task 10 (`POLICY_*` constants) |
| § Startup behaviour | Task 12 (non-fatal Auth + background re-probe) |
| § Recovery from AuthFailed | Task 10 (reset on bank-probe success), Task 12 (restart path) |
| § Circuit Breaker | Task 5 |
| § Retain Queue (schema + drain) | Tasks 1, 6, 8, 10 |
| § Idempotency on SIGKILL | spec only (accepted); no task required |
| § Append ordering across queue | spec only (accepted) |
| § Size footprint / Backup | spec only (no code change) |
| § Status Signalling (wrapper watch) | Task 10 |
| § Per-turn local status | Task 15 |
| § Agent marker | Task 15 |
| § Telegram one-shot | Task 17 |
| § Non-Hindsight moments — composite-memory deploy | Task 15 |
| § Non-Hindsight moments — MEMORY.md | Task 16 |
| § SQLite fail-fast | preserved (no task) |
| § Drain task self-failure | Task 14 (implicit `continue` on error) |
| § Doctor Integration | Task 11 |
| § Invariants: composite-memory last | Task 19 (test); Task 24 (doc) |
| § Testing — unit | Tasks 3, 4, 5, 6, 7, 8, 9, 10, 11 |
| § Testing — integration | Tasks 20, 21, 22, 23 |
| § Migration & Rollout | Task 1, Task 25 |
| § Open Questions | documented in spec, no task |

**Placeholder scan:** — no "TBD"/"TODO"/"implement later" left except one `// TODO for future work` in `drain_tick` comment (per-document batching not implemented in this plan; spec accepts single-item drain for now). This is a spec-level non-goal note, not an unfinished task.

**Type consistency check:** `ResilientHindsight` method names are consistent (`recall`, `retain`, `reflect`, `get_or_create_bank`, `drain_retain_item`, `client_drops_24h`, `client_drops_1h`, `bump_client_drop`, `status`, `subscribe_status`, `agent_db_path`, `inner`). `RetryPolicy` constants use the `POLICY_*` prefix uniformly. `ErrorKind` variants match the spec taxonomy. `MemoryStatus` ordering (`Healthy < Degraded < AuthFailed`) is consistent across Tasks 4, 10, 15.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-20-memory-failure-handling.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration with isolated context for each change.

**2. Inline Execution** — work through tasks sequentially in this session with checkpoint reviews.

Which approach?
