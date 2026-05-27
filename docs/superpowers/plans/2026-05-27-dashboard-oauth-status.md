# Dashboard OAuth Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route MCP OAuth completion status back to the dashboard instead of Telegram DMs.

**Architecture:** Add a bot-local in-memory OAuth status store keyed by OAuth `state`, expose it through a Mini-App-authenticated dashboard status route, and update it from the existing OAuth callback completion task. The aggregator remains responsible for token persistence and MCP reconnect readiness; the dashboard starts OAuth and observes the transient result.

**Tech Stack:** Rust 2024, axum, tokio, serde, right-bot dashboard routes, Vue 3, TypeScript, Vitest, Vite.

---

## Pre-Flight

- Run commands through `devenv shell --`.
- Before writing Rust, load `rust-dev:rust-dev` if available. If unavailable, record that and follow `AGENTS.rust.md`.
- If using a worktree, create it under `.worktrees/` with `superpowers:using-git-worktrees`.
- Baseline:

```bash
devenv shell -- cargo test -p right-bot --lib telegram::dashboard::tests::dashboard_mcp_oauth_start_success_returns_auth_url_and_stores_pending_auth
devenv shell -- npm run test --prefix crates/right-dashboard/frontend -- McpView
```

Expected: PASS before edits, or record pre-existing failures.

## File Structure

- Create `crates/bot/src/telegram/oauth_status.rs`: transient OAuth status store and tests.
- Modify `crates/bot/src/telegram/mod.rs`: expose `oauth_status`.
- Modify `crates/bot/src/lib.rs`: instantiate and pass the shared status store.
- Modify `crates/bot/src/telegram/dashboard.rs`: add store to `DashboardState`, route, and tests.
- Modify `crates/bot/src/telegram/dashboard/mcp.rs`: return `flow_id`, insert pending status, expose status handler.
- Modify `crates/bot/src/telegram/oauth_callback.rs`: update status instead of broadcasting OAuth completion.
- Modify `crates/right-dashboard/frontend/src/types.ts`, `api.ts`, `views/mcpViewModel.ts`, `views/McpView.test.ts`, `views/McpView.vue`: typed status polling.
- Modify `docs/architecture/mcp.md`: document dashboard-owned OAuth completion.
- Regenerate `crates/right-dashboard/static/dashboard/`.

### Task 1: Add OAuth Status Store

**Files:**
- Create: `crates/bot/src/telegram/oauth_status.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Write the failing store tests**

Create `crates/bot/src/telegram/oauth_status.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[tokio::test]
    async fn pending_status_round_trips_by_flow_id() {
        let store = OAuthFlowStatusStore::default();
        store.insert_pending("flow-1".to_string(), "composio".to_string()).await;

        let status = store.status("flow-1").await;

        assert_eq!(status.flow_id, "flow-1");
        assert_eq!(status.server_name.as_deref(), Some("composio"));
        assert_eq!(status.status, OAuthFlowStatus::Pending);
        assert_eq!(status.message, None);
    }

    #[tokio::test]
    async fn terminal_status_updates_existing_flow() {
        let store = OAuthFlowStatusStore::default();
        store.insert_pending("flow-1".to_string(), "composio".to_string()).await;

        store.mark_failed("flow-1", "MCP readiness failed").await;
        assert_eq!(store.status("flow-1").await.status, OAuthFlowStatus::Failed);

        store.mark_succeeded("flow-1").await;
        let status = store.status("flow-1").await;
        assert_eq!(status.status, OAuthFlowStatus::Succeeded);
        assert_eq!(status.message, None);
    }

    #[tokio::test]
    async fn unknown_flow_returns_terminal_unknown_status() {
        let status = OAuthFlowStatusStore::default().status("missing").await;

        assert_eq!(status.flow_id, "missing");
        assert_eq!(status.server_name, None);
        assert_eq!(status.status, OAuthFlowStatus::Unknown);
        assert_eq!(status.message.as_deref(), Some("OAuth flow is no longer active."));
    }

    #[tokio::test]
    async fn cleanup_marks_old_pending_flows_expired() {
        let store = OAuthFlowStatusStore::default();
        store.insert_pending("flow-1".to_string(), "composio".to_string()).await;
        store.force_started_at_for_test("flow-1", Instant::now() - Duration::from_secs(700)).await;

        assert_eq!(store.expire_pending_older_than(Duration::from_secs(600)).await, 1);
        let status = store.status("flow-1").await;
        assert_eq!(status.status, OAuthFlowStatus::Expired);
        assert_eq!(status.message.as_deref(), Some("OAuth flow expired before completion."));
    }

    #[test]
    fn compact_internal_client_error_removes_secret_bearing_body() {
        let message = compact_dashboard_error(
            "Server error (502): {\"error\":\"mcp_reconnect_failed\",\"detail\":\"Unavailable resource\",\"access_token\":\"secret\"}",
        );

        assert_eq!(message, "Server error (502): mcp_reconnect_failed");
        assert!(!message.contains("secret"));
        assert!(!message.contains("access_token"));
        assert!(!message.contains("Unavailable resource"));
    }
}
```

In `crates/bot/src/telegram/mod.rs`, add near `oauth_callback`:

```rust
pub(crate) mod oauth_status;
```

- [ ] **Step 2: Verify the tests fail**

```bash
devenv shell -- cargo test -p right-bot --lib telegram::oauth_status
```

Expected: FAIL because the store types are missing.

- [ ] **Step 3: Implement the store**

Replace `crates/bot/src/telegram/oauth_status.rs` with:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub(crate) struct OAuthFlowStatusStore {
    inner: Arc<Mutex<HashMap<String, OAuthFlowStatusEntry>>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct OAuthFlowStatusResponse {
    pub flow_id: String,
    pub server_name: Option<String>,
    pub status: OAuthFlowStatus,
    pub message: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OAuthFlowStatus {
    Pending,
    Succeeded,
    Failed,
    Expired,
    Unknown,
}

#[derive(Debug, Clone)]
struct OAuthFlowStatusEntry {
    server_name: String,
    status: OAuthFlowStatus,
    message: Option<String>,
    started_at: Instant,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl OAuthFlowStatusStore {
    pub(crate) async fn insert_pending(&self, flow_id: String, server_name: String) {
        self.inner.lock().await.insert(
            flow_id,
            OAuthFlowStatusEntry {
                server_name,
                status: OAuthFlowStatus::Pending,
                message: None,
                started_at: Instant::now(),
                updated_at: chrono::Utc::now(),
            },
        );
    }

    pub(crate) async fn mark_succeeded(&self, flow_id: &str) {
        self.update(flow_id, OAuthFlowStatus::Succeeded, None).await;
    }

    pub(crate) async fn mark_failed(&self, flow_id: &str, message: impl Into<String>) {
        self.update(flow_id, OAuthFlowStatus::Failed, Some(message.into())).await;
    }

    pub(crate) async fn status(&self, flow_id: &str) -> OAuthFlowStatusResponse {
        match self.inner.lock().await.get(flow_id) {
            Some(entry) => OAuthFlowStatusResponse {
                flow_id: flow_id.to_string(),
                server_name: Some(entry.server_name.clone()),
                status: entry.status,
                message: entry.message.clone(),
                updated_at: entry.updated_at.to_rfc3339(),
            },
            None => OAuthFlowStatusResponse {
                flow_id: flow_id.to_string(),
                server_name: None,
                status: OAuthFlowStatus::Unknown,
                message: Some("OAuth flow is no longer active.".to_string()),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        }
    }

    pub(crate) async fn expire_pending_older_than(&self, max_age: Duration) -> usize {
        let mut expired = 0;
        let mut inner = self.inner.lock().await;
        for entry in inner.values_mut() {
            if entry.status == OAuthFlowStatus::Pending && entry.started_at.elapsed() >= max_age {
                entry.status = OAuthFlowStatus::Expired;
                entry.message = Some("OAuth flow expired before completion.".to_string());
                entry.updated_at = chrono::Utc::now();
                expired += 1;
            }
        }
        expired
    }

    async fn update(&self, flow_id: &str, status: OAuthFlowStatus, message: Option<String>) {
        if let Some(entry) = self.inner.lock().await.get_mut(flow_id) {
            entry.status = status;
            entry.message = message;
            entry.updated_at = chrono::Utc::now();
        }
    }

    #[cfg(test)]
    async fn force_started_at_for_test(&self, flow_id: &str, started_at: Instant) {
        if let Some(entry) = self.inner.lock().await.get_mut(flow_id) {
            entry.started_at = started_at;
        }
    }
}

pub(crate) fn compact_dashboard_error(detail: impl AsRef<str>) -> String {
    let detail = detail.as_ref();
    if let Some((status, body)) = detail.split_once(": {") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&format!("{{{body}")) {
            if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
                return format!("{}: {}", status.trim(), error);
            }
        }
    }

    let lower = detail.to_ascii_lowercase();
    let secret_words = ["access_token", "refresh_token", "client_secret", "code_verifier", "authorization"];
    if secret_words.iter().any(|word| lower.contains(word)) {
        return "OAuth failed; sensitive error detail was redacted.".to_string();
    }

    detail.chars().take(240).collect()
}
```

Keep the tests from Step 1 below this implementation.

- [ ] **Step 4: Verify and commit**

```bash
devenv shell -- cargo test -p right-bot --lib telegram::oauth_status
devenv shell -- git add crates/bot/src/telegram/oauth_status.rs crates/bot/src/telegram/mod.rs
devenv shell -- git commit -m "feat(mcp): add oauth status store"
```

Expected: tests PASS; commit succeeds.

### Task 2: Wire Status Store Into Bot And Dashboard State

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/bot/src/telegram/dashboard/mcp.rs`
- Modify: `crates/bot/src/telegram/oauth_callback.rs`
- Test: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Add failing dashboard status-route tests**

In `crates/bot/src/telegram/dashboard.rs`, add this helper after `get_json`:

```rust
    async fn get_json_with_state(
        path: &str,
        auth: Option<String>,
        state: super::DashboardState,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(state);
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router.oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }
```

Add tests near the MCP tests:

```rust
    #[tokio::test]
    async fn dashboard_mcp_oauth_status_requires_auth() {
        let temp = tempfile::tempdir().expect("tempdir");
        let status = get(
            "/dashboard/alpha/api/v1/mcp/oauth/flow-1/status",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn dashboard_mcp_oauth_status_returns_pending_and_unknown() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut state = test_state(temp.path().to_path_buf());
        let store = super::super::oauth_status::OAuthFlowStatusStore::default();
        store.insert_pending("flow-1".to_string(), "composio".to_string()).await;
        state.oauth_status = store;

        let (status, body) = get_json_with_state(
            "/dashboard/alpha/api/v1/mcp/oauth/flow-1/status",
            Some(signed_init_data(42)),
            state.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "pending");
        assert_eq!(body["server_name"], "composio");

        let (status, body) = get_json_with_state(
            "/dashboard/alpha/api/v1/mcp/oauth/missing/status",
            Some(signed_init_data(42)),
            state,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "unknown");
        assert_eq!(body["message"], "OAuth flow is no longer active.");
    }
```

Add this field to `test_state` after `pending_auth` so the new tests compile after implementation:

```rust
            oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
```

- [ ] **Step 2: Verify the route tests fail**

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_mcp_oauth_status
```

Expected: FAIL because `DashboardState.oauth_status` and the route handler are not implemented.

- [ ] **Step 3: Add state fields and runtime wiring**

In `crates/bot/src/telegram/dashboard.rs`, add to `DashboardState`:

```rust
    pub oauth_status: super::oauth_status::OAuthFlowStatusStore,
```

In `build_dashboard_router`, add after the OAuth start route:

```rust
        .route(
            "/dashboard/{agent}/api/v1/mcp/oauth/{flow_id}/status",
            get(mcp::handle_mcp_oauth_status),
        )
```

In `crates/bot/src/telegram/oauth_callback.rs`, add to `OAuthCallbackState`:

```rust
    pub oauth_status: super::oauth_status::OAuthFlowStatusStore,
```

Initialize that field in the two test helpers that build `OAuthCallbackState`:

```rust
            oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
```

In `crates/bot/src/lib.rs`, create the store after `pending_auth`:

```rust
    let oauth_status = telegram::oauth_status::OAuthFlowStatusStore::default();
```

Pass it to both state structs:

```rust
        oauth_status: oauth_status.clone(),
```

Change cleanup spawn:

```rust
    tokio::spawn(run_pending_auth_cleanup(
        Arc::clone(&pending_auth),
        oauth_status.clone(),
    ));
```

- [ ] **Step 4: Implement status handler and cleanup expiry**

In `crates/bot/src/telegram/dashboard/mcp.rs`, add:

```rust
pub(crate) async fn handle_mcp_oauth_status(
    AxumPath((agent, flow_id)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    Json(state.oauth_status.status(&flow_id).await).into_response()
}
```

In `crates/bot/src/telegram/oauth_callback.rs`, replace `run_pending_auth_cleanup` with:

```rust
pub async fn run_pending_auth_cleanup(
    pending_auth: PendingAuthMap,
    oauth_status: super::oauth_status::OAuthFlowStatusStore,
) {
    const CHECK_INTERVAL: Duration = Duration::from_secs(60);
    const EXPIRY: Duration = Duration::from_secs(600);

    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        let mut map = pending_auth.lock().await;
        let before = map.len();
        map.retain(|_state, auth| auth.created_at.elapsed() < EXPIRY);
        let after = map.len();
        drop(map);

        let expired_statuses = oauth_status.expire_pending_older_than(EXPIRY).await;
        if before != after || expired_statuses > 0 {
            tracing::debug!(
                removed_pending_auth = before - after,
                expired_statuses,
                remaining_pending_auth = after,
                "pending auth cleanup completed"
            );
        }
    }
}
```

- [ ] **Step 5: Verify and commit**

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_mcp_oauth_status
devenv shell -- git add crates/bot/src/lib.rs crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/mcp.rs crates/bot/src/telegram/oauth_callback.rs
devenv shell -- git commit -m "feat(mcp): expose dashboard oauth status"
```

Expected: tests PASS; commit succeeds.

### Task 3: Return Flow ID From OAuth Start

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/mcp.rs`
- Test: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Update the success test first**

In `dashboard_mcp_oauth_start_success_returns_auth_url_and_stores_pending_auth`, after creating `state`, add:

```rust
        let oauth_status = state.oauth_status.clone();
```

Replace the response key assertion:

```rust
        let mut keys = body.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, vec!["auth_url", "flow_id"]);
```

After extracting `state_param`, add:

```rust
        assert_eq!(body["flow_id"].as_str(), Some(state_param.as_str()));
        let status = oauth_status.status(state_param).await;
        assert_eq!(status.server_name.as_deref(), Some("linear"));
        assert_eq!(
            status.status,
            super::super::oauth_status::OAuthFlowStatus::Pending
        );
```

- [ ] **Step 2: Verify the test fails**

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_mcp_oauth_start_success_returns_auth_url_and_stores_pending_auth
```

Expected: FAIL because `flow_id` is missing and status is not inserted.

- [ ] **Step 3: Implement flow id response**

In `crates/bot/src/telegram/dashboard/mcp.rs`, change:

```rust
#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpOAuthStartResponse {
    pub auth_url: String,
    pub flow_id: String,
}
```

After inserting `PendingAuth`, add:

```rust
    state
        .oauth_status
        .insert_pending(oauth_state.clone(), server_name.clone())
        .await;
```

Return:

```rust
    Json(DashboardMcpOAuthStartResponse {
        auth_url,
        flow_id: oauth_state,
    })
    .into_response()
```

- [ ] **Step 4: Verify and commit**

```bash
devenv shell -- cargo test -p right-bot --lib dashboard_mcp_oauth_start_success_returns_auth_url_and_stores_pending_auth
devenv shell -- git add crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/mcp.rs
devenv shell -- git commit -m "feat(mcp): return dashboard oauth flow ids"
```

Expected: test PASS; commit succeeds.

### Task 4: Update OAuth Callback Completion Status

**Files:**
- Modify: `crates/bot/src/telegram/oauth_callback.rs`
- Test: `crates/bot/src/telegram/oauth_callback.rs`

- [ ] **Step 1: Add helper tests**

Replace the existing `set_token_failure_message_*` tests with:

```rust
    #[test]
    fn set_token_failure_dashboard_message_does_not_claim_success() {
        let msg = set_token_failure_dashboard_message("Server error (502): {\"error\":\"mcp_reconnect_failed\"}");

        assert!(!msg.contains("Authenticated"));
        assert!(!msg.contains("succeeded"));
        assert!(msg.contains("Token exchange completed"));
        assert!(msg.contains("mcp_reconnect_failed"));
    }

    #[test]
    fn callback_received_html_points_back_to_dashboard_not_telegram() {
        let html = callback_received_html();

        assert!(html.contains("dashboard"));
        assert!(!html.contains("Telegram"));
        assert!(!html.contains("Check Telegram"));
    }
```

- [ ] **Step 2: Verify the helper tests fail**

```bash
devenv shell -- cargo test -p right-bot --lib set_token_failure_dashboard_message_does_not_claim_success
devenv shell -- cargo test -p right-bot --lib callback_received_html_points_back_to_dashboard_not_telegram
```

Expected: FAIL because the new helpers do not exist.

- [ ] **Step 3: Update callback status paths**

In the provider error branch of `handle_oauth_callback`, before returning, add:

```rust
        if let Some(state_param) = params.state.as_deref() {
            state
                .oauth_status
                .mark_failed(state_param, format!("OAuth provider error: {err} -- {desc}"))
                .await;
        }
```

In the invalid-state branch, before returning, add:

```rust
            state
                .oauth_status
                .mark_failed(&received_state, "OAuth state is invalid or expired.")
                .await;
```

Change the success HTML response to:

```rust
        axum::response::Html(callback_received_html()),
```

In `complete_oauth_flow`, replace token exchange `?` usage with a `match` that records failure:

```rust
        Err(error) => {
            let detail = format!("{error:#}");
            cb_state
                .oauth_status
                .mark_failed(
                    &pending.state,
                    format!(
                        "Token exchange failed: {}",
                        super::oauth_status::compact_dashboard_error(&detail)
                    ),
                )
                .await;
            return Err(miette::miette!("token exchange failed: {detail}"));
        }
```

Replace the internal `set_token` success/failure notification code with:

```rust
    match cb_state.internal_client.set_token(&set_token_req).await {
        Ok(_resp) => {
            cb_state.oauth_status.mark_succeeded(&pending.state).await;
        }
        Err(error) => {
            let detail = format!("{error:#}");
            tracing::error!(
                agent = %agent_name,
                server = %pending.server_name,
                "set_token failed: {detail}"
            );
            cb_state
                .oauth_status
                .mark_failed(&pending.state, set_token_failure_dashboard_message(&detail))
                .await;
        }
    }
```

Replace `set_token_failure_message` with:

```rust
fn set_token_failure_dashboard_message(err: impl std::fmt::Display) -> String {
    format!(
        "Token exchange completed, but MCP readiness failed: {}",
        super::oauth_status::compact_dashboard_error(err.to_string())
    )
}

fn callback_received_html() -> &'static str {
    "<!DOCTYPE html><html><body><h1>Authorization received</h1>\
     <p>You may close this window. The dashboard will update when MCP readiness finishes.</p></body></html>"
}
```

Remove these aliases because OAuth completion no longer broadcasts:

```rust
use super::broadcast_html_to_chats as notify_html_telegram;
use super::broadcast_to_chats as notify_telegram;
```

- [ ] **Step 4: Verify callback behavior**

```bash
devenv shell -- cargo test -p right-bot --lib telegram::oauth_callback
devenv shell -- rg -n "notify_telegram|notify_html_telegram|set_token_failure_message|Check Telegram" crates/bot/src/telegram/oauth_callback.rs
```

Expected: tests PASS. `rg` exits `1` with no matches.

- [ ] **Step 5: Commit**

```bash
devenv shell -- git add crates/bot/src/telegram/oauth_callback.rs
devenv shell -- git commit -m "fix(mcp): report oauth completion to dashboard"
```

### Task 5: Add Frontend Types And Model Helpers

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/api.ts`
- Modify: `crates/right-dashboard/frontend/src/views/mcpViewModel.ts`
- Test: `crates/right-dashboard/frontend/src/views/McpView.test.ts`

- [ ] **Step 1: Add failing model tests**

In `McpView.test.ts`, add imports:

```ts
  isOAuthTerminalStatus,
  oauthStatusMessage,
  shouldApplyOAuthPollResult,
```

Add tests:

```ts
  it('treats only non-pending OAuth statuses as terminal', () => {
    expect(isOAuthTerminalStatus('pending')).toBe(false)
    expect(isOAuthTerminalStatus('succeeded')).toBe(true)
    expect(isOAuthTerminalStatus('failed')).toBe(true)
    expect(isOAuthTerminalStatus('expired')).toBe(true)
    expect(isOAuthTerminalStatus('unknown')).toBe(true)
  })

  it('ignores stale OAuth poll results after a newer flow starts', () => {
    expect(shouldApplyOAuthPollResult('flow-new', 'flow-new')).toBe(true)
    expect(shouldApplyOAuthPollResult('flow-old', 'flow-new')).toBe(false)
    expect(shouldApplyOAuthPollResult('flow-old', undefined)).toBe(false)
  })

  it('formats OAuth status messages without relying on Telegram', () => {
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'pending', message: null, updated_at: 'now' })).toBe('OAuth pending')
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'succeeded', message: null, updated_at: 'now' })).toBe('OAuth connected')
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'failed', message: 'MCP readiness failed', updated_at: 'now' })).toBe('MCP readiness failed')
  })
```

- [ ] **Step 2: Verify frontend tests fail**

```bash
devenv shell -- npm run test --prefix crates/right-dashboard/frontend -- McpView
```

Expected: FAIL because the helpers and status types are missing.

- [ ] **Step 3: Add types, API, and helpers**

In `types.ts`, replace `McpOAuthStartResponse` and add status types:

```ts
export interface McpOAuthStartResponse {
  auth_url: string
  flow_id: string
}

export type McpOAuthFlowStatus = 'pending' | 'succeeded' | 'failed' | 'expired' | 'unknown'

export interface McpOAuthStatusResponse {
  flow_id: string
  server_name: string | null
  status: McpOAuthFlowStatus
  message: string | null
  updated_at: string
}
```

In `api.ts`, import `McpOAuthStatusResponse` and add:

```ts
export function mcpOAuthStatus(flowId: string): Promise<McpOAuthStatusResponse> {
  return requestJson<McpOAuthStatusResponse>(`api/v1/mcp/oauth/${encodeURIComponent(flowId)}/status`)
}
```

In `mcpViewModel.ts`, update the type import:

```ts
import type { McpHeaderInput, McpOAuthFlowStatus, McpOAuthStatusResponse, McpServerSummary } from '../types'
```

Append:

```ts
export function isOAuthTerminalStatus(status: McpOAuthFlowStatus): boolean {
  return status !== 'pending'
}

export function shouldApplyOAuthPollResult(responseFlowId: string, currentFlowId: string | undefined): boolean {
  return currentFlowId !== undefined && responseFlowId === currentFlowId
}

export function oauthStatusMessage(status: McpOAuthStatusResponse): string {
  if (status.message) {
    return status.message
  }
  if (status.status === 'pending') {
    return 'OAuth pending'
  }
  if (status.status === 'succeeded') {
    return 'OAuth connected'
  }
  if (status.status === 'expired') {
    return 'OAuth flow expired'
  }
  if (status.status === 'unknown') {
    return 'OAuth flow is no longer active'
  }
  return 'OAuth failed'
}
```

- [ ] **Step 4: Verify and commit**

```bash
devenv shell -- npm run test --prefix crates/right-dashboard/frontend -- McpView
devenv shell -- git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/right-dashboard/frontend/src/views/mcpViewModel.ts crates/right-dashboard/frontend/src/views/McpView.test.ts
devenv shell -- git commit -m "feat(dashboard): add oauth status api"
```

Expected: tests PASS; commit succeeds.

### Task 6: Poll OAuth Status In MCP View

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/McpView.vue`

- [ ] **Step 1: Wire polling state and imports**

In `McpView.vue`, make the Vue import:

```ts
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
```

Add `mcpOAuthStatus` to API imports, `McpOAuthStatusResponse` to type imports, and these model helpers:

```ts
  isOAuthTerminalStatus,
  oauthStatusMessage,
  shouldApplyOAuthPollResult,
```

Add state near `busyAction`:

```ts
const oauthFlows = ref<Record<string, string>>({})
const oauthStatuses = ref<Record<string, McpOAuthStatusResponse>>({})
const oauthPollTimers = new Map<string, ReturnType<typeof window.setTimeout>>()
```

Add cleanup:

```ts
onBeforeUnmount(() => {
  for (const timer of oauthPollTimers.values()) {
    window.clearTimeout(timer)
  }
  oauthPollTimers.clear()
})
```

- [ ] **Step 2: Add polling functions**

Add before `removeServer`:

```ts
function scheduleOAuthPoll(serverName: string, flowId: string): void {
  clearOAuthPoll(serverName)
  const timer = window.setTimeout(() => {
    void pollOAuthStatus(serverName, flowId)
  }, 1500)
  oauthPollTimers.set(serverName, timer)
}

function clearOAuthPoll(serverName: string): void {
  const timer = oauthPollTimers.get(serverName)
  if (timer !== undefined) {
    window.clearTimeout(timer)
    oauthPollTimers.delete(serverName)
  }
}

async function pollOAuthStatus(serverName: string, flowId: string): Promise<void> {
  try {
    const response = await mcpOAuthStatus(flowId)
    if (!shouldApplyOAuthPollResult(response.flow_id, oauthFlows.value[serverName])) {
      return
    }
    oauthStatuses.value = { ...oauthStatuses.value, [serverName]: response }
    if (isOAuthTerminalStatus(response.status)) {
      clearOAuthPoll(serverName)
      await refresh()
      return
    }
    scheduleOAuthPoll(serverName, flowId)
  } catch (err) {
    if (!shouldApplyOAuthPollResult(flowId, oauthFlows.value[serverName])) {
      return
    }
    oauthStatuses.value = {
      ...oauthStatuses.value,
      [serverName]: {
        flow_id: flowId,
        server_name: serverName,
        status: 'failed',
        message: err instanceof Error ? err.message : 'OAuth status unavailable',
        updated_at: new Date().toISOString(),
      },
    }
    clearOAuthPoll(serverName)
    await refresh()
  }
}
```

- [ ] **Step 3: Start polling and render status**

In `startOAuth`, after `const response = await mcpStartOAuth(server.name)`, add:

```ts
    oauthFlows.value = { ...oauthFlows.value, [server.name]: response.flow_id }
    oauthStatuses.value = {
      ...oauthStatuses.value,
      [server.name]: {
        flow_id: response.flow_id,
        server_name: server.name,
        status: 'pending',
        message: null,
        updated_at: new Date().toISOString(),
      },
    }
    scheduleOAuthPoll(server.name, response.flow_id)
```

In the server row template, after the row action buttons, add:

```vue
        <p
          v-if="oauthStatuses[server.name]"
          class="notice inline oauth-status"
          :class="`oauth-status-${oauthStatuses[server.name].status}`"
        >
          {{ oauthStatusMessage(oauthStatuses[server.name]) }}
        </p>
```

Add CSS:

```css
.oauth-status {
  grid-column: 1 / -1;
}

.oauth-status-succeeded {
  border-color: rgba(25, 135, 84, 0.35);
}

.oauth-status-failed,
.oauth-status-expired,
.oauth-status-unknown {
  border-color: rgba(176, 42, 55, 0.35);
}
```

- [ ] **Step 4: Verify and commit**

```bash
devenv shell -- npm run test --prefix crates/right-dashboard/frontend -- McpView
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- git add crates/right-dashboard/frontend/src/views/McpView.vue
devenv shell -- git commit -m "feat(dashboard): poll mcp oauth status"
```

Expected: tests and typecheck PASS; commit succeeds.

### Task 7: Update Architecture Docs And Generated Assets

**Files:**
- Modify: `docs/architecture/mcp.md`
- Modify: `crates/right-dashboard/static/dashboard/`

- [ ] **Step 1: Update MCP architecture doc**

In `docs/architecture/mcp.md`, update the dashboard OAuth start paragraph to include:

```markdown
Dashboard OAuth start stores an in-memory `PendingAuth` keyed by generated
state, stores a matching transient dashboard OAuth status under the same state
value, and returns the authorization URL plus `flow_id`. The OAuth callback
records completion in that transient status store instead of sending Telegram
DMs. The dashboard polls
`/dashboard/<agent>/api/v1/mcp/oauth/<flow_id>/status` until terminal status,
then refreshes the MCP server list. Bot restarts lose in-flight status; the
dashboard treats that as `unknown`, and the user starts OAuth again.
```

In the `OAuth callback readiness` section, replace the sentence that says Telegram reports failure with:

```markdown
If readiness fails, the dashboard OAuth status reports a failure instead of
showing a false success.
```

- [ ] **Step 2: Build assets and commit**

```bash
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
devenv shell -- git add docs/architecture/mcp.md crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "docs(mcp): document dashboard oauth status"
```

Expected: build PASS; commit succeeds.

### Task 8: Final Verification

**Files:**
- No code edits.

- [ ] **Step 1: Run focused Rust tests**

```bash
devenv shell -- cargo test -p right-bot --lib telegram::oauth_status
devenv shell -- cargo test -p right-bot --lib dashboard_mcp_oauth
devenv shell -- cargo test -p right-bot --lib telegram::oauth_callback
```

Expected: PASS.

- [ ] **Step 2: Run frontend checks**

```bash
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: PASS.

- [ ] **Step 3: Run mandatory full workspace test**

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 4: Confirm clean worktree**

```bash
devenv shell -- git status --short
```

Expected: clean. If the final frontend build changed only `crates/right-dashboard/static/dashboard/`, commit it:

```bash
devenv shell -- git add crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "build(dashboard): refresh oauth status assets"
```
