# OAuth Reconnect Race Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix race where a stale startup reconnect task overwrites a working backend's Connected status with NeedsAuth after an OAuth callback already delivered a fresh token.

**Architecture:** Extract the inline reconnect logic from main.rs into a testable `ReconnectManager` in `rightclaw::mcp::reconnect`. The manager tracks in-flight reconnect tasks via `CancellationToken` per server. When `handle_set_token` delivers a fresh token, it cancels any stale reconnect. Defense-in-depth: the retry loop checks `backend.status()` before setting NeedsAuth.

**Tech Stack:** tokio-util (CancellationToken), wiremock (test mock HTTP), existing rightclaw types (ProxyBackend, OAuthServerState, RefreshMessage)

**Spec:** `docs/superpowers/specs/2026-04-15-oauth-reconnect-race-design.md`

---

### Task 1: Add dependencies

**Files:**
- Modify: `crates/rightclaw/Cargo.toml`
- Modify: `Cargo.toml` (workspace — add wiremock)

- [ ] **Step 1: Add tokio-util and wiremock to rightclaw crate**

In `crates/rightclaw/Cargo.toml`, add to `[dependencies]`:
```toml
tokio-util = { workspace = true }
```

Add to `[dev-dependencies]`:
```toml
wiremock = "0.6"
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/molt/dev/rightclaw && cargo check -p rightclaw`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/Cargo.toml Cargo.lock
git commit -m "chore: add tokio-util and wiremock deps to rightclaw crate"
```

---

### Task 2: Create `reconnect.rs` with error type and `do_refresh_cancellable`

**Files:**
- Create: `crates/rightclaw/src/mcp/reconnect.rs`
- Modify: `crates/rightclaw/src/mcp/mod.rs`

This task implements the cancellable refresh function and its test. The function is a near-copy of `do_refresh` from `refresh.rs` but checks a `CancellationToken` between retry backoff sleeps.

- [ ] **Step 1: Add module declaration**

In `crates/rightclaw/src/mcp/mod.rs`, add after `pub mod refresh;`:
```rust
pub mod reconnect;
```

- [ ] **Step 2: Write the failing test for cancellation**

Create `crates/rightclaw/src/mcp/reconnect.rs`:

```rust
//! Cancellable OAuth reconnect with race-safe status management.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::mcp::proxy::{BackendStatus, ProxyBackend};
use crate::mcp::refresh::{OAuthServerState, RefreshMessage};

/// Errors from reconnect operations.
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    #[error("reconnect cancelled — fresh token arrived")]
    Cancelled,

    #[error("token refresh failed after retries: {0:#}")]
    RefreshFailed(miette::Report),

    #[error("failed to connect after refresh: {0:#}")]
    ConnectFailed(crate::mcp::proxy::ProxyError),

    #[error("failed to persist token: {0:#}")]
    PersistFailed(miette::Report),
}

const MAX_RETRIES: u32 = 3;
const BACKOFFS: [u64; 3] = [30, 60, 120];

/// Attempt token refresh with retries, aborting early if `cancel` fires.
///
/// Identical to `refresh::do_refresh` except backoff sleeps race against
/// the cancellation token, allowing `handle_set_token` to kill stale retries.
pub async fn do_refresh_cancellable(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    max_retries: u32,
    cancel: &CancellationToken,
) -> Result<(OAuthServerState, String), ReconnectError> {
    let refresh_token = entry.refresh_token.as_deref()
        .ok_or_else(|| ReconnectError::RefreshFailed(miette::miette!("no refresh_token")))?;

    for attempt in 0..max_retries {
        // Check cancellation before each attempt.
        if cancel.is_cancelled() {
            return Err(ReconnectError::Cancelled);
        }

        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &entry.client_id),
        ];
        if let Some(ref secret) = entry.client_secret {
            form.push(("client_secret", secret));
        }

        let resp = client
            .post(&entry.token_endpoint)
            .form(&form)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let token_resp: crate::mcp::oauth::TokenResponse = r.json().await
                    .map_err(|e| ReconnectError::RefreshFailed(miette::miette!("failed to parse token response: {e:#}")))?;

                let expires_in = token_resp.expires_in.unwrap_or(3600);
                let has_new_refresh = token_resp.refresh_token.is_some();
                let expires_at = chrono::Utc::now()
                    + chrono::Duration::seconds(expires_in as i64);

                tracing::info!(
                    attempt,
                    expires_in,
                    has_new_refresh,
                    %expires_at,
                    "refresh succeeded",
                );

                let access_token = token_resp.access_token.clone();
                return Ok((OAuthServerState {
                    refresh_token: token_resp.refresh_token.or(entry.refresh_token.clone()),
                    token_endpoint: entry.token_endpoint.clone(),
                    client_id: entry.client_id.clone(),
                    client_secret: entry.client_secret.clone(),
                    expires_at,
                    server_url: entry.server_url.clone(),
                }, access_token));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(attempt, %status, %body, "refresh attempt failed");
            }
            Err(e) => {
                tracing::warn!(attempt, "refresh request error: {e:#}");
            }
        }

        if attempt < max_retries - 1 {
            let delay = BACKOFFS.get(attempt as usize).copied().unwrap_or(120);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
                _ = cancel.cancelled() => {
                    tracing::debug!(attempt, "reconnect cancelled during backoff");
                    return Err(ReconnectError::Cancelled);
                },
            }
        }
    }

    Err(ReconnectError::RefreshFailed(miette::miette!("token refresh failed after {max_retries} attempts")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_aborts_refresh_during_backoff() {
        // Mock server that always returns 401.
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401)
                .set_body_string(r#"{"error":"invalid_grant"}"#))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let entry = OAuthServerState {
            refresh_token: Some("stale-rt".into()),
            token_endpoint: mock.uri(),
            client_id: "cid".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Cancel after first attempt completes (during the 30s backoff sleep).
        // We use pause to advance time instantly.
        tokio::time::pause();

        let handle = tokio::spawn(async move {
            do_refresh_cancellable(&client, &entry, 3, &cancel_clone).await
        });

        // Let first attempt execute and enter backoff sleep.
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;

        // Cancel while sleeping.
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(ReconnectError::Cancelled)),
            "expected Cancelled, got: {result:?}",
        );
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw cancellation_aborts_refresh_during_backoff -- --nocapture`
Expected: PASS — the cancellation token fires during the backoff sleep after the first 401, and the function returns `Cancelled` instead of retrying.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/mcp/mod.rs crates/rightclaw/src/mcp/reconnect.rs
git commit -m "feat(mcp): add do_refresh_cancellable with cancellation test"
```

---

### Task 3: Add `reconnect_task` and `ReconnectManager`

**Files:**
- Modify: `crates/rightclaw/src/mcp/reconnect.rs`

This task adds the full reconnect task function and the manager struct. The reconnect task orchestrates: cancellable refresh → write token → persist to SQLite → send NewEntry → connect backend. On failure, it guards against overwriting a Connected backend.

- [ ] **Step 1: Write the defense-in-depth test (retries exhausted, backend already Connected)**

Add to the `tests` module in `reconnect.rs`:

```rust
    #[tokio::test]
    async fn exhausted_retries_do_not_overwrite_connected_status() {
        // Mock server that always returns 401.
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401)
                .set_body_string(r#"{"error":"invalid_grant"}"#))
            .mount(&mock)
            .await;

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(Some("fresh-token".into())));
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().to_path_buf();

        // Initialize DB schema so persist doesn't fail.
        let mut conn = rusqlite::Connection::open(agent_dir.join("data.db")).unwrap();
        crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        drop(conn);

        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            agent_dir.clone(),
            "https://example.com/mcp".into(),
            token_arc.clone(),
            crate::mcp::proxy::AuthMethod::Bearer,
        ));
        // Simulate OAuth callback having already fixed the backend.
        backend.set_status(BackendStatus::Connected).await;

        let (refresh_tx, mut refresh_rx) = mpsc::channel::<RefreshMessage>(8);
        let cancel = CancellationToken::new();

        let entry = OAuthServerState {
            refresh_token: Some("stale-rt".into()),
            token_endpoint: mock.uri(),
            client_id: "cid".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };

        // Use time::pause to skip the 30s/60s backoff sleeps.
        tokio::time::pause();

        let result = reconnect_task(
            "composio".into(),
            backend.clone(),
            entry,
            token_arc.clone(),
            reqwest::Client::new(),
            agent_dir,
            refresh_tx,
            cancel,
        ).await;

        // Refresh failed — task returns error.
        assert!(result.is_err());

        // But backend status must remain Connected (defense-in-depth guard).
        assert_eq!(backend.status().await, BackendStatus::Connected);

        // No NewEntry should have been sent (refresh failed).
        assert!(refresh_rx.try_recv().is_err());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw exhausted_retries_do_not_overwrite -- --nocapture`
Expected: FAIL — `reconnect_task` doesn't exist yet.

- [ ] **Step 3: Implement `reconnect_task`**

Add above the `#[cfg(test)]` block in `reconnect.rs`:

```rust
/// Execute a single reconnect attempt for one server.
///
/// Orchestrates: cancellable refresh → write token → persist to SQLite →
/// send NewEntry to scheduler → connect backend.
///
/// On failure: sets NeedsAuth only if no one else already fixed the backend.
/// On cancellation: exits silently — no status change.
pub async fn reconnect_task(
    server_name: String,
    backend: Arc<ProxyBackend>,
    oauth_state: OAuthServerState,
    token_arc: Arc<RwLock<Option<String>>>,
    http_client: reqwest::Client,
    agent_dir: PathBuf,
    refresh_tx: mpsc::Sender<RefreshMessage>,
    cancel: CancellationToken,
) -> Result<(), ReconnectError> {
    match do_refresh_cancellable(&http_client, &oauth_state, MAX_RETRIES, &cancel).await {
        Ok((new_state, access_token)) => {
            *token_arc.write().await = Some(access_token.clone());

            // Persist to SQLite.
            let conn = crate::memory::open_connection(&agent_dir)
                .map_err(|e| ReconnectError::PersistFailed(miette::miette!("{e:#}")))?;
            crate::mcp::credentials::db_update_oauth_token(
                &conn,
                &server_name,
                &access_token,
                new_state.refresh_token.as_deref(),
                &new_state.expires_at.to_rfc3339(),
            )
            .map_err(|e| ReconnectError::PersistFailed(miette::miette!("{e:#}")))?;

            // Register with scheduler.
            let _ = refresh_tx.send(RefreshMessage::NewEntry {
                server_name: server_name.clone(),
                state: new_state,
                token: token_arc,
            }).await;

            backend.connect(http_client).await.map_err(ReconnectError::ConnectFailed)?;
            Ok(())
        }
        Err(ReconnectError::Cancelled) => {
            tracing::debug!(server = %server_name, "reconnect cancelled — fresh token arrived");
            Err(ReconnectError::Cancelled)
        }
        Err(e) => {
            tracing::warn!(server = %server_name, "reconnect failed: {e:#}");
            // Defense-in-depth: don't overwrite Connected if someone else fixed it.
            if backend.status().await != BackendStatus::Connected {
                backend.set_status(BackendStatus::NeedsAuth).await;
            }
            Err(e)
        }
    }
}
```

- [ ] **Step 4: Run both tests**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw reconnect -- --nocapture`
Expected: both `cancellation_aborts_refresh_during_backoff` and `exhausted_retries_do_not_overwrite_connected_status` PASS.

- [ ] **Step 5: Add `ReconnectManager` struct**

Add after `reconnect_task` in `reconnect.rs`:

```rust
/// Manages in-flight reconnect tasks with cancellation support.
///
/// Each server gets at most one reconnect task. Starting a new reconnect
/// for a server cancels any existing one. `handle_set_token` calls `cancel()`
/// when a fresh OAuth token arrives, killing stale retries immediately.
pub struct ReconnectManager {
    in_flight: HashMap<String, CancellationToken>,
    refresh_tx: mpsc::Sender<RefreshMessage>,
    agent_dir: PathBuf,
}

impl ReconnectManager {
    pub fn new(refresh_tx: mpsc::Sender<RefreshMessage>, agent_dir: PathBuf) -> Self {
        Self {
            in_flight: HashMap::new(),
            refresh_tx,
            agent_dir,
        }
    }

    /// Spawn a reconnect task for `server_name`.
    ///
    /// Cancels any existing in-flight reconnect for the same server.
    /// Returns `JoinHandle` so callers (tests) can await completion.
    pub fn start_reconnect(
        &mut self,
        server_name: String,
        backend: Arc<ProxyBackend>,
        oauth_state: OAuthServerState,
        token_arc: Arc<RwLock<Option<String>>>,
        http_client: reqwest::Client,
    ) -> tokio::task::JoinHandle<Result<(), ReconnectError>> {
        // Cancel existing reconnect for this server.
        if let Some(old) = self.in_flight.remove(&server_name) {
            old.cancel();
            tracing::debug!(server = %server_name, "cancelled previous reconnect");
        }

        let cancel = CancellationToken::new();
        self.in_flight.insert(server_name.clone(), cancel.clone());

        let agent_dir = self.agent_dir.clone();
        let refresh_tx = self.refresh_tx.clone();
        let sn = server_name.clone();

        tokio::spawn(async move {
            let result = reconnect_task(
                sn, backend, oauth_state, token_arc,
                http_client, agent_dir, refresh_tx, cancel,
            ).await;
            if let Err(ref e) = result {
                match e {
                    ReconnectError::Cancelled => {} // already logged inside reconnect_task
                    _ => {} // already logged inside reconnect_task
                }
            }
            result
        })
    }

    /// Cancel in-flight reconnect for a server.
    ///
    /// Called by `handle_set_token` when a fresh OAuth token arrives.
    pub fn cancel(&mut self, server_name: &str) {
        if let Some(token) = self.in_flight.remove(server_name) {
            token.cancel();
            tracing::info!(server = %server_name, "cancelled stale reconnect — fresh token arrived");
        }
    }

    /// Cancel all in-flight reconnects. For shutdown.
    pub fn cancel_all(&mut self) {
        for (server, token) in self.in_flight.drain() {
            token.cancel();
            tracing::debug!(server = %server, "cancelled reconnect on shutdown");
        }
    }
}
```

- [ ] **Step 6: Run all reconnect tests**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw reconnect -- --nocapture`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/mcp/reconnect.rs
git commit -m "feat(mcp): add reconnect_task and ReconnectManager with defense-in-depth guard"
```

---

### Task 4: Add the happy-path test

**Files:**
- Modify: `crates/rightclaw/src/mcp/reconnect.rs`

- [ ] **Step 1: Write the happy-path test**

Add to the `tests` module in `reconnect.rs`:

```rust
    #[tokio::test]
    async fn successful_refresh_writes_token_and_sends_new_entry() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

        let mock = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({
                    "access_token": "new-access-tok",
                    "refresh_token": "new-refresh-tok",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                })))
            .mount(&mock)
            .await;

        let token_arc: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(Some("old-token".into())));
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().to_path_buf();

        // Initialize DB with schema and a matching mcp_servers row.
        let mut conn = rusqlite::Connection::open(agent_dir.join("data.db")).unwrap();
        crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type) VALUES ('composio', 'https://example.com/mcp', 'oauth')",
            [],
        ).unwrap();
        drop(conn);

        // ProxyBackend won't actually connect (no real MCP server),
        // so reconnect_task will fail at backend.connect(). That's OK —
        // we're testing that token + NewEntry are set before connect.
        let backend = Arc::new(ProxyBackend::new(
            "composio".into(),
            agent_dir.clone(),
            "https://not-a-real-mcp-server.invalid/mcp".into(),
            token_arc.clone(),
            crate::mcp::proxy::AuthMethod::Bearer,
        ));

        let (refresh_tx, mut refresh_rx) = mpsc::channel::<RefreshMessage>(8);
        let cancel = CancellationToken::new();

        let entry = OAuthServerState {
            refresh_token: Some("old-rt".into()),
            token_endpoint: mock.uri(),
            client_id: "cid".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };

        let _result = reconnect_task(
            "composio".into(),
            backend,
            entry,
            token_arc.clone(),
            reqwest::Client::new(),
            agent_dir,
            refresh_tx,
            cancel,
        ).await;
        // Result may be ConnectFailed (no real MCP server) — that's fine.

        // Token was updated in-memory.
        let token = token_arc.read().await;
        assert_eq!(token.as_deref(), Some("new-access-tok"));

        // NewEntry was sent to refresh scheduler.
        let msg = refresh_rx.try_recv().expect("expected NewEntry message");
        match msg {
            RefreshMessage::NewEntry { server_name, state, .. } => {
                assert_eq!(server_name, "composio");
                assert_eq!(state.refresh_token.as_deref(), Some("new-refresh-tok"));
            }
            _ => panic!("expected NewEntry, got: {msg:?}"),
        }
    }
```

- [ ] **Step 2: Run the test**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw successful_refresh_writes_token -- --nocapture`
Expected: PASS — refresh succeeds, token is written, NewEntry is sent, connect fails (no real MCP) but we don't assert on that.

- [ ] **Step 3: Run all reconnect tests together**

Run: `cd /Users/molt/dev/rightclaw && cargo test -p rightclaw reconnect -- --nocapture`
Expected: all 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/mcp/reconnect.rs
git commit -m "test(mcp): add happy-path reconnect test"
```

---

### Task 5: Wire ReconnectManager into main.rs

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

Replace the inline reconnect block (lines 578-691) with `ReconnectManager` calls. The non-OAuth and valid-token paths stay as simple `tokio::spawn` — only the expired-token-with-refresh path uses the manager.

- [ ] **Step 1: Replace the reconnect block**

In `crates/rightclaw-cli/src/main.rs`, replace the block from `// Spawn background reconnect tasks (fire-and-forget).` (line 578) through the end of the `for (server_name, backend) in proxies_snapshot` loop (line 691, just before `refresh_senders_map.insert`) with:

```rust
                // Spawn background reconnect tasks — cancellable via ReconnectManager.
                let mut reconnect_mgr = rightclaw::mcp::reconnect::ReconnectManager::new(
                    refresh_tx.clone(),
                    agent_dir.clone(),
                );

                for (server_name, backend) in proxies_snapshot {
                    let http = http_client.clone();
                    let agent_name_owned = agent_name.clone();

                    if let Some((oauth_state, token_arc)) = oauth_map.get(&server_name) {
                        // OAuth server — check token expiry before connecting.
                        let due_in = rightclaw::mcp::refresh::refresh_due_in(oauth_state);
                        tracing::info!(
                            agent = agent_name.as_str(),
                            server = server_name.as_str(),
                            due_secs = due_in.as_secs(),
                            expires_at = %oauth_state.expires_at,
                            has_refresh_token = oauth_state.refresh_token.is_some(),
                            "reconnect: checking OAuth token",
                        );
                        if due_in == std::time::Duration::ZERO {
                            // Token expired — try refresh or mark NeedsAuth.
                            if oauth_state.refresh_token.is_some() {
                                reconnect_mgr.start_reconnect(
                                    server_name,
                                    backend,
                                    oauth_state.clone(),
                                    token_arc.clone(),
                                    http,
                                );
                            } else {
                                // No refresh_token — cannot refresh.
                                let b = backend.clone();
                                tokio::spawn(async move {
                                    b.set_status(
                                        rightclaw::mcp::proxy::BackendStatus::NeedsAuth,
                                    ).await;
                                });
                            }
                        } else {
                            // Token still valid — just connect.
                            tokio::spawn(async move {
                                if let Err(e) = backend.connect(http).await {
                                    tracing::warn!(
                                        agent = agent_name_owned.as_str(),
                                        server = server_name.as_str(),
                                        "reconnect failed: {e:#}",
                                    );
                                }
                            });
                        }
                    } else if oauth_server_names.contains(&server_name) {
                        // OAuth server with incomplete DB fields — cannot refresh.
                        tracing::warn!(
                            agent = agent_name.as_str(),
                            server = server_name.as_str(),
                            "OAuth server missing token_endpoint/client_id/expires_at — marking NeedsAuth",
                        );
                        let b = backend.clone();
                        tokio::spawn(async move {
                            b.set_status(
                                rightclaw::mcp::proxy::BackendStatus::NeedsAuth,
                            ).await;
                        });
                    } else {
                        // Non-OAuth server — just connect.
                        tokio::spawn(async move {
                            if let Err(e) = backend.connect(http).await {
                                tracing::warn!(
                                    agent = agent_name_owned.as_str(),
                                    server = server_name.as_str(),
                                    "reconnect failed: {e:#}",
                                );
                            }
                        });
                    }
                }

                reconnect_managers.insert(agent_name.clone(), tokio::sync::Mutex::new(reconnect_mgr));
```

- [ ] **Step 2: Add `reconnect_managers` declaration and pass to aggregator**

Before the `for (agent_name, agent_dir) in ...` loop (around line 440), add:

```rust
            let mut reconnect_managers: std::collections::HashMap<
                String,
                tokio::sync::Mutex<rightclaw::mcp::reconnect::ReconnectManager>,
            > = std::collections::HashMap::new();
```

After the for loop ends (around the line with `let refresh_senders: ...`), wrap `reconnect_managers` in an `Arc`:

```rust
            let reconnect_managers: std::sync::Arc<
                std::collections::HashMap<
                    String,
                    tokio::sync::Mutex<rightclaw::mcp::reconnect::ReconnectManager>,
                >,
            > = std::sync::Arc::new(reconnect_managers);
```

Update `run_aggregator_http` call to pass `reconnect_managers`:

```rust
            aggregator::run_aggregator_http(
                port, token_map, dispatcher, agents_dir, home, refresh_senders, reconnect_managers,
            ).await
```

- [ ] **Step 3: Update `run_aggregator_http` signature and `internal_router` call**

In `crates/rightclaw-cli/src/aggregator.rs`, update `run_aggregator_http` to accept the new parameter:

```rust
pub(crate) type ReconnectManagers = Arc<
    std::collections::HashMap<
        String,
        tokio::sync::Mutex<rightclaw::mcp::reconnect::ReconnectManager>,
    >,
>;

pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
) -> miette::Result<()> {
```

Pass `reconnect_managers` to `internal_router`:

```rust
    let internal_app = crate::internal_api::internal_router(dispatcher, refresh_senders, reconnect_managers);
```

- [ ] **Step 4: Update `InternalState` and `internal_router`**

In `crates/rightclaw-cli/src/internal_api.rs`, add to `InternalState`:

```rust
use crate::aggregator::ReconnectManagers;

#[derive(Clone)]
pub(crate) struct InternalState {
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
}
```

Update `internal_router`:

```rust
pub(crate) fn internal_router(
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
) -> Router {
    let state = InternalState { dispatcher, refresh_senders, reconnect_managers };
    // ... rest unchanged
}
```

Update `make_test_router` in tests to pass a default:

```rust
    fn make_test_router(tmp: &std::path::Path) -> Router {
        let dispatcher = make_test_dispatcher(tmp);
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());
        internal_router(dispatcher, refresh_senders, reconnect_managers)
    }
```

- [ ] **Step 5: Verify it compiles**

Run: `cd /Users/molt/dev/rightclaw && cargo check --workspace`
Expected: compiles with no errors.

- [ ] **Step 6: Run all tests**

Run: `cd /Users/molt/dev/rightclaw && cargo test --workspace`
Expected: all existing tests plus the 3 new reconnect tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/src/aggregator.rs crates/rightclaw-cli/src/internal_api.rs
git commit -m "refactor(mcp): replace inline reconnect with ReconnectManager in main.rs"
```

---

### Task 6: Wire cancellation into `handle_set_token`

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Add cancellation call**

In `handle_set_token` in `internal_api.rs`, add immediately before the `// Reconnect in background with the new token.` comment (around line 427):

```rust
    // Cancel stale reconnect if one is running for this server.
    if let Some(mgr) = state.reconnect_managers.get(&req.agent) {
        mgr.lock().await.cancel(&req.server);
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/molt/dev/rightclaw && cargo check --workspace`
Expected: compiles with no errors.

- [ ] **Step 3: Run all tests**

Run: `cd /Users/molt/dev/rightclaw && cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs
git commit -m "fix(mcp): cancel stale reconnect in handle_set_token before delivering fresh token"
```

---

### Task 7: Final workspace build

**Files:** None — verification only.

- [ ] **Step 1: Full debug build**

Run: `cd /Users/molt/dev/rightclaw && cargo build --workspace`
Expected: compiles with no errors.

- [ ] **Step 2: Full test suite**

Run: `cd /Users/molt/dev/rightclaw && cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 3: Clippy**

Run: `cd /Users/molt/dev/rightclaw && cargo clippy --workspace`
Expected: no warnings from project code (dependency warnings OK).
