# Post-Aggregator Cleanup & MCP Instructions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clean up dead code after MCP Aggregator merge, deliver upstream MCP instructions via generated `MCP_INSTRUCTIONS.md`, fix `/mcp list` to use Aggregator, make TOOLS.md agent-owned.

**Architecture:** Bottom-up — SQLite schema first, then core library changes (credentials, proxy), then CLI/aggregator changes, then bot integration, then documentation. Each task produces a compilable, testable state.

**Tech Stack:** Rust (edition 2024), rusqlite, rmcp, axum, hyper (UDS), serde, tokio

---

## Dependency Graph

```
Task 1 (V9 migration)
  → Task 2 (credentials: McpServerEntry, db_update_instructions, upsert fix)
    → Task 3 (codegen/mcp_instructions.rs)
    → Task 4 (proxy.rs: agent_dir, instructions on connect)
      → Task 5 (aggregator: regenerate file after connect)

Task 6 (move types from memory_server_http → aggregator) — independent
  → Task 7 (delete memory_server_http.rs)

Task 8 (internal_api: /mcp-list endpoint) — depends on Task 6
  → Task 9 (internal_client: mcp_list method)
    → Task 10 (handler.rs: rewrite handle_mcp_list)

Task 2 → Task 11 (migrate mcp_auth_status callers to db_list_servers)
  → Task 12 (delete detect.rs)

Task 3 → Task 13 (agent_def + pipeline: MCP_INSTRUCTIONS.md + TOOLS.md)

Task 14 (documentation updates) — after all code tasks
Task 15 (templates/right/AGENTS.md update)
Task 16 (run /review-loop)
```

---

### Task 1: V9 Migration — Add `instructions` Column

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v9_mcp_instructions.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the migration SQL**

Create `crates/rightclaw/src/memory/sql/v9_mcp_instructions.sql`:

```sql
ALTER TABLE mcp_servers ADD COLUMN instructions TEXT;
```

- [ ] **Step 2: Write the failing test**

In `crates/rightclaw/src/memory/migrations.rs`, add test at the end of the `tests` module:

```rust
#[test]
fn user_version_is_9() {
    let dir = tempfile::tempdir().unwrap();
    let conn = super::open_connection(dir.path()).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 9, "expected schema version 9 after migrations");
}

#[test]
fn v9_mcp_servers_has_instructions_column() {
    let dir = tempfile::tempdir().unwrap();
    let conn = super::open_connection(dir.path()).unwrap();
    conn.execute(
        "INSERT INTO mcp_servers (name, url) VALUES ('test', 'https://example.com/mcp')",
        [],
    )
    .unwrap();
    // instructions column should exist and default to NULL
    let instructions: Option<String> = conn
        .query_row(
            "SELECT instructions FROM mcp_servers WHERE name = 'test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(instructions.is_none());
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib memory::migrations::tests::user_version_is_9`
Expected: FAIL — version is 8

- [ ] **Step 4: Register V9 migration**

In `crates/rightclaw/src/memory/migrations.rs`, add the V9 schema constant and update the migration list.

Find the line:

```rust
const V8_SCHEMA: &str = include_str!("sql/v8_mcp_servers.sql");
```

Add after it:

```rust
const V9_SCHEMA: &str = include_str!("sql/v9_mcp_instructions.sql");
```

In the `MIGRATIONS` lazy static, add `M::up(V9_SCHEMA)` after the V8 entry.

Also update the existing `user_version_is_8` test — rename it or update its expected value. Since V9 is now the latest, the old test checking for version 8 will fail. Change it:

```rust
// Rename test from user_version_is_8 to user_version_is_latest
// and update assertion to 9
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib memory::migrations::tests`
Expected: all migration tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v9_mcp_instructions.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(schema): add V9 migration — instructions column on mcp_servers"
```

---

### Task 2: Credentials — `McpServerEntry`, `db_update_instructions`, Upsert Fix

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs`

- [ ] **Step 1: Write failing tests**

Add to the `db_tests` module in `credentials.rs`:

```rust
#[test]
fn update_and_list_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rightclaw::memory::open_connection(dir.path()).unwrap();
    db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();

    // Instructions start as None
    let servers = db_list_servers(&conn).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "notion");
    assert_eq!(servers[0].url, "https://mcp.notion.com/mcp");
    assert!(servers[0].instructions.is_none());

    // Update instructions
    db_update_instructions(&conn, "notion", Some("Use Notion tools to search pages")).unwrap();
    let servers = db_list_servers(&conn).unwrap();
    assert_eq!(servers[0].instructions.as_deref(), Some("Use Notion tools to search pages"));

    // Clear instructions
    db_update_instructions(&conn, "notion", None).unwrap();
    let servers = db_list_servers(&conn).unwrap();
    assert!(servers[0].instructions.is_none());
}

#[test]
fn upsert_preserves_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rightclaw::memory::open_connection(dir.path()).unwrap();
    db_add_server(&conn, "notion", "https://old.notion.com/mcp").unwrap();
    db_update_instructions(&conn, "notion", Some("Notion instructions")).unwrap();

    // Re-add with new URL — instructions should survive
    db_add_server(&conn, "notion", "https://new.notion.com/mcp").unwrap();
    let servers = db_list_servers(&conn).unwrap();
    assert_eq!(servers[0].url, "https://new.notion.com/mcp");
    assert_eq!(servers[0].instructions.as_deref(), Some("Notion instructions"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib mcp::credentials::db_tests`
Expected: FAIL — `McpServerEntry` not found, `db_update_instructions` not found

- [ ] **Step 3: Implement `McpServerEntry` and update `db_list_servers`**

In `credentials.rs`, remove the unused import:

```rust
// DELETE this line:
use std::net::IpAddr;
```

Add the struct before `db_add_server`:

```rust
/// Entry returned by `db_list_servers`.
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub url: String,
    pub instructions: Option<String>,
}
```

Replace the `db_list_servers` function body (currently returns `Vec<(String, String)>`):

```rust
pub fn db_list_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    let mut stmt = conn
        .prepare("SELECT name, url, instructions FROM mcp_servers ORDER BY name")
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(McpServerEntry {
                name: row.get(0)?,
                url: row.get(1)?,
                instructions: row.get(2)?,
            })
        })
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))
}
```

- [ ] **Step 4: Fix `db_add_server` upsert**

Replace the `INSERT OR REPLACE` in `db_add_server`:

```rust
conn.execute(
    "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2) \
     ON CONFLICT(name) DO UPDATE SET url = excluded.url",
    rusqlite::params![name, url],
)
```

- [ ] **Step 5: Implement `db_update_instructions`**

Add after `db_remove_server`:

```rust
/// Update the cached instructions for an MCP server.
pub fn db_update_instructions(
    conn: &Connection,
    name: &str,
    instructions: Option<&str>,
) -> Result<(), CredentialError> {
    let changed = conn
        .execute(
            "UPDATE mcp_servers SET instructions = ?1 WHERE name = ?2",
            rusqlite::params![instructions, name],
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}
```

- [ ] **Step 6: Fix existing tests that destructure `(String, String)`**

The existing `add_and_list_servers` test destructures tuples. Update it to use `McpServerEntry` fields:

```rust
// Old: let (name, url) = &servers[0];
// New: access via servers[0].name, servers[0].url
```

Also update `add_and_remove_server` and `list_servers_empty` tests if they destructure tuples.

- [ ] **Step 7: Run all credentials tests**

Run: `cargo test -p rightclaw --lib mcp::credentials`
Expected: all PASS

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/mcp/credentials.rs
git commit -m "feat(credentials): McpServerEntry, db_update_instructions, upsert fix"
```

---

### Task 3: `codegen/mcp_instructions.rs` — Pure Generation Function

**Files:**
- Create: `crates/rightclaw/src/codegen/mcp_instructions.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Create the module with tests first**

Create `crates/rightclaw/src/codegen/mcp_instructions.rs`:

```rust
use crate::mcp::credentials::McpServerEntry;

/// Generate `MCP_INSTRUCTIONS.md` content from registered MCP servers.
///
/// Only includes servers that have cached instructions (non-None).
/// Returns just the heading if no servers have instructions.
pub fn generate_mcp_instructions_md(servers: &[McpServerEntry]) -> String {
    let mut out = String::from("# MCP Server Instructions\n");

    for server in servers {
        if let Some(ref instructions) = server.instructions {
            out.push_str(&format!("\n## {}\n\n{}\n", server.name, instructions));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_servers_returns_header_only() {
        let result = generate_mcp_instructions_md(&[]);
        assert_eq!(result, "# MCP Server Instructions\n");
    }

    #[test]
    fn servers_without_instructions_skipped() {
        let servers = vec![McpServerEntry {
            name: "notion".into(),
            url: "https://mcp.notion.com/mcp".into(),
            instructions: None,
        }];
        let result = generate_mcp_instructions_md(&servers);
        assert_eq!(result, "# MCP Server Instructions\n");
    }

    #[test]
    fn servers_with_instructions_included() {
        let servers = vec![McpServerEntry {
            name: "notion".into(),
            url: "https://mcp.notion.com/mcp".into(),
            instructions: Some("Search and update Notion pages.".into()),
        }];
        let result = generate_mcp_instructions_md(&servers);
        assert!(result.contains("## notion"));
        assert!(result.contains("Search and update Notion pages."));
    }

    #[test]
    fn mixed_servers_only_with_instructions() {
        let servers = vec![
            McpServerEntry {
                name: "composio".into(),
                url: "https://connect.composio.dev/mcp".into(),
                instructions: Some("Connect with 250+ apps.".into()),
            },
            McpServerEntry {
                name: "linear".into(),
                url: "https://mcp.linear.app/mcp".into(),
                instructions: None,
            },
            McpServerEntry {
                name: "notion".into(),
                url: "https://mcp.notion.com/mcp".into(),
                instructions: Some("Notion tools.".into()),
            },
        ];
        let result = generate_mcp_instructions_md(&servers);
        assert!(result.contains("## composio"));
        assert!(result.contains("## notion"));
        assert!(!result.contains("## linear"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/rightclaw/src/codegen/mod.rs`, add:

```rust
pub mod mcp_instructions;
```

And add the public export:

```rust
pub use mcp_instructions::generate_mcp_instructions_md;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw --lib codegen::mcp_instructions`
Expected: all PASS

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/codegen/mcp_instructions.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "feat(codegen): add generate_mcp_instructions_md pure function"
```

---

### Task 4: `proxy.rs` — `agent_dir` Field, Instructions on Connect

**Files:**
- Modify: `crates/rightclaw/src/mcp/proxy.rs`

- [ ] **Step 1: Add `agent_dir` field to `ProxyBackend`**

In `proxy.rs`, add `agent_dir: PathBuf` to the `ProxyBackend` struct fields (after `server_name`):

```rust
pub struct ProxyBackend {
    server_name: String,
    agent_dir: PathBuf,
    url: String,
    // ... rest unchanged
}
```

Add `use std::path::PathBuf;` to imports if not present.

- [ ] **Step 2: Update `ProxyBackend::new()` to accept `agent_dir`**

```rust
pub fn new(
    server_name: String,
    agent_dir: PathBuf,
    url: String,
    token: Arc<RwLock<Option<String>>>,
) -> Self {
    Self {
        server_name,
        agent_dir,
        url,
        cached_tools: RwLock::new(Vec::new()),
        status: RwLock::new(BackendStatus::Unreachable),
        token,
        client: RwLock::new(None),
    }
}
```

- [ ] **Step 3: Remove `cached_instructions` field and `instructions()` method**

Delete the `cached_instructions: RwLock<Option<String>>` field from the struct.

Delete the `instructions()` method (around line 276-278):

```rust
// DELETE:
pub async fn instructions(&self) -> Option<String> {
    self.cached_instructions.read().await.clone()
}
```

Remove the `cached_instructions` initialization from `new()` if present.

- [ ] **Step 4: Update `connect()` to write instructions to SQLite and return them**

Change `connect()` signature from `Result<(), ProxyError>` to `Result<Option<String>, ProxyError>`.

After the successful connect (where tools are cached), add:

```rust
// Cache instructions in SQLite
let instructions = running.peer_server_info().instructions.clone();
if let Ok(conn) = crate::memory::open_connection(&self.agent_dir) {
    if let Err(e) = crate::mcp::credentials::db_update_instructions(
        &conn,
        &self.server_name,
        instructions.as_deref(),
    ) {
        tracing::warn!(server = %self.server_name, "failed to cache instructions: {e:#}");
    }
}
```

Remove any line that writes to `cached_instructions`.

At the end of the successful path, return `Ok(instructions)` instead of `Ok(())`.

For error paths that currently return `Ok(())` (e.g. NeedsAuth), return `Ok(None)`.

- [ ] **Step 5: Fix test compilation**

Update tests in `proxy.rs` that call `ProxyBackend::new()` to pass an `agent_dir` parameter. Use `tempfile::tempdir()` for test agent dirs. Also update any assertions on `connect()` return type.

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw --lib mcp::proxy`
Expected: PASS (network tests may be ignored)

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/mcp/proxy.rs
git commit -m "feat(proxy): add agent_dir, write instructions to SQLite on connect"
```

---

### Task 5: Aggregator — Regenerate `MCP_INSTRUCTIONS.md` After Connect

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Remove dead code from `aggregator.rs`**

Delete these items from `aggregator.rs`:

```rust
// DELETE: constant
const INSTRUCTIONS_TRUNCATION_LIMIT: usize = 4000;

// DELETE: BackendRegistry::build_instructions() method (entire async fn)

// DELETE: ToolDispatcher::instructions() method (entire async fn)
```

- [ ] **Step 2: Add `regenerate_mcp_instructions` helper to `BackendRegistry`**

```rust
impl BackendRegistry {
    /// Regenerate MCP_INSTRUCTIONS.md from SQLite-cached instructions.
    pub(crate) fn regenerate_mcp_instructions(&self) -> Result<(), anyhow::Error> {
        let conn = rightclaw::memory::open_connection(&self.agent_dir)?;
        let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
        let content = rightclaw::codegen::generate_mcp_instructions_md(&servers);
        std::fs::write(self.agent_dir.join("MCP_INSTRUCTIONS.md"), &content)?;
        // Also copy to .claude/agents/ for @ ref resolution
        let agents_dir = self.agent_dir.join(".claude/agents");
        if agents_dir.exists() {
            std::fs::write(agents_dir.join("MCP_INSTRUCTIONS.md"), &content)?;
        }
        tracing::debug!(agent_dir = %self.agent_dir.display(), "regenerated MCP_INSTRUCTIONS.md");
        Ok(())
    }
}
```

- [ ] **Step 3: Call regenerate after connect in `dispatch_to_proxy`**

In `BackendRegistry::dispatch_to_proxy`, after the lazy `connect()` call succeeds and returns instructions, call `regenerate_mcp_instructions`:

Find the section in `dispatch_to_proxy` where `proxy.connect(...)` is called. After it, add:

```rust
if instructions.is_some() {
    if let Err(e) = self.regenerate_mcp_instructions() {
        tracing::warn!("failed to regenerate MCP_INSTRUCTIONS.md: {e:#}");
    }
}
```

- [ ] **Step 4: Update `ProxyBackend::new()` callsites to pass `agent_dir`**

In `internal_api.rs`, find `handle_mcp_add` where `ProxyBackend::new(...)` is called. Add `agent_dir` parameter. The handler has access to the agent's `BackendRegistry` which has `agent_dir`. Extract it:

```rust
let agent_dir = registry.agent_dir.clone();
let proxy = Arc::new(ProxyBackend::new(
    req.name.clone(),
    agent_dir,
    req.url.clone(),
    token,
));
```

Also update the aggregator startup code in `main.rs` where `BackendRegistry` is constructed — ensure `agent_dir` is passed through to any `ProxyBackend::new()` calls at startup (if any exist for restoring from SQLite).

- [ ] **Step 5: Run build**

Run: `cargo build --workspace`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs crates/rightclaw-cli/src/internal_api.rs
git commit -m "feat(aggregator): regenerate MCP_INSTRUCTIONS.md after backend connect"
```

---

### Task 6: Move Types from `memory_server_http.rs` to `aggregator.rs`

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs` (will be deleted in Task 7)

- [ ] **Step 1: Copy `AgentInfo`, `AgentTokenMap`, `bearer_auth_middleware` to `aggregator.rs`**

Add these to the top of `aggregator.rs` (after existing imports):

```rust
use std::sync::RwLock;

/// Token -> agent mapping for multi-agent HTTP mode.
pub(crate) type AgentTokenMap = Arc<std::sync::RwLock<HashMap<String, AgentInfo>>>;

/// Agent identity resolved from a Bearer token.
#[derive(Clone, Debug)]
pub(crate) struct AgentInfo {
    pub name: String,
    pub dir: PathBuf,
}

/// Axum middleware: extract Bearer token, look up agent, inject into request extensions.
pub(crate) async fn bearer_auth_middleware(
    axum::extract::State(token_map): axum::extract::State<AgentTokenMap>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Copy the implementation from memory_server_http.rs
    // (extract Authorization header, look up in token_map, insert AgentInfo)
}
```

Copy the exact implementation from `memory_server_http.rs` lines 546-588.

- [ ] **Step 2: Update imports in `aggregator.rs`**

Remove the import from `memory_server_http`:

```rust
// DELETE:
use crate::memory_server_http::{AgentInfo, AgentTokenMap, bearer_auth_middleware};
```

- [ ] **Step 3: Verify build**

Run: `cargo build --workspace`
Expected: compiles (memory_server_http.rs still exists but its exports are no longer used by aggregator)

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "refactor: move AgentInfo, AgentTokenMap, bearer_auth_middleware to aggregator"
```

---

### Task 7: Delete `memory_server_http.rs`

**Files:**
- Delete: `crates/rightclaw-cli/src/memory_server_http.rs`
- Delete: `crates/rightclaw-cli/src/memory_server_http_tests.rs` (if exists)
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Remove `mod memory_server_http` from `main.rs`**

Delete the line:

```rust
mod memory_server_http;
```

Also remove `Commands::MemoryServerHttp` variant from the CLI enum and its dispatch in the match statement. Remove the `MemoryServerHttp` arm from the unreachable comment. Remove any remaining imports from `memory_server_http`.

- [ ] **Step 2: Delete the files**

```bash
rm crates/rightclaw-cli/src/memory_server_http.rs
rm -f crates/rightclaw-cli/src/memory_server_http_tests.rs
```

- [ ] **Step 3: Verify build**

Run: `cargo build --workspace`
Expected: compiles. If any other file still imports from `memory_server_http`, fix those imports.

- [ ] **Step 4: Commit**

```bash
git add -A crates/rightclaw-cli/src/
git commit -m "refactor: delete dead memory_server_http.rs (replaced by aggregator)"
```

---

### Task 8: Internal API — `/mcp-list` Endpoint

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`

- [ ] **Step 1: Add request/response types**

Add after existing types in `internal_api.rs`:

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct McpListRequest {
    pub agent: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct McpListResponse {
    pub servers: Vec<McpServerStatus>,
}

#[derive(Debug, Serialize)]
pub(crate) struct McpServerStatus {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
}
```

- [ ] **Step 2: Write the handler**

```rust
async fn handle_mcp_list(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpListRequest>,
) -> axum::response::Response {
    let Some(registry) = dispatcher.agents.get(&req.agent) else {
        return not_found("agent_not_found");
    };

    let mut servers = Vec::new();

    // Right backend (always connected)
    servers.push(McpServerStatus {
        name: "right".into(),
        url: None,
        status: "connected".into(),
        tool_count: registry.right.tools_list().len(),
    });

    // External proxy backends
    let proxies = registry.proxies.read().await;
    for (name, proxy) in proxies.iter() {
        let status = proxy.status().await;
        let tool_count = proxy.try_tools().map(|t| t.len()).unwrap_or(0);
        servers.push(McpServerStatus {
            name: name.clone(),
            url: Some(proxy.url().to_string()),
            status: match status {
                rightclaw::mcp::proxy::BackendStatus::Connected => "connected",
                rightclaw::mcp::proxy::BackendStatus::NeedsAuth => "needs_auth",
                rightclaw::mcp::proxy::BackendStatus::Unreachable => "unreachable",
            }
            .into(),
            tool_count,
        });
    }

    Json(McpListResponse { servers }).into_response()
}
```

Note: `ProxyBackend` may need a public `url()` accessor. If it doesn't exist, add `pub fn url(&self) -> &str { &self.url }` to `proxy.rs`.

- [ ] **Step 3: Register the route**

In `internal_router`, add:

```rust
.route("/mcp-list", post(handle_mcp_list))
```

- [ ] **Step 4: Write test**

Add to tests module in `internal_api.rs`:

```rust
#[tokio::test]
async fn mcp_list_returns_right_backend() {
    let (dispatcher, _, _tmp) = make_test_dispatcher();
    let app = internal_router(dispatcher);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/mcp-list")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_string(&McpListRequest {
                        agent: "test-agent".into(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let result: McpListResponse = serde_json::from_slice(&body).unwrap();
    assert!(result.servers.iter().any(|s| s.name == "right" && s.status == "connected"));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw-cli --lib internal_api`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs crates/rightclaw/src/mcp/proxy.rs
git commit -m "feat(internal-api): add POST /mcp-list endpoint"
```

---

### Task 9: InternalClient — `mcp_list()` Method

**Files:**
- Modify: `crates/rightclaw/src/mcp/internal_client.rs`

- [ ] **Step 1: Add response types**

Add after existing response types:

```rust
#[derive(Debug, Deserialize)]
pub struct McpListResponse {
    pub servers: Vec<McpServerStatus>,
}

#[derive(Debug, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
}
```

- [ ] **Step 2: Add `mcp_list()` method**

```rust
pub async fn mcp_list(&self, agent: &str) -> Result<McpListResponse, InternalClientError> {
    self.post("/mcp-list", &serde_json::json!({"agent": agent})).await
}
```

- [ ] **Step 3: Verify build**

Run: `cargo check -p rightclaw`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/mcp/internal_client.rs
git commit -m "feat(internal-client): add mcp_list() method"
```

---

### Task 10: Bot Handler — Rewrite `handle_mcp_list`

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Rewrite `handle_mcp_list`**

Replace the current `handle_mcp_list` function. It currently calls `mcp_auth_status`. Change to call `internal_client.mcp_list()`:

```rust
/// `/mcp list` -- show all MCP servers via Aggregator internal API.
async fn handle_mcp_list(
    bot: &BotType,
    msg: &Message,
    agent_name: &str,
    internal: &rightclaw::mcp::internal_client::InternalClient,
) -> Result<(), RequestError> {
    tracing::info!(agent = %agent_name, "mcp list");

    let result = match internal.mcp_list(agent_name).await {
        Ok(r) => r,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error listing MCP servers: {e:#}"))
                .await?;
            return Ok(());
        }
    };

    if result.servers.is_empty() {
        bot.send_message(msg.chat.id, "No MCP servers configured.")
            .await?;
        return Ok(());
    }

    let mut text = String::from("MCP Servers:\n\n");
    for s in &result.servers {
        let url_part = s.url.as_deref().map(|u| format!(" [{u}]")).unwrap_or_default();
        text.push_str(&format!(
            "  {} -- {} ({} tools){}\n",
            s.name, s.status, s.tool_count, url_part
        ));
    }
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}
```

- [ ] **Step 2: Update the callsite**

Find where `handle_mcp_list` is called (in the `/mcp` command handler). Update parameters — it now needs `agent_name: &str` and `internal: &InternalClient` instead of `agent_dir: &Path`. These should already be available in the handler via DI (InternalApi and AgentName newtypes).

- [ ] **Step 3: Add MCP_INSTRUCTIONS.md sync after `/mcp add`, `/mcp remove`, `/mcp auth`**

After each successful internal API call in `handle_mcp_add`, `handle_mcp_remove`, and after the OAuth callback completes, add a sync step. In the handler functions, after the internal API call succeeds:

```rust
// Sync MCP_INSTRUCTIONS.md to sandbox
let mcp_instr_path = agent_dir.join("MCP_INSTRUCTIONS.md");
if mcp_instr_path.exists() {
    // Copy to .claude/agents/ for @ ref
    let agents_subdir = agent_dir.join(".claude/agents");
    if agents_subdir.exists() {
        let _ = std::fs::copy(&mcp_instr_path, agents_subdir.join("MCP_INSTRUCTIONS.md"));
    }
    // Upload to sandbox if sandbox mode
    if let Some(ref sandbox) = sandbox_name {
        if let Err(e) = rightclaw::openshell::upload_file(sandbox, &mcp_instr_path, "/sandbox/").await {
            tracing::warn!("failed to sync MCP_INSTRUCTIONS.md to sandbox: {e:#}");
        }
    }
}
```

The `sandbox_name` should be available from the handler's DI context. Check what's available.

- [ ] **Step 4: Verify build**

Run: `cargo build --workspace`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): rewrite /mcp list via internal API, sync MCP_INSTRUCTIONS.md"
```

---

### Task 11: Migrate Remaining `mcp_auth_status` Callers

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/rightclaw-cli/src/memory_server.rs`
- Modify: `crates/rightclaw-cli/src/main.rs`
- Modify: `crates/rightclaw-cli/src/right_backend.rs`

- [ ] **Step 1: Migrate `doctor.rs`**

Find the MCP check in `doctor.rs` that calls `mcp_auth_status`. Replace with `db_list_servers`:

```rust
// Old: let statuses = mcp::detect::mcp_auth_status(agent_dir)?;
// New:
let conn = crate::memory::open_connection(agent_dir)?;
let servers = crate::mcp::credentials::db_list_servers(&conn)?;
```

Adjust the diagnostic output to use `McpServerEntry` fields instead of `ServerStatus` fields.

- [ ] **Step 2: Migrate `bot/src/lib.rs`**

Find the startup warning that calls `mcp_auth_status`. Replace:

```rust
// Old: let statuses = rightclaw::mcp::detect::mcp_auth_status(&agent_dir)?;
// New:
let conn = rightclaw::memory::open_connection(&agent_dir)?;
let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
```

Adjust warning logic to use `McpServerEntry`.

- [ ] **Step 3: Migrate `memory_server.rs` stdio `mcp_list`**

Find the `mcp_list` handler in `memory_server.rs`. Replace `mcp_auth_status` with `db_list_servers`:

```rust
// Old: let statuses = rightclaw::mcp::detect::mcp_auth_status(&agent_dir)?;
// New:
let conn = rightclaw::memory::open_connection(&agent_dir)?;
let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
```

Format the response using `McpServerEntry` fields.

- [ ] **Step 4: Migrate `main.rs` `cmd_mcp_status`**

Find `cmd_mcp_status` function. Replace `mcp_auth_status` with `db_list_servers`:

```rust
let conn = rightclaw::memory::open_connection(&agent_dir)?;
let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
```

- [ ] **Step 5: Migrate `right_backend.rs` `call_mcp_list`**

This one is special — `right_backend.rs` is inside the aggregator and has access to `BackendRegistry::do_mcp_list()`. But `call_mcp_list` is called via `tools_call` which doesn't have direct access to the registry.

Since the MCP tools (`mcp_add`, `mcp_remove`, `mcp_list`, `mcp_auth`) were moved to the bot's Telegram handler (they call internal API), check if `call_mcp_list` in `right_backend.rs` is still reachable. If agents can call `mcp_list` as an MCP tool (via `rightmeta__mcp_list`), that's handled by `BackendRegistry::do_mcp_list()`, not `right_backend.rs`.

If `call_mcp_list` in `right_backend.rs` is dead code (no longer routed to), delete it and update the `tools_list()` to not include `mcp_list`. If still routed to, replace with `db_list_servers`.

- [ ] **Step 6: Verify build**

Run: `cargo build --workspace`
Expected: compiles with no references to `mcp::detect`

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace`
Expected: PASS (or pre-existing failures only)

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/doctor.rs crates/bot/src/lib.rs crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/src/right_backend.rs
git commit -m "refactor: migrate all mcp_auth_status callers to db_list_servers"
```

---

### Task 12: Delete `detect.rs`

**Files:**
- Delete: `crates/rightclaw/src/mcp/detect.rs`
- Modify: `crates/rightclaw/src/mcp/mod.rs`

- [ ] **Step 1: Remove module declaration**

In `crates/rightclaw/src/mcp/mod.rs`, delete:

```rust
pub mod detect;
```

- [ ] **Step 2: Delete the file**

```bash
rm crates/rightclaw/src/mcp/detect.rs
```

- [ ] **Step 3: Verify build**

Run: `cargo build --workspace`
Expected: compiles with no errors. If any file still references `mcp::detect`, fix the import.

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A crates/rightclaw/src/mcp/
git commit -m "refactor: delete detect.rs — all callers migrated to SQLite"
```

---

### Task 13: Agent Definition + Pipeline — `MCP_INSTRUCTIONS.md` + TOOLS.md

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs`
- Modify: `crates/rightclaw/src/codegen/pipeline.rs`
- Modify: `crates/rightclaw/src/codegen/tools.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Add `MCP_INSTRUCTIONS.md` to `CONTENT_MD_FILES`**

In `crates/rightclaw/src/codegen/agent_def.rs`, add to the array:

```rust
pub const CONTENT_MD_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
    "MCP_INSTRUCTIONS.md",
];
```

- [ ] **Step 2: Add `@./MCP_INSTRUCTIONS.md` to agent definition**

In `generate_agent_definition()`, add after `@./TOOLS.md`:

```rust
    format!(
        "\
---
name: {name}
model: {model}
description: \"RightClaw agent: {name}\"
---

@./AGENTS.md

---

@./SOUL.md

---

@./IDENTITY.md

---

@./USER.md

---

@./TOOLS.md

---

@./MCP_INSTRUCTIONS.md
"
    )
```

- [ ] **Step 3: Update pipeline — TOOLS.md create-only, MCP_INSTRUCTIONS.md create-only**

In `crates/rightclaw/src/codegen/pipeline.rs`, find the TOOLS.md generation block. Replace:

```rust
// Old:
let tools_md = crate::codegen::generate_tools_md(&agent.name, &agent_sandbox_mode);
std::fs::write(agent.path.join("TOOLS.md"), &tools_md).map_err(|e| {
    miette::miette!("failed to write TOOLS.md for '{}': {e:#}", agent.name)
})?;
tracing::debug!(agent = %agent.name, "wrote TOOLS.md");

// New:
let tools_path = agent.path.join("TOOLS.md");
if !tools_path.exists() {
    std::fs::write(&tools_path, "").map_err(|e| {
        miette::miette!("failed to create TOOLS.md for '{}': {e:#}", agent.name)
    })?;
    tracing::debug!(agent = %agent.name, "created empty TOOLS.md (agent-owned)");
}

let mcp_instr_path = agent.path.join("MCP_INSTRUCTIONS.md");
if !mcp_instr_path.exists() {
    std::fs::write(&mcp_instr_path, "# MCP Server Instructions\n").map_err(|e| {
        miette::miette!("failed to create MCP_INSTRUCTIONS.md for '{}': {e:#}", agent.name)
    })?;
    tracing::debug!(agent = %agent.name, "created empty MCP_INSTRUCTIONS.md");
}
```

- [ ] **Step 4: Delete `generate_tools_md` from `tools.rs`**

Delete the `generate_tools_md` function and its tests from `crates/rightclaw/src/codegen/tools.rs`. If `tools.rs` becomes empty, delete the file and remove `mod tools` from `codegen/mod.rs`.

In `crates/rightclaw/src/codegen/mod.rs`, remove:

```rust
pub use tools::generate_tools_md;
```

- [ ] **Step 5: Write tests**

Add to pipeline tests in `pipeline.rs`:

```rust
#[test]
fn tools_md_not_overwritten_if_exists() {
    // Setup: create agent dir with TOOLS.md containing custom content
    // Run: run_single_agent_codegen
    // Assert: TOOLS.md content unchanged
}

#[test]
fn tools_md_created_empty_if_missing() {
    // Setup: create agent dir without TOOLS.md
    // Run: run_single_agent_codegen
    // Assert: TOOLS.md exists and is empty
}

#[test]
fn mcp_instructions_md_created_if_missing() {
    // Setup: create agent dir without MCP_INSTRUCTIONS.md
    // Run: run_single_agent_codegen
    // Assert: MCP_INSTRUCTIONS.md exists with header
}

#[test]
fn mcp_instructions_in_content_md_files() {
    assert!(rightclaw::codegen::CONTENT_MD_FILES.contains(&"MCP_INSTRUCTIONS.md"));
}

#[test]
fn agent_def_includes_mcp_instructions_ref() {
    let def = rightclaw::codegen::generate_agent_definition("test", None);
    assert!(def.contains("@./MCP_INSTRUCTIONS.md"));
}
```

- [ ] **Step 6: Update existing pipeline tests**

The existing `run_single_agent_codegen_produces_expected_files` test checks for `TOOLS.md`. It should still pass since we create it when missing. But verify the assertion doesn't check content.

Also update the `tools_list_returns_expected_count` test in `right_backend_tests.rs` if removing MCP tools from RightBackend changes the count.

- [ ] **Step 7: Run all tests**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/codegen/
git commit -m "feat(codegen): MCP_INSTRUCTIONS.md support, TOOLS.md agent-owned"
```

---

### Task 14: Documentation Updates

**Files:**
- Modify: `CLAUDE.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update `CLAUDE.md`**

Find the `with_instructions()` maintenance line and replace:

```
OLD: Update `with_instructions()` in both `memory_server.rs` and `memory_server_http.rs` to reflect the current tool set and descriptions.
NEW: Update `with_instructions()` in both `memory_server.rs` and `aggregator.rs` to reflect the current tool set and descriptions.
```

- [ ] **Step 2: Update `PROMPT_SYSTEM.md`**

Find the section referencing `memory_server_http.rs` for `with_instructions()` maintenance and update to reference `aggregator.rs`.

- [ ] **Step 3: Update `ARCHITECTURE.md`**

Multiple changes:

1. Module map for `rightclaw-cli`: remove `memory_server_http.rs` line, update `memory_server.rs` comment to "MCP stdio server (CLI-only)", add note that `aggregator.rs` now contains `AgentInfo`/`AgentTokenMap`/`bearer_auth_middleware`

2. Module map for `rightclaw`: remove `detect.rs` line, add `codegen/mcp_instructions.rs`

3. Configuration hierarchy table: change TOOLS.md to "agent-owned (created empty on init, then agent-edited)", add MCP_INSTRUCTIONS.md row as "generated by Aggregator from SQLite mcp_servers cache"

4. `mcp_config.rs` comment: change to "only right entry; externals managed by Aggregator"

5. Memory schema: add `instructions TEXT` to mcp_servers table definition

6. Data flow: note that ProxyBackend writes instructions to SQLite on connect, Aggregator regenerates `MCP_INSTRUCTIONS.md`

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: update CLAUDE.md, PROMPT_SYSTEM.md, ARCHITECTURE.md for post-aggregator changes"
```

---

### Task 15: Update `templates/right/AGENTS.md`

**Files:**
- Modify: `templates/right/AGENTS.md`

- [ ] **Step 1: Update MCP Management section**

Replace lines 32-40 (the MCP Management section) with:

```markdown
## MCP Management

To install, remove, or authorize MCP servers at runtime, use the `right` MCP tools:

- `mcp_add(name, url)` — register an external MCP server with the Aggregator
- `mcp_remove(name)` — unregister an MCP server (`right` is protected)
- `mcp_list()` — list all MCP servers with connection status and tool count
- `mcp_auth(server_name)` — get the OAuth authorization URL; send the link to the user via Telegram

MCP servers are proxied through the Aggregator — credentials never enter the sandbox.
Usage instructions from connected servers are automatically included in your context
via MCP_INSTRUCTIONS.md.
```

- [ ] **Step 2: Commit**

```bash
git add templates/right/AGENTS.md
git commit -m "docs: update AGENTS.md template for Aggregator-based MCP management"
```

---

### Task 16: Run `/review-loop`

- [ ] **Step 1: Run the review loop**

Use the review-loop skill to get a final review of all changes.

---

## Self-Review Checklist

**Spec coverage:**
- ✅ §1 `/mcp list` via internal API → Tasks 8, 9, 10
- ✅ §1 Migration of 6 callers → Task 11
- ✅ §1 Delete detect.rs → Task 12
- ✅ §2 Move types → Task 6
- ✅ §2 Delete memory_server_http → Task 7
- ✅ §2 Dead code in aggregator → Task 5 step 1
- ✅ §2 Dead code in proxy → Task 4 step 3
- ✅ §2 Dead code in credentials → Task 2 step 3
- ✅ §2 Delete generate_tools_md → Task 13 step 4
- ✅ §3 V9 migration → Task 1
- ✅ §3 Upsert fix → Task 2 step 4
- ✅ §3 db_update_instructions → Task 2 step 5
- ✅ §3 McpServerEntry + db_list_servers update → Task 2 step 3
- ✅ §3 generate_mcp_instructions_md → Task 3
- ✅ §3 ProxyBackend agent_dir + instructions on connect → Task 4
- ✅ §3 Caller regenerates file → Task 5
- ✅ §3 Agent def integration → Task 13 steps 1-2
- ✅ §3 Pipeline empty file creation → Task 13 step 3
- ✅ §3 Regeneration triggers → Task 5 (connect), Task 10 (bot sync)
- ✅ §4 TOOLS.md agent-owned → Task 13 steps 3-4
- ✅ §5 CLAUDE.md → Task 14
- ✅ §5 PROMPT_SYSTEM.md → Task 14
- ✅ §5 ARCHITECTURE.md → Task 14
- ✅ §5 templates/right/AGENTS.md → Task 15
- ✅ §6 All test types covered in respective tasks

**Type consistency:**
- `McpServerEntry` defined in Task 2, used in Tasks 3, 5, 8, 11
- `McpListResponse`/`McpServerStatus` defined in Task 8 (server-side) and Task 9 (client-side)
- `db_update_instructions` defined in Task 2, called in Task 4
- `generate_mcp_instructions_md` defined in Task 3, called in Task 5
- `ProxyBackend::new()` signature changed in Task 4, callsites updated in Task 5
- `connect()` return type changed in Task 4, callers updated in Task 5
