# API Key MCP Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support MCP servers that authenticate via static API keys (Bearer, custom header, or query string), in addition to the existing OAuth flow.

**Architecture:** Extend `mcp_servers` SQLite table with auth columns (replacing `oauth-state.json`). `DynamicAuthClient` gains an `AuthMethod` enum to inject credentials differently per auth type. `/mcp add` runs OAuth discovery first, then falls back to haiku-based classification (in sandbox) for public URLs or assumes Bearer for private URLs. Connection verification before persisting prevents bad credentials.

**Tech Stack:** Rust, SQLite (rusqlite), tokio, reqwest, teloxide, Claude CLI (haiku model)

---

### Task 1: SQLite Migration v10 — Add Auth Columns

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v10_mcp_auth.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write migration test**

Add to `crates/rightclaw/src/memory/migrations.rs` at the bottom of the `mod tests` block:

```rust
#[test]
fn v10_mcp_servers_has_auth_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO mcp_servers (name, url, auth_type, auth_token) VALUES (?1, ?2, ?3, ?4)",
        ("test", "https://example.com/mcp", "bearer", "sk-123"),
    )
    .unwrap();

    let auth_type: Option<String> = conn
        .query_row(
            "SELECT auth_type FROM mcp_servers WHERE name = ?1",
            ["test"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(auth_type.as_deref(), Some("bearer"));

    // Verify all new columns exist
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for col in [
        "auth_type",
        "auth_header",
        "auth_token",
        "refresh_token",
        "token_endpoint",
        "client_id",
        "client_secret",
        "expires_at",
    ] {
        assert!(cols.contains(&col.to_string()), "{col} column missing");
    }
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `devenv shell -- cargo test -p rightclaw v10_mcp_servers_has_auth_columns`
Expected: compilation error — `V10_SCHEMA` not defined.

- [ ] **Step 3: Write the SQL migration file**

Create `crates/rightclaw/src/memory/sql/v10_mcp_auth.sql`:

```sql
ALTER TABLE mcp_servers ADD COLUMN auth_type      TEXT;
ALTER TABLE mcp_servers ADD COLUMN auth_header    TEXT;
ALTER TABLE mcp_servers ADD COLUMN auth_token     TEXT;
ALTER TABLE mcp_servers ADD COLUMN refresh_token  TEXT;
ALTER TABLE mcp_servers ADD COLUMN token_endpoint TEXT;
ALTER TABLE mcp_servers ADD COLUMN client_id      TEXT;
ALTER TABLE mcp_servers ADD COLUMN client_secret  TEXT;
ALTER TABLE mcp_servers ADD COLUMN expires_at     TEXT;
```

- [ ] **Step 4: Register migration in migrations.rs**

In `crates/rightclaw/src/memory/migrations.rs`, add after line 11:

```rust
const V10_SCHEMA: &str = include_str!("sql/v10_mcp_auth.sql");
```

And add `M::up(V10_SCHEMA),` after `M::up(V9_SCHEMA),` in the migrations vec (line 24).

- [ ] **Step 5: Run test, verify it passes**

Run: `devenv shell -- cargo test -p rightclaw v10_mcp_servers_has_auth_columns`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v10_mcp_auth.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat: add v10 migration for MCP auth columns"
```

---

### Task 2: Extend `McpServerEntry` and Credential Functions

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs`

- [ ] **Step 1: Write tests for extended McpServerEntry and new DB functions**

Add to `crates/rightclaw/src/mcp/credentials.rs` test module (or create test file if tests exist separately). These tests need a helper to open an in-memory DB with migrations applied:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn db_add_server_with_auth() {
        let conn = test_conn();
        db_add_server(&conn, "test", "https://example.com/mcp").unwrap();
        db_set_auth(&conn, "test", "bearer", None, Some("sk-123")).unwrap();

        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].auth_type.as_deref(), Some("bearer"));
        assert_eq!(servers[0].auth_token.as_deref(), Some("sk-123"));
    }

    #[test]
    fn db_set_oauth_state() {
        let conn = test_conn();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            None,
            "2026-04-13T12:00:00Z",
        )
        .unwrap();

        let servers = db_list_servers(&conn).unwrap();
        let s = &servers[0];
        assert_eq!(s.auth_type.as_deref(), Some("oauth"));
        assert_eq!(s.auth_token.as_deref(), Some("access-tok"));
        assert_eq!(s.refresh_token.as_deref(), Some("refresh-tok"));
        assert_eq!(s.token_endpoint.as_deref(), Some("https://accounts.notion.com/oauth/token"));
        assert_eq!(s.client_id.as_deref(), Some("client-123"));
    }

    #[test]
    fn db_update_oauth_token() {
        let conn = test_conn();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "old-tok",
            Some("refresh-tok"),
            "https://example.com/token",
            "cid",
            None,
            "2026-04-13T12:00:00Z",
        )
        .unwrap();

        db_update_oauth_token(&conn, "notion", "new-tok", "2026-04-13T13:00:00Z").unwrap();

        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers[0].auth_token.as_deref(), Some("new-tok"));
        assert_eq!(servers[0].expires_at.as_deref(), Some("2026-04-13T13:00:00Z"));
    }

    #[test]
    fn db_list_oauth_servers() {
        let conn = test_conn();
        db_add_server(&conn, "oauth-srv", "https://a.com/mcp").unwrap();
        db_set_oauth_state(&conn, "oauth-srv", "tok", Some("rt"), "https://a.com/token", "c", None, "2026-04-13T12:00:00Z").unwrap();
        db_add_server(&conn, "bearer-srv", "https://b.com/mcp").unwrap();
        db_set_auth(&conn, "bearer-srv", "bearer", None, Some("key")).unwrap();

        let oauth = db_list_oauth_servers(&conn).unwrap();
        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].name, "oauth-srv");
    }

    #[test]
    fn redact_url_strips_query() {
        assert_eq!(redact_url("https://example.com/mcp?key=secret&foo=bar"), "https://example.com/mcp?<redacted>");
        assert_eq!(redact_url("https://example.com/mcp"), "https://example.com/mcp");
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `devenv shell -- cargo test -p rightclaw db_add_server_with_auth db_set_oauth_state db_update_oauth_token db_list_oauth_servers redact_url_strips_query`
Expected: compilation errors — new functions and fields don't exist yet.

- [ ] **Step 3: Extend `McpServerEntry` struct**

In `crates/rightclaw/src/mcp/credentials.rs`, replace the `McpServerEntry` struct (line 242-247):

```rust
/// Entry returned by `db_list_servers`.
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub url: String,
    pub instructions: Option<String>,
    pub auth_type: Option<String>,
    pub auth_header: Option<String>,
    pub auth_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_endpoint: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub expires_at: Option<String>,
}
```

- [ ] **Step 4: Update `db_list_servers` to read new columns**

Replace the `db_list_servers` function (line 305-325) to read all columns:

```rust
pub fn db_list_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, url, instructions, auth_type, auth_header, auth_token, \
             refresh_token, token_endpoint, client_id, client_secret, expires_at \
             FROM mcp_servers ORDER BY name",
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(McpServerEntry {
                name: row.get(0)?,
                url: row.get(1)?,
                instructions: row.get(2)?,
                auth_type: row.get(3)?,
                auth_header: row.get(4)?,
                auth_token: row.get(5)?,
                refresh_token: row.get(6)?,
                token_endpoint: row.get(7)?,
                client_id: row.get(8)?,
                client_secret: row.get(9)?,
                expires_at: row.get(10)?,
            })
        })
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))
}
```

- [ ] **Step 5: Add new DB helper functions**

Add after `db_update_instructions`:

```rust
/// Set auth type and optional token for a server (bearer, header, query_string).
pub fn db_set_auth(
    conn: &Connection,
    name: &str,
    auth_type: &str,
    auth_header: Option<&str>,
    auth_token: Option<&str>,
) -> Result<(), CredentialError> {
    let rows = conn
        .execute(
            "UPDATE mcp_servers SET auth_type = ?1, auth_header = ?2, auth_token = ?3 WHERE name = ?4",
            rusqlite::params![auth_type, auth_header, auth_token, name],
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_owned()));
    }
    Ok(())
}

/// Set full OAuth state for a server.
pub fn db_set_oauth_state(
    conn: &Connection,
    name: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    expires_at: &str,
) -> Result<(), CredentialError> {
    let rows = conn
        .execute(
            "UPDATE mcp_servers SET auth_type = 'oauth', auth_token = ?1, refresh_token = ?2, \
             token_endpoint = ?3, client_id = ?4, client_secret = ?5, expires_at = ?6 \
             WHERE name = ?7",
            rusqlite::params![access_token, refresh_token, token_endpoint, client_id, client_secret, expires_at, name],
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_owned()));
    }
    Ok(())
}

/// Update just the access token and expiry (used by refresh scheduler).
pub fn db_update_oauth_token(
    conn: &Connection,
    name: &str,
    access_token: &str,
    expires_at: &str,
) -> Result<(), CredentialError> {
    let rows = conn
        .execute(
            "UPDATE mcp_servers SET auth_token = ?1, expires_at = ?2 WHERE name = ?3",
            rusqlite::params![access_token, expires_at, name],
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_owned()));
    }
    Ok(())
}

/// List servers with auth_type = 'oauth' that have a refresh_token.
pub fn db_list_oauth_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, url, instructions, auth_type, auth_header, auth_token, \
             refresh_token, token_endpoint, client_id, client_secret, expires_at \
             FROM mcp_servers WHERE auth_type = 'oauth' AND refresh_token IS NOT NULL \
             ORDER BY name",
        )
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(McpServerEntry {
                name: row.get(0)?,
                url: row.get(1)?,
                instructions: row.get(2)?,
                auth_type: row.get(3)?,
                auth_header: row.get(4)?,
                auth_token: row.get(5)?,
                refresh_token: row.get(6)?,
                token_endpoint: row.get(7)?,
                client_id: row.get(8)?,
                client_secret: row.get(9)?,
                expires_at: row.get(10)?,
            })
        })
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))
}

/// Redact query string from URL for display purposes.
/// Returns `scheme://host/path?<redacted>` if URL has query params,
/// or the URL unchanged if no query string.
pub fn redact_url(url: &str) -> String {
    match url.find('?') {
        Some(pos) => format!("{}?<redacted>", &url[..pos]),
        None => url.to_owned(),
    }
}
```

- [ ] **Step 6: Fix any compilation errors in existing code**

The `McpServerEntry` struct changed — find all places constructing it (mainly in tests in `mcp_instructions.rs`) and add the new fields with `None` defaults. Search for `McpServerEntry {` across the codebase and update each occurrence to include the new fields.

Check: `devenv shell -- cargo check --workspace`

- [ ] **Step 7: Run tests, verify they pass**

Run: `devenv shell -- cargo test -p rightclaw db_add_server_with_auth db_set_oauth_state db_update_oauth_token db_list_oauth_servers redact_url_strips_query`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/mcp/credentials.rs crates/rightclaw/src/codegen/mcp_instructions.rs
git commit -m "feat: extend McpServerEntry with auth fields, add DB helpers"
```

---

### Task 3: `AuthMethod` Enum and `DynamicAuthClient` Changes

**Files:**
- Modify: `crates/rightclaw/src/mcp/proxy.rs`

- [ ] **Step 1: Write tests for AuthMethod behavior**

Add to the existing test module in `crates/rightclaw/src/mcp/proxy.rs`:

```rust
#[test]
fn auth_method_default_is_bearer() {
    assert_eq!(AuthMethod::default(), AuthMethod::Bearer);
}

#[test]
fn auth_method_display() {
    assert_eq!(AuthMethod::Bearer.to_string(), "bearer");
    assert_eq!(AuthMethod::Header("X-Api-Key".into()).to_string(), "header");
    assert_eq!(AuthMethod::QueryString.to_string(), "query_string");
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `devenv shell -- cargo test -p rightclaw auth_method_default auth_method_display`
Expected: compilation error — `AuthMethod` not defined.

- [ ] **Step 3: Add `AuthMethod` enum**

Add before `DynamicAuthClient` (before line 79) in `crates/rightclaw/src/mcp/proxy.rs`:

```rust
/// How a proxy backend authenticates with the upstream MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    /// `Authorization: Bearer <token>` header (default for OAuth and static bearer keys).
    Bearer,
    /// Custom header, e.g. `X-Api-Key: <token>`.
    Header(String),
    /// Key is embedded in the URL query string. No header injection needed.
    QueryString,
}

impl Default for AuthMethod {
    fn default() -> Self {
        Self::Bearer
    }
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer => f.write_str("bearer"),
            Self::Header(_) => f.write_str("header"),
            Self::QueryString => f.write_str("query_string"),
        }
    }
}
```

- [ ] **Step 4: Update `DynamicAuthClient` to support `AuthMethod`**

Change `DynamicAuthClient` struct (line 86-89) to:

```rust
pub(crate) struct DynamicAuthClient {
    inner: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
    auth_method: AuthMethod,
}
```

Update the constructor (line 91-97):

```rust
impl DynamicAuthClient {
    pub(crate) fn new(
        client: reqwest::Client,
        token: Arc<RwLock<Option<String>>>,
        auth_method: AuthMethod,
    ) -> Self {
        Self {
            inner: client,
            token,
            auth_method,
        }
    }

    async fn current_auth(&self) -> Option<String> {
        match self.auth_method {
            AuthMethod::QueryString => None, // no header injection — key is in URL
            AuthMethod::Bearer => {
                self.token.read().await.as_ref().map(|t| format!("Bearer {t}"))
            }
            AuthMethod::Header(ref header_name) => {
                // Return raw token — caller must set the custom header
                self.token.read().await.clone()
            }
        }
    }

    /// Returns the custom header name if using Header auth method.
    fn custom_header_name(&self) -> Option<&str> {
        match &self.auth_method {
            AuthMethod::Header(name) => Some(name),
            _ => None,
        }
    }
}
```

- [ ] **Step 5: Update `StreamableHttpClient` implementation**

The current implementation passes `dynamic_auth` as the `auth_token` parameter, which the inner `reqwest::Client` interprets as a Bearer token. For custom headers, we need to use `custom_headers` instead.

Update `post_message` (and similarly `delete_session`, `get_stream`) in the `StreamableHttpClient for DynamicAuthClient` impl:

```rust
impl StreamableHttpClient for DynamicAuthClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!("DynamicAuthClient: ignoring caller-provided auth_token for post_message");
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await;
        custom_headers.extend(extra_headers);
        self.inner
            .post_message(uri, message, session_id, dynamic_auth, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!("DynamicAuthClient: ignoring caller-provided auth_token for delete_session");
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await;
        custom_headers.extend(extra_headers);
        self.inner
            .delete_session(uri, session_id, dynamic_auth, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!("DynamicAuthClient: ignoring caller-provided auth_token for get_stream");
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await;
        custom_headers.extend(extra_headers);
        self.inner
            .get_stream(uri, session_id, last_event_id, dynamic_auth, custom_headers)
            .await
    }
}
```

Add `build_auth` helper to `DynamicAuthClient`:

```rust
/// Build auth token and custom headers based on auth method.
/// Returns (auth_token_for_bearer, extra_custom_headers).
async fn build_auth(&self) -> (Option<String>, HashMap<HeaderName, HeaderValue>) {
    let mut extra = HashMap::new();
    match &self.auth_method {
        AuthMethod::Bearer => {
            let token = self.token.read().await.clone();
            (token, extra)
        }
        AuthMethod::Header(header_name) => {
            if let Some(token) = self.token.read().await.as_ref() {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(header_name.as_bytes()),
                    HeaderValue::from_str(token),
                ) {
                    extra.insert(name, value);
                }
            }
            (None, extra)
        }
        AuthMethod::QueryString => (None, extra),
    }
}
```

- [ ] **Step 6: Update `ProxyBackend` to store `AuthMethod`**

Add `auth_method` field to `ProxyBackend` struct (line 166-175):

```rust
pub struct ProxyBackend {
    server_name: String,
    agent_dir: PathBuf,
    url: String,
    cached_tools: RwLock<Vec<Tool>>,
    status: RwLock<BackendStatus>,
    token: Arc<RwLock<Option<String>>>,
    auth_method: AuthMethod,
    client: RwLock<Option<RunningService<RoleClient, ()>>>,
}
```

Update `new()` (line 178-193):

```rust
pub fn new(
    server_name: String,
    agent_dir: PathBuf,
    url: String,
    token: Arc<RwLock<Option<String>>>,
    auth_method: AuthMethod,
) -> Self {
    Self {
        server_name,
        agent_dir,
        url,
        cached_tools: RwLock::new(Vec::new()),
        status: RwLock::new(BackendStatus::Unreachable),
        token,
        auth_method,
        client: RwLock::new(None),
    }
}
```

Update `connect()` (line 202) to pass `auth_method`:

```rust
let dynamic = DynamicAuthClient::new(http_client, self.token.clone(), self.auth_method.clone());
```

Add accessor:

```rust
/// Auth method used by this backend.
pub fn auth_method(&self) -> &AuthMethod {
    &self.auth_method
}
```

- [ ] **Step 7: Fix existing tests**

Update the two existing tests in `proxy.rs` that call `ProxyBackend::new` to pass `AuthMethod::default()` as the new parameter.

- [ ] **Step 8: Fix all callers of `ProxyBackend::new`**

Search for `ProxyBackend::new(` in the codebase. Update each call to pass the appropriate `AuthMethod`. For now, use `AuthMethod::Bearer` (default) — the handler changes in later tasks will pass the correct value.

Files likely affected:
- `crates/rightclaw-cli/src/internal_api.rs` (line 188)
- `crates/rightclaw-cli/src/aggregator.rs` (if ProxyBackend is created there on startup)

Run: `devenv shell -- cargo check --workspace`

- [ ] **Step 9: Run all tests, verify pass**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add crates/rightclaw/src/mcp/proxy.rs crates/rightclaw-cli/src/internal_api.rs crates/rightclaw-cli/src/aggregator.rs
git commit -m "feat: add AuthMethod enum, extend DynamicAuthClient for header/query auth"
```

---

### Task 4: Migrate Refresh Scheduler from `oauth-state.json` to SQLite

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs`
- Modify: `crates/bot/src/lib.rs` (scheduler startup)
- Modify: `crates/rightclaw-cli/src/internal_api.rs` (handle_set_token)

- [ ] **Step 1: Write test for SQLite-based refresh state loading**

Add to `crates/rightclaw/src/mcp/refresh.rs` test module:

```rust
#[test]
fn load_oauth_entries_from_db() {
    let mut conn = Connection::open_in_memory().unwrap();
    crate::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();

    // Insert an OAuth server
    conn.execute(
        "INSERT INTO mcp_servers (name, url, auth_type, auth_token, refresh_token, token_endpoint, client_id, expires_at) \
         VALUES ('notion', 'https://mcp.notion.com/mcp', 'oauth', 'tok', 'rt', 'https://ex.com/token', 'cid', '2026-04-13T12:00:00Z')",
        [],
    )
    .unwrap();

    let entries = load_oauth_entries_from_db(&conn).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "notion");
    assert_eq!(entries[0].1.client_id, "cid");
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `devenv shell -- cargo test -p rightclaw load_oauth_entries_from_db`
Expected: compilation error — function not defined.

- [ ] **Step 3: Add `load_oauth_entries_from_db` function**

In `crates/rightclaw/src/mcp/refresh.rs`, add:

```rust
use rusqlite::Connection;

/// Load OAuth server entries from SQLite for refresh scheduling.
/// Returns (server_name, OAuthServerState) pairs.
pub fn load_oauth_entries_from_db(
    conn: &Connection,
) -> miette::Result<Vec<(String, OAuthServerState)>> {
    let servers = crate::mcp::credentials::db_list_oauth_servers(conn)
        .map_err(|e| miette::miette!("failed to list OAuth servers: {e:#}"))?;

    let mut entries = Vec::new();
    for s in servers {
        let Some(ref token_endpoint) = s.token_endpoint else { continue };
        let Some(ref client_id) = s.client_id else { continue };
        let Some(ref expires_at_str) = s.expires_at else { continue };

        let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        entries.push((
            s.name.clone(),
            OAuthServerState {
                refresh_token: s.refresh_token.clone(),
                token_endpoint: token_endpoint.clone(),
                client_id: client_id.clone(),
                client_secret: s.client_secret.clone(),
                expires_at,
                server_url: s.url.clone(),
            },
        ));
    }
    Ok(entries)
}
```

- [ ] **Step 4: Update `run_refresh_scheduler` to use SQLite**

Change the function signature to take `agent_dir: PathBuf` instead of `oauth_state_path: PathBuf`:

```rust
pub async fn run_refresh_scheduler(
    agent_dir: std::path::PathBuf,
    mut rx: tokio::sync::mpsc::Receiver<RefreshMessage>,
    notify_tx: tokio::sync::mpsc::Sender<String>,
) {
```

Replace the state loading at the top (lines 100-109) with:

```rust
    let http_client = reqwest::Client::new();

    // Load existing OAuth state from SQLite
    let initial_entries = match crate::memory::open_connection(&agent_dir) {
        Ok(conn) => match load_oauth_entries_from_db(&conn) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::error!("failed to load OAuth entries from DB: {e:#}");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::error!("failed to open DB for refresh scheduler: {e:#}");
            Vec::new()
        }
    };
```

Replace the timer-building loop to use `initial_entries` instead of `state.servers`.

Replace all `save_oauth_state` calls with SQLite writes. In the `NewEntry` handler:

```rust
RefreshMessage::NewEntry { server_name, state: entry_state, token } => {
    let due = refresh_due_in(&entry_state);
    timers.insert(server_name.clone(), tokio::time::Instant::now() + due);

    // Persist to SQLite
    if let Ok(conn) = crate::memory::open_connection(&agent_dir) {
        let expires_at = entry_state.expires_at.to_rfc3339();
        if let Err(e) = crate::mcp::credentials::db_set_oauth_state(
            &conn,
            &server_name,
            token.read().await.as_deref().unwrap_or(""),
            entry_state.refresh_token.as_deref(),
            &entry_state.token_endpoint,
            &entry_state.client_id,
            entry_state.client_secret.as_deref(),
            &expires_at,
        ) {
            tracing::error!("failed to persist OAuth state: {e:#}");
        }
    }

    entries.insert(server_name.clone(), entry_state);
    token_handles.insert(server_name.clone(), token);
    tracing::info!(server = %server_name, due_secs = due.as_secs(), "new refresh scheduled");
}
```

Similarly for `RemoveServer` — clear auth columns by setting `auth_type = NULL`:

```rust
RefreshMessage::RemoveServer { server_name } => {
    timers.remove(&server_name);
    entries.remove(&server_name);
    token_handles.remove(&server_name);
    // Clear OAuth state in SQLite (keep server registered)
    if let Ok(conn) = crate::memory::open_connection(&agent_dir) {
        let _ = conn.execute(
            "UPDATE mcp_servers SET auth_type = NULL, auth_token = NULL, refresh_token = NULL, \
             token_endpoint = NULL, client_id = NULL, client_secret = NULL, expires_at = NULL \
             WHERE name = ?1",
            [&server_name],
        );
    }
    tracing::info!(server = %server_name, "refresh cancelled — server removed");
}
```

On refresh success, update SQLite:

```rust
Ok((new_entry, access_token)) => {
    if let Some(token_arc) = token_handles.get(&name) {
        *token_arc.write().await = Some(access_token.clone());
        tracing::info!(server = %name, "token refreshed in-memory");
    }

    let due = refresh_due_in(&new_entry);
    timers.insert(name.clone(), tokio::time::Instant::now() + due);

    // Persist refreshed token to SQLite
    if let Ok(conn) = crate::memory::open_connection(&agent_dir) {
        let expires_at = new_entry.expires_at.to_rfc3339();
        if let Err(e) = crate::mcp::credentials::db_update_oauth_token(
            &conn, &name, &access_token, &expires_at,
        ) {
            tracing::error!("failed to persist refreshed token: {e:#}");
        }
    }

    entries.insert(name.clone(), new_entry);
}
```

Remove `load_oauth_state`, `save_oauth_state`, and `OAuthState` (the HashMap wrapper). Keep `OAuthServerState` — it's used by `RefreshMessage` and `do_refresh`.

- [ ] **Step 5: Update `handle_set_token` in internal_api.rs**

In `crates/rightclaw-cli/src/internal_api.rs`, replace the oauth-state.json section (lines 311-337) with SQLite writes:

```rust
    // Persist OAuth state to SQLite
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(req.expires_in as i64);
    let expires_at_str = expires_at.to_rfc3339();
    {
        let conn_arc = {
            let Some(registry) = dispatcher.agents.get(&req.agent) else {
                return not_found("agent_not_found").into_response();
            };
            match registry.right.get_conn(&req.agent) {
                Ok(c) => c,
                Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
            }
        };
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        if let Err(e) = credentials::db_set_oauth_state(
            &conn,
            &req.server,
            &req.access_token,
            Some(&req.refresh_token),
            &req.token_endpoint,
            &req.client_id,
            req.client_secret.as_deref(),
            &expires_at_str,
        ) {
            return internal_error(format!("db_set_oauth_state: {e:#}")).into_response();
        }
    }
```

- [ ] **Step 6: Update bot/src/lib.rs scheduler startup**

In `crates/bot/src/lib.rs`, change the scheduler spawn (lines 232-237) from:

```rust
let oauth_state_path = agent_dir.join("oauth-state.json");
tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
    oauth_state_path,
    refresh_rx,
    notify_refresh_tx,
));
```

To:

```rust
tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
    agent_dir.clone(),
    refresh_rx,
    notify_refresh_tx,
));
```

- [ ] **Step 7: Build and test**

Run: `devenv shell -- cargo check --workspace && cargo test --workspace`
Expected: PASS (existing refresh tests may need updating since `load_oauth_state`/`save_oauth_state` are removed — update or remove those tests).

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs crates/rightclaw-cli/src/internal_api.rs crates/bot/src/lib.rs
git commit -m "feat: migrate refresh scheduler from oauth-state.json to SQLite"
```

---

### Task 5: `is_public_url` Helper

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs`

- [ ] **Step 1: Write tests**

Add to credentials.rs test module:

```rust
#[test]
fn is_public_url_rejects_private() {
    assert!(!is_public_url("https://localhost/mcp"));
    assert!(!is_public_url("https://192.168.1.1/mcp"));
    assert!(!is_public_url("https://10.0.0.1/mcp"));
    assert!(!is_public_url("https://172.16.0.1/mcp"));
    assert!(!is_public_url("https://[::1]/mcp"));
}

#[test]
fn is_public_url_accepts_public() {
    assert!(is_public_url("https://mcp.notion.com/mcp"));
    assert!(is_public_url("https://api.example.com/mcp"));
    assert!(is_public_url("https://8.8.8.8/mcp"));
}

#[test]
fn is_public_url_rejects_non_https() {
    assert!(!is_public_url("http://example.com/mcp"));
    assert!(!is_public_url("not-a-url"));
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `devenv shell -- cargo test -p rightclaw is_public_url`
Expected: compilation error.

- [ ] **Step 3: Implement `is_public_url`**

Add to `crates/rightclaw/src/mcp/credentials.rs`:

```rust
/// Check if a URL points to a public domain (not localhost, not private IP).
/// Uses the same checks as `validate_server_url` but returns bool instead of error.
pub fn is_public_url(url_str: &str) -> bool {
    validate_server_url(url_str).is_ok()
}
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `devenv shell -- cargo test -p rightclaw is_public_url`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/mcp/credentials.rs
git commit -m "feat: add is_public_url helper for auth type detection"
```

---

### Task 6: URL Redaction in Agent-Facing `mcp_list`

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs`

- [ ] **Step 1: Update `do_mcp_list` to redact URLs**

In `crates/rightclaw-cli/src/aggregator.rs`, update the format line in `do_mcp_list` (line 145-148):

```rust
lines.push(format!(
    "- {name}: {status} ({tool_count} tools) url={url}",
    url = rightclaw::mcp::credentials::redact_url(handle.url())
));
```

- [ ] **Step 2: Build and verify**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "feat: redact query strings in agent-facing mcp_list"
```

---

### Task 7: User-Facing `/mcp list` Shows Auth Type

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs` (response type + handler)
- Modify: `crates/rightclaw/src/mcp/internal_client.rs` (response type)
- Modify: `crates/bot/src/telegram/handler.rs` (display)

- [ ] **Step 1: Add `auth_type` to `McpServerStatus`**

In `crates/rightclaw-cli/src/internal_api.rs`, update `McpServerStatus` (line 76-83):

```rust
#[derive(Debug, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    pub auth_type: Option<String>,
}
```

- [ ] **Step 2: Update `handle_mcp_list` to include auth_type**

In `handle_mcp_list` (line 354-386), for each proxy, read auth_type from DB and include it:

When building the "right" entry, set `auth_type: None`.
When building proxy entries, look up `auth_type` from the SQLite row. Since `handle_mcp_list` doesn't currently read from DB, the simplest approach is to read from `ProxyBackend`'s stored auth_method:

```rust
let auth_type = Some(handle.auth_method().to_string());
```

This requires `ProxyBackend::auth_method()` to be `pub` (already added in Task 3).

- [ ] **Step 3: Update `McpServerStatus` in internal_client.rs**

In `crates/rightclaw/src/mcp/internal_client.rs`, update `McpServerStatus` (line 186-192):

```rust
#[derive(Debug, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    pub auth_type: Option<String>,
}
```

- [ ] **Step 4: Update Telegram display**

In `crates/bot/src/telegram/handler.rs`, update `handle_mcp_list` (line 506-510):

```rust
let mut text = String::from("MCP Servers:\n\n");
for s in &result.servers {
    let url_part = s.url.as_deref().map(|u| format!(" [{u}]")).unwrap_or_default();
    let auth_part = s.auth_type.as_deref().map(|a| format!(" [{a}]")).unwrap_or_default();
    text.push_str(&format!("  {} -- {} ({} tools){}{}\n", s.name, s.status, s.tool_count, auth_part, url_part));
}
```

- [ ] **Step 5: Build and verify**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs crates/rightclaw/src/mcp/internal_client.rs crates/bot/src/telegram/handler.rs
git commit -m "feat: show auth_type in user-facing /mcp list"
```

---

### Task 8: Extend Internal API `/mcp-add` with Auth Fields

**Files:**
- Modify: `crates/rightclaw-cli/src/internal_api.rs`
- Modify: `crates/rightclaw/src/mcp/internal_client.rs`

- [ ] **Step 1: Extend `McpAddRequest`**

In `crates/rightclaw-cli/src/internal_api.rs`, update `McpAddRequest` (line 20-24):

```rust
#[derive(Debug, Deserialize)]
pub struct McpAddRequest {
    pub agent: String,
    pub name: String,
    pub url: String,
    pub auth_type: Option<String>,
    pub auth_header: Option<String>,
    pub auth_token: Option<String>,
}
```

- [ ] **Step 2: Update `handle_mcp_add` to persist auth and verify connection**

Replace the handler (lines 150-221). The new flow:
1. Validate name & URL
2. Determine `AuthMethod` from request fields
3. Add server to SQLite
4. Set auth fields in SQLite
5. Create `ProxyBackend` with auth method and token
6. **Attempt connection** — if fails, remove from SQLite and return error
7. On success, register in proxies map

```rust
async fn handle_mcp_add(
    State(dispatcher): State<Arc<ToolDispatcher>>,
    Json(req): Json<McpAddRequest>,
) -> axum::response::Response {
    if let Err(e) = credentials::validate_server_name(&req.name) {
        return validation_error(format!("{e}")).into_response();
    }
    if let Err(e) = credentials::validate_server_url(&req.url) {
        return validation_error(format!("{e}")).into_response();
    }

    let auth_method = match req.auth_type.as_deref() {
        Some("header") => {
            let header_name = req.auth_header.clone().unwrap_or_else(|| "Authorization".into());
            AuthMethod::Header(header_name)
        }
        Some("query_string") => AuthMethod::QueryString,
        _ => AuthMethod::Bearer, // "bearer", "oauth", or None → Bearer
    };

    let (conn_arc, agent_dir) = {
        let Some(registry) = dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        let conn = match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        };
        (conn, registry.agent_dir.clone())
    };

    // Add to SQLite
    {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        if let Err(e) = credentials::db_add_server(&conn, &req.name, &req.url) {
            return internal_error(format!("db_add_server: {e:#}")).into_response();
        }
        if let Some(ref auth_type) = req.auth_type {
            if let Err(e) = credentials::db_set_auth(
                &conn,
                &req.name,
                auth_type,
                req.auth_header.as_deref(),
                req.auth_token.as_deref(),
            ) {
                return internal_error(format!("db_set_auth: {e:#}")).into_response();
            }
        }
    }

    // Create backend and attempt connection
    let token = Arc::new(tokio::sync::RwLock::new(req.auth_token.clone()));
    let backend = ProxyBackend::new(
        req.name.clone(),
        agent_dir.clone(),
        req.url.clone(),
        token,
        auth_method,
    );
    let handle = Arc::new(backend);

    let http_client = reqwest::Client::new();
    match handle.connect(http_client).await {
        Ok(instructions) => {
            let tools_count = handle.try_tools().map(|t| t.len()).unwrap_or(0);

            // Register in proxies map
            let proxies_lock = {
                let Some(registry) = dispatcher.agents.get(&req.agent) else {
                    return not_found("agent_not_found").into_response();
                };
                Arc::clone(&registry.proxies)
            };
            proxies_lock.write().await.insert(req.name.clone(), Arc::clone(&handle));

            (
                StatusCode::OK,
                Json(McpAddResponse {
                    tools_count,
                    excluded: Vec::new(),
                    warning: None,
                }),
            )
                .into_response()
        }
        Err(e) => {
            // Connection failed — clean up SQLite entry
            {
                let conn = conn_arc.lock().unwrap();
                let _ = credentials::db_remove_server(&conn, &req.name);
            }
            validation_error(format!("Connection failed: {e:#}")).into_response()
        }
    }
}
```

- [ ] **Step 3: Update `InternalClient::mcp_add` to pass auth fields**

In `crates/rightclaw/src/mcp/internal_client.rs`, change `mcp_add` (lines 107-120):

```rust
pub async fn mcp_add(
    &self,
    agent: &str,
    name: &str,
    url: &str,
    auth_type: Option<&str>,
    auth_header: Option<&str>,
    auth_token: Option<&str>,
) -> Result<McpAddResponse, InternalClientError> {
    self.post(
        "/mcp-add",
        &serde_json::json!({
            "agent": agent,
            "name": name,
            "url": url,
            "auth_type": auth_type,
            "auth_header": auth_header,
            "auth_token": auth_token,
        }),
    )
    .await
}
```

- [ ] **Step 4: Fix existing callers of `mcp_add`**

The old `mcp_add(agent, name, url)` now takes 6 params. Find all callers — currently only `handle_mcp_add` in `handler.rs` (line 720). Update to pass `None, None, None` for now (will be replaced in Task 10):

```rust
match internal.mcp_add(agent_name, name, url, None, None, None).await {
```

- [ ] **Step 5: Build and test**

Run: `devenv shell -- cargo check --workspace && cargo test --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/internal_api.rs crates/rightclaw/src/mcp/internal_client.rs crates/bot/src/telegram/handler.rs
git commit -m "feat: extend /mcp-add API with auth fields, verify connection before persisting"
```

---

### Task 9: Aggregator Startup — Load Auth from SQLite

**Files:**
- Modify: `crates/rightclaw-cli/src/aggregator.rs` (or wherever ProxyBackends are restored on startup)

- [ ] **Step 1: Find where ProxyBackends are restored on startup**

Search for where `db_list_servers` is called during aggregator initialization and ProxyBackends are created. When the aggregator starts, it needs to load existing servers from SQLite and create ProxyBackends with the correct `AuthMethod` and token.

- [ ] **Step 2: Update startup to load auth method and token**

When creating `ProxyBackend` from a stored `McpServerEntry`, map the auth fields:

```rust
let auth_method = match entry.auth_type.as_deref() {
    Some("header") => AuthMethod::Header(entry.auth_header.clone().unwrap_or_default()),
    Some("query_string") => AuthMethod::QueryString,
    _ => AuthMethod::Bearer,
};
let token = Arc::new(tokio::sync::RwLock::new(entry.auth_token.clone()));
let backend = ProxyBackend::new(
    entry.name.clone(),
    agent_dir.clone(),
    entry.url.clone(),
    token,
    auth_method,
);
```

- [ ] **Step 3: Build and verify**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/aggregator.rs
git commit -m "feat: load auth method and token from SQLite on aggregator startup"
```

---

### Task 10: Rewrite `handle_mcp_add` in Bot Handler

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

This is the biggest change. The new flow:
1. Parse name & URL
2. Try OAuth discovery on bare URL
3. If OAuth → existing flow (call `/mcp add`, then `/mcp auth`)
4. If not OAuth → check public domain
   - Public → dispatch haiku in sandbox for classification
   - Private → assume bearer
5. Based on classification: ask user for token or proceed with query_string
6. Call internal API with auth fields
7. Report result

- [ ] **Step 1: Add haiku auth detection types**

Add near the top of `handler.rs`:

```rust
/// Result of AI-based auth type detection.
#[derive(Debug, serde::Deserialize)]
struct AuthDetectionResult {
    auth_type: String,
    #[serde(default)]
    header_name: Option<String>,
}
```

- [ ] **Step 2: Add haiku invocation helper**

Add a helper function that runs `claude -p --bare -m haiku` in the sandbox to classify auth type. This reuses the SSH infrastructure from `worker.rs`:

```rust
/// Run haiku in sandbox to detect MCP server auth type.
/// Returns parsed AuthDetectionResult or error.
async fn detect_auth_type_via_haiku(
    bare_url: &str,
    agent_name: &str,
    ssh_config_path: Option<&Path>,
) -> Result<AuthDetectionResult, String> {
    let prompt = format!(
        "What authentication method does the MCP server at {bare_url} use? \
         Search the web for its documentation. Respond with ONLY a JSON object, no other text. \
         One of:\n\
         {{\"auth_type\": \"bearer\"}} — if it uses Authorization: Bearer header\n\
         {{\"auth_type\": \"header\", \"header_name\": \"X-Custom-Header\"}} — if it uses a custom header\n\
         {{\"auth_type\": \"query_string\"}} — if the API key goes in the URL query string\n\
         If you cannot determine, default to: {{\"auth_type\": \"bearer\"}}"
    );

    let mut claude_args = vec![
        "claude".to_string(),
        "-p".into(),
        "--bare".into(),
        "--dangerously-skip-permissions".into(),
        "-m".into(),
        "haiku".into(),
    ];

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let ssh_host = rightclaw::openshell::ssh_host(agent_name);
        let escaped_prompt = prompt.replace('\'', "'\\''");
        let script = format!("echo '{}' | {} 2>/dev/null", escaped_prompt, claude_args.join(" "));
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(script);
        c
    } else {
        let cc_bin = which::which("claude")
            .or_else(|_| which::which("claude-bun"))
            .map_err(|_| "claude binary not found".to_string())?;
        let mut c = tokio::process::Command::new(&cc_bin);
        for arg in &claude_args[1..] {
            c.arg(arg);
        }
        c
    };

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("spawn haiku failed: {e:#}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(prompt.as_bytes()).await
            .map_err(|e| format!("stdin write failed: {e:#}"))?;
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "haiku timed out after 60s".to_string())?
    .map_err(|e| format!("haiku failed: {e:#}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Extract JSON from output (haiku may include surrounding text)
    let json_start = stdout.find('{').ok_or("no JSON in haiku output")?;
    let json_end = stdout.rfind('}').ok_or("no JSON in haiku output")? + 1;
    let json_str = &stdout[json_start..json_end];

    serde_json::from_str::<AuthDetectionResult>(json_str)
        .map_err(|e| format!("failed to parse haiku response: {e:#}\nRaw: {json_str}"))
}
```

- [ ] **Step 3: Rewrite `handle_mcp_add`**

Replace `handle_mcp_add` (lines 696-739) with the new flow:

```rust
async fn handle_mcp_add(
    bot: &BotType,
    msg: &Message,
    config_str: &str,
    agent_dir: &Path,
    internal: &rightclaw::mcp::internal_client::InternalClient,
    pending_auth: PendingAuthMap,
    home: &Path,
) -> Result<(), RequestError> {
    tracing::info!(agent_dir = %agent_dir.display(), "mcp add");
    let parts: Vec<&str> = config_str.split_whitespace().collect();
    if parts.len() < 2 {
        bot.send_message(msg.chat.id, "Usage: /mcp add <name> <url>")
            .await?;
        return Ok(());
    }
    let name = parts[0];
    let original_url = parts[1];

    let agent_name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let eff_thread_id = effective_thread_id(msg);

    // Strip query string for OAuth discovery
    let bare_url = match original_url.find('?') {
        Some(pos) => &original_url[..pos],
        None => original_url,
    };

    // Step 1: Try OAuth AS discovery
    let http_client = reqwest::Client::new();
    let oauth_discovered = rightclaw::mcp::oauth::discover_as(&http_client, bare_url).await.is_ok();

    if oauth_discovered {
        // OAuth server — register and then tell user to run /mcp auth
        match internal.mcp_add(agent_name, name, bare_url, Some("oauth"), None, None).await {
            Ok(resp) => {
                let escaped = super::markdown::html_escape(name);
                let mut reply = format!("Added MCP server <b>{escaped}</b> (OAuth).");
                if let Some(ref w) = resp.warning {
                    reply.push_str(&format!("\n{}", super::markdown::html_escape(w)));
                }
                reply.push_str(&format!("\nRun <code>/mcp auth {name}</code> to authenticate."));
                send_html_reply(bot, msg.chat.id, eff_thread_id, &reply).await?;
            }
            Err(e) => {
                send_html_reply(bot, msg.chat.id, eff_thread_id, &format!("Failed: {e:#}")).await?;
            }
        }
        return Ok(());
    }

    // Step 2: Determine auth type for non-OAuth servers
    let has_query = original_url.contains('?');
    let is_public = rightclaw::mcp::credentials::is_public_url(bare_url);

    let (auth_type, auth_header, auth_token): (String, Option<String>, Option<String>) = if has_query {
        // Query string auth — key already in URL
        ("query_string".into(), None, None)
    } else if is_public {
        // Public URL — ask haiku
        bot.send_message(msg.chat.id, "Detecting authentication method...")
            .await?;

        let ssh_config_path = agent_dir.join("..").join("..").join("run")
            .join("ssh").join(format!("{agent_name}.ssh-config"));
        let ssh_config = if ssh_config_path.exists() {
            Some(ssh_config_path.as_path())
        } else {
            None
        };

        match detect_auth_type_via_haiku(bare_url, agent_name, ssh_config).await {
            Ok(result) => {
                tracing::info!(auth_type = %result.auth_type, header = ?result.header_name, "haiku detected auth type");
                (result.auth_type, result.header_name, None)
            }
            Err(e) => {
                tracing::warn!("haiku auth detection failed: {e}, falling back to bearer");
                ("bearer".into(), None, None)
            }
        }
    } else {
        // Private/local URL — assume bearer
        ("bearer".into(), None, None)
    };

    // Step 3: If bearer or header — ask user for token
    let final_token = if auth_type == "bearer" || auth_type == "header" {
        let header_desc = auth_header.as_deref().unwrap_or("Bearer token");
        bot.send_message(
            msg.chat.id,
            format!("Send the {header_desc} for {name}:"),
        )
        .await?;

        // Wait for next message as token (with timeout)
        // Use a oneshot channel to receive the token from the message handler
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        {
            let mut pending = pending_auth.lock().await;
            pending.insert(
                (msg.chat.id.0, eff_thread_id),
                PendingTokenRequest { sender: tx },
            );
        }

        match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(token)) => Some(token),
            _ => {
                bot.send_message(msg.chat.id, "Timed out waiting for token. /mcp add cancelled.")
                    .await?;
                return Ok(());
            }
        }
    } else {
        None // query_string — no token needed
    };

    // Step 4: Call internal API with auth fields (connection verified server-side)
    let url_to_register = if auth_type == "query_string" {
        original_url // keep query string
    } else {
        bare_url
    };

    match internal
        .mcp_add(
            agent_name,
            name,
            url_to_register,
            Some(&auth_type),
            auth_header.as_deref(),
            final_token.as_deref(),
        )
        .await
    {
        Ok(resp) => {
            let escaped = super::markdown::html_escape(name);
            let mut reply = format!("Added MCP server <b>{escaped}</b>.");
            if resp.tools_count > 0 {
                reply.push_str(&format!(" {} tools available.", resp.tools_count));
            }
            if let Some(ref w) = resp.warning {
                reply.push_str(&format!("\n{}", super::markdown::html_escape(w)));
            }
            send_html_reply(bot, msg.chat.id, eff_thread_id, &reply).await?;
        }
        Err(e) => {
            send_html_reply(bot, msg.chat.id, eff_thread_id, &format!("Failed: {e:#}")).await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Add `PendingTokenRequest` type and intercept logic**

Add the pending token request type:

```rust
pub struct PendingTokenRequest {
    pub sender: tokio::sync::oneshot::Sender<String>,
}

/// Map of (chat_id, thread_id) → pending token request.
pub type PendingTokenMap = Arc<tokio::sync::Mutex<HashMap<(i64, i64), PendingTokenRequest>>>;
```

In the main message handler (where text messages are routed), add an early check: if there's a pending token request for this (chat_id, thread_id), consume it and send the message text as the token:

```rust
// Check for pending token requests (from /mcp add flow)
{
    let mut pending = pending_tokens.lock().await;
    if let Some(req) = pending.remove(&(chat_id, eff_thread_id)) {
        let _ = req.sender.send(text.to_owned());
        return Ok(()); // consumed — don't route to worker
    }
}
```

- [ ] **Step 5: Update `handle_mcp` dispatcher**

Update the call to `handle_mcp_add` in the `handle_mcp` dispatcher (line 455-457) to pass the additional `pending_auth` and `home` parameters:

```rust
Some("add") => {
    let rest = parts[1..].join(" ");
    handle_mcp_add(&bot, &msg, &rest, &agent_dir.0, &internal.0, pending_tokens.clone(), &home.0).await
}
```

Wire `pending_tokens: PendingTokenMap` through the dispatch chain similar to how `pending_auth: PendingAuthMap` is wired.

- [ ] **Step 6: Build and verify**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles (may need to fix imports, adjust dispatch.rs DI).

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs
git commit -m "feat: rewrite /mcp add with OAuth discovery, haiku fallback, token prompt"
```

---

### Task 11: Update System Prompt

**Files:**
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md`

- [ ] **Step 1: Update MCP Management section**

Replace the MCP Management section (lines 31-60) with:

```markdown
## MCP Management

You CANNOT add, remove, or authenticate MCP servers yourself.
The user manages them via Telegram commands:

- `/mcp add <name> <url>` — register an external MCP server (auto-detects auth type)
- `/mcp remove <name>` — unregister a server (`right` is protected)
- `/mcp auth <name>` — start OAuth flow (for servers requiring OAuth authentication)
- `/mcp list` — show all servers with status and auth type

Authentication for all MCP servers is managed transparently by the RightClaw aggregator.
You do not need to handle credentials or authentication flows — requests are authenticated
automatically. If a server reports `needs_auth`, tell the user to run `/mcp auth <server>`
in Telegram.

**When the user asks to connect an MCP server:**

1. **Find the MCP endpoint.** Search for the service's Claude Code, Codex,
   or Claude Desktop integration docs — these typically describe an MCP endpoint
   (streamable HTTP or SSE). Search queries like
   `"<service> MCP Claude Code"` or `"<service> MCP server"` work best.

2. **Tell the user to run:** `/mcp add <name> <url>`
   The system auto-detects the authentication method (OAuth, Bearer token,
   custom header, or API key in URL) and handles the setup flow.

3. **NEVER ask the user for API keys or tokens directly** — the `/mcp add` flow
   handles credential collection when needed.

To check registered servers from code, use the `mcp_list()` tool.
```

- [ ] **Step 2: Verify the test still passes**

The test in `agent_def_tests.rs` checks for `"## MCP Management"` — this heading is preserved so the test should still pass.

Run: `devenv shell -- cargo test -p rightclaw -- agent_def`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs: update system prompt for transparent MCP auth"
```

---

### Task 12: One-Time Migration from `oauth-state.json`

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add migration function**

Add a function in `crates/bot/src/lib.rs` (or a suitable module) that runs once on startup:

```rust
/// Migrate OAuth state from oauth-state.json to SQLite (one-time).
fn migrate_oauth_state_to_db(agent_dir: &Path) -> miette::Result<()> {
    let json_path = agent_dir.join("oauth-state.json");
    if !json_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&json_path)
        .map_err(|e| miette::miette!("failed to read oauth-state.json: {e:#}"))?;
    let state: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| miette::miette!("failed to parse oauth-state.json: {e:#}"))?;

    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| miette::miette!("failed to open DB: {e:#}"))?;

    if let Some(servers) = state.get("servers").and_then(|s| s.as_object()) {
        for (name, entry) in servers {
            let token_endpoint = entry.get("token_endpoint").and_then(|v| v.as_str()).unwrap_or("");
            let client_id = entry.get("client_id").and_then(|v| v.as_str()).unwrap_or("");
            let client_secret = entry.get("client_secret").and_then(|v| v.as_str());
            let refresh_token = entry.get("refresh_token").and_then(|v| v.as_str());
            let expires_at = entry.get("expires_at").and_then(|v| v.as_str()).unwrap_or("");

            // Only migrate if the server exists in mcp_servers table
            if let Err(e) = rightclaw::mcp::credentials::db_set_oauth_state(
                &conn,
                name,
                "", // access token not stored in json (was in-memory only)
                refresh_token,
                token_endpoint,
                client_id,
                client_secret,
                expires_at,
            ) {
                tracing::warn!(server = %name, "skipping oauth-state migration: {e:#}");
            }
        }
    }

    // Remove the json file
    if let Err(e) = std::fs::remove_file(&json_path) {
        tracing::warn!("failed to remove oauth-state.json: {e:#}");
    } else {
        tracing::info!("migrated oauth-state.json to SQLite and removed file");
    }

    Ok(())
}
```

- [ ] **Step 2: Call it early in bot startup**

In `crates/bot/src/lib.rs`, call `migrate_oauth_state_to_db(&agent_dir)` before the refresh scheduler starts. Non-fatal — log and continue on error.

- [ ] **Step 3: Build and verify**

Run: `devenv shell -- cargo check --workspace`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat: one-time migration from oauth-state.json to SQLite"
```

---

### Task 13: Final Build and Review

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: compiles without errors.

- [ ] **Step 2: Full test suite**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 3: Clippy**

Run: `devenv shell -- cargo clippy --workspace`
Expected: no warnings/errors.

- [ ] **Step 4: Review with rust code reviewer**

Dispatch `review-rust-code` subagent to review all changed files.

- [ ] **Step 5: Update ARCHITECTURE.md**

Add note about auth column additions to `mcp_servers` table in the Memory Schema section. Mention that `oauth-state.json` is deprecated and migrated to SQLite.

- [ ] **Step 6: Final commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md for MCP auth in SQLite"
```
