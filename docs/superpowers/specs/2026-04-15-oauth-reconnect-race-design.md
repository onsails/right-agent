# OAuth Reconnect Race Fix — Design Spec

**Date:** 2026-04-15
**Branch:** cron-fix (or new branch)

## Problem

Three independent systems touch the same server's token and connection status:

1. **Startup reconnect** (main.rs:603) — fire-and-forget `tokio::spawn`, calls `do_refresh()` with 30/60/120s backoffs
2. **Refresh scheduler** (refresh.rs:91) — single-threaded select loop, owns timers
3. **OAuth callback** (internal_api.rs:390) — `handle_set_token`, sets token + sends `NewEntry` + spawns reconnect

The race sequence observed in production:

1. Aggregator starts. Token for `composio` expired 4 min ago. Refresh token in SQLite is stale (rotated by a previous session).
2. Startup reconnect task begins `do_refresh()` — attempts fail with 401 "invalid refresh token", retrying with 30s/60s backoffs (~90s total).
3. User re-authenticates via OAuth. `handle_set_token` delivers a fresh token, reconnects the backend (Connected), and sends `NewEntry` to the scheduler.
4. The stale reconnect task exhausts its retries and calls `backend.set_status(NeedsAuth)` — overwriting the Connected status set by step 3.

Result: a working backend is marked as needing auth. The scheduler has a valid timer set (from step 3), so it may self-heal when that timer fires ~50 min later. But in the meantime, the agent has no access to the server's tools.

### Why tests didn't catch it

The reconnect logic is a 65-line inline block in `main.rs` with 6 captured variables, spawned as fire-and-forget tasks. There is no way to:

- Inject a mock HTTP server to control refresh responses
- Observe backend status after the reconnect task completes
- Simulate a `set-token` arriving mid-retry

Existing refresh tests only cover pure functions (`refresh_due_in`, `load_oauth_entries_from_db`). No async tests exist for the retry loop, cancellation, or concurrent token delivery.

## Solution: Extract `ReconnectManager`

Pull reconnect-on-startup logic out of main.rs into a testable struct in `rightclaw::mcp::reconnect`.

### Interface

```rust
// crates/rightclaw/src/mcp/reconnect.rs

pub struct ReconnectManager {
    /// In-flight reconnect tasks, keyed by server name.
    in_flight: HashMap<String, CancellationToken>,

    /// Channel to the refresh scheduler for registering new timers.
    refresh_tx: mpsc::Sender<RefreshMessage>,

    /// Agent directory (for SQLite persistence).
    agent_dir: PathBuf,
}
```

Three public methods:

- **`start_reconnect(&mut self, server, backend, oauth_state, token_arc, http_client)`** — cancels any existing in-flight task for this server, creates a new `CancellationToken`, spawns the retry loop. Returns `JoinHandle<()>` (for tests to await).

- **`cancel(&mut self, server: &str)`** — cancels in-flight reconnect for a server. Called by `handle_set_token` when a fresh OAuth token arrives.

- **`cancel_all(&mut self)`** — cancels all in-flight reconnects. For shutdown.

### Retry loop

The spawned task is a standalone async function with all dependencies injected:

```rust
async fn reconnect_task(
    server_name: String,
    backend: Arc<ProxyBackend>,
    oauth_state: OAuthServerState,
    token_arc: Arc<RwLock<Option<String>>>,
    http_client: reqwest::Client,
    agent_dir: PathBuf,
    refresh_tx: mpsc::Sender<RefreshMessage>,
    cancel: CancellationToken,
) -> Result<(), ReconnectError>
```

`do_refresh_cancellable` wraps the existing retry loop but checks for cancellation between retries:

```rust
// Instead of:
tokio::time::sleep(Duration::from_secs(delay)).await;

// Do:
tokio::select! {
    _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
    _ = cancel.cancelled() => return Err(ReconnectError::Cancelled),
}
```

On failure (all retries exhausted), defense-in-depth guard before setting NeedsAuth:

```rust
if backend.status().await != BackendStatus::Connected {
    backend.set_status(BackendStatus::NeedsAuth).await;
}
```

On cancellation, the task exits silently with a debug log. No status change.

### Integration: main.rs

The 65-line inline reconnect block (lines 578-644) is replaced with:

```rust
let mut reconnect_mgr = ReconnectManager::new(refresh_tx.clone(), agent_dir.clone());

for (server_name, backend) in proxies_snapshot {
    if let Some((oauth_state, token_arc)) = oauth_map.get(&server_name) {
        let due_in = refresh_due_in(oauth_state);
        if due_in == Duration::ZERO && oauth_state.refresh_token.is_some() {
            reconnect_mgr.start_reconnect(
                server_name, backend, oauth_state.clone(),
                token_arc.clone(), http_client.clone(),
            );
        } else if due_in == Duration::ZERO {
            backend.set_status(BackendStatus::NeedsAuth).await;
        } else {
            tokio::spawn(async move { backend.connect(http).await });
        }
    } else {
        tokio::spawn(async move { backend.connect(http).await });
    }
}
```

`ReconnectManager` is stored per-agent in `InternalState` as `HashMap<String, tokio::sync::Mutex<ReconnectManager>>`.

### Integration: internal_api.rs

One addition in `handle_set_token`, before the existing reconnect spawn:

```rust
if let Some(mgr) = state.reconnect_managers.get(&req.agent) {
    mgr.lock().await.cancel(&req.server);
}
```

The rest of `handle_set_token` stays the same.

### Integration: refresh.rs

No changes. The scheduler's select loop is self-healing: a queued `NewEntry` from `handle_set_token` overwrites any stale timer on the next loop iteration.

## Tests

All in `reconnect.rs` using `#[tokio::test]`. Mock HTTP server via `wiremock`.

### 1. Happy path — refresh succeeds

`wiremock::MockServer` returns a valid token response. Create a real `ProxyBackend` (won't connect to real MCP — we check token_arc and refresh_tx, not MCP session). Call `reconnect_task()` directly. Assert:

- `token_arc` contains the new access token
- `refresh_tx` received a `NewEntry` message

### 2. Cancellation mid-retry — the actual race

MockServer returns 401 on all requests. Call `reconnect_task()` with a `CancellationToken`. After a short delay, cancel the token. Assert:

- Task returns `Err(Cancelled)`
- Backend status is NOT `NeedsAuth` (stays `Unreachable` — the initial default)

### 3. Defense-in-depth — retries exhausted but backend already Connected

MockServer returns 401 on all requests. Before calling `reconnect_task()`, set backend status to `Connected` (simulating the OAuth callback winning the race). Let all retries fail. Assert:

- Backend status remains `Connected`, not `NeedsAuth`

### What we don't test

- `do_refresh` HTTP retry logic — already works, just gets the cancellation wrapper
- `run_refresh_scheduler` select loop — self-healing, not part of this bug
- Full main.rs startup flow — integration test territory, too much wiring for unit tests

## File changes

| File | Change |
|------|--------|
| `crates/rightclaw/src/mcp/reconnect.rs` | **New.** ReconnectManager, reconnect_task, do_refresh_cancellable, ReconnectError, 3 tests. ~250 lines. |
| `crates/rightclaw/src/mcp/mod.rs` | Add `pub mod reconnect;` |
| `crates/rightclaw-cli/src/main.rs` | Replace inline reconnect block with ReconnectManager calls |
| `crates/rightclaw-cli/src/internal_api.rs` | Add `cancel()` call in `handle_set_token` |
| `crates/rightclaw/Cargo.toml` | Add `wiremock` dev dependency |

## Dependencies

- `tokio-util` for `CancellationToken` — already in workspace (`0.7` with `rt` feature), needs adding to `crates/rightclaw/Cargo.toml`
- `wiremock` for mock HTTP in tests — dev-dependency on `crates/rightclaw/Cargo.toml`
