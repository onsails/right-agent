# OAuth Refresh Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OAuth token refresh resilient to transient upstream/network failures, surface permanent failures correctly via `BackendStatus::NeedsAuth`, and ensure `mcp_list` reflects the true state of each backend (including when a tool call hits a 401 at runtime).

**Architecture:**
- `do_refresh_cancellable` classifies token-endpoint failures as `Transient` (network/5xx/408/429 — keep retrying) vs `Permanent` (other 4xx — refresh token is dead).
- The per-agent refresh scheduler reads the classification, retries `Transient` indefinitely with exponential backoff (60s → 120s → 300s → 600s → 1200s → 1800s cap), and flips backend status to `NeedsAuth` only on `Permanent`.
- `ProxyBackend::tools_call` detects upstream `Auth required` from rmcp at runtime, flips status to `NeedsAuth`, and returns `ProxyError::NeedsAuth` instead of an opaque `tool_failed`.
- After a successful background refresh while status was `NeedsAuth`, the scheduler triggers a background `backend.connect()` to re-establish the rmcp session.

**Tech Stack:** Rust 2024, tokio, rmcp 1.3, reqwest, wiremock (tests), thiserror.

**Out of scope:**
- Telegram-level user notifications when status flips to `NeedsAuth` (separate UX work).
- Refactoring the unified bootup-vs-runtime refresh paths.
- Changes to OAuth callback / `internal_api::set_token`.

---

## File Structure

**Files modified:**

- `crates/right-mcp/src/reconnect.rs` — split `ReconnectError::RefreshFailed`/`ConnectFailed` into `Refresh(RefreshFailure)` + `Connect(String)`; add `RefreshFailure { Transient, Permanent }`; classify token-endpoint responses inside `do_refresh_cancellable`; pass through `RefreshMessage::NewEntry` callsites.
- `crates/right-mcp/src/refresh.rs` — replace `timers.remove(&name)` on failure with classification-aware retry; track `retry_attempts: HashMap<String, u32>`; add `backend_handles: HashMap<String, Arc<ProxyBackend>>`; on `Permanent` call `backend.set_status(NeedsAuth)`; on success during `NeedsAuth` spawn `backend.connect()`.
- `crates/right-mcp/src/proxy.rs` — in `tools_call`, detect rmcp `Auth required` substring in formatted error; flip status to `NeedsAuth` and return `ProxyError::NeedsAuth`.
- `crates/right/src/main.rs` — at line 1036, pass `backend` into `RefreshMessage::NewEntry`.
- `crates/right/src/internal_api.rs` — at line 486, pass `handle` (backend) into `RefreshMessage::NewEntry`.
- `crates/right-mcp/src/lib.rs` — no change (types already re-exported as needed).

**Files unchanged:**
- `crates/right/src/aggregator.rs` — `do_mcp_list` already reads `BackendStatus` correctly; nothing to change there.
- `crates/bot/src/telegram/handler.rs` — `/mcp list` goes through the same path.

---

## Task 1: Add `RefreshFailure` enum and restructure `ReconnectError`

**Files:**
- Modify: `crates/right-mcp/src/reconnect.rs`

- [ ] **Step 1: Add `RefreshFailure` and update `ReconnectError`**

In `crates/right-mcp/src/reconnect.rs`, replace the existing `ReconnectError` enum (lines ~25-42) with:

```rust
/// Classification of a token endpoint refresh failure.
#[derive(Debug, thiserror::Error)]
pub enum RefreshFailure {
    /// Transient — network error, 5xx, 408, or 429. Retry later.
    #[error("transient refresh failure: {0}")]
    Transient(String),

    /// Permanent — token endpoint returned a non-recoverable 4xx (typically
    /// `invalid_grant` / `invalid_client`). Refresh token is dead; user must
    /// re-authenticate via `/mcp auth <server>`.
    #[error("permanent refresh failure: {0}")]
    Permanent(String),
}

impl RefreshFailure {
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }
}

/// Errors returned by [`do_refresh_cancellable`] and [`reconnect_task`].
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    /// The operation was cancelled via the [`CancellationToken`].
    #[error("refresh cancelled")]
    Cancelled,

    /// The token endpoint refresh step failed (classified).
    #[error("refresh failed: {0}")]
    Refresh(#[from] RefreshFailure),

    /// Post-refresh `backend.connect()` (to the MCP server) failed.
    #[error("backend connect failed: {0}")]
    Connect(String),

    /// Refresh succeeded but the result could not be persisted.
    #[error("failed to persist refreshed token: {0}")]
    PersistFailed(String),
}
```

- [ ] **Step 2: Classify failures inside `do_refresh_cancellable`**

Replace the loop body in `do_refresh_cancellable` (currently lines ~80-127) so that:

- `Ok(r)` with success → return Ok as today.
- `Ok(r)` with status `408` / `429` / `5xx` → treat as transient; record last error; continue retrying.
- `Ok(r)` with any other non-success status → return `Err(ReconnectError::Refresh(RefreshFailure::Permanent(...)))` immediately (no point retrying a dead refresh_token).
- `Err(e)` (reqwest network/IO/timeout) → treat as transient; record; continue.

After the loop exits without success, return `Err(ReconnectError::Refresh(RefreshFailure::Transient(last_error)))`.

Replace lines ~115-145 with:

```rust
Ok(r) => {
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    let is_transient_http =
        status == http::StatusCode::REQUEST_TIMEOUT
            || status == http::StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error();
    if is_transient_http {
        tracing::warn!(attempt, %status, %body, "cancellable refresh attempt failed (transient http)");
        last_error = Some(format!("HTTP {status}: {body}"));
        // fall through to backoff
    } else {
        tracing::warn!(attempt, %status, %body, "cancellable refresh attempt failed (permanent http)");
        return Err(ReconnectError::Refresh(RefreshFailure::Permanent(format!(
            "HTTP {status}: {body}"
        ))));
    }
}
Err(e) => {
    let msg = format!("{e:#}");
    tracing::warn!(attempt, "cancellable refresh request error: {msg}");
    last_error = Some(msg);
}
```

Change the `let mut last_connect_error: Option<String> = None;` line (~63) to:

```rust
let mut last_error: Option<String> = None;
```

Replace the tail (lines ~141-145) with:

```rust
let detail = last_error.unwrap_or_else(|| format!("exhausted {MAX_RETRIES} attempts"));
Err(ReconnectError::Refresh(RefreshFailure::Transient(detail)))
```

Also update the early return at line ~58-61 (when `refresh_token` is `None`) — replace with:

```rust
let refresh_token = entry
    .refresh_token
    .as_deref()
    .ok_or_else(|| ReconnectError::Refresh(RefreshFailure::Permanent(
        "no refresh_token available".into(),
    )))?;
```

And the parse-error path at line ~85-86:

```rust
let token_resp: crate::oauth::TokenResponse = r.json().await.map_err(|e| {
    tracing::warn!(attempt, "failed to parse token response: {e:#}");
    ReconnectError::Refresh(RefreshFailure::Transient(format!(
        "malformed token response: {e:#}"
    )))
})?;
```

- [ ] **Step 3: Update `reconnect_task` to use the new variants**

Replace the error-handling branch in `reconnect_task` (currently lines ~180-189) with:

```rust
Err(ReconnectError::Cancelled) => {
    tracing::debug!(server = %server_name, "reconnect cancelled during refresh");
    return Err(ReconnectError::Cancelled);
}
Err(ReconnectError::Refresh(failure)) => {
    tracing::warn!(server = %server_name, "reconnect refresh failed: {failure:#}");
    if failure.is_permanent()
        && backend.status().await != BackendStatus::Connected
    {
        backend.set_status(BackendStatus::NeedsAuth).await;
    }
    return Err(ReconnectError::Refresh(failure));
}
Err(e) => {
    tracing::warn!(server = %server_name, "reconnect refresh errored: {e:#}");
    return Err(e);
}
```

Update the `backend.connect(...)` `.map_err` (line ~223-225) to:

```rust
backend
    .connect(http_client)
    .await
    .map_err(|e| ReconnectError::Connect(format!("{e:#}")))?;
```

- [ ] **Step 4: Update existing tests in `reconnect.rs`**

In test `cancellation_aborts_refresh_during_backoff` (line ~319): the mock currently returns 401, which under the new code returns `Refresh(Permanent(...))` immediately (no backoff sleep, nothing to cancel into). Change the mock status to 503 so the inner loop enters its 30s backoff sleep where the cancellation can take effect:

```rust
.respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
```

The rest of the test logic (paused time + cancel) stays the same. Final assertion `matches!(result, Err(ReconnectError::Cancelled))` is unchanged.

In test `exhausted_retries_do_not_overwrite_connected_status` (line ~367): change the mock to 503 for the same reason (we want the loop to exhaust all retries, not short-circuit on 401-as-Permanent). The new behavior: all 3 attempts get 503 → `Refresh(Transient(...))`. `is_permanent()` is false, so the `if is_permanent && status != Connected` guard does not trigger. Backend stays Connected. Assertion at line ~432 still holds.

Replace the mock setup:

```rust
Mock::given(method("POST"))
    .respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
    .mount(&server)
    .await;
```

In test `successful_refresh_writes_token_and_sends_new_entry` (line ~447), update the error match (line ~509) to:

```rust
Err(ReconnectError::Connect(_)) => {} // Expected — fake URL fails to connect
```

- [ ] **Step 5: Run existing reconnect tests**

Run: `cargo test -p right-mcp --lib reconnect`
Expected: all existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/reconnect.rs
git commit -m "refactor(right-mcp): classify refresh failures as Transient/Permanent

Splits ReconnectError::RefreshFailed/ConnectFailed into Refresh(RefreshFailure)
+ Connect, with RefreshFailure distinguishing network/5xx (Transient — retry
with backoff) from non-recoverable 4xx (Permanent — refresh token dead).
do_refresh_cancellable short-circuits on Permanent. Scheduler will consume
this classification next commit."
```

---

## Task 2: Test classification — Transient (5xx and network)

**Files:**
- Modify: `crates/right-mcp/src/reconnect.rs` (tests module)

- [ ] **Step 1: Add failing test for transient 5xx classification**

Append to `mod tests` in `crates/right-mcp/src/reconnect.rs`:

```rust
#[tokio::test]
async fn refresh_classifies_5xx_as_transient() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream broke"))
        .mount(&server)
        .await;

    let entry = make_entry(format!("{}/token", server.uri()));
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();

    tokio::time::pause();
    let handle = tokio::spawn(async move {
        do_refresh_cancellable(&client, &entry, &cancel).await
    });
    // Burn through all backoffs deterministically.
    for _ in 0..MAX_RETRIES {
        tokio::time::advance(Duration::from_secs(200)).await;
        tokio::task::yield_now().await;
    }
    let result = handle.await.expect("task panicked");
    assert!(
        matches!(
            result,
            Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
        ),
        "expected Transient for 5xx, got {result:?}"
    );
}
```

- [ ] **Step 2: Run it to verify it passes**

Run: `cargo test -p right-mcp --lib refresh_classifies_5xx_as_transient`
Expected: PASS.

- [ ] **Step 3: Add failing test for network-error classification**

Append:

```rust
#[tokio::test]
async fn refresh_classifies_network_error_as_transient() {
    // Use a URL that fails DNS / TCP — port 1 on 127.0.0.1 should be closed.
    let entry = make_entry("http://127.0.0.1:1/token".into());
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(100))
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let cancel = CancellationToken::new();

    tokio::time::pause();
    let handle = tokio::spawn(async move {
        do_refresh_cancellable(&client, &entry, &cancel).await
    });
    for _ in 0..MAX_RETRIES {
        tokio::time::advance(Duration::from_secs(200)).await;
        tokio::task::yield_now().await;
    }
    let result = handle.await.expect("task panicked");
    assert!(
        matches!(
            result,
            Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
        ),
        "expected Transient for network error, got {result:?}"
    );
}
```

- [ ] **Step 4: Run it**

Run: `cargo test -p right-mcp --lib refresh_classifies_network_error_as_transient`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/reconnect.rs
git commit -m "test(right-mcp): cover transient refresh classification (5xx + network)"
```

---

## Task 3: Test classification — Permanent (4xx)

**Files:**
- Modify: `crates/right-mcp/src/reconnect.rs` (tests module)

- [ ] **Step 1: Add failing test for permanent 400 invalid_grant**

Append:

```rust
#[tokio::test]
async fn refresh_classifies_400_as_permanent_no_retry() {
    let server = MockServer::start().await;
    // 400 invalid_grant — refresh token revoked.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"error":"invalid_grant"}"#),
        )
        .expect(1) // must NOT retry — first response is enough
        .mount(&server)
        .await;

    let entry = make_entry(format!("{}/token", server.uri()));
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();
    let result = do_refresh_cancellable(&client, &entry, &cancel).await;

    assert!(
        matches!(
            result,
            Err(ReconnectError::Refresh(RefreshFailure::Permanent(_)))
        ),
        "expected Permanent for 400 invalid_grant, got {result:?}"
    );
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p right-mcp --lib refresh_classifies_400_as_permanent_no_retry`
Expected: PASS — `expect(1)` on the mock verifies that we did not retry.

- [ ] **Step 3: Add test for 429 → Transient (rate limit must retry)**

Append:

```rust
#[tokio::test]
async fn refresh_classifies_429_as_transient() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let entry = make_entry(format!("{}/token", server.uri()));
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();

    tokio::time::pause();
    let handle = tokio::spawn(async move {
        do_refresh_cancellable(&client, &entry, &cancel).await
    });
    for _ in 0..MAX_RETRIES {
        tokio::time::advance(Duration::from_secs(200)).await;
        tokio::task::yield_now().await;
    }
    let result = handle.await.expect("task panicked");
    assert!(
        matches!(
            result,
            Err(ReconnectError::Refresh(RefreshFailure::Transient(_)))
        ),
        "expected Transient for 429, got {result:?}"
    );
}
```

- [ ] **Step 4: Run it**

Run: `cargo test -p right-mcp --lib refresh_classifies_429_as_transient`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/reconnect.rs
git commit -m "test(right-mcp): cover permanent (400) and transient (429) refresh classification"
```

---

## Task 4: Wire `backend: Arc<ProxyBackend>` into `RefreshMessage::NewEntry`

**Files:**
- Modify: `crates/right-mcp/src/refresh.rs`
- Modify: `crates/right-mcp/src/reconnect.rs`
- Modify: `crates/right/src/main.rs`
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Add `backend` field to `RefreshMessage::NewEntry`**

In `crates/right-mcp/src/refresh.rs`, replace the `NewEntry` variant (lines ~27-33) with:

```rust
/// New or updated OAuth token — schedule refresh timer.
NewEntry {
    server_name: String,
    state: OAuthServerState,
    /// Shared token handle — scheduler writes new tokens here.
    token: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Backend handle — scheduler updates status on permanent failure
    /// and triggers reconnect after recovery.
    backend: Arc<crate::proxy::ProxyBackend>,
},
```

- [ ] **Step 2: Update sender at `reconnect.rs:210`**

Replace the `refresh_tx.send(RefreshMessage::NewEntry { ... })` block (lines ~209-219) with:

```rust
refresh_tx
    .send(RefreshMessage::NewEntry {
        server_name: server_name.clone(),
        state: new_state,
        token: token_arc.clone(),
        backend: backend.clone(),
    })
    .await
    .map_err(|e| {
        tracing::error!("refresh scheduler dropped: {e:#}");
        ReconnectError::PersistFailed(format!("refresh scheduler unavailable: {e:#}"))
    })?;
```

- [ ] **Step 3: Update sender at `main.rs:1036`**

In `crates/right/src/main.rs`, the loop at line 1032 iterates `oauth_map` entries but only has `(name, (state, token_arc))`. We need the matching backend. Replace lines 1032-1050 with:

```rust
// Send NewEntry for non-expired OAuth servers. Expired tokens
// are handled by the reconnect task which sends NewEntry after refresh.
let proxies_by_name: std::collections::HashMap<String, std::sync::Arc<right_mcp::proxy::ProxyBackend>> =
    proxies_snapshot.iter().cloned().collect();
for (name, (state, token_arc)) in &oauth_map {
    if state.refresh_token.is_some() {
        let due_in = right_mcp::refresh::refresh_due_in(state);
        if due_in > std::time::Duration::ZERO
            && let Some(backend) = proxies_by_name.get(name)
        {
            let msg = right_mcp::refresh::RefreshMessage::NewEntry {
                server_name: name.clone(),
                state: state.clone(),
                token: token_arc.clone(),
                backend: backend.clone(),
            };
            if let Err(e) = refresh_tx.send(msg).await {
                tracing::warn!(
                    agent = agent_name.as_str(),
                    server = name.as_str(),
                    "failed to send refresh entry: {e:#}",
                );
            }
        }
    }
}
```

(`proxies_snapshot` is `Vec<(String, Arc<ProxyBackend>)>` — verify by checking the type at the line above. If it's not `Clone`, take a reference instead and adjust accordingly.)

- [ ] **Step 4: Update sender at `internal_api.rs:486`**

In `crates/right/src/internal_api.rs`, replace lines 477-499 with:

```rust
if let Some(tx) = state.refresh_senders.get(&req.agent) {
    let entry = OAuthServerState {
        refresh_token: Some(req.refresh_token.clone()),
        token_endpoint: req.token_endpoint.clone(),
        client_id: req.client_id.clone(),
        client_secret: req.client_secret.clone(),
        expires_at,
        server_url: handle.url().to_string(),
    };
    if let Err(e) = tx
        .send(RefreshMessage::NewEntry {
            server_name: req.server.clone(),
            state: entry,
            token: handle.token().clone(),
            backend: handle.clone(),
        })
        .await
    {
        tracing::warn!(
            agent = req.agent.as_str(),
            server = req.server.as_str(),
            "failed to notify refresh scheduler: {e:#}"
        );
    }
}
```

(`handle` is already `Arc<ProxyBackend>` per line 460 `let handle_clone = Arc::clone(&handle);`. Confirm by checking the existing code.)

- [ ] **Step 5: Update scheduler to store backend handles**

In `crates/right-mcp/src/refresh.rs`, inside `run_refresh_scheduler` (after line 113), add a new map alongside `token_handles`:

```rust
let mut backend_handles: HashMap<String, Arc<crate::proxy::ProxyBackend>> =
    HashMap::new();
```

Inside the `RefreshMessage::NewEntry` match arm (line ~123), destructure the new field and store it:

```rust
RefreshMessage::NewEntry { server_name, state: entry_state, token, backend } => {
    // ... existing logic ...
    entries.insert(server_name.clone(), entry_state);
    token_handles.insert(server_name.clone(), token);
    backend_handles.insert(server_name.clone(), backend);
}
```

Inside `RemoveServer` (line ~162), also remove from `backend_handles`:

```rust
RefreshMessage::RemoveServer { server_name } => {
    timers.remove(&server_name);
    entries.remove(&server_name);
    token_handles.remove(&server_name);
    backend_handles.remove(&server_name);
    tracing::info!(server = %server_name, "refresh cancelled — server removed");
}
```

- [ ] **Step 6: Update existing test `successful_refresh_writes_token_and_sends_new_entry`**

In `crates/right-mcp/src/reconnect.rs`, the test asserts on the `NewEntry` variant payload (line ~524). Extend the match to also accept the new `backend` field:

```rust
RefreshMessage::NewEntry {
    server_name,
    state,
    backend: _,
    ..
} => {
    // ...
}
```

- [ ] **Step 7: Build to verify all callsites compile**

Run: `cargo build --workspace`
Expected: succeeds.

- [ ] **Step 8: Run the full right-mcp test suite + right tests**

Run: `cargo test -p right-mcp -p right`
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/right-mcp/src/refresh.rs crates/right-mcp/src/reconnect.rs crates/right/src/main.rs crates/right/src/internal_api.rs
git commit -m "feat(right-mcp): scheduler tracks ProxyBackend handles per server

NewEntry now carries Arc<ProxyBackend> so the scheduler can update status
on permanent refresh failures and trigger reconnects after recovery. All
three NewEntry senders (boot, OAuth callback, post-refresh) updated."
```

---

## Task 5: Indefinite transient retry with exponential backoff + bigger refresh margin

**Files:**
- Modify: `crates/right-mcp/src/refresh.rs`

### Task 5a: Expand refresh margin (early-warning window)

**Why:** With the new indefinite-retry policy (Task 5 main body), exponential backoff caps at 30 min. A 10-minute margin is insufficient — a network outage that lasts > 10 min will cause the token to expire before retries can recover. Bump the early-warning window to up to 1 hour so the retry loop has time to succeed.

**Constraint:** Short-lived tokens (e.g. tokens with `expires_in < 2 * margin`) would cause `refresh_due_in` to return `ZERO` every cycle, busy-looping the scheduler. Apply a sliding cap: `margin = min(MAX_MARGIN, remaining_lifetime / 2)`.

- [ ] **Step 1a-1: Rename `REFRESH_MARGIN` → `REFRESH_MARGIN_MAX` and bump value**

In `crates/right-mcp/src/refresh.rs:10-11`, replace:

```rust
/// Refresh margin: refresh token 10 minutes before expiry.
const REFRESH_MARGIN: Duration = Duration::from_secs(600);
```

with:

```rust
/// Maximum refresh margin: refresh tokens up to 1 hour before expiry so
/// transient network outages (which can last minutes on laptops) have time
/// to resolve via exponential-backoff retries before the token actually dies.
///
/// Used as an upper bound — actual margin is `min(MAX, remaining_lifetime / 2)`
/// to avoid busy-looping the scheduler on short-lived tokens.
const REFRESH_MARGIN_MAX: Duration = Duration::from_secs(3600);
```

- [ ] **Step 1a-2: Update `refresh_due_in` to use sliding margin**

Replace the existing `refresh_due_in` (lines ~99-110) with:

```rust
/// Calculate how long until refresh should fire.
///
/// Margin = `min(REFRESH_MARGIN_MAX, remaining_lifetime / 2)`. This gives
/// long-lived tokens (1h+) a full 1-hour buffer for retry recovery while
/// keeping short-lived tokens from busy-looping (refresh fires no sooner
/// than half-life).
///
/// Returns `Duration::ZERO` if the token is already past margin.
pub fn refresh_due_in(entry: &OAuthServerState) -> Duration {
    let now = chrono::Utc::now();
    let remaining = (entry.expires_at - now).to_std().unwrap_or(Duration::ZERO);
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    let margin = std::cmp::min(REFRESH_MARGIN_MAX, remaining / 2);
    remaining.saturating_sub(margin)
}
```

- [ ] **Step 1a-3: Update existing `refresh_due_in` tests**

Three existing tests in `refresh.rs` need updating to match the sliding-margin behavior:

In `refresh_due_in_future` (line ~269): expires_at is 30 min from now. New margin = `min(60min, 15min) = 15min`. So `due_in ≈ 15min = 900s`. Replace assertion:

```rust
#[test]
fn refresh_due_in_future() {
    let entry = OAuthServerState {
        refresh_token: Some("rt".into()),
        token_endpoint: "https://example.com/token".into(),
        client_id: "c".into(),
        client_secret: None,
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(30),
        server_url: "https://example.com/mcp".into(),
    };
    // Margin = min(MAX=3600s, 1800s/2=900s) = 900s. due ≈ 1800s - 900s = 900s.
    let due = refresh_due_in(&entry);
    assert!(
        due.as_secs() > 850 && due.as_secs() < 950,
        "expected ~900s, got {}s",
        due.as_secs()
    );
}
```

In `refresh_due_in_returns_zero_when_expired` (line ~287): logic unchanged (expired → ZERO). Keep test verbatim.

Replace `refresh_due_in_within_margin` (line ~301) — "within fixed margin" is no longer a concept. Rename and rewrite to test the sliding cap:

```rust
#[test]
fn refresh_due_in_uses_half_lifetime_for_short_tokens() {
    let entry = OAuthServerState {
        refresh_token: Some("rt".into()),
        token_endpoint: "https://example.com/token".into(),
        client_id: "c".into(),
        client_secret: None,
        // 5-minute lifetime — far shorter than 1-hour MAX margin.
        // Margin must clamp to lifetime/2 = 150s; due = 300s - 150s = 150s.
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        server_url: "https://example.com/mcp".into(),
    };
    let due = refresh_due_in(&entry);
    assert!(
        due.as_secs() > 120 && due.as_secs() < 180,
        "expected ~150s (half of 5-min lifetime), got {}s",
        due.as_secs()
    );
}

#[test]
fn refresh_due_in_caps_at_max_for_long_tokens() {
    let entry = OAuthServerState {
        refresh_token: Some("rt".into()),
        token_endpoint: "https://example.com/token".into(),
        client_id: "c".into(),
        client_secret: None,
        // 24-hour lifetime — half is 12 hours, but MAX caps margin at 1 hour.
        // due = 24h - 1h = 23h.
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
        server_url: "https://example.com/mcp".into(),
    };
    let due = refresh_due_in(&entry);
    // 23 hours = 82800s. Allow a few seconds of clock skew.
    assert!(
        due.as_secs() > 82700 && due.as_secs() < 82900,
        "expected ~82800s (24h - 1h MAX margin), got {}s",
        due.as_secs()
    );
}
```

- [ ] **Step 1a-4: Verify**

Run: `devenv shell -- cargo test -p right-mcp --lib refresh_due_in`
Expected: 4 tests pass (one renamed/rewritten, one new, two updated/unchanged).

### Task 5b: Indefinite transient retry with exponential backoff

- [ ] **Step 1: Add `transient_backoff_secs` helper**

In `crates/right-mcp/src/refresh.rs`, immediately above `run_refresh_scheduler` (line ~92), add:

```rust
/// Compute the delay before the next transient-retry attempt.
///
/// `attempt` is 1-indexed (1 = first retry after initial failure).
/// Sequence: 60, 120, 300, 600, 1200, 1800, 1800, ... (cap at 30 min).
pub(crate) fn transient_backoff_secs(attempt: u32) -> u64 {
    const STEPS: &[u64] = &[60, 120, 300, 600, 1200, 1800];
    STEPS
        .get((attempt.saturating_sub(1)) as usize)
        .copied()
        .unwrap_or(1800)
}
```

- [ ] **Step 2: Track per-server retry state**

In `run_refresh_scheduler`, beside `timers` (line ~113), add:

```rust
let mut retry_attempts: HashMap<String, u32> = HashMap::new();
```

Inside the `RemoveServer` arm, also clear it:

```rust
retry_attempts.remove(&server_name);
```

- [ ] **Step 3: Replace the failure branch with classification-aware retry**

Replace the timer-fire match block (lines ~186-222) with:

```rust
let result = crate::reconnect::do_refresh_cancellable(
    &http_client,
    &entry,
    &tokio_util::sync::CancellationToken::new(),
)
.await;

match result {
    Ok((new_entry, access_token)) => {
        // Reset retry counter on success.
        retry_attempts.remove(&name);

        let was_needs_auth =
            if let Some(backend) = backend_handles.get(&name) {
                backend.status().await == crate::proxy::BackendStatus::NeedsAuth
            } else {
                false
            };

        // Write token directly to ProxyBackend's shared state
        if let Some(token_arc) = token_handles.get(&name) {
            *token_arc.write().await = Some(access_token.clone());
            tracing::info!(server = %name, "token refreshed in-memory");
        }

        // Schedule next refresh based on new expiry.
        let due = refresh_due_in(&new_entry);
        timers.insert(name.clone(), tokio::time::Instant::now() + due);

        // Persist refreshed token to SQLite
        match right_db::open_connection(&agent_dir, false) {
            Ok(conn) => {
                let expires_at = new_entry.expires_at.to_rfc3339();
                if let Err(e) = crate::credentials::db_update_oauth_token(
                    &conn,
                    &name,
                    &access_token,
                    new_entry.refresh_token.as_deref(),
                    &expires_at,
                ) {
                    tracing::error!("failed to persist refreshed token: {e:#}");
                }
            }
            Err(e) => {
                tracing::error!(
                    "failed to open memory DB for token refresh persistence: {e:#}"
                );
            }
        }
        entries.insert(name.clone(), new_entry);

        // If backend was NeedsAuth (set by a 401 at tool-call time or by a
        // previous permanent failure that has since cleared), the rmcp
        // session is probably dead. Spawn a background reconnect.
        if was_needs_auth
            && let Some(backend) = backend_handles.get(&name).cloned()
        {
            let http = http_client.clone();
            let name_owned = name.clone();
            tokio::spawn(async move {
                if let Err(e) = backend.connect(http).await {
                    tracing::warn!(
                        server = %name_owned,
                        "post-refresh reconnect failed: {e:#}"
                    );
                }
            });
        }
    }
    Err(crate::reconnect::ReconnectError::Refresh(failure)) => {
        let permanent = failure.is_permanent();
        tracing::warn!(server = %name, %permanent, "token refresh failed: {failure:#}");
        if permanent {
            if let Some(backend) = backend_handles.get(&name) {
                backend.set_status(crate::proxy::BackendStatus::NeedsAuth).await;
                tracing::warn!(
                    server = %name,
                    "marked NeedsAuth after permanent refresh failure"
                );
            }
            // Do not reschedule — user must re-OAuth.
            timers.remove(&name);
            retry_attempts.remove(&name);
        } else {
            // Transient — bump retry count and reschedule.
            let attempt = retry_attempts
                .entry(name.clone())
                .and_modify(|n| *n += 1)
                .or_insert(1);
            let delay = transient_backoff_secs(*attempt);
            tracing::info!(
                server = %name,
                attempt = *attempt,
                delay_secs = delay,
                "scheduling transient retry"
            );
            timers.insert(
                name.clone(),
                tokio::time::Instant::now() + Duration::from_secs(delay),
            );
        }
    }
    Err(other) => {
        // Cancelled / PersistFailed / Connect — none should occur in this
        // path (we pass a never-cancelled token and don't call backend.connect).
        // Treat as transient to be safe.
        tracing::warn!(server = %name, "unexpected refresh outcome: {other:#}");
        let attempt = retry_attempts
            .entry(name.clone())
            .and_modify(|n| *n += 1)
            .or_insert(1);
        let delay = transient_backoff_secs(*attempt);
        timers.insert(
            name.clone(),
            tokio::time::Instant::now() + Duration::from_secs(delay),
        );
    }
}
```

- [ ] **Step 4: Remove the old `do_refresh` wrapper**

Delete `pub async fn do_refresh(...)` (lines ~228-242) — no callers remain after Step 3 replaces the only callsite.

- [ ] **Step 5: Add test — transient failure keeps the timer alive**

Append to `mod tests` in `crates/right-mcp/src/refresh.rs`:

```rust
#[tokio::test]
async fn scheduler_retries_transient_indefinitely() {
    let server = MockServer::start().await;
    // First scheduler fire: do_refresh_cancellable internally retries 3
    // times. We want all 3 of those to see 503, then later attempts see
    // 200. up_to_n_times caps the first mock at 3 hits; subsequent hits
    // fall through to the second mock.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(503).set_body_string("temporarily down"))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-tok",
            "refresh_token": "new-rt",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    conn.execute(
        "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
        [],
    )
    .unwrap();
    drop(conn);

    let entry_state = OAuthServerState {
        refresh_token: Some("rt".into()),
        token_endpoint: format!("{}/token", server.uri()),
        client_id: "c".into(),
        client_secret: None,
        // Already past margin → fires immediately
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        server_url: "https://x/mcp".into(),
    };
    let token_arc: Arc<tokio::sync::RwLock<Option<String>>> =
        Arc::new(tokio::sync::RwLock::new(Some("old".into())));
    let backend = Arc::new(crate::proxy::ProxyBackend::new(
        "s".into(),
        tmp.path().to_path_buf(),
        "https://x/mcp".into(),
        token_arc.clone(),
        crate::proxy::AuthMethod::Bearer,
    ));

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let scheduler = tokio::spawn(run_refresh_scheduler(tmp.path().to_path_buf(), rx));

    tx.send(RefreshMessage::NewEntry {
        server_name: "s".into(),
        state: entry_state,
        token: token_arc.clone(),
        backend: backend.clone(),
    })
    .await
    .unwrap();

    // First scheduler fire calls do_refresh_cancellable, which retries
    // internally 3 times with 30/60s backoff (~90s virtual). All return
    // 503 → Transient. Scheduler reschedules in 60s. Second fire's first
    // attempt hits the 200 mock → success.
    //
    // Drive virtual time forward until token_arc updates or we time out.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        if *token_arc.read().await == Some("new-tok".to_string()) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "scheduler did not refresh in time; received_requests={}",
                server.received_requests().await.unwrap().len()
            );
        }
    }

    let n_requests = server.received_requests().await.unwrap().len();
    assert!(
        n_requests >= 4,
        "scheduler must keep retrying transient failures; got {n_requests} requests"
    );

    scheduler.abort();
}
```

Add this import near the top of the tests module (or inside the test if not used elsewhere):

```rust
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
```

(If the tests module already has these imports for the existing `cancellation_aborts_refresh_during_backoff` style tests in `reconnect.rs`, mirror those imports here.)

Note: `tokio::time::pause()` must be active for `tokio::time::advance` to work. Add `tokio::time::pause();` at the top of the test, after the MockServer setup but before spawning the scheduler.

- [ ] **Step 6: Run the test**

Run: `cargo test -p right-mcp --lib scheduler_retries_transient_indefinitely`
Expected: PASS within a few seconds (uses paused time, not wall clock).

- [ ] **Step 7: Add test — permanent failure flips backend to NeedsAuth**

Append:

```rust
#[tokio::test]
async fn scheduler_marks_needs_auth_on_permanent_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"error":"invalid_grant"}"#),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).unwrap();
    conn.execute(
        "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('s', 'https://x/mcp', 'oauth')",
        [],
    )
    .unwrap();
    drop(conn);

    let entry_state = OAuthServerState {
        refresh_token: Some("rt".into()),
        token_endpoint: format!("{}/token", server.uri()),
        client_id: "c".into(),
        client_secret: None,
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        server_url: "https://x/mcp".into(),
    };
    let token_arc: Arc<tokio::sync::RwLock<Option<String>>> =
        Arc::new(tokio::sync::RwLock::new(Some("old".into())));
    let backend = Arc::new(crate::proxy::ProxyBackend::new(
        "s".into(),
        tmp.path().to_path_buf(),
        "https://x/mcp".into(),
        token_arc.clone(),
        crate::proxy::AuthMethod::Bearer,
    ));
    // Pre-set to Unreachable so the permanent flip is observable.
    backend.set_status(crate::proxy::BackendStatus::Unreachable).await;

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let scheduler = tokio::spawn(run_refresh_scheduler(tmp.path().to_path_buf(), rx));

    tokio::time::pause();
    tx.send(RefreshMessage::NewEntry {
        server_name: "s".into(),
        state: entry_state,
        token: token_arc.clone(),
        backend: backend.clone(),
    })
    .await
    .unwrap();

    // Allow the timer to fire and the permanent response to be processed.
    for _ in 0..60 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        if backend.status().await == crate::proxy::BackendStatus::NeedsAuth {
            break;
        }
    }

    assert_eq!(
        backend.status().await,
        crate::proxy::BackendStatus::NeedsAuth,
        "permanent refresh failure must flip backend to NeedsAuth"
    );

    scheduler.abort();
}
```

- [ ] **Step 8: Run the test**

Run: `cargo test -p right-mcp --lib scheduler_marks_needs_auth_on_permanent_failure`
Expected: PASS.

- [ ] **Step 9: Run the whole right-mcp test suite**

Run: `cargo test -p right-mcp`
Expected: all pass.

- [ ] **Step 10: Commit**

```bash
git add crates/right-mcp/src/refresh.rs
git commit -m "fix(right-mcp): scheduler retries transient refresh failures indefinitely

Replaces \`timers.remove()\` on failure with classification-aware logic:
- Transient (network/5xx/429): reschedule with exponential backoff
  (60s/120s/300s/600s/1200s/1800s cap), preserving entries/token_handles
  so the timer keeps firing until success.
- Permanent (4xx invalid_grant/etc.): flip backend to NeedsAuth, stop
  rescheduling — user must re-OAuth via /mcp auth.
- On success during NeedsAuth, spawn backend.connect() to re-establish
  the rmcp session that the server likely killed.

Fixes the bug where one transient network blip permanently silenced the
refresh scheduler, leaving mcp_list lying about \`connected\` status."
```

---

## Task 6: Detect upstream 401 in `tools_call`

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs`

- [ ] **Step 1: Write failing test — classifier helper recognises rmcp Auth required surface**

Append to `mod tests` in `crates/right-mcp/src/proxy.rs`:

```rust
#[test]
fn is_upstream_auth_error_matches_rmcp_auth_required_surface() {
    // The exact wording produced by ServiceError::TransportSend wrapping
    // StreamableHttpError::AuthRequired (rmcp 1.3, `#[error("Auth required")]`).
    let real = "Transport send error: Transport [\
        rmcp::transport::worker::WorkerTransport<\
        rmcp::transport::streamable_http_client::StreamableHttpClientWorker<\
        right_mcp::proxy::DynamicAuthClient>>\
    ] error: Auth required";
    assert!(is_upstream_auth_error(real));

    // Negative cases — must NOT be misclassified as auth.
    assert!(!is_upstream_auth_error("connection refused"));
    assert!(!is_upstream_auth_error(
        "Transport send error: Transport [foo] error: timeout"
    ));
    assert!(!is_upstream_auth_error("Mcp error: invalid_params"));
}
```

This pins the substring-matching contract. The full `tools_call` integration (constructing a real `RunningService` whose `call_tool` returns `AuthRequired`) requires a complete MCP handshake against a custom server — out of scope for a unit test. Integration is verified manually via Task 7 Step 4 (log-fingerprint check against a real expired token).

- [ ] **Step 2: Add the classifier helper**

In `crates/right-mcp/src/proxy.rs`, add a public-in-crate helper above the `impl ProxyBackend` block (or just below the error enum):

```rust
/// Detect whether an rmcp error string indicates upstream OAuth/auth failure.
///
/// rmcp surfaces 401-style failures from `StreamableHttpClient` as
/// `ServiceError::TransportSend(DynamicTransportError)` whose `Display`
/// includes `"Auth required"` (from `StreamableHttpError::AuthRequired`).
/// We match on the substring rather than downcasting through `Box<dyn Error>`
/// generic transports.
pub(crate) fn is_upstream_auth_error(msg: &str) -> bool {
    msg.contains("Auth required")
}
```

- [ ] **Step 3: Modify `tools_call` to detect 401**

In `tools_call` (lines ~381-390), replace:

```rust
let result =
    client
        .peer()
        .call_tool(params)
        .await
        .map_err(|e| ProxyError::CallToolFailed {
            server: self.server_name.clone(),
            tool: tool_name.to_owned(),
            source: e,
        })?;

Ok(result)
```

with:

```rust
let result = client.peer().call_tool(params).await;
match result {
    Ok(r) => Ok(r),
    Err(e) => {
        let msg = format!("{e:#}");
        if is_upstream_auth_error(&msg) {
            tracing::warn!(
                server = %self.server_name,
                tool = tool_name,
                "upstream returned auth-required; flipping backend to NeedsAuth"
            );
            *self.status.write().await = BackendStatus::NeedsAuth;
            return Err(ProxyError::NeedsAuth {
                server: self.server_name.clone(),
            });
        }
        Err(ProxyError::CallToolFailed {
            server: self.server_name.clone(),
            tool: tool_name.to_owned(),
            source: e,
        })
    }
}
```

- [ ] **Step 4: Run the classifier test**

Run: `cargo test -p right-mcp --lib tools_call_upstream_401_flips_to_needs_auth`
Expected: PASS.

- [ ] **Step 5: Run the full right-mcp test suite**

Run: `cargo test -p right-mcp`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/proxy.rs
git commit -m "fix(right-mcp): flip backend to NeedsAuth on upstream tool-call 401

When rmcp returns \`AuthRequired\` during \`call_tool\`, the proxy was
returning an opaque \`tool_failed\` while leaving status=Connected, so
mcp_list lied about composio being healthy. Now we detect the rmcp
\`Auth required\` surface, flip status to NeedsAuth, and return the
canonical NeedsAuth error with the \`/mcp auth\` hint."
```

---

## Task 7: Final workspace verification

Per `AGENTS.md` "Verification cadence": targeted package tests during implementation, mandatory full-workspace test at the end. This task is the final gate before merge.

**Files:**
- (no edits — verification only)

All commands use `devenv shell --` prefix (project convention — `devenv.nix` is present at repo root).

- [ ] **Step 1: Workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: succeeds with no warnings beyond the pre-existing baseline.

- [ ] **Step 2: Workspace clippy**

Run: `devenv shell -- cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 3: Full workspace test (mandatory final gate)**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass. This is the project's mandatory end-of-work verification per `AGENTS.md` Verification cadence — targeted package tests during implementation do NOT replace it.

- [ ] **Step 4: Re-read the daily aggregator log to confirm bug fingerprint**

The original incident log line was:

```
2026-05-13T20:47:53.997720Z  WARN right_mcp::refresh: token refresh failed after retries: ...
(after which no further \`composio\` lines until next bot restart)
```

After this plan, the analogous failure should produce:

```
WARN right_mcp::refresh: token refresh failed: transient refresh failure: ...
INFO right_mcp::refresh: scheduling transient retry server=composio attempt=1 delay_secs=60
INFO right_mcp::refresh: scheduling transient retry server=composio attempt=2 delay_secs=120
...
INFO right_mcp::refresh: token refreshed in-memory server=composio   (when network recovers)
```

For permanent (invalid_grant) failures, instead:

```
WARN right_mcp::refresh: token refresh failed: permanent refresh failure: HTTP 400: {"error":"invalid_grant"}
WARN right_mcp::refresh: marked NeedsAuth after permanent refresh failure server=composio
```

There is no automated check for this — just confirm by reading the new log messages match the implementation.

- [ ] **Step 5: Final commit (or amend if everything passes cleanly)**

No additional commit needed if all earlier commits passed verification. If clippy or build flagged anything, fix and commit:

```bash
git add -p
git commit -m "chore: clippy fixes for OAuth refresh resilience"
```

---

## Task 8: Multi-pass code review via `/review-loop`

After Task 7 passes, run the project's automated multi-pass review skill against the worktree's branch.

**Files:**
- (no edits — orchestrated review)

- [ ] **Step 1: Invoke `/review-loop` against the `master` base**

From the worktree directory, run the `review-loop:review-loop` skill targeting `master` as the comparison branch. The skill discovers project guidelines (`CLAUDE.md`, `AGENTS.md`, `AGENTS.rust.md`, `ARCHITECTURE.md`, CI configs) and synthesizes a multi-pass review with per-issue fix subagents.

This is invoked by the human controller (not by a subagent during plan execution). The expected outcome:

- Findings are categorized by severity; each non-trivial finding gets a fix-or-rationalize loop.
- The loop completes when all findings are resolved or explicitly waived.

- [ ] **Step 2: Address findings**

For each finding the review-loop surfaces:
- Apply the fix in a new commit (if straightforward).
- Or document a written rationale for why the suggestion does not apply.

No findings remain unaddressed before merging.

---

## Self-review notes (filled by author)

- Coverage of user requirements:
  - "Сетевые проблемы должны увеличивать задержки между ретрайс до разумного предела но не прекращать их" → Task 5 (Transient retry with capped exponential backoff).
  - "Если фейл структурный от платформы то токен должен протухать и агент корректно узнает проблему" → Task 5 (Permanent → NeedsAuth + stop retry) + Task 6 (tool-call 401 also flips status).
  - "/mcp list должен корректную инфу показывать о токене" → Already routes through `BackendStatus`; Tasks 5 and 6 ensure status is accurate; no separate task needed.
- Type names are consistent: `RefreshFailure { Transient, Permanent }`, `ReconnectError::Refresh(_)`, `RefreshMessage::NewEntry { ..., backend }`, `transient_backoff_secs(attempt)`, `is_upstream_auth_error(msg)`.
- No placeholders or "TBD" strings.
