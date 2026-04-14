# MCP Reconnect on Restart — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** External MCP servers reconnect automatically when the Aggregator restarts, and the OAuth refresh scheduler runs in the Aggregator process where it can update ProxyBackend tokens in-memory.

**Architecture:** Move refresh scheduler from bot to Aggregator. On startup, iterate restored ProxyBackends: refresh expired OAuth tokens via `do_refresh()`, then `connect()`. Send `RefreshMessage::NewEntry` to per-agent schedulers for future refreshes. Clean up dead refresh code from bot.

**Tech Stack:** Rust, tokio, reqwest, SQLite (rusqlite), rmcp

---

### Task 1: Make `do_refresh` public

The startup reconnect needs to call `do_refresh()` for OAuth servers with expired tokens. Currently it's private.

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs:239`

- [ ] **Step 1: Change visibility of `do_refresh`**

In `crates/rightclaw/src/mcp/refresh.rs`, change line 239:

```rust
// Before:
async fn do_refresh(
// After:
pub async fn do_refresh(
```

- [ ] **Step 2: Verify build**

Run: `devenv shell -- cargo check -p rightclaw`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs
git commit -m "refactor: make do_refresh public for startup reconnect"
```

---

### Task 2: Remove `notify_tx` parameter from `run_refresh_scheduler`

The Aggregator has no Telegram access. Replace the `notify_tx: Sender<String>` parameter with `tracing::warn!` at the call site.

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs:91-94` (signature)
- Modify: `crates/rightclaw/src/mcp/refresh.rs:229` (usage)
- Modify: `crates/bot/src/lib.rs:236-239` (caller)

- [ ] **Step 1: Remove `notify_tx` from scheduler signature and replace usage**

In `crates/rightclaw/src/mcp/refresh.rs`, change the function signature:

```rust
// Before:
pub async fn run_refresh_scheduler(
    agent_dir: std::path::PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
    notify_tx: tokio::sync::mpsc::Sender<String>,
) {

// After:
pub async fn run_refresh_scheduler(
    agent_dir: std::path::PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
) {
```

And replace the `notify_tx.send()` call (line ~229):

```rust
// Before:
                    Err(e) => {
                        tracing::error!(server = %name, "token refresh failed after retries: {e:#}");
                        timers.remove(&name);
                        let _ = notify_tx.send(format!("OAuth refresh failed for {name}: {e:#}")).await;
                    }

// After:
                    Err(e) => {
                        tracing::warn!(server = %name, "token refresh failed after retries: {e:#}");
                        timers.remove(&name);
                    }
```

- [ ] **Step 2: Update bot caller**

In `crates/bot/src/lib.rs`, update the scheduler spawn (lines ~235-240):

```rust
// Before:
    // Spawn OAuth refresh scheduler
    tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
        agent_dir.clone(),
        refresh_rx,
        notify_refresh_tx,
    ));

// After:
    // Spawn OAuth refresh scheduler
    tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
        agent_dir.clone(),
        refresh_rx,
    ));
```

- [ ] **Step 3: Verify build**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles (may have warnings about unused `notify_refresh_tx` — that's fine, cleaned up in Task 5)

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs crates/bot/src/lib.rs
git commit -m "refactor: remove notify_tx from refresh scheduler, use tracing instead"
```

---

### Task 3: Add `RefreshChannels` type to Aggregator and spawn per-agent schedulers

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Add type alias and modify `run_aggregator_http` to accept refresh senders**

In `crates/rightclaw-cli/src/aggregator.rs`, add after the existing `use` block (after line ~28):

```rust
use rightclaw::mcp::refresh::RefreshMessage;

/// Per-agent refresh scheduler sender map.
pub(crate) type RefreshSenders = Arc<std::collections::HashMap<String, tokio::sync::mpsc::Sender<RefreshMessage>>>;
```

Change `run_aggregator_http` signature:

```rust
// Before:
pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
) -> miette::Result<()> {

// After:
pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
    refresh_senders: RefreshSenders,
) -> miette::Result<()> {
```

Pass `refresh_senders` to `internal_router`:

```rust
// Before:
    let internal_app = crate::internal_api::internal_router(dispatcher);

// After:
    let internal_app = crate::internal_api::internal_router(dispatcher, refresh_senders);
```

- [ ] **Step 2: Verify build**

Run: `devenv shell -- cargo check -p rightclaw-cli`
Expected: compile errors about `internal_router` signature mismatch and caller in `main.rs` — that's expected, fixed in Tasks 4 and 5.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "refactor: add RefreshSenders type, thread through aggregator"
```

---

### Task 4: Thread `RefreshSenders` through `internal_router` and send `NewEntry` in `handle_set_token`

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Change router state to include refresh senders**

In `crates/rightclaw-cli/src/internal_api.rs`, add imports:

```rust
use crate::aggregator::RefreshSenders;
use rightclaw::mcp::refresh::{OAuthServerState, RefreshMessage};
```

Change `internal_router` to accept and pass `RefreshSenders`:

```rust
// Before:
pub(crate) fn internal_router(dispatcher: Arc<ToolDispatcher>) -> Router {
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
        .with_state(dispatcher)
}

// After:
#[derive(Clone)]
pub(crate) struct InternalState {
    pub dispatcher: Arc<ToolDispatcher>,
    pub refresh_senders: RefreshSenders,
}

pub(crate) fn internal_router(dispatcher: Arc<ToolDispatcher>, refresh_senders: RefreshSenders) -> Router {
    let state = InternalState { dispatcher, refresh_senders };
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .route("/mcp-list", post(handle_mcp_list))
        .route("/mcp-instructions", post(handle_mcp_instructions))
        .with_state(state)
}
```

- [ ] **Step 2: Update all handlers to extract from `InternalState`**

Each handler currently has `State(dispatcher): State<Arc<ToolDispatcher>>`. Change them all to:

```rust
State(state): State<InternalState>
```

And replace `dispatcher` references with `state.dispatcher` in the handler bodies. The handlers to update are:
- `handle_mcp_add`
- `handle_mcp_remove`
- `handle_set_token`
- `handle_mcp_list`
- `handle_mcp_instructions`

For example, `handle_mcp_add`:

```rust
// Before:
async fn handle_mcp_add(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpAddRequest>,
) -> axum::response::Response {
    // ...
    let Some(registry) = dispatcher.agents.get(&req.agent) else {

// After:
async fn handle_mcp_add(
    State(state): State<InternalState>,
    Json(req): Json<McpAddRequest>,
) -> axum::response::Response {
    // ...
    let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
```

Apply the same pattern to all five handlers.

- [ ] **Step 3: Add `RefreshMessage::NewEntry` send in `handle_set_token`**

After the existing reconnect spawn in `handle_set_token` (after line ~429), add:

```rust
    // Notify refresh scheduler so it schedules future token refreshes
    if let Some(tx) = state.refresh_senders.get(&req.agent) {
        let entry = OAuthServerState {
            refresh_token: Some(req.refresh_token.clone()),
            token_endpoint: req.token_endpoint.clone(),
            client_id: req.client_id.clone(),
            client_secret: req.client_secret.clone(),
            expires_at: expires_at,
            server_url: handle.url().to_string(),
        };
        let _ = tx.send(RefreshMessage::NewEntry {
            server_name: req.server.clone(),
            state: entry,
            token: handle.token().clone(),
        }).await;
    }
```

This requires access to `handle.url()` — check if `ProxyBackend` exposes `url`. If not, add a public getter.

- [ ] **Step 4: Add `url()` getter to `ProxyBackend` if needed**

Check `crates/rightclaw/src/mcp/proxy.rs` for a `url()` method. If missing, add:

```rust
    /// The upstream server URL.
    pub fn url(&self) -> &str {
        &self.url
    }
```

- [ ] **Step 5: Verify build**

Run: `devenv shell -- cargo check -p rightclaw-cli`
Expected: compile errors only from `main.rs` caller (fixed in Task 5)

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs crates/rightclaw/src/mcp/proxy.rs
git commit -m "feat: thread refresh senders through internal API, send NewEntry on set-token"
```

---

### Task 5: Startup reconnect + scheduler spawn in `main.rs`

This is the core fix. After creating `ProxyBackend` instances, spawn per-agent refresh schedulers and background reconnect tasks.

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (the `Commands::McpServer` arm, lines ~406-479)

- [ ] **Step 1: Create per-agent refresh channels and spawn schedulers**

Replace the `Commands::McpServer` block (lines ~406-479) with the updated version. After the existing `ProxyBackend` creation loop, add scheduler creation and startup reconnect:

```rust
        Commands::McpServer { port, ref token_map } => {
            let agents_dir = rightclaw::config::agents_dir(&home);
            let token_map_content = std::fs::read_to_string(token_map)
                .map_err(|e| miette::miette!("failed to read token map: {e:#}"))?;
            let token_entries: std::collections::HashMap<String, String> =
                serde_json::from_str(&token_map_content)
                    .map_err(|e| miette::miette!("failed to parse token map: {e:#}"))?;

            let token_map = {
                let mut map = std::collections::HashMap::new();
                for (agent_name, token) in &token_entries {
                    let agent_dir = agents_dir.join(agent_name);
                    map.insert(
                        token.clone(),
                        aggregator::AgentInfo {
                            name: agent_name.clone(),
                            dir: agent_dir,
                        },
                    );
                }
                std::sync::Arc::new(tokio::sync::RwLock::new(map))
            };

            let dispatcher = std::sync::Arc::new(aggregator::ToolDispatcher {
                agents: dashmap::DashMap::new(),
            });

            // Per-agent refresh scheduler senders
            let mut refresh_senders_map = std::collections::HashMap::new();

            // Register agents in dispatcher, restoring proxy backends from SQLite
            for (agent_name, _token) in &token_entries {
                let agent_dir = agents_dir.join(agent_name);
                let mtls_dir = match rightclaw::agent::discovery::parse_agent_config(&agent_dir) {
                    Ok(Some(config))
                        if *config.sandbox_mode() == rightclaw::agent::SandboxMode::Openshell =>
                    {
                        match rightclaw::openshell::preflight_check() {
                            rightclaw::openshell::OpenShellStatus::Ready(dir) => Some(dir),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let right = right_backend::RightBackend::new(agents_dir.clone(), mtls_dir);

                // Load existing MCP servers from SQLite and create ProxyBackends
                let mut proxies = std::collections::HashMap::new();
                let mut oauth_entries: Vec<(String, rightclaw::mcp::refresh::OAuthServerState, std::sync::Arc<tokio::sync::RwLock<Option<String>>>)> = Vec::new();

                if let Ok(conn) = rightclaw::memory::open_connection(&agent_dir) {
                    if let Ok(servers) = rightclaw::mcp::credentials::db_list_servers(&conn) {
                        for s in servers {
                            let auth_method = rightclaw::mcp::proxy::AuthMethod::from_db(
                                s.auth_type.as_deref(),
                                s.auth_header.as_deref(),
                            );
                            let token = std::sync::Arc::new(tokio::sync::RwLock::new(s.auth_token.clone()));
                            let backend = rightclaw::mcp::proxy::ProxyBackend::new(
                                s.name.clone(),
                                agent_dir.clone(),
                                s.url.clone(),
                                token.clone(),
                                auth_method.clone(),
                            );
                            let backend = std::sync::Arc::new(backend);
                            proxies.insert(s.name.clone(), std::sync::Arc::clone(&backend));

                            // Collect OAuth entries for refresh scheduling
                            if s.auth_type.as_deref() == Some("oauth") {
                                if let (Some(ref token_endpoint), Some(ref client_id), Some(ref expires_at_str)) =
                                    (&s.token_endpoint, &s.client_id, &s.expires_at)
                                {
                                    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_str)
                                        .map(|dt| dt.with_timezone(&chrono::Utc))
                                        .unwrap_or_else(|_| chrono::Utc::now());
                                    oauth_entries.push((
                                        s.name.clone(),
                                        rightclaw::mcp::refresh::OAuthServerState {
                                            refresh_token: s.refresh_token.clone(),
                                            token_endpoint: token_endpoint.clone(),
                                            client_id: client_id.clone(),
                                            client_secret: s.client_secret.clone(),
                                            expires_at,
                                            server_url: s.url.clone(),
                                        },
                                        token,
                                    ));
                                }
                            }
                        }
                    }
                }

                let registry = aggregator::BackendRegistry {
                    right,
                    proxies: std::sync::Arc::new(tokio::sync::RwLock::new(proxies.clone())),
                    agent_dir: agent_dir.clone(),
                };
                dispatcher.agents.insert(agent_name.clone(), registry);

                // Spawn per-agent refresh scheduler
                let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshMessage>(32);
                tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
                    agent_dir.clone(),
                    refresh_rx,
                ));

                // Send NewEntry for each OAuth server with a refresh_token
                for (server_name, state, token_arc) in &oauth_entries {
                    if state.refresh_token.is_some() {
                        let _ = refresh_tx.send(rightclaw::mcp::refresh::RefreshMessage::NewEntry {
                            server_name: server_name.clone(),
                            state: state.clone(),
                            token: std::sync::Arc::clone(token_arc),
                        }).await;
                    }
                }

                // Spawn background reconnect tasks for each server
                let http_client = reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new());

                for (server_name, backend) in &proxies {
                    // Find the OAuth entry for this server (if any)
                    let oauth_state = oauth_entries.iter()
                        .find(|(name, _, _)| name == server_name)
                        .map(|(_, state, _)| state.clone());

                    let backend = std::sync::Arc::clone(backend);
                    let server_name = server_name.clone();
                    let http_client = http_client.clone();
                    let agent_dir = agent_dir.clone();

                    tokio::spawn(async move {
                        if let Some(state) = oauth_state {
                            // OAuth server — check if token needs refresh first
                            let now = chrono::Utc::now();
                            if state.expires_at <= now {
                                // Token expired
                                if state.refresh_token.is_some() {
                                    // Try to refresh
                                    match rightclaw::mcp::refresh::do_refresh(&http_client, &state, 3).await {
                                        Ok((new_state, access_token)) => {
                                            // Update in-memory token
                                            *backend.token().write().await = Some(access_token.clone());
                                            // Persist to SQLite
                                            if let Ok(conn) = rightclaw::memory::open_connection(&agent_dir) {
                                                let _ = rightclaw::mcp::credentials::db_update_oauth_token(
                                                    &conn,
                                                    &server_name,
                                                    &access_token,
                                                    &new_state.expires_at.to_rfc3339(),
                                                );
                                            }
                                            tracing::info!(server = %server_name, "refreshed expired token on startup");
                                        }
                                        Err(e) => {
                                            tracing::warn!(server = %server_name, "startup refresh failed, marking NeedsAuth: {e:#}");
                                            backend.set_status(rightclaw::mcp::proxy::BackendStatus::NeedsAuth).await;
                                            return;
                                        }
                                    }
                                } else {
                                    // No refresh token, can't recover
                                    tracing::warn!(server = %server_name, "OAuth token expired, no refresh_token — marking NeedsAuth");
                                    backend.set_status(rightclaw::mcp::proxy::BackendStatus::NeedsAuth).await;
                                    return;
                                }
                            }
                        }

                        // Connect (token is either valid, refreshed, or non-OAuth)
                        match backend.connect(http_client).await {
                            Ok(_) => tracing::info!(server = %server_name, "reconnected on startup"),
                            Err(e) => tracing::warn!(server = %server_name, "startup connect failed: {e:#}"),
                        }
                    });
                }

                refresh_senders_map.insert(agent_name.clone(), refresh_tx);
            }

            let refresh_senders = std::sync::Arc::new(refresh_senders_map);
            aggregator::run_aggregator_http(port, token_map, dispatcher, agents_dir, home, refresh_senders).await
        }
```

- [ ] **Step 2: Verify build**

Run: `devenv shell -- cargo check -p rightclaw-cli`
Expected: compiles with no errors

- [ ] **Step 3: Run existing tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all existing tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: reconnect MCP servers on startup, spawn per-agent refresh schedulers"
```

---

### Task 6: Clean up bot — remove refresh scheduler code

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Remove refresh channels, scheduler spawn, and notify forwarder from `lib.rs`**

In `crates/bot/src/lib.rs`, remove lines ~201-252 (the refresh-related block). Specifically remove:

```rust
    // These lines should be REMOVED:

    // Create refresh scheduler channels
    let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshMessage>(32);
    let (notify_refresh_tx, mut notify_refresh_rx) = tokio::sync::mpsc::channel::<String>(32);

    let refresh_tx_for_handler = refresh_tx.clone();
```

And the scheduler spawn + notify forwarder (lines ~235-252):

```rust
    // These lines should be REMOVED:

    // Spawn OAuth refresh scheduler
    tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
        agent_dir.clone(),
        refresh_rx,
    ));

    // Forward refresh error notifications to Telegram
    let bot_for_notify = teloxide::Bot::new(token.clone());
    let ids_for_notify: Vec<i64> = config.allowed_chat_ids.clone();
    tokio::spawn(async move {
        use teloxide::requests::Requester as _;
        while let Some(msg) = notify_refresh_rx.recv().await {
            for &chat_id in &ids_for_notify {
                let _ = bot_for_notify.send_message(teloxide::types::ChatId(chat_id), &msg).await;
            }
        }
    });
```

And update `run_telegram` call to remove `refresh_tx_for_handler` argument (line ~431).

- [ ] **Step 2: Remove `refresh_tx` from `run_telegram` signature in `dispatch.rs`**

In `crates/bot/src/telegram/dispatch.rs`, remove `refresh_tx` parameter from `run_telegram`:

```rust
// Before (line ~69):
    refresh_tx: tokio::sync::mpsc::Sender<rightclaw::mcp::refresh::RefreshMessage>,

// After: remove this line entirely
```

Remove `refresh_tx_arc` wrapping (line ~104):

```rust
// Remove:
    let refresh_tx_arc: Arc<RefreshTx> = Arc::new(RefreshTx(refresh_tx));
```

Remove from `dptree::deps!` (line ~175):

```rust
// Remove:
            Arc::clone(&refresh_tx_arc),
```

Remove `RefreshTx` from the import line (line ~25).

- [ ] **Step 3: Remove `RefreshTx` from `handler.rs`**

In `crates/bot/src/telegram/handler.rs`, remove the `RefreshTx` struct definition (lines ~68-70):

```rust
// Remove:
/// Channel sender for notifying the refresh scheduler about server removals.
#[derive(Clone)]
pub struct RefreshTx(pub tokio::sync::mpsc::Sender<rightclaw::mcp::refresh::RefreshMessage>);
```

- [ ] **Step 4: Verify build**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles with no errors

- [ ] **Step 5: Run all tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs
git commit -m "cleanup: remove refresh scheduler from bot process"
```

---

### Task 7: Final build and verify

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: compiles with no errors

- [ ] **Step 2: Run clippy**

Run: `devenv shell -- cargo clippy --workspace`
Expected: no warnings (or only pre-existing ones)

- [ ] **Step 3: Run all tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass

- [ ] **Step 4: Commit any fixups if needed**
