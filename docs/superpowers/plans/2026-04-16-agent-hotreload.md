# Agent Hot-Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two bugs that prevent hot-adding agents: bot fails on unmigrated DB, aggregator doesn't register new agents at runtime.

**Architecture:** Bot gains `migrate: true` on its own `data.db`. Aggregator gains a `/reload` endpoint on its internal Unix socket API that re-reads `agent-tokens.json` and registers new agents in memory.

**Tech Stack:** Rust, axum, serde_json, tokio, hyper (UDS client)

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/rightclaw/src/mcp/internal_client.rs` | Modify | Add `ReloadResponse` type + `reload()` method |
| `crates/rightclaw-cli/src/internal_api.rs` | Modify | Add `token_map`, `token_map_path`, `agents_dir` to `InternalState`; add `/reload` route + handler |
| `crates/rightclaw-cli/src/aggregator.rs` | Modify | Thread new fields through `run_aggregator_http()` → `internal_router()` |
| `crates/rightclaw-cli/src/main.rs` | Modify | Pass `token_map_path` to aggregator; call `/reload` in `cmd_reload` |
| `crates/bot/src/lib.rs` | Modify | `migrate: false` → `true` (line 173) |
| `crates/rightclaw/src/memory/mod.rs` | Modify | Update `open_connection` doc-comment |
| `ARCHITECTURE.md` | Modify | Update Migration Ownership section |

---

### Task 1: Bot migrates its own DB

**Files:**
- Modify: `crates/bot/src/lib.rs:173`
- Modify: `crates/rightclaw/src/memory/mod.rs:29-32`
- Modify: `ARCHITECTURE.md:438-440`

- [ ] **Step 1: Change migrate flag in bot**

In `crates/bot/src/lib.rs:173`, change:

```rust
    let _conn = open_connection(&agent_dir, false)
```

to:

```rust
    let _conn = open_connection(&agent_dir, true)
```

- [ ] **Step 2: Update `open_connection` doc-comment**

In `crates/rightclaw/src/memory/mod.rs:29-32`, replace:

```rust
/// Only the MCP aggregator should pass `migrate: true`. Bot processes must
/// pass `migrate: false` — they depend on the aggregator starting first.
```

with:

```rust
/// Both the MCP aggregator and bot processes pass `migrate: true` for their
/// per-agent databases. Migrations are idempotent so concurrent callers are safe.
```

- [ ] **Step 3: Update ARCHITECTURE.md Migration Ownership section**

In `ARCHITECTURE.md:438-440`, replace:

```
### Migration Ownership

Only the MCP aggregator (`right-mcp-server`) runs schema migrations via `open_connection(path, migrate: true)`. All other processes (bots, CLI commands, runtime code) open the database with `migrate: false`. Bot processes declare `depends_on: right-mcp-server: condition: process_started` in process-compose to ensure the aggregator migrates before bots start.
```

with:

```
### Migration Ownership

Both the MCP aggregator (`right-mcp-server`) and bot processes run schema migrations on per-agent `data.db` via `open_connection(path, migrate: true)`. Migrations are idempotent — concurrent callers are safe (WAL mode + busy_timeout). CLI commands and other processes open with `migrate: false`. Bot processes still declare `depends_on: right-mcp-server` for MCP readiness, but no longer depend on it for schema migrations.
```

- [ ] **Step 4: Build to verify**

Run: `cargo build --workspace`
Expected: clean build, no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/lib.rs crates/rightclaw/src/memory/mod.rs ARCHITECTURE.md
git commit -m "fix(bot): migrate own data.db instead of depending on aggregator"
```

---

### Task 2: Add `ReloadResponse` type and `reload()` method to InternalClient

**Files:**
- Modify: `crates/rightclaw/src/mcp/internal_client.rs`

- [ ] **Step 1: Write test for `ReloadResponse` deserialization**

Add to the `#[cfg(test)] mod tests` block in `crates/rightclaw/src/mcp/internal_client.rs`:

```rust
    #[test]
    fn reload_response_deserializes() {
        let json = r#"{"added":["him","test"],"total":3}"#;
        let resp: ReloadResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.added, vec!["him", "test"]);
        assert_eq!(resp.total, 3);
    }

    #[test]
    fn reload_response_empty_added() {
        let json = r#"{"added":[],"total":2}"#;
        let resp: ReloadResponse = serde_json::from_str(json).unwrap();
        assert!(resp.added.is_empty());
        assert_eq!(resp.total, 2);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib mcp::internal_client::tests::reload_response`
Expected: FAIL — `ReloadResponse` not found.

- [ ] **Step 3: Add `ReloadResponse` type and `reload()` method**

In `crates/rightclaw/src/mcp/internal_client.rs`, add the response type in the "Response types" section (after `McpInstructionsResponse`):

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct ReloadResponse {
    pub added: Vec<String>,
    pub total: usize,
}
```

Note: `Serialize` is needed because the server side (in `internal_api.rs`) will also use this type for the JSON response. Add `Serialize` to the derive.

Add the `reload()` method to `impl InternalClient` (after the `set_token` method):

```rust
    /// Tell the aggregator to re-read agent-tokens.json and register new agents.
    pub async fn reload(&self) -> Result<ReloadResponse, InternalClientError> {
        self.post("/reload", &serde_json::json!({})).await
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib mcp::internal_client::tests::reload_response`
Expected: PASS — both tests green.

- [ ] **Step 5: Build workspace**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/mcp/internal_client.rs
git commit -m "feat(core): add ReloadResponse type and reload() to InternalClient"
```

---

### Task 3: Add `/reload` endpoint to internal API

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Write test for `/reload` endpoint — no new agents**

Add to `#[cfg(test)] mod tests` in `crates/rightclaw-cli/src/internal_api.rs`:

```rust
    #[tokio::test]
    async fn reload_no_new_agents() {
        let tmp = tempfile::tempdir().unwrap();

        // Write a token map that matches the existing test-agent
        let token_map_path = tmp.path().join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"test-agent": "tok-test"}).to_string(),
        ).unwrap();

        let dispatcher = make_test_dispatcher(tmp.path());
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert("tok-test".into(), crate::aggregator::AgentInfo {
                name: "test-agent".into(),
                dir: tmp.path().join("agents/test-agent"),
            });
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        let app = internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            tmp.path().join("agents"),
        );

        let (status, body) = send_json(app, "/reload", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["added"].as_array().unwrap().is_empty());
        assert_eq!(body["total"], 1);
    }
```

- [ ] **Step 2: Write test for `/reload` endpoint — new agent added**

Add to the same test module:

```rust
    #[tokio::test]
    async fn reload_registers_new_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");

        // Create two agent directories with data.db
        let agent1_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent1_dir).unwrap();
        rightclaw::memory::open_db(&agent1_dir, true).unwrap();

        let agent2_dir = agents_dir.join("new-agent");
        std::fs::create_dir_all(&agent2_dir).unwrap();
        rightclaw::memory::open_db(&agent2_dir, true).unwrap();

        // Token map on disk has both agents
        let token_map_path = tmp.path().join("agent-tokens.json");
        std::fs::write(
            &token_map_path,
            serde_json::json!({"test-agent": "tok-test", "new-agent": "tok-new"}).to_string(),
        ).unwrap();

        // Dispatcher only has test-agent registered
        let dispatcher = make_test_dispatcher(tmp.path());

        // In-memory token map only has test-agent
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert("tok-test".into(), crate::aggregator::AgentInfo {
                name: "test-agent".into(),
                dir: agent1_dir,
            });
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };

        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        let app = internal_router(
            dispatcher.clone(),
            refresh_senders,
            reconnect_managers,
            token_map.clone(),
            token_map_path,
            agents_dir,
        );

        let (status, body) = send_json(app, "/reload", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);

        let added = body["added"].as_array().unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0], "new-agent");
        assert_eq!(body["total"], 2);

        // Verify dispatcher has the new agent
        assert!(dispatcher.agents.contains_key("new-agent"));

        // Verify token map has the new token
        let map = token_map.read().await;
        assert!(map.contains_key("tok-new"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rightclaw-cli --lib internal_api::tests::reload`
Expected: FAIL — `internal_router` wrong number of arguments.

- [ ] **Step 4: Add state fields and `/reload` handler**

In `crates/rightclaw-cli/src/internal_api.rs`:

**4a.** Add import at top of file:

```rust
use std::path::PathBuf;
```

**4b.** Add `ReloadResponse` import. Change the line:

```rust
use crate::aggregator::{ReconnectManagers, RefreshSenders, ToolDispatcher};
```

to:

```rust
use crate::aggregator::{AgentInfo, AgentTokenMap, ReconnectManagers, RefreshSenders, ToolDispatcher};
```

**4c.** Add fields to `InternalState`:

```rust
#[derive(Clone)]
pub(crate) struct InternalState {
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map: AgentTokenMap,
    token_map_path: PathBuf,
    agents_dir: PathBuf,
}
```

**4d.** Update `internal_router` signature and construction:

```rust
pub(crate) fn internal_router(
    dispatcher: Arc<ToolDispatcher>,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map: AgentTokenMap,
    token_map_path: PathBuf,
    agents_dir: PathBuf,
) -> Router {
    let state = InternalState {
        dispatcher,
        refresh_senders,
        reconnect_managers,
        token_map,
        token_map_path,
        agents_dir,
    };
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
        .route("/reload", post(handle_reload))
        .with_state(state)
}
```

**4e.** Add the handler (after `handle_mcp_instructions`, before the `#[cfg(test)]` module):

```rust
async fn handle_reload(
    State(state): State<InternalState>,
) -> axum::response::Response {
    // 1. Read token map from disk
    let content = match std::fs::read_to_string(&state.token_map_path) {
        Ok(c) => c,
        Err(e) => return internal_error(format!("read token map: {e:#}")).into_response(),
    };
    let disk_entries: std::collections::HashMap<String, String> = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => return internal_error(format!("parse token map: {e:#}")).into_response(),
    };

    // 2. Find new agents (in disk but not in dispatcher)
    let mut added = Vec::new();
    for (agent_name, token) in &disk_entries {
        if state.dispatcher.agents.contains_key(agent_name) {
            continue;
        }

        let agent_dir = state.agents_dir.join(agent_name);
        if !agent_dir.exists() {
            tracing::warn!(agent = agent_name.as_str(), "reload: agent dir missing, skipping");
            continue;
        }

        // Determine mTLS dir for sandbox agents
        let agent_config = rightclaw::agent::discovery::parse_agent_config(&agent_dir)
            .ok()
            .flatten();
        let mtls_dir = match &agent_config {
            Some(config)
                if *config.sandbox_mode() == rightclaw::agent::SandboxMode::Openshell =>
            {
                match rightclaw::openshell::preflight_check() {
                    rightclaw::openshell::OpenShellStatus::Ready(dir) => Some(dir),
                    _ => None,
                }
            }
            _ => None,
        };

        // Create backend registry
        let right = crate::right_backend::RightBackend::new(state.agents_dir.clone(), mtls_dir);
        let registry = crate::aggregator::BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            agent_dir: agent_dir.clone(),
            hindsight: None,
        };
        state.dispatcher.agents.insert(agent_name.clone(), registry);

        // Add to in-memory token map
        {
            let mut map = state.token_map.write().await;
            map.insert(token.clone(), AgentInfo {
                name: agent_name.clone(),
                dir: agent_dir,
            });
        }

        added.push(agent_name.clone());
        tracing::info!(agent = agent_name.as_str(), "reload: registered new agent");
    }

    let total = state.dispatcher.agents.len();
    (
        StatusCode::OK,
        Json(rightclaw::mcp::internal_client::ReloadResponse { added, total }),
    ).into_response()
}
```

- [ ] **Step 5: Update `make_test_router` helper in tests**

In the test module, update `make_test_router`:

```rust
    fn make_test_router(tmp: &std::path::Path) -> Router {
        let dispatcher = make_test_dispatcher(tmp);
        let refresh_senders: RefreshSenders = Arc::new(std::collections::HashMap::new());
        let reconnect_managers: ReconnectManagers = Arc::new(std::collections::HashMap::new());

        // For existing tests: create a token map file and in-memory map
        let token_map_path = tmp.join("agent-tokens.json");
        if !token_map_path.exists() {
            std::fs::write(
                &token_map_path,
                serde_json::json!({"test-agent": "tok-test"}).to_string(),
            ).unwrap();
        }
        let token_map: crate::aggregator::AgentTokenMap = {
            let mut map = std::collections::HashMap::new();
            map.insert("tok-test".into(), crate::aggregator::AgentInfo {
                name: "test-agent".into(),
                dir: tmp.join("agents/test-agent"),
            });
            std::sync::Arc::new(tokio::sync::RwLock::new(map))
        };

        internal_router(
            dispatcher,
            refresh_senders,
            reconnect_managers,
            token_map,
            token_map_path,
            tmp.join("agents"),
        )
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw-cli --lib internal_api::tests`
Expected: all tests pass (existing + two new reload tests).

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs
git commit -m "feat(aggregator): add /reload endpoint to internal API"
```

---

### Task 4: Thread new state through aggregator and main

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs:495-539`
- Modify: `crates/rightclaw-cli/src/main.rs:442-732` (McpServer command) and `cmd_reload`

- [ ] **Step 1: Update `run_aggregator_http` to accept and pass `token_map_path`**

In `crates/rightclaw-cli/src/aggregator.rs`, change the `run_aggregator_http` signature to add `token_map_path: PathBuf`:

```rust
pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    token_map_path: PathBuf,
) -> miette::Result<()> {
```

Update the `internal_router` call at line 539 to pass the new fields:

```rust
    let internal_app = crate::internal_api::internal_router(
        dispatcher,
        refresh_senders,
        reconnect_managers,
        token_map.clone(),
        token_map_path,
        agents_dir.clone(),
    );
```

Note: `token_map` needs `.clone()` because it's also used by the bearer auth middleware above. `agents_dir` needs `.clone()` because it's used in the log statement below.

- [ ] **Step 2: Update call site in main.rs**

In `crates/rightclaw-cli/src/main.rs`, at the `run_aggregator_http` call (around line 730), change:

```rust
            aggregator::run_aggregator_http(
                port, token_map, dispatcher, agents_dir, home, refresh_senders, reconnect_managers,
            ).await
```

to:

```rust
            let token_map_path_owned = token_map_arg.to_path_buf();
            aggregator::run_aggregator_http(
                port, token_map, dispatcher, agents_dir, home, refresh_senders, reconnect_managers, token_map_path_owned,
            ).await
```

The `token_map` CLI arg variable has been shadowed by the in-memory `token_map` map at this point. We need to capture the original `PathBuf` before shadowing. Find the `Commands::McpServer { port, ref token_map }` match arm (line 442) and rename the binding:

```rust
        Commands::McpServer { port, ref token_map: ref token_map_arg } => {
```

Wait — that syntax won't work. Instead, capture the path before shadowing. At line 442:

```rust
        Commands::McpServer { port, ref token_map } => {
```

The variable `token_map` is `&PathBuf` from the CLI arg. It gets shadowed at line 450-462 by the in-memory `AgentTokenMap`. Save the path before shadowing. After line 443 (`let agents_dir = ...`), add:

```rust
            let token_map_path = token_map.clone();
```

Then at the `run_aggregator_http` call, pass `token_map_path`:

```rust
            aggregator::run_aggregator_http(
                port, token_map, dispatcher, agents_dir, home, refresh_senders, reconnect_managers, token_map_path,
            ).await
```

- [ ] **Step 3: Add `/reload` call to `cmd_reload`**

In `crates/rightclaw-cli/src/main.rs`, in `cmd_reload` (line 1615), after `client.reload_configuration().await?;`, add:

```rust
    // Notify aggregator to pick up new agents from updated token map
    let socket_path = home.join("run/internal.sock");
    let internal = rightclaw::mcp::InternalClient::new(&socket_path);
    match internal.reload().await {
        Ok(resp) => {
            if !resp.added.is_empty() {
                println!(
                    "Registered {} new agent(s) in aggregator: {}",
                    resp.added.len(),
                    resp.added.join(", "),
                );
            }
        }
        Err(e) => {
            eprintln!("warning: failed to reload aggregator: {e:#}");
        }
    }
```

- [ ] **Step 4: Build workspace**

Run: `cargo build --workspace`
Expected: clean build, no errors.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs crates/rightclaw-cli/src/main.rs
git commit -m "feat(reload): wire aggregator /reload through CLI reload command"
```
