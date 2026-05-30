# MCP Health Reconciler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make external MCP `BackendStatus` reflect upstream reality (Connected↔Unreachable, plus demote-to-NeedsAuth on auth probe) via a per-agent background reconciler, so the dashboard and the agent both see accurate status without waiting for a tool call.

**Architecture:** A new `right-mcp/src/health.rs` module spawns one `run_health_reconciler` task per agent (sibling of the refresh scheduler in `main.rs`). It periodically probes each `ProxyBackend` on an adaptive cadence (Connected 120s, Unreachable 20s, NeedsAuth never), using a new lightweight `ProxyBackend::probe_live()`. Status-transition policy (3-strike outage debounce, immediate auth flip, instant reset) is an extracted pure function so it is exhaustively unit-testable without I/O.

**Tech Stack:** Rust 2024, tokio (`test-util` for `time::pause`), rmcp 1.7 (client+server), wiremock, existing `ProxyBackend`/`BackendStatus` in `crates/right-mcp/src/proxy.rs`.

**Spec:** `docs/superpowers/specs/2026-05-31-mcp-health-reconciler-design.md`

---

## File Structure

- **Modify** `crates/right-mcp/src/proxy.rs` — add `ProbeOutcome` enum, `classify_probe_error()` (pure), `ProxyBackend::probe_live()`, and a test-only successful-connect helper.
- **Create** `crates/right-mcp/src/health.rs` — `run_health_reconciler()`, the pure decision function `decide_connected()`, `cadence_for()`, and cadence/strike consts.
- **Modify** `crates/right-mcp/src/lib.rs` — add `pub mod health;`.
- **Modify** `crates/right/src/main.rs` — spawn `run_health_reconciler` per agent at the existing startup block (~line 1250, next to `run_refresh_scheduler`).

No frontend changes — `McpView` already renders whatever status string it receives.

---

## Baseline (run once at worktree start)

- [ ] **Step 0: Baseline test run**

Run: `devenv shell -- cargo test -p right-mcp`
Expected: PASS (record any pre-existing failures; none expected). This is the baseline before any change.

---

## Task 1: Pure probe-error classification

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs` (add enum + pure fn near `is_upstream_auth_error`, ~line 75)
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/right-mcp/src/proxy.rs`:

```rust
#[test]
fn classify_probe_error_maps_auth_required_to_authrequired() {
    let msg = "Transport send error: ... error: Auth required";
    assert!(matches!(classify_probe_error(msg), ProbeOutcome::AuthRequired));
}

#[test]
fn classify_probe_error_maps_other_to_dead() {
    assert!(matches!(classify_probe_error("connection refused"), ProbeOutcome::Dead(_)));
    assert!(matches!(classify_probe_error("HTTP 502 Bad Gateway"), ProbeOutcome::Dead(_)));
}

#[test]
fn probe_outcome_dead_preserves_detail() {
    match classify_probe_error("HTTP 502 Bad Gateway") {
        ProbeOutcome::Dead(d) => assert!(d.contains("502")),
        other => panic!("expected Dead, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-mcp classify_probe_error`
Expected: FAIL — `cannot find function classify_probe_error` / `cannot find type ProbeOutcome`.

- [ ] **Step 3: Write minimal implementation**

In `crates/right-mcp/src/proxy.rs`, immediately after `is_upstream_auth_error` (line ~74):

```rust
/// Outcome of a lightweight liveness probe against a backend's live session.
#[derive(Debug)]
pub enum ProbeOutcome {
    /// Session responded; tools listed successfully.
    Alive,
    /// Upstream reported auth-required (`"Auth required"`).
    AuthRequired,
    /// Any other failure (transport, 5xx, timeout, no session). Carries detail.
    Dead(String),
}

/// Classify a probe error string into an outcome. Pure — no I/O.
pub fn classify_probe_error(msg: &str) -> ProbeOutcome {
    if is_upstream_auth_error(msg) {
        ProbeOutcome::AuthRequired
    } else {
        ProbeOutcome::Dead(msg.to_owned())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-mcp classify_probe_error probe_outcome_dead`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/proxy.rs
git commit -m "feat(mcp): add ProbeOutcome and pure probe-error classifier"
```

---

## Task 2: `probe_live()` — no-session path

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs` (add method in `impl ProxyBackend`, near `status()` ~line 516)
- Test: same file `tests` module

This task covers the part of `probe_live` that needs no live server: when there is no active session, return `Dead` and do not touch status. The `Alive` (real-server) path is Task 4.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn probe_live_no_session_is_dead_and_keeps_status() {
    let tmp = tempfile::tempdir().unwrap();
    let token = Arc::new(RwLock::new(None));
    let backend = ProxyBackend::new(
        "composio".into(),
        tmp.path().to_path_buf(),
        "http://localhost:9999/mcp".into(),
        token,
        AuthMethod::default(),
    );
    // Pretend it was Connected but the session is actually absent.
    backend.set_status(BackendStatus::Connected).await;

    let outcome = backend.probe_live().await;

    assert!(matches!(outcome, ProbeOutcome::Dead(_)), "no session must be Dead");
    // probe_live must NOT mutate status — the reconciler owns that decision.
    assert_eq!(backend.status().await, BackendStatus::Connected);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-mcp probe_live_no_session`
Expected: FAIL — `no method named probe_live`.

- [ ] **Step 3: Write minimal implementation**

In `impl ProxyBackend` (after `status()`, before `set_status()`), add:

```rust
/// Lightweight liveness probe against the live session.
///
/// Lists tools on the existing rmcp session; on success refreshes
/// `cached_tools`. Returns the outcome and does NOT mutate `status` — the
/// health reconciler's debounce owns the flip decision.
pub async fn probe_live(&self) -> ProbeOutcome {
    let client_guard = self.client.read().await;
    let Some(client) = client_guard.as_ref() else {
        return ProbeOutcome::Dead("no active session".into());
    };
    match client.peer().list_all_tools().await {
        Ok(tools) => {
            let filtered: Vec<Tool> =
                tools.into_iter().filter(|t| !t.name.contains("__")).collect();
            *self.cached_tools.write().await = filtered;
            ProbeOutcome::Alive
        }
        Err(e) => classify_probe_error(&format!("{e:#}")),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-mcp probe_live_no_session`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/proxy.rs
git commit -m "feat(mcp): add ProxyBackend::probe_live (no-session path)"
```

---

## Task 3: Pure status-decision and cadence functions

**Files:**
- Create: `crates/right-mcp/src/health.rs`
- Modify: `crates/right-mcp/src/lib.rs` (add `pub mod health;`)
- Test: `crates/right-mcp/src/health.rs` `tests` module

This is the heart of the state machine, extracted as pure functions so the debounce/cadence/auth-flip logic is exhaustively testable with zero I/O.

- [ ] **Step 1: Create the module and register it**

Create `crates/right-mcp/src/health.rs` with:

```rust
//! Periodic health reconciler: keeps external MCP `BackendStatus` honest on the
//! Connected↔Unreachable axis (and demotes to NeedsAuth when a probe reveals
//! auth death). See docs/superpowers/specs/2026-05-31-mcp-health-reconciler-design.md.

use std::time::Duration;

use crate::proxy::{BackendStatus, ProbeOutcome};

/// Probe cadence for a healthy backend (light backstop; the tool-call event
/// path catches death between ticks).
pub(crate) const CONNECTED_CADENCE: Duration = Duration::from_secs(120);
/// Probe cadence for a down backend (the only path that detects recovery).
pub(crate) const UNREACHABLE_CADENCE: Duration = Duration::from_secs(20);
/// Consecutive Dead probes required before flipping Connected → Unreachable.
pub(crate) const MAX_STRIKES: u32 = 3;
/// Per-probe timeout — a black-holed connection must not wedge a tick.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Decision for a `Connected` backend after a probe, given its prior strike count.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectedDecision {
    /// Stay Connected; carry this strike count forward.
    Stay { strikes: u32 },
    /// Flip to Unreachable (strike budget exhausted).
    Unreachable,
    /// Flip to NeedsAuth (probe revealed auth death; debounce-exempt).
    NeedsAuth,
}

/// Pure debounce policy for a Connected backend. `strikes` is the count BEFORE
/// this probe. A Dead probe increments; reaching `MAX_STRIKES` flips Unreachable.
/// Alive resets to 0; AuthRequired flips immediately.
pub(crate) fn decide_connected(strikes: u32, outcome: &ProbeOutcome) -> ConnectedDecision {
    match outcome {
        ProbeOutcome::Alive => ConnectedDecision::Stay { strikes: 0 },
        ProbeOutcome::AuthRequired => ConnectedDecision::NeedsAuth,
        ProbeOutcome::Dead(_) => {
            let next = strikes + 1;
            if next >= MAX_STRIKES {
                ConnectedDecision::Unreachable
            } else {
                ConnectedDecision::Stay { strikes: next }
            }
        }
    }
}

/// Cadence for the next probe of a backend in `status`. `None` = never probe
/// (NeedsAuth — owned by refresh/reconnect).
pub(crate) fn cadence_for(status: BackendStatus) -> Option<Duration> {
    match status {
        BackendStatus::Connected => Some(CONNECTED_CADENCE),
        BackendStatus::Unreachable => Some(UNREACHABLE_CADENCE),
        BackendStatus::NeedsAuth => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alive_resets_strikes() {
        assert_eq!(
            decide_connected(2, &ProbeOutcome::Alive),
            ConnectedDecision::Stay { strikes: 0 }
        );
    }

    #[test]
    fn auth_required_flips_immediately_regardless_of_strikes() {
        assert_eq!(decide_connected(0, &ProbeOutcome::AuthRequired), ConnectedDecision::NeedsAuth);
        assert_eq!(decide_connected(2, &ProbeOutcome::AuthRequired), ConnectedDecision::NeedsAuth);
    }

    #[test]
    fn dead_increments_below_threshold() {
        assert_eq!(
            decide_connected(0, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 1 }
        );
        assert_eq!(
            decide_connected(1, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 2 }
        );
    }

    #[test]
    fn dead_flips_on_third_strike_not_second() {
        // strikes=1 (so this is the 2nd Dead) → still Stay
        assert_eq!(
            decide_connected(1, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Stay { strikes: 2 }
        );
        // strikes=2 (so this is the 3rd Dead) → flip
        assert_eq!(
            decide_connected(2, &ProbeOutcome::Dead("x".into())),
            ConnectedDecision::Unreachable
        );
    }

    #[test]
    fn cadence_never_for_needs_auth() {
        assert_eq!(cadence_for(BackendStatus::NeedsAuth), None);
        assert_eq!(cadence_for(BackendStatus::Connected), Some(CONNECTED_CADENCE));
        assert_eq!(cadence_for(BackendStatus::Unreachable), Some(UNREACHABLE_CADENCE));
    }
}
```

Add to `crates/right-mcp/src/lib.rs` after `pub mod detect;` (keep alphabetical-ish order; place after `pub mod credentials;`):

```rust
pub mod health;
```

- [ ] **Step 2: Run tests to verify they pass (no red phase needed — pure code written with tests together)**

Run: `devenv shell -- cargo test -p right-mcp health::`
Expected: PASS (5 tests). If any fail, fix the function, not the test.

- [ ] **Step 3: Verify the off-by-one explicitly**

Confirm `dead_flips_on_third_strike_not_second` passed. This is the debounce contract: with `MAX_STRIKES = 3`, the flip happens when the prior count is 2 (the third consecutive Dead), never on the second.

- [ ] **Step 4: Commit**

```bash
git add crates/right-mcp/src/health.rs crates/right-mcp/src/lib.rs
git commit -m "feat(mcp): pure status-decision and cadence policy for health reconciler"
```

---

## Task 4: Live-server test harness + `probe_live` Alive path

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs` (`tests` module — add a minimal in-test rmcp server helper and the Alive test)

> **VERIFY-API GATE (project convention "Domain research before implementation"):** There is currently no passing test in the codebase that performs a *successful* `ProxyBackend::connect()`. Before writing the helper below, confirm the rmcp 1.7 server-side API by consulting context7 (`rmcp` crate) and the existing server usage in `crates/right/src/aggregator.rs:519-577` (the `ServerHandler` impl) and `:727` (`StreamableHttpService::new`). The skeleton below mirrors those proven call sites; adjust type/method names to the exact rmcp 1.7 surface if they differ. Acceptance is behavioral (the test below passes), not textual.

- [ ] **Step 1: Write the failing test (with helper)**

Add to the `tests` module in `crates/right-mcp/src/proxy.rs`. The helper stands up a minimal MCP server exposing exactly two tools on a loopback port, then a `ProxyBackend` connects to it.

```rust
// --- minimal in-test MCP server exposing two tools ---
// Mirrors the StreamableHttpService wiring in crates/right/src/aggregator.rs.
// Adjust to the exact rmcp 1.7 ServerHandler surface (see VERIFY-API GATE).
use rmcp::handler::server::ServerHandler;
use rmcp::model::{ListToolsResult, PaginatedRequestParam, ServerCapabilities, ServerInfo, Tool};
use rmcp::service::{RequestContext, RoleServer};

#[derive(Clone)]
struct TwoToolServer;

impl ServerHandler for TwoToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
    async fn list_tools(
        &self,
        _req: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let mk = |n: &str| Tool::new(n.to_string(), "test tool", std::sync::Arc::new(
            serde_json::from_value(serde_json::json!({"type":"object"})).unwrap()
        ));
        Ok(ListToolsResult { tools: vec![mk("alpha"), mk("beta")], next_cursor: None })
    }
}

/// Bind the TwoToolServer on a loopback port; return its `http://127.0.0.1:<port>/mcp` URL.
/// Uses rmcp's StreamableHttpService exactly like run_aggregator_http.
async fn serve_two_tool_server() -> (tokio::task::JoinHandle<()>, String) {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, StreamableHttpServerConfig,
        session::local::LocalSessionManager,
    };
    setup_crypto();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = StreamableHttpServerConfig { allowed_hosts: None, ..Default::default() };
    let service = StreamableHttpService::new(
        || Ok(TwoToolServer),
        std::sync::Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (handle, format!("http://127.0.0.1:{port}/mcp"))
}

#[tokio::test]
async fn probe_live_alive_refreshes_tool_cache() {
    let (_srv, url) = serve_two_tool_server().await;
    let tmp = tempfile::tempdir().unwrap();
    // connect() writes instructions to SQLite — needs an initialized DB.
    let _ = right_db::open_connection(tmp.path(), true).await.unwrap();
    let token = Arc::new(RwLock::new(None));
    let backend = ProxyBackend::new(
        "twotool".into(),
        tmp.path().to_path_buf(),
        url,
        token,
        AuthMethod::default(),
    );

    backend.connect(reqwest::Client::new()).await.expect("connect should succeed");
    assert_eq!(backend.status().await, BackendStatus::Connected);
    assert_eq!(backend.tools().await.len(), 2);

    // Clear the cache to prove probe_live repopulates it.
    *backend.cached_tools.write().await = Vec::new();
    let outcome = backend.probe_live().await;

    assert!(matches!(outcome, ProbeOutcome::Alive));
    assert_eq!(backend.tools().await.len(), 2, "probe_live must refresh the cache");
    // probe_live does not write status.
    assert_eq!(backend.status().await, BackendStatus::Connected);
}
```

If `axum` is not already a dev-dependency of `right-mcp`, add it:

Run: `cd crates/right-mcp && cargo add --dev axum` then revert if the rmcp service mounts without axum (some rmcp versions provide their own hyper service — prefer the pattern that matches `aggregator.rs`).

- [ ] **Step 2: Run test to verify it fails (then iterate to green)**

Run: `devenv shell -- cargo test -p right-mcp probe_live_alive`
Expected initially: FAIL (compile error or connect failure) until the rmcp server API is matched per the VERIFY-API GATE. Iterate on the helper until it compiles and the assertions pass. Do NOT weaken the assertions.

- [ ] **Step 3: Confirm green**

Run: `devenv shell -- cargo test -p right-mcp probe_live_alive`
Expected: PASS — `connect()` reaches `Connected` with 2 tools, and `probe_live()` returns `Alive` and repopulates the cache to 2.

- [ ] **Step 4: Commit**

```bash
git add crates/right-mcp/src/proxy.rs crates/right-mcp/Cargo.toml
git commit -m "test(mcp): live-server harness; probe_live Alive refreshes tool cache"
```

---

## Task 5: The reconciler loop

**Files:**
- Modify: `crates/right-mcp/src/health.rs` (add `run_health_reconciler` + tests)

The loop maintains per-backend strike counts and next-due instants, sleeps until the earliest due, probes all due backends concurrently (lock dropped before probing), applies decisions, and reschedules by cadence.

- [ ] **Step 1: Write the failing recovery test (the May-27 regression anchor)**

Add to `health.rs` `tests`:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::proxy::{AuthMethod, ProxyBackend};

type Proxies = Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>;

fn backend_at(status: BackendStatus, url: String) -> Arc<ProxyBackend> {
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let b = Arc::new(ProxyBackend::new(
        "composio".into(),
        tmp.path().to_path_buf(),
        url,
        Arc::new(RwLock::new(None)),
        AuthMethod::default(),
    ));
    // status set by caller via a blocking handle below
    let _ = status;
    b
}

#[tokio::test]
async fn unreachable_backend_recovers_to_connected_on_probe() {
    // Stand up a real two-tool server (see proxy.rs helper, made pub(crate) for reuse),
    // start the backend Unreachable, run ONE reconcile pass, and assert it connects.
    // NOTE: reuse crate::proxy::tests::serve_two_tool_server by promoting it to a
    // pub(crate) test helper, OR inline an equivalent server here.
    // ... (see Step 3 for the helper-sharing approach)
}
```

> **Helper sharing:** promote `serve_two_tool_server` and `TwoToolServer` from Task 4 into a `pub(crate)` test-support location (e.g. a `#[cfg(test)] pub(crate) mod test_server;` in `proxy.rs` or a small `crates/right-mcp/src/test_server.rs` gated on `#[cfg(test)]`) so both `proxy.rs` and `health.rs` tests use one server. Do this refactor as the first step here and re-run Task 4's test to confirm it still passes.

- [ ] **Step 2: Implement `run_health_reconciler`**

In `health.rs` (above the `tests` module):

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::proxy::ProxyBackend;

/// Per-agent health reconciler. Runs for process lifetime. Holds only a clone of
/// the shared `proxies` map (also held by the dispatcher), so it lives as long as
/// the process — matching the refresh scheduler's fire-and-forget model.
pub async fn run_health_reconciler(
    proxies: Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    http_client: reqwest::Client,
) {
    let mut strikes: HashMap<String, u32> = HashMap::new();
    let mut next_due: HashMap<String, tokio::time::Instant> = HashMap::new();

    loop {
        // Snapshot handles under a brief read lock; never probe while holding it.
        let snapshot: Vec<(String, Arc<ProxyBackend>)> = {
            let guard = proxies.read().await;
            guard.iter().map(|(n, b)| (n.clone(), b.clone())).collect()
        };

        let now = tokio::time::Instant::now();
        // Determine which backends are due (or newly seen → due now).
        let mut due: Vec<(String, Arc<ProxyBackend>)> = Vec::new();
        for (name, backend) in &snapshot {
            let status = backend.status().await;
            if cadence_for(status).is_none() {
                // NeedsAuth — never probe; clear any schedule/strikes.
                next_due.remove(name);
                strikes.remove(name);
                continue;
            }
            let is_due = next_due.get(name).map(|t| *t <= now).unwrap_or(true);
            if is_due {
                due.push((name.clone(), backend.clone()));
            }
        }

        // Probe all due backends concurrently (lock already dropped).
        let results = futures::future::join_all(due.into_iter().map(|(name, backend)| {
            let http = http_client.clone();
            async move {
                let status = backend.status().await;
                let new_status = match status {
                    BackendStatus::Connected => {
                        let outcome = match tokio::time::timeout(PROBE_TIMEOUT, backend.probe_live()).await {
                            Ok(o) => o,
                            Err(_) => ProbeOutcome::Dead("probe timeout".into()),
                        };
                        outcome_to_status(&name, status, outcome, &mut 0) // placeholder; real strike applied below
                    }
                    BackendStatus::Unreachable => {
                        // Attempt a full reconnect; connect() sets status itself.
                        let _ = tokio::time::timeout(PROBE_TIMEOUT, backend.connect(http)).await;
                        backend.status().await
                    }
                    BackendStatus::NeedsAuth => status,
                };
                (name, new_status)
            }
        }))
        .await;
        // NOTE: strike state must be threaded per-name. The closure above cannot
        // borrow `strikes` mutably across concurrent tasks. Implement the Connected
        // branch by reading strikes BEFORE the join, deciding AFTER, like this:
        //   (see corrected implementation in Step 2b)
        let _ = results;

        // Reschedule every known backend by its (possibly new) status.
        // ... see Step 2b for the corrected single-pass implementation.
        # break; // placeholder to be removed
    }
}
```

> The sketch above intentionally exposes the concurrency hazard (you cannot mutably share `strikes` across `join_all` tasks). Implement the **corrected** version in Step 2b.

- [ ] **Step 2b: Corrected implementation (use this)**

Replace the loop body with: read each due backend's strike count, run probes concurrently returning `(name, status, ProbeOutcome)` for Connected and `(name, status_after_connect)` for Unreachable, then apply `decide_connected` serially after the join (single-threaded mutation of `strikes`/`next_due`):

```rust
pub async fn run_health_reconciler(
    proxies: Arc<RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    http_client: reqwest::Client,
) {
    let mut strikes: HashMap<String, u32> = HashMap::new();
    let mut next_due: HashMap<String, tokio::time::Instant> = HashMap::new();

    loop {
        let snapshot: Vec<(String, Arc<ProxyBackend>)> = {
            let guard = proxies.read().await;
            guard.iter().map(|(n, b)| (n.clone(), b.clone())).collect()
        };
        let now = tokio::time::Instant::now();

        // Build the due set; NeedsAuth backends are pruned from schedule/strikes.
        let mut due = Vec::new();
        for (name, backend) in &snapshot {
            let status = backend.status().await;
            if cadence_for(status).is_none() {
                next_due.remove(name);
                strikes.remove(name);
                continue;
            }
            if next_due.get(name).map(|t| *t <= now).unwrap_or(true) {
                due.push((name.clone(), backend.clone(), status));
            }
        }

        // Concurrent probes. Returns the post-probe outcome/status per backend.
        enum Probed { Connected(ProbeOutcome), Settled(BackendStatus) }
        let probed = futures::future::join_all(due.into_iter().map(|(name, backend, status)| {
            let http = http_client.clone();
            async move {
                let p = match status {
                    BackendStatus::Connected => {
                        let o = match tokio::time::timeout(PROBE_TIMEOUT, backend.probe_live()).await {
                            Ok(o) => o,
                            Err(_) => ProbeOutcome::Dead("probe timeout".into()),
                        };
                        Probed::Connected(o)
                    }
                    BackendStatus::Unreachable => {
                        let _ = tokio::time::timeout(PROBE_TIMEOUT, backend.connect(http)).await;
                        Probed::Settled(backend.status().await)
                    }
                    BackendStatus::NeedsAuth => Probed::Settled(status),
                };
                (name, backend, p)
            }
        })).await;

        // Apply decisions serially (single mutator of strikes/next_due).
        for (name, backend, p) in probed {
            let new_status = match p {
                Probed::Connected(outcome) => {
                    let prev = *strikes.get(&name).unwrap_or(&0);
                    match decide_connected(prev, &outcome) {
                        ConnectedDecision::Stay { strikes: s } => {
                            strikes.insert(name.clone(), s);
                            if !matches!(outcome, ProbeOutcome::Alive) {
                                tracing::debug!(server = %name, strikes = s, max = MAX_STRIKES, "health: dead probe");
                            }
                            BackendStatus::Connected
                        }
                        ConnectedDecision::Unreachable => {
                            strikes.remove(&name);
                            backend.set_status(BackendStatus::Unreachable).await;
                            tracing::warn!(server = %name, "health: connected → unreachable (strike {MAX_STRIKES}/{MAX_STRIKES})");
                            BackendStatus::Unreachable
                        }
                        ConnectedDecision::NeedsAuth => {
                            strikes.remove(&name);
                            backend.set_status(BackendStatus::NeedsAuth).await;
                            tracing::warn!(server = %name, "health: connected → needs_auth (auth probe)");
                            BackendStatus::NeedsAuth
                        }
                    }
                }
                Probed::Settled(s) => {
                    // Unreachable→connect() result, or NeedsAuth passthrough.
                    if s == BackendStatus::Connected {
                        strikes.remove(&name);
                        tracing::info!(server = %name, "health: unreachable → connected");
                    }
                    s
                }
            };
            if let Some(cadence) = cadence_for(new_status) {
                next_due.insert(name, now + cadence);
            } else {
                next_due.remove(&name);
            }
        }

        // Sleep until the earliest next_due; if nothing scheduled, poll on the
        // shorter cadence so newly-added backends get picked up promptly.
        let wake = next_due.values().min().copied()
            .unwrap_or_else(|| tokio::time::Instant::now() + UNREACHABLE_CADENCE);
        tokio::time::sleep_until(wake).await;
    }
}
```

- [ ] **Step 3: Flesh out the recovery test and add the remaining state-machine tests**

Use shared `serve_two_tool_server` (promoted to `pub(crate)`), `tokio::time::pause`, and a helper to set initial status. Implement:

```rust
async fn set_status(b: &Arc<ProxyBackend>, s: BackendStatus) { b.set_status(s).await; }

#[tokio::test]
async fn unreachable_backend_recovers_to_connected_on_probe() {
    let (_srv, url) = crate::test_server::serve_two_tool_server().await; // promoted helper
    let tmp = tempfile::tempdir().unwrap();
    let _ = right_db::open_connection(tmp.path(), true).await.unwrap();
    let backend = Arc::new(ProxyBackend::new(
        "composio".into(), tmp.path().to_path_buf(), url,
        Arc::new(RwLock::new(None)), AuthMethod::default(),
    ));
    set_status(&backend, BackendStatus::Unreachable).await;
    let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([("composio".into(), backend.clone())])));

    // Run the reconciler in the background; it should connect within one
    // UNREACHABLE_CADENCE-ish pass. Drive virtual time.
    tokio::time::pause();
    let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));
    // Yield + advance so the first pass (due immediately) runs.
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(50)).await;
    // Poll for the transition with a bounded number of yields.
    for _ in 0..50 {
        if backend.status().await == BackendStatus::Connected { break; }
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(backend.status().await, BackendStatus::Connected, "recovered backend must reconnect");
    h.abort();
}
```

Add the debounce/skip tests using a backend pointed at a **dead** URL (so probes are `Dead`) for the outage path, and a backend in `NeedsAuth` to assert it is never probed:

```rust
#[tokio::test]
async fn connected_flips_unreachable_after_three_dead_probes() {
    // Backend "Connected" but with NO session and a dead URL → probe_live = Dead each pass.
    let tmp = tempfile::tempdir().unwrap();
    let backend = Arc::new(ProxyBackend::new(
        "composio".into(), tmp.path().to_path_buf(),
        "http://127.0.0.1:1/mcp".into(),
        Arc::new(RwLock::new(None)), AuthMethod::default(),
    ));
    set_status(&backend, BackendStatus::Connected).await;
    let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([("composio".into(), backend.clone())])));

    tokio::time::pause();
    let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));

    // Each Connected pass is CONNECTED_CADENCE apart. Need 3 Dead probes to flip.
    // Advance through > 3 cadences, yielding so the loop runs each pass.
    for _ in 0..6 {
        tokio::task::yield_now().await;
        tokio::time::advance(CONNECTED_CADENCE + std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.status().await, BackendStatus::Unreachable, "3 dead probes must flip to Unreachable");
    h.abort();
}

#[tokio::test]
async fn needs_auth_is_never_probed() {
    // A NeedsAuth backend pointed at a server that would count requests — assert zero.
    // Use serve_two_tool_server and check it receives no connections by keeping it
    // NeedsAuth; simplest assertion: status stays NeedsAuth across several passes.
    let tmp = tempfile::tempdir().unwrap();
    let backend = Arc::new(ProxyBackend::new(
        "composio".into(), tmp.path().to_path_buf(),
        "http://127.0.0.1:1/mcp".into(),
        Arc::new(RwLock::new(None)), AuthMethod::default(),
    ));
    set_status(&backend, BackendStatus::NeedsAuth).await;
    let proxies: Proxies = Arc::new(RwLock::new(HashMap::from([("composio".into(), backend.clone())])));

    tokio::time::pause();
    let h = tokio::spawn(run_health_reconciler(proxies, reqwest::Client::new()));
    for _ in 0..6 {
        tokio::task::yield_now().await;
        tokio::time::advance(CONNECTED_CADENCE + std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.status().await, BackendStatus::NeedsAuth, "NeedsAuth must never be touched by health");
    h.abort();
}
```

- [ ] **Step 4: Run the reconciler tests**

Run: `devenv shell -- cargo test -p right-mcp health::`
Expected: PASS — recovery, 3-strike flip, and NeedsAuth-skip all green. Iterate on virtual-time advancing if a test hangs (ensure `yield_now` between `advance` calls so the spawned loop runs).

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/health.rs crates/right-mcp/src/proxy.rs crates/right-mcp/src/lib.rs
git commit -m "feat(mcp): health reconciler loop with adaptive cadence and debounce"
```

---

## Task 6: Wire the reconciler into bot startup

**Files:**
- Modify: `crates/right/src/main.rs` (per-agent startup block, after the `run_refresh_scheduler` spawn at ~line 1250)

- [ ] **Step 1: Add the spawn**

In `crates/right/src/main.rs`, immediately after the `tokio::spawn(right_mcp::refresh::run_refresh_scheduler(...))` block (ends ~line 1253), add:

```rust
                // Spawn per-agent MCP health reconciler — keeps BackendStatus
                // honest on the Connected↔Unreachable axis so the dashboard and
                // the agent see accurate status without waiting for a tool call.
                {
                    let proxies = std::sync::Arc::clone(&dispatcher
                        .agents
                        .get(&agent_name)
                        .expect("registry just inserted")
                        .proxies);
                    let http = http_client.clone();
                    tokio::spawn(right_mcp::health::run_health_reconciler(proxies, http));
                }
```

> Verify `dispatcher.agents.get(...)` returns a guard whose `.proxies` is an `Arc` you can clone (it is — `BackendRegistry.proxies: Arc<RwLock<...>>`, see `aggregator.rs:361`). `dispatcher.agents` is a `DashMap`; clone the `Arc` then drop the guard before the spawn if needed. If holding the DashMap `Ref` across the `Arc::clone` is awkward, clone into a local first:
> ```rust
> let proxies = { dispatcher.agents.get(&agent_name).unwrap().proxies.clone() };
> ```

- [ ] **Step 2: Compile**

Run: `devenv shell -- cargo build -p right`
Expected: clean build. Fix any borrow/guard issues per the note above.

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy -p right -p right-mcp --all-targets`
Expected: no warnings introduced by the new code.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(right): spawn per-agent MCP health reconciler at startup"
```

---

## Task 7: Final verification

- [ ] **Step 1: Full workspace test (MANDATORY per AGENTS.md)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Compare against the Step 0 baseline — no new failures. Record any pre-existing failures that were already present.

- [ ] **Step 2: Workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: clean.

- [ ] **Step 3: Rust review subagent (per global standards)**

Dispatch the `rust-dev:review-rust-code` subagent over the diff. Convert any findings to TODOs and fix one by one.

- [ ] **Step 4: Manual smoke (optional, if a live composio is available)**

Per AGENTS.md "Reproduce a sandbox claude invocation" / debugging notes: with the bot running, force a backend Unreachable (e.g. block its URL), confirm `mcp_list` flips to `unreachable` within ~3×120s and that restoring connectivity flips it back to `connected` within ~20s. Check `right-mcp-server` logs for the `health: ... → ...` transition lines.

---

## Self-Review (completed by plan author)

**Spec coverage:**
- Periodic reconciler, per-agent, reuses `registry.proxies` → Tasks 5, 6. ✓
- Probe = `list_all_tools` on live session, refresh cache → Task 2, 4. ✓
- State machine (Connected probe / Unreachable connect / NeedsAuth skip) → Tasks 3, 5. ✓
- 3-strike debounce, counter in reconciler loop state, immediate auth flip, reset on success → Task 3 (`decide_connected`) + Task 5 (application). ✓
- Adaptive cadence 120/20/never → Task 3 (`cadence_for`) + Task 5 (rescheduling). ✓
- 10s probe timeout, concurrent fan-out, lock dropped before probing → Task 5. ✓
- Transition logging → Task 5. ✓
- `probe_live` does not write status → Tasks 2, 4 assertions. ✓
- Tests incl. recovery regression anchor, off-by-one strike, NeedsAuth-skip, cadence → Tasks 3, 4, 5. ✓
- No frontend change → confirmed; no task. ✓
- Final full workspace test → Task 7. ✓

**Placeholder scan:** The Step 2 sketch in Task 5 is intentionally labeled a hazard-exposing sketch and is *superseded by the complete Step 2b implementation* — Step 2b is the real, complete code. The VERIFY-API GATE in Task 4 is a project-mandated API-verification step with a behavioral acceptance test, not a content placeholder. No `TODO`/`TBD` left in shipped code.

**Type consistency:** `ProbeOutcome` (proxy.rs) used uniformly; `ConnectedDecision`, `decide_connected`, `cadence_for`, consts (`CONNECTED_CADENCE`/`UNREACHABLE_CADENCE`/`MAX_STRIKES`/`PROBE_TIMEOUT`) defined in Task 3 and used unchanged in Task 5. `run_health_reconciler(proxies, http_client)` signature matches the Task 6 call site. `probe_live(&self) -> ProbeOutcome` matches across Tasks 2/4/5.

**Known risk flagged for executor:** the live-server harness (Task 4) is the only unverified-API surface; it is gated and behavior-tested. If the rmcp 1.7 server wiring differs from the `aggregator.rs` pattern, adjust the helper until `probe_live_alive_refreshes_tool_cache` passes without weakening assertions.
