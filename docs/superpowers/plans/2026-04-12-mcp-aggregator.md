# MCP Aggregator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace HttpMemoryServer with an MCP Aggregator that proxies external MCP servers, hiding credentials from agent sandboxes.

**Architecture:** Shared process with enum dispatch. Aggregator (rmcp ServerHandler) → ToolDispatcher (prefix routing) → BackendRegistry (per-agent). RightBackend handles built-in tools in-process. ProxyBackend proxies to remote HTTP MCP servers via DynamicAuthClient (zero-downtime token rotation). Internal REST API on Unix domain socket for bot→aggregator IPC.

**Tech Stack:** rmcp 1.3 (server + client), axum (HTTP + UDS), tokio, rusqlite, hyper (UDS client), serde_json

**Spec:** `docs/superpowers/specs/2026-04-12-mcp-aggregator-design.md`

---

## File Map

### New Files

| File | Crate | Responsibility |
|------|-------|----------------|
| `crates/rightclaw/src/mcp/proxy.rs` | core | ProxyBackend, DynamicAuthClient, BackendStatus |
| `crates/rightclaw/src/mcp/internal_client.rs` | core | Hyper UDS client for bot→aggregator IPC |
| `crates/rightclaw/src/memory/sql/v8_mcp_servers.sql` | core | Migration SQL |
| `crates/rightclaw-cli/src/aggregator.rs` | cli | Aggregator, ToolDispatcher, BackendRegistry |
| `crates/rightclaw-cli/src/aggregator_tests.rs` | cli | Integration tests |
| `crates/rightclaw-cli/src/right_backend.rs` | cli | RightBackend (extracted tool methods) |
| `crates/rightclaw-cli/src/right_backend_tests.rs` | cli | RightBackend tests |
| `crates/rightclaw-cli/src/internal_api.rs` | cli | Unix socket REST handlers |
| `crates/rightclaw-cli/src/internal_api_tests.rs` | cli | Internal API tests |

### Modified Files

| File | Changes |
|------|---------|
| `crates/rightclaw/src/mcp/mod.rs` | Add `pub mod proxy; pub mod internal_client;` |
| `crates/rightclaw/src/memory/migrations.rs` | Add V8 migration |
| `crates/rightclaw/src/mcp/refresh.rs` | New signature: accept token Arc handle instead of mcp_json_path |
| `crates/rightclaw/src/mcp/credentials.rs` | Add SQLite-based server registry functions |
| `crates/rightclaw/src/codegen/mcp_config.rs` | Write from scratch instead of merge |
| `crates/rightclaw-cli/src/main.rs` | Wire Aggregator + internal API instead of HttpMemoryServer |
| `crates/rightclaw-cli/src/memory_server_http.rs` | Deprecated (kept but unused) |
| `crates/bot/src/telegram/handler.rs` | `/mcp` commands call InternalClient |
| `crates/bot/src/telegram/oauth_callback.rs` | `complete_oauth_flow` calls set-token via InternalClient |
| `Cargo.toml` (workspace) | Add rmcp client features |
| `crates/rightclaw/Cargo.toml` | Add hyper, http-body-util deps |
| `ARCHITECTURE.md` | Update module map, data flows |

---

## Task 1: SQLite Migration — `mcp_servers` Table

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v8_mcp_servers.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write migration SQL**

```sql
-- v8_mcp_servers.sql
CREATE TABLE IF NOT EXISTS mcp_servers (
    name       TEXT PRIMARY KEY,
    url        TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [ ] **Step 2: Register migration in migrations.rs**

In `crates/rightclaw/src/memory/migrations.rs`, add after V7:

```rust
const V8_SCHEMA: &str = include_str!("sql/v8_mcp_servers.sql");
```

And add `M::up(V8_SCHEMA),` to the `MIGRATIONS` LazyLock vec.

- [ ] **Step 3: Write migration test**

In `crates/rightclaw/src/memory/migrations.rs`, add test:

```rust
#[test]
fn v8_mcp_servers_table() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    
    // Insert a server
    conn.execute(
        "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
        ("notion", "https://mcp.notion.com/mcp"),
    ).unwrap();
    
    // Verify it's there
    let url: String = conn.query_row(
        "SELECT url FROM mcp_servers WHERE name = ?1",
        ["notion"],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(url, "https://mcp.notion.com/mcp");
    
    // Verify upsert works
    conn.execute(
        "INSERT OR REPLACE INTO mcp_servers (name, url) VALUES (?1, ?2)",
        ("notion", "https://new-url.com/mcp"),
    ).unwrap();
    let url: String = conn.query_row(
        "SELECT url FROM mcp_servers WHERE name = ?1",
        ["notion"],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(url, "https://new-url.com/mcp");
}
```

- [ ] **Step 4: Update schema version test**

Find the existing test that checks migration version count and update it from V7 to V8.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw -- migrations`
Expected: All pass, including new v8 test.

- [ ] **Step 6: Commit**

```
feat(mcp): add mcp_servers SQLite migration (V8)
```

---

## Task 2: Credential Registry — SQLite Functions

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs`
- Test: same file, `#[cfg(test)]` module

- [ ] **Step 1: Write failing tests for SQLite server registry**

In `crates/rightclaw/src/mcp/credentials.rs`, add tests:

```rust
#[cfg(test)]
mod db_tests {
    use rusqlite::Connection;
    use crate::memory::migrations::MIGRATIONS;
    use super::*;

    fn setup_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn add_and_list_servers() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_add_server(&conn, "github", "https://mcp.github.com/mcp").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].0, "github"); // sorted
        assert_eq!(servers[1].0, "notion");
    }

    #[test]
    fn remove_server() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_remove_server(&conn, "notion").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn remove_nonexistent_server() {
        let conn = setup_db();
        let result = db_remove_server(&conn, "ghost");
        assert!(result.is_err());
    }

    #[test]
    fn upsert_server() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://old.com/mcp").unwrap();
        db_add_server(&conn, "notion", "https://new.com/mcp").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].1, "https://new.com/mcp");
    }

    #[test]
    fn validate_server_name() {
        assert!(validate_server_name("notion").is_ok());
        assert!(validate_server_name("right").is_err());
        assert!(validate_server_name("rightmeta").is_err());
        assert!(validate_server_name("no__tion").is_err());
        assert!(validate_server_name("").is_err());
    }

    #[test]
    fn validate_server_url() {
        assert!(validate_server_url("https://mcp.notion.com/mcp").is_ok());
        assert!(validate_server_url("http://mcp.notion.com/mcp").is_err());
        assert!(validate_server_url("https://127.0.0.1/mcp").is_err());
        assert!(validate_server_url("https://10.0.0.1/mcp").is_err());
        assert!(validate_server_url("https://192.168.1.1/mcp").is_err());
        assert!(validate_server_url("https://169.254.1.1/mcp").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw -- db_tests`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement SQLite server registry functions**

In `crates/rightclaw/src/mcp/credentials.rs`, add:

```rust
use rusqlite::Connection;
use std::net::IpAddr;
use url::Url;

/// Add or update an MCP server in the SQLite registry.
pub fn db_add_server(conn: &Connection, name: &str, url: &str) -> Result<(), CredentialError> {
    validate_server_name(name)?;
    validate_server_url(url)?;
    conn.execute(
        "INSERT OR REPLACE INTO mcp_servers (name, url) VALUES (?1, ?2)",
        (name, url),
    ).map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}"))))?;
    Ok(())
}

/// Remove an MCP server from the SQLite registry.
pub fn db_remove_server(conn: &Connection, name: &str) -> Result<(), CredentialError> {
    let rows = conn.execute(
        "DELETE FROM mcp_servers WHERE name = ?1",
        [name],
    ).map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}"))))?;
    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// List all registered MCP servers, sorted by name.
pub fn db_list_servers(conn: &Connection) -> Result<Vec<(String, String)>, CredentialError> {
    let mut stmt = conn.prepare(
        "SELECT name, url FROM mcp_servers ORDER BY name"
    ).map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}"))))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}"))))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}"))))?);
    }
    Ok(result)
}

/// Validate server name: not reserved, no `__`, not empty.
pub fn validate_server_name(name: &str) -> Result<(), CredentialError> {
    if name.is_empty() {
        return Err(CredentialError::InvalidPath("Server name cannot be empty".into()));
    }
    if name == "right" || name == "rightmeta" {
        return Err(CredentialError::InvalidPath(format!("Name '{name}' is reserved")));
    }
    if name.contains("__") {
        return Err(CredentialError::InvalidPath("Server name cannot contain '__'".into()));
    }
    Ok(())
}

/// Validate server URL: HTTPS only, no private IPs.
pub fn validate_server_url(url_str: &str) -> Result<(), CredentialError> {
    let url = Url::parse(url_str)
        .map_err(|e| CredentialError::InvalidPath(format!("Invalid URL: {e}")))?;
    
    if url.scheme() != "https" {
        return Err(CredentialError::InvalidPath("Only HTTPS URLs are allowed".into()));
    }
    
    if let Some(host) = url.host_str() {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(ip) {
                return Err(CredentialError::InvalidPath(format!("Private IP address blocked: {ip}")));
            }
        }
        if host == "localhost" {
            return Err(CredentialError::InvalidPath("localhost is blocked".into()));
        }
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.x
            || v4.is_private()        // 10.x, 172.16-31.x, 192.168.x
            || v4.is_link_local()     // 169.254.x
        }
        IpAddr::V6(v6) => v6.is_loopback(), // ::1
    }
}
```

- [ ] **Step 4: Add `url` dependency if not present**

Check `crates/rightclaw/Cargo.toml` for `url` crate. If missing, add `url = "2"`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw -- db_tests`
Expected: All pass.

- [ ] **Step 6: Commit**

```
feat(mcp): add SQLite-based server registry with validation
```

---

## Task 3: Core Types — BackendStatus, DynamicAuthClient

**Files:**
- Create: `crates/rightclaw/src/mcp/proxy.rs`
- Modify: `crates/rightclaw/src/mcp/mod.rs`
- Modify: `Cargo.toml` (workspace root — add rmcp client features)
- Modify: `crates/rightclaw/Cargo.toml` (add http, sse-stream if needed)

- [ ] **Step 1: Add rmcp client features to workspace Cargo.toml**

In root `Cargo.toml`, update rmcp entry:

```toml
rmcp = { version = "1.3", default-features = false, features = [
    "server",
    "transport-io",
    "transport-streamable-http-server",
    "transport-streamable-http-client",
    "transport-streamable-http-client-reqwest",
    "macros",
] }
```

- [ ] **Step 2: Write BackendStatus and DynamicAuthClient**

Create `crates/rightclaw/src/mcp/proxy.rs`:

```rust
//! MCP proxy types for aggregating external MCP servers.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Error as SseError, Sse};
use tokio::sync::RwLock;

/// Status of a ProxyBackend connection to an upstream MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    /// initialize + tools/list succeeded
    Connected,
    /// Got 401, token is None or expired
    NeedsAuth,
    /// Transient error (connection refused, DNS, timeout), retrying
    Unreachable,
}

/// Wraps `reqwest::Client` with dynamic Bearer token injection.
///
/// Reads token from shared `Arc<RwLock>` on every request,
/// enabling zero-downtime token rotation without reconnecting
/// the MCP session.
#[derive(Clone)]
pub struct DynamicAuthClient {
    inner: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
}

impl DynamicAuthClient {
    pub fn new(client: reqwest::Client, token: Arc<RwLock<Option<String>>>) -> Self {
        Self { inner: client, token }
    }

    async fn current_auth(&self) -> Option<String> {
        self.token.read().await.clone().map(|t| format!("Bearer {t}"))
    }
}

impl StreamableHttpClient for DynamicAuthClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let auth = self.current_auth().await;
        if _auth_token.is_some() {
            tracing::debug!("DynamicAuthClient: ignoring static auth_token param from transport config");
        }
        self.inner
            .post_message(uri, message, session_id, auth, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let auth = self.current_auth().await;
        self.inner
            .delete_session(uri, session_id, auth, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let auth = self.current_auth().await;
        self.inner
            .get_stream(uri, session_id, last_event_id, auth, custom_headers)
            .await
    }
}
```

- [ ] **Step 3: Register module**

In `crates/rightclaw/src/mcp/mod.rs`, add:

```rust
pub mod proxy;
```

- [ ] **Step 4: Build to verify compilation**

Run: `cargo build -p rightclaw`
Expected: Compiles. Verify the rmcp client features resolve correctly and `StreamableHttpClient` trait is available.

- [ ] **Step 5: Write DynamicAuthClient unit test**

At the bottom of `crates/rightclaw/src/mcp/proxy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dynamic_auth_reads_from_shared_state() {
        let token = Arc::new(RwLock::new(Some("initial-token".to_string())));
        let client = DynamicAuthClient::new(reqwest::Client::new(), token.clone());

        // Read initial
        assert_eq!(client.current_auth().await, Some("Bearer initial-token".to_string()));

        // Update token
        *token.write().await = Some("refreshed-token".to_string());
        assert_eq!(client.current_auth().await, Some("Bearer refreshed-token".to_string()));

        // Clear token
        *token.write().await = None;
        assert_eq!(client.current_auth().await, None);
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw -- proxy::tests`
Expected: All pass.

- [ ] **Step 7: Commit**

```
feat(mcp): add BackendStatus enum and DynamicAuthClient
```

---

## Task 4: RightBackend — Extract Tools from HttpMemoryServer

This is the largest refactoring task. Extract the 19 tool methods from `HttpMemoryServer` into a standalone `RightBackend` struct that can be used by the Aggregator.

**Files:**
- Create: `crates/rightclaw-cli/src/right_backend.rs`
- Create: `crates/rightclaw-cli/src/right_backend_tests.rs`
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs` (keep as deprecated, don't break yet)

- [ ] **Step 1: Create RightBackend struct with tool method signatures**

Create `crates/rightclaw-cli/src/right_backend.rs`. This file extracts the tool logic from `HttpMemoryServer` — same code, but methods take explicit `&Connection` and agent context instead of using rmcp tool macros.

```rust
//! RightBackend — in-process rightclaw tools (memory, cron, bootstrap).
//!
//! Extracted from HttpMemoryServer. These tools are unprefixed in the
//! Aggregator — they keep their current names for backward compatibility.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use rusqlite::Connection;
use serde_json::Value;

// Re-use existing tool logic from the memory_server module
use crate::memory_server::{
    do_store_record, do_query_records, do_search_records, do_delete_record,
    do_cron_create, do_cron_update, do_cron_delete, do_cron_list,
    do_cron_list_runs, do_cron_show_run, do_cron_trigger,
    do_mcp_list, do_mcp_auth,
    do_bootstrap_done,
};

pub type ConnCache = Arc<DashMap<String, Arc<Mutex<Connection>>>>;

pub struct RightBackend {
    conn_cache: ConnCache,
    agents_dir: PathBuf,
    rightclaw_home: PathBuf,
}
```

The key insight: we need to extract the **body** of each tool method in `HttpMemoryServer` into standalone functions (prefixed `do_`) that take `(&Connection, agent_name, agent_dir, args)` and return `Result<CallToolResult>`. Then `RightBackend` calls these, and `HttpMemoryServer` (deprecated) can also call them during transition.

This is a large mechanical refactoring. The subagent implementing this task should:

1. Read `memory_server_http.rs` fully
2. For each `#[tool]` method, extract the body into a `do_<name>()` function in a shared location
3. `RightBackend` wraps these functions with connection resolution
4. Provide `tools_list()` that returns the static list of tool definitions
5. Provide `tools_call(name, args)` that dispatches by name

- [ ] **Step 2: Extract tool bodies into free functions**

In `memory_server_http.rs` (or a new shared module), extract each tool's logic. Example for `store_record`:

```rust
/// Extracted tool logic — can be called from both HttpMemoryServer and RightBackend.
pub fn do_store_record(
    conn: &Connection,
    agent_name: &str,
    content: &str,
    tags: &[String],
    importance: Option<i32>,
) -> Result<String, anyhow::Error> {
    // ... existing body from HttpMemoryServer::store_record ...
}
```

Repeat for all 19 tools. This is mechanical — move the body, adjust parameter types.

- [ ] **Step 3: Implement RightBackend dispatch**

```rust
impl RightBackend {
    pub fn new(conn_cache: ConnCache, agents_dir: PathBuf, rightclaw_home: PathBuf) -> Self {
        Self { conn_cache, agents_dir, rightclaw_home }
    }

    /// Return static tool definitions (name, description, inputSchema).
    pub fn tools_list(&self) -> Vec<rmcp::model::Tool> {
        // Return the same tool definitions that HttpMemoryServer registers
        // via #[tool] macros — but as data, not as macro-generated code.
        vec![
            // Build each Tool with name, description, inputSchema
            // Use serde_json::json! for schemas
        ]
    }

    /// Dispatch a tool call by name.
    pub async fn tools_call(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        tool_name: &str,
        args: Value,
    ) -> Result<rmcp::model::CallToolResult, anyhow::Error> {
        let conn = self.get_conn(agent_name, agent_dir)?;
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        
        match tool_name {
            "store_record" => { /* parse args, call do_store_record */ }
            "query_records" => { /* parse args, call do_query_records */ }
            // ... all 19 tools ...
            other => anyhow::bail!("Unknown tool: {other}"),
        }
    }

    fn get_conn(&self, agent_name: &str, agent_dir: &Path) -> Result<Arc<Mutex<Connection>>, anyhow::Error> {
        // Same logic as HttpMemoryServer::get_conn_for_agent
        if let Some(conn) = self.conn_cache.get(agent_name) {
            return Ok(conn.clone());
        }
        let db_path = agent_dir.join("memory.db");
        let conn = Connection::open(&db_path)?;
        rightclaw::memory::migrations::MIGRATIONS.to_latest(&mut conn)?;
        let conn = Arc::new(Mutex::new(conn));
        self.conn_cache.insert(agent_name.to_string(), conn.clone());
        Ok(conn)
    }
}
```

- [ ] **Step 4: Write basic RightBackend test**

Create `crates/rightclaw-cli/src/right_backend_tests.rs`:

```rust
use super::*;
use tempfile::TempDir;

fn setup() -> (RightBackend, TempDir) {
    let dir = TempDir::new().unwrap();
    let agents_dir = dir.path().join("agents");
    std::fs::create_dir_all(agents_dir.join("test-agent")).unwrap();
    let backend = RightBackend::new(
        Arc::new(DashMap::new()),
        agents_dir,
        dir.path().to_path_buf(),
    );
    (backend, dir)
}

#[tokio::test]
async fn store_and_query_record() {
    let (backend, _dir) = setup();
    let agent_dir = _dir.path().join("agents/test-agent");
    
    let result = backend.tools_call(
        "test-agent", &agent_dir, "store_record",
        serde_json::json!({"content": "test memory", "tags": ["tag1"]}),
    ).await.unwrap();
    
    // Verify it was stored
    let result = backend.tools_call(
        "test-agent", &agent_dir, "query_records",
        serde_json::json!({"tag": "tag1"}),
    ).await.unwrap();
    // Assert result contains "test memory"
}

#[tokio::test]
async fn tools_list_returns_all_tools() {
    let (backend, _dir) = setup();
    let tools = backend.tools_list();
    assert!(tools.len() >= 15, "Expected at least 15 tools, got {}", tools.len());
    
    // Verify key tools exist
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"store_record"));
    assert!(names.contains(&"cron_create"));
    assert!(names.contains(&"bootstrap_done"));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw-cli -- right_backend`
Expected: All pass.

- [ ] **Step 6: Commit**

```
refactor(mcp): extract RightBackend from HttpMemoryServer
```

---

## Task 5: Backend Enum + ToolDispatcher + BackendRegistry

**Files:**
- Create: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Define Backend enum and Aggregator structs**

Create `crates/rightclaw-cli/src/aggregator.rs`:

```rust
//! MCP Aggregator — replaces HttpMemoryServer.
//!
//! Three-layer architecture:
//! - Aggregator: rmcp ServerHandler, HTTP auth, delegates to ToolDispatcher
//! - ToolDispatcher: prefix parsing, per-agent routing
//! - BackendRegistry: per-agent backend management

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::right_backend::RightBackend;
use rightclaw::mcp::proxy::BackendStatus;

/// Split tool name on first `__` delimiter.
/// Returns None if no `__` found (→ RightBackend tool).
pub fn split_prefix(tool_name: &str) -> Option<(&str, &str)> {
    tool_name.split_once("__")
}

/// Per-agent backend management.
pub struct BackendRegistry {
    /// In-process rightclaw tools (memory, cron, bootstrap)
    pub right: RightBackend,
    /// External MCP proxies — keyed by name for O(1) lookup
    pub proxies: HashMap<String, Arc<ProxyBackendHandle>>,
    /// Agent identity
    pub agent_name: String,
    pub agent_dir: PathBuf,
}

/// Placeholder for ProxyBackend handle (implemented in Task 8).
pub struct ProxyBackendHandle {
    pub name: String,
    pub url: String,
    pub status: RwLock<BackendStatus>,
    pub tools: RwLock<Vec<rmcp::model::Tool>>,
    pub instructions: RwLock<Option<String>>,
}

/// Prefix parsing + routing.
pub struct ToolDispatcher {
    pub agents: DashMap<String, BackendRegistry>,
}

/// Top-level aggregator. Implements rmcp::ServerHandler.
pub struct Aggregator {
    pub dispatcher: Arc<ToolDispatcher>,
    pub token_map: Arc<RwLock<HashMap<String, AgentInfo>>>,
}

#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub name: String,
    pub dir: PathBuf,
}

impl ToolDispatcher {
    pub fn new() -> Self {
        Self { agents: DashMap::new() }
    }

    /// Resolve agent and dispatch tool call.
    pub async fn dispatch(
        &self,
        agent_name: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<rmcp::model::CallToolResult, anyhow::Error> {
        let registry = self.agents.get(agent_name)
            .ok_or_else(|| anyhow::anyhow!("Agent '{agent_name}' not found"))?;

        if let Some((prefix, tool)) = split_prefix(tool_name) {
            match prefix {
                "rightmeta" => registry.handle_read_only_tool(tool, &args).await,
                other => registry.dispatch_to_proxy(other, tool, args).await,
            }
        } else {
            // No prefix → RightBackend
            registry.right.tools_call(
                &registry.agent_name,
                &registry.agent_dir,
                tool_name,
                args,
            ).await
        }
    }

    /// Build merged tool list for an agent.
    pub async fn tools_list(&self, agent_name: &str) -> Vec<rmcp::model::Tool> {
        let Some(registry) = self.agents.get(agent_name) else {
            return vec![];
        };

        let mut tools = registry.right.tools_list();

        // Add rightmeta__mcp_list
        tools.push(registry.mcp_list_tool_def());

        // Add prefixed proxy tools
        for (name, proxy) in &registry.proxies {
            let proxy_tools = proxy.tools.read().await;
            for tool in proxy_tools.iter() {
                let mut prefixed = tool.clone();
                prefixed.name = format!("{name}__{}", tool.name).into();
                tools.push(prefixed);
            }
        }

        tools
    }

    /// Build merged instructions for an agent.
    pub async fn instructions(&self, agent_name: &str) -> String {
        let Some(registry) = self.agents.get(agent_name) else {
            return String::new();
        };
        registry.build_instructions().await
    }
}

impl BackendRegistry {
    /// Handle read-only management tools (only mcp_list).
    pub async fn handle_read_only_tool(
        &self,
        tool: &str,
        _args: &Value,
    ) -> Result<rmcp::model::CallToolResult, anyhow::Error> {
        match tool {
            "mcp_list" => self.do_mcp_list().await,
            other => anyhow::bail!("Unknown management tool: {other}"),
        }
    }

    /// Dispatch to a ProxyBackend by name.
    pub async fn dispatch_to_proxy(
        &self,
        proxy_name: &str,
        tool: &str,
        args: Value,
    ) -> Result<rmcp::model::CallToolResult, anyhow::Error> {
        let proxy = self.proxies.get(proxy_name)
            .ok_or_else(|| anyhow::anyhow!(
                "Server '{proxy_name}' not found. It may have been removed."
            ))?;
        // Proxy dispatch implemented in Task 8
        anyhow::bail!("ProxyBackend dispatch not yet implemented")
    }

    async fn do_mcp_list(&self) -> Result<rmcp::model::CallToolResult, anyhow::Error> {
        let mut lines = vec!["Configured MCP servers:".to_string()];
        if self.proxies.is_empty() {
            lines.push("  (none)".to_string());
        }
        for (name, proxy) in &self.proxies {
            let status = *proxy.status.read().await;
            let tool_count = proxy.tools.read().await.len();
            let status_str = match status {
                BackendStatus::Connected => "connected",
                BackendStatus::NeedsAuth => "needs auth",
                BackendStatus::Unreachable => "unreachable",
            };
            lines.push(format!("  {name}: {status_str} ({tool_count} tools) — {}", proxy.url));
        }
        Ok(rmcp::model::CallToolResult::success(vec![
            rmcp::model::Content::text(lines.join("\n"))
        ]))
    }

    fn mcp_list_tool_def(&self) -> rmcp::model::Tool {
        rmcp::model::Tool::new(
            "rightmeta__mcp_list",
            "List all configured external MCP servers and their status (connected/needs_auth/unreachable) with tool counts.",
            serde_json::json!({"type": "object", "properties": {}}),
        )
    }

    async fn build_instructions(&self) -> String {
        let mut parts = vec![
            "RightClaw MCP Aggregator.\n".to_string(),
            "## Management\n- rightmeta__mcp_list: List configured MCP servers and status\n- To add/remove/authenticate MCP servers, ask the user to use Telegram commands: /mcp add, /mcp remove, /mcp auth\n".to_string(),
        ];

        for (name, proxy) in &self.proxies {
            if let Some(instructions) = proxy.instructions.read().await.as_ref() {
                let truncated = if instructions.len() > 4000 {
                    format!("{}... [truncated]", &instructions[..4000])
                } else {
                    instructions.clone()
                };
                parts.push(format!("## {name}\n{truncated}\n"));
            }
        }

        parts.join("\n")
    }
}
```

- [ ] **Step 2: Write split_prefix test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prefix_with_delimiter() {
        assert_eq!(split_prefix("notion__search"), Some(("notion", "search")));
        assert_eq!(split_prefix("rightmeta__mcp_list"), Some(("rightmeta", "mcp_list")));
    }

    #[test]
    fn split_prefix_no_delimiter() {
        assert_eq!(split_prefix("store_record"), None);
        assert_eq!(split_prefix("cron_create"), None);
    }

    #[test]
    fn split_prefix_multiple_delimiters() {
        // Split on first __ only
        assert_eq!(split_prefix("notion__my__tool"), Some(("notion", "my__tool")));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw-cli -- aggregator::tests`
Expected: All pass.

- [ ] **Step 4: Commit**

```
feat(mcp): add Aggregator, ToolDispatcher, BackendRegistry structs
```

---

## Task 6: Aggregator as rmcp ServerHandler

Wire the Aggregator as an `rmcp::ServerHandler` and make it serveable via `StreamableHttpService`.

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Implement rmcp::ServerHandler for Aggregator**

Add to `aggregator.rs`:

```rust
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequest, CallToolResult, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;

impl Aggregator {
    /// Factory closure for StreamableHttpService.
    /// Each MCP session gets a new Aggregator instance sharing the same Arc state.
    pub fn factory(
        dispatcher: Arc<ToolDispatcher>,
        token_map: Arc<RwLock<HashMap<String, AgentInfo>>>,
    ) -> impl FnMut() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> + Clone {
        move || Ok(Self {
            dispatcher: dispatcher.clone(),
            token_map: token_map.clone(),
        })
    }
}

#[rmcp::tool_handler]
impl ServerHandler for Aggregator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rightclaw", env!("CARGO_PKG_VERSION")))
            // instructions are dynamic (per-agent), set in list_tools context
    }

    async fn list_tools(
        &self,
        _request: rmcp::model::PaginatedRequest,
        context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::Error> {
        let agent_name = self.resolve_agent_from_context(&context)?;
        let tools = self.dispatcher.tools_list(&agent_name).await;
        Ok(rmcp::model::ListToolsResult { tools, next_cursor: None })
    }

    async fn call_tool(
        &self,
        request: CallToolRequest,
        context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let agent_name = self.resolve_agent_from_context(&context)?;
        self.dispatcher
            .dispatch(&agent_name, &request.name, request.arguments.unwrap_or_default())
            .await
            .map_err(|e| rmcp::Error {
                code: rmcp::error::INTERNAL_ERROR,
                message: format!("{e:#}"),
                data: None,
            })
    }
}

impl Aggregator {
    fn resolve_agent_from_context(
        &self,
        context: &RequestContext<rmcp::service::RoleServer>,
    ) -> Result<String, rmcp::Error> {
        // Agent info is injected by bearer auth middleware into request extensions.
        // The exact extraction mechanism depends on how rmcp passes HTTP context.
        // This may need to use the Extension pattern from current HttpMemoryServer.
        //
        // For now, this is a placeholder — the exact mechanism will be determined
        // by how StreamableHttpService integrates with axum middleware.
        todo!("Extract agent name from request context — see HttpMemoryServer pattern")
    }
}
```

Note: The exact agent resolution mechanism requires studying how the current `HttpMemoryServer` extracts `AgentInfo` from `http::request::Parts` extensions via the `Extension(parts)` pattern in rmcp tool macros. The implementing subagent must read the current code and replicate this pattern.

- [ ] **Step 2: Build to verify compilation**

Run: `cargo build -p rightclaw-cli`
Expected: Compiles (with todo! — that's OK for now).

- [ ] **Step 3: Commit**

```
feat(mcp): implement rmcp ServerHandler for Aggregator
```

---

## Task 7: Wire Aggregator into main.rs

Replace the HttpMemoryServer with the Aggregator in the CLI entry point.

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Update MemoryServerHttp handler**

In `main.rs`, replace the `HttpMemoryServer` wiring (lines ~271-299) with Aggregator:

```rust
if let Commands::MemoryServerHttp { port, ref token_map } = cli.command {
    let home = resolve_home(...)?;
    let agents_dir = agents_dir(&home);
    
    // Load token map (same as before)
    let token_map_content = std::fs::read_to_string(token_map)?;
    let raw_map: HashMap<String, String> = serde_json::from_str(&token_map_content)?;
    let mut agent_map = HashMap::new();
    for (name, token) in &raw_map {
        let dir = agents_dir.join(name);
        agent_map.insert(token.clone(), AgentInfo { name: name.clone(), dir });
    }
    let token_map = Arc::new(RwLock::new(agent_map));
    
    // Build ToolDispatcher with BackendRegistry per agent
    let dispatcher = Arc::new(ToolDispatcher::new());
    let conn_cache = Arc::new(DashMap::new());
    
    for (name, token) in &raw_map {
        let agent_dir = agents_dir.join(name);
        let right = RightBackend::new(conn_cache.clone(), agents_dir.clone(), home.clone());
        
        // Load existing proxy backends from SQLite
        // (will be implemented when ProxyBackend is ready)
        let proxies = HashMap::new();
        
        dispatcher.agents.insert(name.clone(), BackendRegistry {
            right,
            proxies,
            agent_name: name.clone(),
            agent_dir,
        });
    }
    
    return run_aggregator_http(port, dispatcher, token_map, agents_dir, home).await;
}
```

- [ ] **Step 2: Create run_aggregator_http function**

Add a new function (in aggregator.rs or a new file) that starts the HTTP server:

```rust
pub async fn run_aggregator_http(
    port: u16,
    dispatcher: Arc<ToolDispatcher>,
    token_map: Arc<RwLock<HashMap<String, AgentInfo>>>,
    agents_dir: PathBuf,
    home: PathBuf,
) -> miette::Result<()> {
    let ct = tokio_util::sync::CancellationToken::new();
    
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct.clone());
    
    let factory = Aggregator::factory(dispatcher, token_map.clone());
    let mcp_service = StreamableHttpService::new(factory, config);
    
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await
        .map_err(|e| miette::miette!("bind: {e:#}"))?;
    
    let app = axum::Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(axum::middleware::from_fn_with_state(
            token_map,
            bearer_auth_middleware,
        ));
    
    tracing::info!(port, "MCP Aggregator listening");
    
    axum::serve(listener, app)
        .with_graceful_shutdown(ct.cancelled_owned())
        .await
        .map_err(|e| miette::miette!("HTTP server error: {e:#}"))
}
```

- [ ] **Step 3: Run the full workspace build**

Run: `cargo build --workspace`
Expected: Compiles. HttpMemoryServer still exists but is no longer wired.

- [ ] **Step 4: Test manually**

Start the aggregator and verify basic tool calls work with the existing right tools (store_record, query_records, etc.).

- [ ] **Step 5: Commit**

```
feat(mcp): wire Aggregator into CLI, replacing HttpMemoryServer
```

---

## Task 8: ProxyBackend — MCP Client to Upstream

**Files:**
- Modify: `crates/rightclaw/src/mcp/proxy.rs`
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Add ProxyBackend struct with connect/tools_list/tools_call**

In `crates/rightclaw/src/mcp/proxy.rs`, add:

```rust
use rmcp::model::{CallToolResult, Tool};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};

pub struct ProxyBackend {
    pub server_name: String,
    pub url: String,
    pub cached_tools: RwLock<Vec<Tool>>,
    pub cached_instructions: RwLock<Option<String>>,
    pub status: RwLock<BackendStatus>,
    pub token: Arc<RwLock<Option<String>>>,
    last_refresh: RwLock<Option<std::time::Instant>>,
    // MCP client session — created on connect()
    // Exact type depends on rmcp's RunningService API
}

impl ProxyBackend {
    pub fn new(server_name: String, url: String, token: Arc<RwLock<Option<String>>>) -> Self {
        Self {
            server_name,
            url,
            cached_tools: RwLock::new(Vec::new()),
            cached_instructions: RwLock::new(None),
            status: RwLock::new(BackendStatus::Unreachable),
            token,
            last_refresh: RwLock::new(None),
        }
    }

    /// Connect to upstream MCP server, initialize session, fetch tools.
    pub async fn connect(&self, http_client: reqwest::Client) -> Result<(), anyhow::Error> {
        let dynamic_client = DynamicAuthClient::new(http_client, self.token.clone());
        let config = StreamableHttpClientTransportConfig {
            uri: self.url.clone().into(),
            allow_stateless: true,
            ..Default::default()
        };
        let transport = StreamableHttpClientTransport::with_client(dynamic_client, config);

        // Connect and initialize MCP session
        // rmcp::serve_client(transport, ClientHandler) or similar
        // This depends on rmcp's client API — the implementing subagent must
        // read rmcp docs to find the exact initialization sequence.
        
        // After successful init:
        // 1. Fetch tools/list from upstream
        // 2. Filter out tools with "__" in name
        // 3. Cache tools + instructions
        // 4. Set status = Connected

        *self.status.write().await = BackendStatus::Connected;
        Ok(())
    }

    /// Forward a tool call to the upstream MCP server.
    pub async fn tools_call(&self, tool_name: &str, args: serde_json::Value) -> Result<CallToolResult, anyhow::Error> {
        let status = *self.status.read().await;
        match status {
            BackendStatus::NeedsAuth => {
                anyhow::bail!("Authentication required for '{}'. Use /mcp auth {} in Telegram.", 
                    self.server_name, self.server_name);
            }
            BackendStatus::Unreachable => {
                anyhow::bail!("Server '{}' is currently unreachable.", self.server_name);
            }
            BackendStatus::Connected => {}
        }
        
        // Forward call via MCP client session
        // On 401: retry with refreshed token, then set NeedsAuth
        // On transient error: set Unreachable
        todo!("Forward tool call to upstream MCP client session")
    }
}
```

- [ ] **Step 2: Wire ProxyBackend into BackendRegistry.dispatch_to_proxy**

In `aggregator.rs`, update `dispatch_to_proxy`:

```rust
pub async fn dispatch_to_proxy(
    &self,
    proxy_name: &str,
    tool: &str,
    args: Value,
) -> Result<CallToolResult, anyhow::Error> {
    let proxy = self.proxies.get(proxy_name)
        .ok_or_else(|| anyhow::anyhow!(
            "Server '{proxy_name}' not found. It may have been removed."
        ))?;
    proxy.tools_call(tool, args).await
}
```

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: Compiles with todo!s.

- [ ] **Step 4: Commit**

```
feat(mcp): add ProxyBackend with upstream MCP client
```

---

## Task 9: Internal REST API — Unix Socket

**Files:**
- Create: `crates/rightclaw-cli/src/internal_api.rs`
- Modify: `crates/rightclaw-cli/src/aggregator.rs` (wire into startup)

- [ ] **Step 1: Define request/response types**

Create `crates/rightclaw-cli/src/internal_api.rs`:

```rust
//! Internal REST API on Unix domain socket.
//! Used by Telegram bot for MCP management (add/remove/set-token).

use std::path::PathBuf;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use crate::aggregator::ToolDispatcher;

#[derive(Deserialize)]
pub struct McpAddRequest {
    pub agent: String,
    pub name: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct McpAddResponse {
    pub tools_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Deserialize)]
pub struct McpRemoveRequest {
    pub agent: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct McpRemoveResponse {
    pub removed: bool,
}

#[derive(Deserialize)]
pub struct SetTokenRequest {
    pub agent: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Serialize)]
pub struct SetTokenResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub fn internal_router(dispatcher: Arc<ToolDispatcher>) -> Router {
    Router::new()
        .route("/mcp-add", post(handle_mcp_add))
        .route("/mcp-remove", post(handle_mcp_remove))
        .route("/set-token", post(handle_set_token))
        .with_state(dispatcher)
}

async fn handle_mcp_add(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpAddRequest>,
) -> impl IntoResponse {
    // 1. Validate name + URL
    // 2. Get BackendRegistry for agent
    // 3. Create ProxyBackend, connect, cache tools
    // 4. Insert into registry.proxies
    // 5. Return response
    todo!()
}

async fn handle_mcp_remove(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpRemoveRequest>,
) -> impl IntoResponse {
    todo!()
}

async fn handle_set_token(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<SetTokenRequest>,
) -> impl IntoResponse {
    todo!()
}
```

- [ ] **Step 2: Wire Unix socket listener into aggregator startup**

In the `run_aggregator_http` function, add UDS listener alongside TCP:

```rust
// Unix socket for internal API
let socket_path = home.join("run/internal.sock");
if socket_path.exists() {
    std::fs::remove_file(&socket_path)?;
}
std::fs::create_dir_all(socket_path.parent().unwrap())?;

let internal_router = internal_api::internal_router(dispatcher.clone());
let uds_listener = tokio::net::UnixListener::bind(&socket_path)?;

// Spawn internal API server
tokio::spawn(async move {
    let uds_stream = tokio_stream::wrappers::UnixListenerStream::new(uds_listener);
    // Serve via axum on UDS — requires axum's unix socket support
    // or manual accept loop with hyper
});
```

- [ ] **Step 3: Implement handler bodies**

Fill in `handle_mcp_add`:

```rust
async fn handle_mcp_add(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpAddRequest>,
) -> Result<Json<McpAddResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate
    rightclaw::mcp::credentials::validate_server_name(&req.name)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "invalid_name".into(),
            detail: Some(format!("{e:#}")),
        })))?;
    
    rightclaw::mcp::credentials::validate_server_url(&req.url)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "invalid_url".into(),
            detail: Some(format!("{e:#}")),
        })))?;

    let registry = dispatcher.agents.get_mut(&req.agent)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(ErrorResponse {
            error: "agent_not_found".into(),
            detail: None,
        })))?;

    // Persist to SQLite
    let conn = registry.right.get_conn(&req.agent, &registry.agent_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
            error: "internal".into(),
            detail: Some(format!("{e:#}")),
        })))?;
    let conn = conn.lock().unwrap();
    rightclaw::mcp::credentials::db_add_server(&conn, &req.name, &req.url)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse {
            error: "validation_error".into(),
            detail: Some(format!("{e:#}")),
        })))?;

    // Create ProxyBackend + connect (best-effort)
    // ... (creates DynamicAuthClient, connects, caches tools)
    
    Ok(Json(McpAddResponse {
        tools_count: 0, // updated after connect
        excluded: vec![],
        warning: Some("Server registered. Tools will be available on next agent session.".into()),
    }))
}
```

Similar implementations for `handle_mcp_remove` and `handle_set_token`.

- [ ] **Step 4: Build**

Run: `cargo build --workspace`

- [ ] **Step 5: Commit**

```
feat(mcp): add internal REST API on Unix socket
```

---

## Task 10: InternalClient — Hyper UDS Client

**Files:**
- Create: `crates/rightclaw/src/mcp/internal_client.rs`
- Modify: `crates/rightclaw/src/mcp/mod.rs`
- Modify: `crates/rightclaw/Cargo.toml` (add hyper, http-body-util)

- [ ] **Step 1: Add dependencies**

In `crates/rightclaw/Cargo.toml`, add:

```toml
hyper = { version = "1", features = ["client", "http1"] }
hyper-util = { version = "0.1", features = ["tokio", "client-legacy"] }
http-body-util = "0.1"
tower = { version = "0.5", features = ["util"] }
```

- [ ] **Step 2: Implement InternalClient**

Create `crates/rightclaw/src/mcp/internal_client.rs`:

```rust
//! Hyper-based Unix domain socket client for bot→aggregator IPC.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum InternalClientError {
    #[error("Connection failed: {0:#}")]
    Connection(#[from] std::io::Error),
    #[error("HTTP error: {0:#}")]
    Http(String),
    #[error("JSON error: {0:#}")]
    Json(#[from] serde_json::Error),
    #[error("Server error ({status}): {body}")]
    Server { status: u16, body: String },
}

pub struct InternalClient {
    socket_path: PathBuf,
}

impl InternalClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// POST JSON to the internal API and parse the response.
    async fn post<Req: Serialize, Res: DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Res, InternalClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(
            hyper_util::rt::TokioIo::new(stream)
        ).await.map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        tokio::spawn(conn);

        let body_bytes = serde_json::to_vec(body)?;
        let req = hyper::Request::post(path)
            .header("content-type", "application/json")
            .body(http_body_util::Full::new(hyper::body::Bytes::from(body_bytes)))
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        let response = sender.send_request(req).await
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        let status = response.status().as_u16();
        let body = http_body_util::BodyExt::collect(response.into_body()).await
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?
            .to_bytes();

        if status >= 400 {
            let body_str = String::from_utf8_lossy(&body).to_string();
            return Err(InternalClientError::Server { status, body: body_str });
        }

        serde_json::from_slice(&body).map_err(Into::into)
    }

    pub async fn mcp_add(&self, agent: &str, name: &str, url: &str) -> Result<McpAddResponse, InternalClientError> {
        self.post("/mcp-add", &serde_json::json!({
            "agent": agent, "name": name, "url": url
        })).await
    }

    pub async fn mcp_remove(&self, agent: &str, name: &str) -> Result<McpRemoveResponse, InternalClientError> {
        self.post("/mcp-remove", &serde_json::json!({
            "agent": agent, "name": name
        })).await
    }

    pub async fn set_token(&self, request: &SetTokenRequest) -> Result<SetTokenResponse, InternalClientError> {
        self.post("/set-token", request).await
    }
}

// Re-export response types from internal_api (or define shared types in core)
#[derive(serde::Deserialize)]
pub struct McpAddResponse {
    pub tools_count: usize,
    pub excluded: Vec<String>,
    pub warning: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct McpRemoveResponse {
    pub removed: bool,
}

#[derive(serde::Serialize)]
pub struct SetTokenRequest {
    pub agent: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct SetTokenResponse {
    pub ok: bool,
    pub warning: Option<String>,
}
```

- [ ] **Step 3: Register module**

In `crates/rightclaw/src/mcp/mod.rs`:

```rust
pub mod internal_client;
```

- [ ] **Step 4: Build**

Run: `cargo build -p rightclaw`

- [ ] **Step 5: Commit**

```
feat(mcp): add InternalClient for bot→aggregator IPC via Unix socket
```

---

## Task 11: Bot Integration — Handler + OAuth Callback

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/oauth_callback.rs`
- Modify: `crates/bot/src/lib.rs` (add InternalClient to DI)

- [ ] **Step 1: Add InternalClient to bot dependency injection**

In `crates/bot/src/lib.rs`, create `InternalClient` and pass it through dptree DI alongside existing dependencies. The socket path is `~/.rightclaw/run/internal.sock`.

- [ ] **Step 2: Update handle_mcp_add**

In `handler.rs`, replace the current `handle_mcp_add`:

```rust
async fn handle_mcp_add(
    bot: Bot,
    msg: Message,
    config_str: String,
    internal: Arc<InternalClient>,
    agent_name: String,
) -> ResponseResult<()> {
    let parts: Vec<&str> = config_str.splitn(2, ' ').collect();
    if parts.len() != 2 {
        send_html_reply(&bot, &msg, "Usage: /mcp add &lt;name&gt; &lt;url&gt;").await?;
        return Ok(());
    }
    let (name, url) = (parts[0], parts[1]);

    match internal.mcp_add(&agent_name, name, url).await {
        Ok(resp) => {
            let mut reply = format!("Added MCP server <b>{name}</b>.");
            if resp.tools_count > 0 {
                reply.push_str(&format!(" {} tools available.", resp.tools_count));
            }
            if !resp.excluded.is_empty() {
                reply.push_str(&format!("\nExcluded tools (contain '__'): {}", resp.excluded.join(", ")));
            }
            if let Some(warning) = resp.warning {
                reply.push_str(&format!("\n⚠️ {warning}"));
            }
            reply.push_str("\nTools available on agent's next session.");
            send_html_reply(&bot, &msg, &reply).await?;
        }
        Err(e) => {
            send_html_reply(&bot, &msg, &format!("Failed to add MCP server: {e}")).await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Update handle_mcp_remove similarly**

Replace direct `remove_http_server` + upload with `internal.mcp_remove()`.

- [ ] **Step 4: Update complete_oauth_flow in oauth_callback.rs**

Replace the file-based token persistence with `internal.set_token()`:

```rust
// After exchanging code for tokens:
let internal = InternalClient::new(socket_path);
match internal.set_token(&SetTokenRequest {
    agent: agent_name.clone(),
    server: server_name.clone(),
    access_token: token_response.access_token,
    refresh_token: token_response.refresh_token.unwrap_or_default(),
    expires_in: token_response.expires_in.unwrap_or(3600),
    token_endpoint: pending_auth.token_endpoint,
    client_id: pending_auth.client_id,
    client_secret: pending_auth.client_secret,
}).await {
    Ok(resp) => {
        // Notify Telegram success
    }
    Err(e) => {
        tracing::error!("set-token failed: {e:#}");
        // Notify Telegram failure
    }
}
```

Remove the old code that writes to `.mcp.json` and uploads to sandbox.

- [ ] **Step 5: Build**

Run: `cargo build --workspace`

- [ ] **Step 6: Commit**

```
feat(bot): route /mcp commands through internal API
```

---

## Task 12: Refresh Scheduler Migration

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs`
- Modify: `crates/rightclaw-cli/src/aggregator.rs` (start scheduler)

- [ ] **Step 1: Update RefreshMessage to carry token Arc**

In `refresh.rs`, update:

```rust
pub enum RefreshMessage {
    NewEntry {
        server_name: String,
        state: OAuthServerState,
        token: Arc<tokio::sync::RwLock<Option<String>>>,
    },
    RemoveServer {
        server_name: String,
    },
}
```

- [ ] **Step 2: Update run_refresh_scheduler signature**

Remove `mcp_json_path` and `sandbox_name`. Add token map:

```rust
pub async fn run_refresh_scheduler(
    oauth_state_path: PathBuf,
    rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
    notify_tx: tokio::sync::mpsc::Sender<String>,
) {
    // On NewEntry: store token Arc in HashMap<server_name, Arc<RwLock<Option<String>>>>
    // On refresh: write new token directly to the Arc
    // Remove: set_server_header() and upload_file() calls
    // Keep: save_oauth_state() for persistence across restarts
}
```

- [ ] **Step 3: Update do_refresh to write to token Arc**

Replace `set_server_header` + `upload_file` with:

```rust
// After successful token exchange:
if let Some(token_arc) = token_handles.get(&server_name) {
    *token_arc.write().await = Some(new_access_token.clone());
}
```

- [ ] **Step 4: Start refresh scheduler in Aggregator**

In `run_aggregator_http`, spawn the refresh scheduler per-agent:

```rust
let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel(32);
let oauth_state_path = agent_dir.join("oauth-state.json");
tokio::spawn(run_refresh_scheduler(oauth_state_path, refresh_rx, notify_tx));
```

- [ ] **Step 5: Build and test**

Run: `cargo build --workspace`

- [ ] **Step 6: Commit**

```
refactor(mcp): migrate refresh scheduler to Aggregator process
```

---

## Task 13: mcp_config.rs Simplification

**Files:**
- Modify: `crates/rightclaw/src/codegen/mcp_config.rs`

- [ ] **Step 1: Update generate_mcp_config_http to write from scratch**

Change the function to not merge with existing entries — always write exactly one `right` entry:

```rust
pub fn generate_mcp_config_http(
    agent_path: &Path,
    agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
) -> Result<(), std::io::Error> {
    let mcp_json = serde_json::json!({
        "mcpServers": {
            "right": {
                "type": "http",
                "url": right_mcp_url,
                "headers": {
                    "Authorization": format!("Bearer {bearer_token}")
                }
            }
        }
    });
    
    let path = agent_path.join(".mcp.json");
    let content = serde_json::to_string_pretty(&mcp_json)?;
    // Atomic write
    let tmp = tempfile::NamedTempFile::new_in(agent_path)?;
    std::io::Write::write_all(&mut &tmp, content.as_bytes())?;
    tmp.persist(&path)?;
    Ok(())
}
```

- [ ] **Step 2: Update tests**

Update existing tests to verify that stale external entries are stripped on regeneration.

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw -- mcp_config`

- [ ] **Step 4: Commit**

```
fix(codegen): write .mcp.json from scratch, strip stale external entries
```

---

## Task 14: Documentation Updates

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md` (if exists)

- [ ] **Step 1: Update ARCHITECTURE.md**

Update module map to show:
- `aggregator.rs` replacing `memory_server_http.rs`
- `right_backend.rs` — extracted tool methods
- `internal_api.rs` — Unix socket REST API
- New types in `mcp/proxy.rs` and `mcp/internal_client.rs`

Update data flows:
- MCP Token Refresh section — now runs in Aggregator process
- Agent Lifecycle — `.mcp.json` always single entry
- Telegram `/mcp` commands — route through internal API

Update Directory Layout:
- Add `~/.rightclaw/run/internal.sock`

- [ ] **Step 2: Commit**

```
docs: update ARCHITECTURE.md for MCP Aggregator
```

---

## Dependency Graph

```
Task 1 (migration) ──┐
Task 2 (credentials) ┤
Task 3 (core types) ──┼──▶ Task 4 (RightBackend) ──▶ Task 5 (Aggregator structs)
                      │                                        │
                      │                                        ▼
                      │                              Task 6 (ServerHandler)
                      │                                        │
                      │                                        ▼
                      │                              Task 7 (wire main.rs) ──▶ Task 8 (ProxyBackend)
                      │                                                              │
                      │                                                              ▼
                      └──────────────────────────────────────────────────── Task 9 (internal API)
                                                                                     │
                                                                                     ▼
                                                                          Task 10 (InternalClient)
                                                                                     │
                                                                                     ▼
                                                                          Task 11 (bot integration)
                                                                                     │
                                                                                     ▼
                                                                          Task 12 (refresh migration)
                                                                                     │
                                                                                     ▼
                                                                          Task 13 (mcp_config cleanup)
                                                                                     │
                                                                                     ▼
                                                                          Task 14 (docs)
```

Tasks 1-3 can run in parallel. Everything else is sequential.
