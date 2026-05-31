# MCP `set-headers` Decoupling + Reconnect Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Saving an MCP header always persists the credential (even when the upstream is down), the backend self-heals via the reconciler, and every reconnect outcome is traceable in logs and in `/mcp-list`.

**Architecture:** In `right-mcp`, `ProxyBackend::connect()` records and logs every outcome (success/transport-failure/auth-failure) into new in-memory fields. In `right`, `handle_mcp_set_headers` persists the header first, swaps a fresh `Unreachable` backend in, returns immediately, and runs a best-effort `connect()` in the background; `/mcp-list` exposes the recorded fields. The `right-dashboard` `McpView` renders the failure cause.

**Tech Stack:** Rust (edition 2024, tokio, axum, rmcp, chrono, thiserror), Vue 3 + TypeScript + vitest.

**Spec:** `docs/superpowers/specs/2026-06-01-mcp-set-headers-decouple-and-observability-design.md`

---

## File Structure

- `crates/right-mcp/src/proxy.rs` — add `redact_query_strings` helper; add `last_attempt_at`/`last_success_at`/`last_connect_error` fields, getters, and record helpers to `ProxyBackend`; wire recording + debug logging into `connect()`. (Tasks 1, 2)
- `crates/right/src/internal_api.rs` — decouple persistence from connection in `handle_mcp_set_headers`; add the three fields to the server-side `McpServerStatus` and populate them in `handle_mcp_list`. (Tasks 3, 4)
- `crates/right-mcp/src/internal_client.rs` — mirror the three new fields on the client-side `McpServerStatus` (kept in sync with the server copy). (Task 4)
- `crates/right-dashboard/frontend/src/types.ts` — add optional fields to `McpServerSummary`. (Task 5)
- `crates/right-dashboard/frontend/src/views/mcpViewModel.ts` — add pure `relativeAgo` + `mcpStatusDetail` helpers. (Task 5)
- `crates/right-dashboard/frontend/src/views/McpView.vue` — render the status detail line. (Task 5)
- `crates/right-dashboard/frontend/src/views/McpView.test.ts` — unit tests for the new helpers. (Task 5)
- `docs/architecture/mcp.md` — cite-on-touch: note decoupled persistence + last-attempt fields. (Task 6)

## Baseline (run once before Task 1)

- [ ] **Record the green baseline.**

Run:
```bash
devenv shell -- cargo test -p right-mcp -p right
devenv shell -- bash -c 'cd crates/right-dashboard/frontend && npm test'
```
Expected: all pass. Note any pre-existing failures so they are not blamed on this work.

---

## Task 1: `redact_query_strings` helper (`right-mcp`)

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs` (add a module-level helper near `classify_probe_error`, ~line 88, and a test in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `crates/right-mcp/src/proxy.rs` inside `mod tests`:
```rust
#[test]
fn redact_query_strings_strips_url_query() {
    assert_eq!(
        redact_query_strings("error sending request for url (http://h:1/mcp?token=abc)"),
        "error sending request for url (http://h:1/mcp?<redacted>)"
    );
    assert_eq!(redact_query_strings("plain message"), "plain message");
    assert_eq!(redact_query_strings("http://h:1/mcp"), "http://h:1/mcp");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-mcp redact_query_strings_strips_url_query`
Expected: FAIL — `cannot find function redact_query_strings`.

- [ ] **Step 3: Write minimal implementation**

Add near `classify_probe_error` in `crates/right-mcp/src/proxy.rs`:
```rust
/// Strip query strings from any URL-like substrings in an error message.
/// `query_string`-auth embeds the credential in the URL, and rmcp transport
/// errors can quote the URL verbatim — this keeps that token out of logs and
/// out of the `last_connect_error` surfaced to the dashboard.
pub(crate) fn redact_query_strings(msg: &str) -> String {
    msg.split(' ')
        .map(|tok| {
            if tok.contains("://") {
                if let Some(idx) = tok.find('?') {
                    let trailing = if tok.ends_with(')') { ")" } else { "" };
                    return format!("{}?<redacted>{trailing}", &tok[..idx]);
                }
            }
            tok.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-mcp redact_query_strings_strips_url_query`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/proxy.rs
git commit -m "feat(right-mcp): add redact_query_strings helper for safe error logging"
```

---

## Task 2: Record + log every `connect()` outcome (`right-mcp`)

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs` — `ProxyBackend` struct (line 334), `new()` (line 350), `connect()` (lines 388-452), add getters + record helpers (after `status()`, ~line 544), tests in `mod tests`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/right-mcp/src/proxy.rs` inside `mod tests`:
```rust
#[tokio::test]
async fn connect_failure_records_error_and_attempt() {
    setup_crypto();
    let tmp = tempfile::tempdir().unwrap();
    let backend = ProxyBackend::new(
        "dead".into(),
        tmp.path().to_path_buf(),
        "http://127.0.0.1:1/mcp".into(),
        Arc::new(RwLock::new(None)),
        AuthMethod::default(),
    );
    let result = backend.connect(reqwest::Client::new()).await;
    assert!(result.is_err(), "connect to dead port must fail");
    assert!(backend.last_attempt_at().await.is_some());
    assert!(backend.last_connect_error().await.is_some());
    assert!(backend.last_success_at().await.is_none());
}

#[tokio::test]
async fn connect_success_records_success() {
    setup_crypto();
    let (_srv, url) = crate::test_server::serve_two_tool_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
    crate::credentials::db_add_server(&conn, "srv", &url).await.unwrap();
    let backend = ProxyBackend::new(
        "srv".into(),
        tmp.path().to_path_buf(),
        url,
        Arc::new(RwLock::new(None)),
        AuthMethod::default(),
    );
    backend.connect(reqwest::Client::new()).await.unwrap();
    assert!(backend.last_success_at().await.is_some());
    assert!(backend.last_connect_error().await.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-mcp connect_failure_records connect_success_records`
Expected: FAIL — `no method named last_attempt_at`/`last_success_at`/`last_connect_error`.

- [ ] **Step 3: Add the fields to the struct**

In `crates/right-mcp/src/proxy.rs`, extend `pub struct ProxyBackend` (line 334) — add after `connect_mutex`:
```rust
    /// Wall-clock of the most recent connect() attempt (any outcome).
    last_attempt_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// Wall-clock of the most recent successful connect().
    last_success_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// Redacted detail of the most recent connect() failure; cleared on success.
    last_connect_error: RwLock<Option<String>>,
```

In `new()` (line 357 `Self { … }`), add after `connect_mutex: Mutex::new(()),`:
```rust
            last_attempt_at: RwLock::new(None),
            last_success_at: RwLock::new(None),
            last_connect_error: RwLock::new(None),
```

- [ ] **Step 4: Add getters + record helpers**

In `crates/right-mcp/src/proxy.rs`, add inside `impl ProxyBackend` (after `status()`, ~line 544):
```rust
    /// Wall-clock of the most recent connect() attempt, if any.
    pub async fn last_attempt_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.last_attempt_at.read().await
    }

    /// Wall-clock of the most recent successful connect(), if any.
    pub async fn last_success_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.last_success_at.read().await
    }

    /// Redacted detail of the most recent connect() failure, if any.
    pub async fn last_connect_error(&self) -> Option<String> {
        self.last_connect_error.read().await.clone()
    }

    async fn record_connect_success(&self) {
        let now = chrono::Utc::now();
        *self.last_attempt_at.write().await = Some(now);
        *self.last_success_at.write().await = Some(now);
        *self.last_connect_error.write().await = None;
    }

    async fn record_connect_failure(&self, detail: String) {
        *self.last_attempt_at.write().await = Some(chrono::Utc::now());
        *self.last_connect_error.write().await = Some(detail);
    }
```

- [ ] **Step 5: Wire recording + logging into `connect()`**

In `crates/right-mcp/src/proxy.rs`, replace the `().serve(transport).await` match arm (lines 388-400) with:
```rust
        let client: RunningService<RoleClient, ()> = match ().serve(transport).await {
            Ok(client) => client,
            Err(e) => {
                let msg = format!("{e:#}");
                let safe = redact_query_strings(&msg);
                self.record_connect_failure(safe.clone()).await;
                if let Some(err) = self.auth_required_connect_error(&msg, "initialize").await {
                    return Err(err);
                }
                tracing::debug!(
                    server = %self.server_name,
                    phase = "initialize",
                    error = %safe,
                    "upstream MCP connect failed"
                );
                return Err(ProxyError::InitFailed {
                    server: self.server_name.clone(),
                    source: e,
                });
            }
        };
```

Replace the `client.peer().list_all_tools().await` match arm (lines 403-415) with:
```rust
        let tools = match client.peer().list_all_tools().await {
            Ok(tools) => tools,
            Err(e) => {
                let msg = format!("{e:#}");
                let safe = redact_query_strings(&msg);
                self.record_connect_failure(safe.clone()).await;
                if let Some(err) = self.auth_required_connect_error(&msg, "list_tools").await {
                    return Err(err);
                }
                tracing::debug!(
                    server = %self.server_name,
                    phase = "list_tools",
                    error = %safe,
                    "upstream MCP connect failed"
                );
                return Err(ProxyError::ListToolsFailed {
                    server: self.server_name.clone(),
                    source: e,
                });
            }
        };
```

In the success path, add `self.record_connect_success().await;` immediately after `*self.status.write().await = BackendStatus::Connected;` (line 444), before the `tracing::info!` call.

- [ ] **Step 6: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-mcp connect_failure_records connect_success_records`
Expected: PASS.

- [ ] **Step 7: Confirm the reconciler still self-heals (no regression)**

Run: `devenv shell -- cargo test -p right-mcp health::`
Expected: PASS, including `unreachable_backend_recovers_to_connected_on_probe`. (The reconciler's `let _ = … connect()` is intentionally unchanged: `connect()` now self-logs, so the call site no longer silently swallows the error.)

- [ ] **Step 8: Commit**

```bash
git add crates/right-mcp/src/proxy.rs
git commit -m "feat(right-mcp): record and log every ProxyBackend connect outcome"
```

---

## Task 3: Decouple persistence from connection in `set-headers` (`right`)

**Files:**
- Modify: `crates/right/src/internal_api.rs` — `handle_mcp_set_headers` (lines 618-697), test in `mod tests`.

- [ ] **Step 1: Write the failing regression test**

Add to `crates/right/src/internal_api.rs` inside `mod tests`:
```rust
#[tokio::test]
async fn mcp_set_headers_persists_against_unreachable_server() {
    setup_crypto();
    let tmp = tempfile::tempdir().unwrap();
    let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;

    let agent_dir = tmp.path().join("agents/test-agent");
    let dead_url = "http://127.0.0.1:1/mcp".to_string();

    // Seed a DB row + an unreachable proxy backend directly: /mcp-add would
    // itself fail to connect to a dead upstream, so we bypass it.
    {
        let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
        credentials::db_add_server(&conn, "obsidian", &dead_url)
            .await
            .unwrap();
    }
    {
        // Clone the Arc out and drop the DashMap ref before awaiting the lock.
        let proxies = Arc::clone(&dispatcher.agents.get("test-agent").unwrap().proxies);
        let backend = Arc::new(ProxyBackend::new(
            "obsidian".into(),
            agent_dir.clone(),
            dead_url.clone(),
            Arc::new(tokio::sync::RwLock::new(None)),
            AuthMethod::default(),
        ));
        proxies.write().await.insert("obsidian".into(), backend);
    }

    // Set headers while the upstream is unreachable.
    let (status, body) = send_json(
        app.clone(),
        "/mcp-set-headers",
        serde_json::json!({
            "agent": "test-agent",
            "name": "obsidian",
            "headers": [{ "name": "X-Api-Key", "value": "secret-key" }]
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "headers must persist even when unreachable: body={body}"
    );

    // The credential was persisted and the server is still retryable.
    let (status, body) =
        send_json(app, "/mcp-list", serde_json::json!({ "agent": "test-agent" })).await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let obsidian = body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["name"] == "obsidian")
        .unwrap();
    assert_eq!(obsidian["auth_type"], "headers");
    assert_eq!(obsidian["header_names"], serde_json::json!(["X-Api-Key"]));
    assert_eq!(
        obsidian["status"], "unreachable",
        "stays retryable, not parked: {obsidian}"
    );
    assert!(
        !body.to_string().contains("secret-key"),
        "header values must never be exposed: {body}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devenv shell -- cargo test -p right mcp_set_headers_persists_against_unreachable_server`
Expected: FAIL — current handler returns `502 BAD_GATEWAY` (connect test fails), so the first `assert_eq!(status, StatusCode::OK)` fails.

- [ ] **Step 3: Rewrite the handler body**

In `crates/right/src/internal_api.rs`, replace the block from `let replacement = Arc::new(ProxyBackend::new(` (line 653) through the final `(StatusCode::OK, Json(McpSetHeadersResponse { ok: true })).into_response()` (line 696) with:
```rust
    // Persist first — a credential write does not depend on upstream reachability.
    {
        let conn = conn_arc.lock().await;
        if let Err(e) = credentials::db_set_http_headers(&conn, &req.name, &header_secrets).await {
            return match e {
                CredentialError::ServerNotFound(_) => {
                    not_found(format!("server '{}' not found", req.name)).into_response()
                }
                _ => internal_error(format!("db_set_http_headers: {e:#}")).into_response(),
            };
        }
    }

    // Swap in a fresh backend carrying the new headers. It starts Unreachable;
    // the reconciler re-probes it (with the new headers) until it connects.
    let replacement = Arc::new(ProxyBackend::new(
        req.name.clone(),
        existing.agent_dir().to_path_buf(),
        existing.url().to_string(),
        Arc::new(tokio::sync::RwLock::new(None)),
        AuthMethod::Headers(header_secrets),
    ));
    {
        let mut proxies = proxies_lock.write().await;
        proxies.insert(req.name.clone(), Arc::clone(&replacement));
    }

    // Best-effort connect in the background: connect() self-logs and records the
    // outcome, so the live status reflects reality without blocking this request.
    let connect_client = match right_mcp::ssrf::hardened_client_builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(e) => return internal_error(format!("reqwest client build: {e:#}")).into_response(),
    };
    tokio::spawn(async move {
        let _ = replacement.connect(connect_client).await;
    });

    (StatusCode::OK, Json(McpSetHeadersResponse { ok: true })).into_response()
```

(The old `connect_client` build at lines 660-669 and the `if let Err(e) = replacement.connect(...)` gate at lines 670-679 are removed — the code above replaces them entirely.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `devenv shell -- cargo test -p right mcp_set_headers_persists_against_unreachable_server`
Expected: PASS.

- [ ] **Step 5: Update `mcp_set_headers_replaces_existing_header_names` for async connect**

The background connect means `/mcp-list` no longer shows `connected`
synchronously right after `set-headers`. Replace that test's final
`/mcp-list` block (from `let (status, body) = send_json(app, "/mcp-list", …)`
through the trailing `new_conn` assertion) with a poll (the same real-time
poll pattern used by `right-mcp`'s `unreachable_backend_recovers_to_connected_on_probe`):
```rust
    // The reconnect now happens in the background; poll until it lands.
    let mut nango = serde_json::Value::Null;
    for _ in 0..50 {
        let (status, body) = send_json(
            app.clone(),
            "/mcp-list",
            serde_json::json!({ "agent": "test-agent" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body={body}");
        nango = body["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["name"] == "nango")
            .cloned()
            .unwrap();
        if nango["status"] == "connected" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(nango["status"], "connected", "background reconnect should land: {nango}");
    assert_eq!(nango["header_names"], serde_json::json!(["connection-id"]));

    let (_, body) = send_json(app, "/mcp-list", serde_json::json!({ "agent": "test-agent" })).await;
    assert!(
        !body.to_string().contains("new_conn"),
        "list response must not expose header values: {body}"
    );
```

Run: `devenv shell -- cargo test -p right mcp_set_headers`
Expected: PASS — `mcp_set_headers_replaces_existing_header_names` (now polling), `mcp_set_headers_rejects_empty_headers`, and `mcp_set_headers_persists_against_unreachable_server` all green.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/internal_api.rs
git commit -m "fix(right): persist MCP headers even when the upstream is unreachable"
```

---

## Task 4: Surface last-attempt fields via `/mcp-list` (`right` + `right-mcp`)

**Files:**
- Modify: `crates/right/src/internal_api.rs` — `McpServerStatus` (line 108), `handle_mcp_list` push sites (lines 925, 981), test in `mod tests`.
- Modify: `crates/right-mcp/src/internal_client.rs` — `McpServerStatus` (line 440).

- [ ] **Step 1: Write the failing test**

Add to `crates/right/src/internal_api.rs` inside `mod tests`:
```rust
#[tokio::test]
async fn mcp_list_exposes_last_connect_error_for_unreachable() {
    setup_crypto();
    let tmp = tempfile::tempdir().unwrap();
    let (app, dispatcher) = make_test_router_and_dispatcher(tmp.path()).await;
    let agent_dir = tmp.path().join("agents/test-agent");
    let dead_url = "http://127.0.0.1:1/mcp".to_string();

    {
        let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
        credentials::db_add_server(&conn, "obsidian", &dead_url)
            .await
            .unwrap();
    }
    let backend = Arc::new(ProxyBackend::new(
        "obsidian".into(),
        agent_dir.clone(),
        dead_url,
        Arc::new(tokio::sync::RwLock::new(None)),
        AuthMethod::default(),
    ));
    // Run the failed connect synchronously so the recorded fields are present.
    let _ = backend.connect(reqwest::Client::new()).await;
    let proxies = Arc::clone(&dispatcher.agents.get("test-agent").unwrap().proxies);
    proxies.write().await.insert("obsidian".into(), backend);

    let (status, body) =
        send_json(app, "/mcp-list", serde_json::json!({ "agent": "test-agent" })).await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let obsidian = body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["name"] == "obsidian")
        .unwrap();
    assert!(obsidian["last_connect_error"].is_string(), "{obsidian}");
    assert!(obsidian["last_attempt_at"].is_string(), "{obsidian}");
    assert!(obsidian["last_success_at"].is_null(), "{obsidian}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devenv shell -- cargo test -p right mcp_list_exposes_last_connect_error_for_unreachable`
Expected: FAIL — `last_connect_error` is `null` (field not yet on `McpServerStatus`).

- [ ] **Step 3: Add the fields to both `McpServerStatus` copies**

In `crates/right/src/internal_api.rs`, extend `pub(crate) struct McpServerStatus` (line 108) — add after `header_names`:
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connect_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<String>,
```

In `crates/right-mcp/src/internal_client.rs`, extend `pub struct McpServerStatus` (line 440) — add after `header_names`:
```rust
    #[serde(default)]
    pub last_connect_error: Option<String>,
    #[serde(default)]
    pub last_attempt_at: Option<String>,
    #[serde(default)]
    pub last_success_at: Option<String>,
```

- [ ] **Step 4: Populate the fields in `handle_mcp_list`**

In `crates/right/src/internal_api.rs`, the "right" built-in push (line 925) — add after `header_names: Vec::new(),`:
```rust
        last_connect_error: None,
        last_attempt_at: None,
        last_success_at: None,
```

The external-proxy push (line 981) — add after `header_names: db_header_names.get(name).cloned().unwrap_or_default(),`:
```rust
            last_connect_error: proxy.last_connect_error().await,
            last_attempt_at: proxy.last_attempt_at().await.map(|t| t.to_rfc3339()),
            last_success_at: proxy.last_success_at().await.map(|t| t.to_rfc3339()),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `devenv shell -- cargo test -p right mcp_list_exposes_last_connect_error_for_unreachable`
Expected: PASS.

- [ ] **Step 6: Run the broader mcp-list + client tests (no regression)**

Run: `devenv shell -- cargo test -p right -p right-mcp mcp_list`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right/src/internal_api.rs crates/right-mcp/src/internal_client.rs
git commit -m "feat(right): expose last connect attempt/error in /mcp-list"
```

---

## Task 5: Render the failure cause in the dashboard (`right-dashboard`)

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts` — `McpServerSummary` (line 31).
- Modify: `crates/right-dashboard/frontend/src/views/mcpViewModel.ts` — add `relativeAgo` + `mcpStatusDetail`.
- Modify: `crates/right-dashboard/frontend/src/views/McpView.vue` — import + render the detail.
- Modify: `crates/right-dashboard/frontend/src/views/McpView.test.ts` — unit tests.

- [ ] **Step 1: Write the failing tests**

Add to `crates/right-dashboard/frontend/src/views/McpView.test.ts` (extend the import from `./mcpViewModel` to include `mcpStatusDetail, relativeAgo`):
```ts
describe('mcpStatusDetail', () => {
  const now = new Date('2026-06-01T12:00:30Z')

  it('returns null for connected servers', () => {
    expect(
      mcpStatusDetail(
        { status: 'connected', last_connect_error: 'x', last_attempt_at: '2026-06-01T12:00:00Z' },
        now,
      ),
    ).toBeNull()
  })

  it('combines error and last-tried for unreachable', () => {
    expect(
      mcpStatusDetail(
        { status: 'unreachable', last_connect_error: 'connection refused', last_attempt_at: '2026-06-01T12:00:18Z' },
        now,
      ),
    ).toBe('connection refused · last tried 12s ago')
  })

  it('returns null when no cause recorded', () => {
    expect(mcpStatusDetail({ status: 'unreachable' }, now)).toBeNull()
  })
})

describe('relativeAgo', () => {
  const now = new Date('2026-06-01T12:00:30Z')
  it('formats seconds and minutes', () => {
    expect(relativeAgo('2026-06-01T12:00:18Z', now)).toBe('12s ago')
    expect(relativeAgo('2026-06-01T11:58:30Z', now)).toBe('2m ago')
  })
  it('returns null for unparseable input', () => {
    expect(relativeAgo('not-a-date', now)).toBeNull()
  })
})
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && npm test -- McpView'`
Expected: FAIL — `mcpStatusDetail`/`relativeAgo` are not exported.

- [ ] **Step 3: Implement the helpers**

Add to `crates/right-dashboard/frontend/src/views/mcpViewModel.ts`:
```ts
export interface McpStatusDetailInput {
  status: string
  last_connect_error?: string | null
  last_attempt_at?: string | null
}

export function relativeAgo(isoTimestamp: string, now: Date): string | null {
  const then = Date.parse(isoTimestamp)
  if (Number.isNaN(then)) {
    return null
  }
  const seconds = Math.max(0, Math.round((now.getTime() - then) / 1000))
  if (seconds < 60) {
    return `${seconds}s ago`
  }
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) {
    return `${minutes}m ago`
  }
  const hours = Math.round(minutes / 60)
  return `${hours}h ago`
}

export function mcpStatusDetail(server: McpStatusDetailInput, now: Date): string | null {
  if (server.status.toLowerCase() === 'connected') {
    return null
  }
  const parts: string[] = []
  if (server.last_connect_error) {
    parts.push(server.last_connect_error)
  }
  if (server.last_attempt_at) {
    const ago = relativeAgo(server.last_attempt_at, now)
    if (ago) {
      parts.push(`last tried ${ago}`)
    }
  }
  return parts.length > 0 ? parts.join(' · ') : null
}
```

- [ ] **Step 4: Add the type fields**

In `crates/right-dashboard/frontend/src/types.ts`, extend `McpServerSummary` (line 31) — add after `protected: boolean`:
```ts
  last_connect_error?: string | null
  last_attempt_at?: string | null
  last_success_at?: string | null
```

- [ ] **Step 5: Render the detail in the view**

In `crates/right-dashboard/frontend/src/views/McpView.vue`, add `mcpStatusDetail` to the existing import from `./mcpViewModel`. Then, in the template, add a detail line after the `<small>{{ server.auth_type ?? 'built-in' }}</small>` line (line 477) inside `.row-side`:
```html
          <small v-if="mcpStatusDetail(server, new Date())" class="status-detail">
            {{ mcpStatusDetail(server, new Date()) }}
          </small>
```

- [ ] **Step 6: Run tests + typecheck to verify they pass**

Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && npm test -- McpView && npm run typecheck'`
Expected: PASS for both.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts \
        crates/right-dashboard/frontend/src/views/mcpViewModel.ts \
        crates/right-dashboard/frontend/src/views/McpView.vue \
        crates/right-dashboard/frontend/src/views/McpView.test.ts
git commit -m "feat(dashboard): show MCP connection failure cause and last-tried time"
```

---

## Task 6: Cite-on-touch doc update

**Files:**
- Modify: `docs/architecture/mcp.md`

- [ ] **Step 1: Update the MCP satellite doc**

In `docs/architecture/mcp.md`, add a short note (find the section describing MCP status / set-headers; if none, add under the aggregator/status discussion):
> **`set-headers` decoupling:** `handle_mcp_set_headers` persists the
> header before connecting and swaps in a fresh `Unreachable` backend; the
> live connection is best-effort (background `connect()`), and the health
> reconciler re-probes with the new credential. `ProxyBackend` records
> `last_attempt_at` / `last_success_at` / `last_connect_error` (redacted via
> `redact_query_strings`), surfaced through `/mcp-list` for the dashboard.

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/mcp.md
git commit -m "docs(architecture): note set-headers decoupling + connect observability"
```

---

## Final Verification (mandatory)

- [ ] **Step 1: Full workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS (record any pre-existing failures noted in the Baseline as out of scope).

- [ ] **Step 2: Full dashboard test + typecheck**

Run: `devenv shell -- bash -c 'cd crates/right-dashboard/frontend && npm test && npm run typecheck'`
Expected: PASS.

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: builds clean.
