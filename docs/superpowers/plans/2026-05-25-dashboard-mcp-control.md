# Dashboard MCP Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move MCP server management into the Telegram Mini App dashboard while `/mcp` opens that dashboard view and MCP domain behavior stays outside `right-dashboard`.

**Architecture:** `right-mcp` owns OAuth discovery, auth models, proxy injection, and credential helpers. `right` owns the Aggregator internal API and live proxy backend state. `right-bot` owns Mini-App-authenticated MCP dashboard routes and the simplified `/mcp` command. The Vue dashboard adds an MCP tab but does not move MCP domain logic into the `right-dashboard` Rust crate.

**Tech Stack:** Rust 2024, axum, tokio, SQLite/Turso migrations through `right-db`, rmcp proxy transport, Vue 3, TypeScript, Vite, Vitest.

---

## Execution Notes

- Execute from the repo root: `/Users/molt/dev/rightclaw`.
- Because `devenv.nix` exists, prefix commands with `devenv shell --`.
- Before implementation, use `superpowers:using-git-worktrees` in the execution session and create a worktree under `.worktrees/`.
- There is a pre-existing unstaged change in `crates/right-db/src/connection.rs` in the original workspace. Do not revert or include it unless the execution worktree also contains it and the task requires it.
- Follow TDD: write the narrow regression test first, run it to see it fail, then implement the minimal code.
- Do not run full workspace tests after every small edit. Run targeted tests per slice and one final full workspace test.
- Final verification must include `devenv shell -- cargo test --workspace`.

## File Map

- Modify `crates/right-mcp/src/oauth.rs`: make speculative OAuth metadata probes tolerant of non-JSON/non-metadata bodies.
- Create `crates/right-mcp/src/detect.rs`: URL-first MCP auth recommendation model and tests.
- Modify `crates/right-mcp/src/lib.rs`: export `detect`.
- Modify `crates/right-db/src/sql/v35_mcp_http_headers.sql`: new side table for multi-header secrets.
- Modify `crates/right-db/src/migrations.rs`: register v35 and test the table.
- Modify `crates/right-mcp/src/credentials.rs`: header-secret persistence helpers and redacted header-name listing.
- Modify `crates/right-mcp/src/proxy.rs`: add multi-header auth injection with redacted debug behavior.
- Modify `crates/right-mcp/src/internal_client.rs`: internal DTOs/methods for multi-header registration, header updates, and redacted list output.
- Modify `crates/right/src/internal_api.rs`: Aggregator internal API support for multi-header auth and list redaction.
- Modify `crates/right/src/main.rs`: restore multi-header backends at Aggregator startup.
- Modify `crates/bot/src/telegram/dashboard.rs`: add Mini-App-authenticated MCP routes and state wiring.
- Create `crates/bot/src/telegram/dashboard/mcp.rs`: dashboard MCP route handlers.
- Modify `crates/bot/src/lib.rs`: pass the internal client and pending OAuth map into dashboard state.
- Modify `crates/bot/src/telegram/handler.rs`: simplify `/mcp` to open the dashboard MCP view and remove command subcommand management.
- Modify `crates/right-dashboard/frontend/src/types.ts`: MCP dashboard API types.
- Modify `crates/right-dashboard/frontend/src/api.ts`: MCP API calls.
- Modify `crates/right-dashboard/frontend/src/App.vue`: deep-link support and MCP tab wiring.
- Create `crates/right-dashboard/frontend/src/views/McpView.vue`: server list and URL-first wizard.
- Create `crates/right-dashboard/frontend/src/components/SecretInput.vue`: masked input with eye toggle.
- Modify `ARCHITECTURE.md`, `docs/architecture/mcp.md`, `docs/architecture/lifecycle.md`, and prompt/user-facing references that mention `/mcp add|auth|remove|list`.
- Rebuild dashboard static assets under `crates/right-dashboard/static/dashboard/`.

---

### Task 1: Baseline Verification

**Files:**
- Read: `docs/superpowers/specs/2026-05-25-dashboard-mcp-control-design.md`
- Read: `docs/architecture/mcp.md`
- Read: `docs/architecture/lifecycle.md`
- Read: `ARCHITECTURE.md`

- [ ] **Step 1: Confirm the design and architecture docs are present**

Run:

```bash
devenv shell -- sed -n '1,260p' docs/superpowers/specs/2026-05-25-dashboard-mcp-control-design.md
devenv shell -- sed -n '1,230p' docs/architecture/mcp.md
devenv shell -- sed -n '60,150p' ARCHITECTURE.md
```

Expected: files render successfully. Record any drift you notice for Task 12.

- [ ] **Step 2: Run targeted baseline tests**

Run:

```bash
devenv shell -- cargo test -p right-mcp oauth::tests::discover_as_linear_pattern_uses_origin_well_known
devenv shell -- cargo test -p right-mcp credentials::db_tests::db_add_server_with_auth
devenv shell -- cargo test -p right-mcp proxy::tests::dynamic_auth_header_injects_custom_header
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_list_returns_right_backend
devenv shell -- cargo test -p rightclaw-bot --lib telegram::dashboard::tests::dashboard_bootstrap_authorized
```

Expected: all pass. If any fail before changes, record the exact failure in the implementation notes and continue with the narrowest affected tests.

---

### Task 2: Make OAuth Discovery Treat Non-Metadata Well-Known Responses As Misses

**Files:**
- Modify: `crates/right-mcp/src/oauth.rs`

- [ ] **Step 1: Add the failing regression test**

In `crates/right-mcp/src/oauth.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
#[tokio::test]
async fn discover_as_skips_html_protected_resource_response() {
    setup_crypto();
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "message": "Authentication failed. The request is missing the Authorization header.",
                "code": "missing_auth_header"
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string("<!doctype html><html><title>Nango</title></html>"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server/mcp"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration/mcp"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/mcp/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let server_url = format!("{}/mcp", server.uri());
    let result = discover_as(&client, &server_url).await;

    let err = result.expect_err("non-metadata HTML should not discover OAuth");
    let detail = format!("{err:#}");
    assert!(
        detail.contains("no AS metadata found"),
        "HTML metadata miss must fall through to normal discovery failure, got: {detail}"
    );
    assert!(
        !detail.contains("failed to parse RFC 9728 response"),
        "HTML metadata miss must not be reported as parse failure: {detail}"
    );
}
```

- [ ] **Step 2: Run the regression test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-mcp oauth::tests::discover_as_skips_html_protected_resource_response -- --nocapture
```

Expected: FAIL with a message containing `failed to parse RFC 9728 response`.

- [ ] **Step 3: Add tolerant JSON parsing helpers**

In `crates/right-mcp/src/oauth.rs`, near `ResourceMetadata`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum MetadataProbe<T> {
    Found(T),
    NotMetadata(String),
}

fn response_content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
}

async fn parse_metadata_response<T>(
    resp: reqwest::Response,
    url: &str,
    label: &str,
) -> Result<MetadataProbe<T>, OAuthError>
where
    T: serde::de::DeserializeOwned,
{
    let content_type = response_content_type(&resp);
    if !content_type.is_empty()
        && !content_type.contains("application/json")
        && !content_type.ends_with("+json")
    {
        debug!(
            "discover_as: {label} response from {url} had non-json content-type {content_type}, treating as no metadata"
        );
        return Ok(MetadataProbe::NotMetadata(format!(
            "non-json content-type {content_type} at {url}"
        )));
    }

    let body = resp.bytes().await.map_err(|e| {
        OAuthError::DiscoveryFailed(format!("failed to read {label} response from {url}: {e}"))
    })?;
    match serde_json::from_slice::<T>(&body) {
        Ok(meta) => Ok(MetadataProbe::Found(meta)),
        Err(e) => {
            let preview = String::from_utf8_lossy(&body);
            let trimmed = preview.trim_start();
            if trimmed.starts_with('<') {
                debug!(
                    "discover_as: {label} response from {url} looked like HTML, treating as no metadata"
                );
                Ok(MetadataProbe::NotMetadata(format!("html body at {url}")))
            } else {
                Err(OAuthError::DiscoveryFailed(format!(
                    "failed to parse {label} response from {url}: {e}"
                )))
            }
        }
    }
}
```

- [ ] **Step 4: Use the helper for RFC 9728 and AS metadata**

Replace the RFC 9728 parse block:

```rust
let meta: ResourceMetadata = resp.json().await.map_err(|e| {
    OAuthError::DiscoveryFailed(format!(
        "failed to parse RFC 9728 response from {rfc9728_url}: {e}"
    ))
})?;
debug!(
    "discover_as: RFC 9728 succeeded, AS URL = {:?}",
    meta.authorization_servers.first()
);
if let Some(meta_resource) = meta.resource.filter(|r| !r.trim().is_empty()) {
    resource = meta_resource;
}
if let Some(scopes) = meta.scopes_supported {
    resource_scopes = scopes;
}
Some(
    meta.authorization_servers
        .into_iter()
        .next()
        .ok_or_else(|| {
            OAuthError::DiscoveryFailed(
                "RFC 9728 authorization_servers array is empty".to_string(),
            )
        })?,
)
```

with:

```rust
match parse_metadata_response::<ResourceMetadata>(
    resp,
    &rfc9728_url,
    "RFC 9728",
)
.await?
{
    MetadataProbe::Found(meta) => {
        debug!(
            "discover_as: RFC 9728 succeeded, AS URL = {:?}",
            meta.authorization_servers.first()
        );
        if let Some(meta_resource) = meta.resource.filter(|r| !r.trim().is_empty()) {
            resource = meta_resource;
        }
        if let Some(scopes) = meta.scopes_supported {
            resource_scopes = scopes;
        }
        Some(
            meta.authorization_servers
                .into_iter()
                .next()
                .ok_or_else(|| {
                    OAuthError::DiscoveryFailed(
                        "RFC 9728 authorization_servers array is empty".to_string(),
                    )
                })?,
        )
    }
    MetadataProbe::NotMetadata(reason) => {
        debug!("discover_as: RFC 9728 response was not metadata: {reason}");
        None
    }
}
```

Replace the AS metadata parse block:

```rust
let meta: AsMetadata = resp.json().await.map_err(|e| {
    OAuthError::DiscoveryFailed(format!(
        "failed to parse AS metadata from {url}: {e}"
    ))
})?;
debug!("discover_as: succeeded via {url}");
let scopes = select_oauth_scopes(&resource_scopes, &meta.scopes_supported);
return Ok(OAuthDiscovery {
    metadata: meta,
    resource,
    scopes,
});
```

with:

```rust
match parse_metadata_response::<AsMetadata>(resp, url, "AS metadata").await? {
    MetadataProbe::Found(meta) => {
        debug!("discover_as: succeeded via {url}");
        let scopes = select_oauth_scopes(&resource_scopes, &meta.scopes_supported);
        return Ok(OAuthDiscovery {
            metadata: meta,
            resource,
            scopes,
        });
    }
    MetadataProbe::NotMetadata(reason) => {
        debug!("discover_as: AS metadata response was not metadata: {reason}");
        last_err = Some(reason);
    }
}
```

- [ ] **Step 5: Run targeted OAuth tests**

Run:

```bash
devenv shell -- cargo test -p right-mcp oauth::tests::discover_as_skips_html_protected_resource_response
devenv shell -- cargo test -p right-mcp oauth::tests::discover_as_linear_pattern_uses_origin_well_known
devenv shell -- cargo test -p right-mcp oauth::tests::discover_oauth_preserves_resource_metadata
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-mcp/src/oauth.rs
devenv shell -- git commit -m "fix(oauth): treat non-metadata probes as misses"
```

---

### Task 3: Add URL-First MCP Detection Model

**Files:**
- Create: `crates/right-mcp/src/detect.rs`
- Modify: `crates/right-mcp/src/lib.rs`

- [ ] **Step 1: Add failing detection tests**

Create `crates/right-mcp/src/detect.rs` with tests first:

```rust
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detect_recommends_url_as_is_for_query_string() {
        let result = detect_mcp_auth(&reqwest::Client::new(), "https://api.example.com/mcp?key=secret")
            .await
            .expect("query URL should classify without network");

        assert_eq!(result.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(result.reason, DetectionReason::QueryStringPresent);
        assert!(!result.oauth_discovered);
    }

    #[tokio::test]
    async fn detect_recommends_url_as_is_for_loopback() {
        let result = detect_mcp_auth(&reqwest::Client::new(), "http://127.0.0.1:3333/mcp")
            .await
            .expect("loopback URL should classify without network");

        assert_eq!(result.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(result.reason, DetectionReason::LoopbackOrPrivate);
    }

    #[tokio::test]
    async fn detect_recommends_oauth_when_discovered() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/mcp/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri())
            })))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&reqwest::Client::new(), &format!("{}/mcp", server.uri()))
            .await
            .expect("detect should complete");

        assert!(result.oauth_discovered);
        assert_eq!(result.recommended_mode, McpAuthMode::OAuth);
        assert_eq!(result.reason, DetectionReason::OAuthDiscovered);
        assert!(result.oauth.is_some());
    }

    #[tokio::test]
    async fn detect_recommends_headers_when_no_oauth_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&reqwest::Client::new(), &format!("{}/mcp", server.uri()))
            .await
            .expect("detect should complete");

        assert!(!result.oauth_discovered);
        assert_eq!(result.recommended_mode, McpAuthMode::Headers);
        assert_eq!(result.reason, DetectionReason::NoOAuthMetadata);
    }
}
```

- [ ] **Step 2: Run tests and verify missing symbols fail**

Run:

```bash
devenv shell -- cargo test -p right-mcp detect::tests
```

Expected: FAIL because `detect` is not exported and the functions/types are not defined.

- [ ] **Step 3: Export the module**

In `crates/right-mcp/src/lib.rs`, add:

```rust
pub mod detect;
```

- [ ] **Step 4: Implement detection types and function**

In `crates/right-mcp/src/detect.rs`, above the tests, add:

```rust
use crate::credentials::{is_loopback_url, is_public_url};
use crate::oauth::{OAuthDiscovery, OAuthError, canonical_resource_uri, discover_oauth};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthMode {
    OAuth,
    Headers,
    UrlAsIs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionReason {
    QueryStringPresent,
    LoopbackOrPrivate,
    OAuthDiscovered,
    NoOAuthMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthDetectionSummary {
    pub resource: String,
    pub scopes: Vec<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAuthDetection {
    pub bare_url: String,
    pub oauth_discovered: bool,
    pub recommended_mode: McpAuthMode,
    pub reason: DetectionReason,
    pub oauth: Option<OAuthDetectionSummary>,
}

pub async fn detect_mcp_auth(
    client: &reqwest::Client,
    original_url: &str,
) -> Result<McpAuthDetection, OAuthError> {
    let parsed = reqwest::Url::parse(original_url)
        .map_err(|e| OAuthError::DiscoveryFailed(format!("invalid server URL {original_url}: {e}")))?;
    let has_query = parsed.query().is_some();
    let mut bare = parsed.clone();
    bare.set_query(None);
    let bare_url = bare.to_string();

    if has_query {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::QueryStringPresent,
            oauth: None,
        });
    }

    if is_loopback_url(original_url) || !is_public_url(&bare_url) {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::LoopbackOrPrivate,
            oauth: None,
        });
    }

    match discover_oauth(client, &bare_url).await {
        Ok(discovery) => Ok(oauth_detected(bare_url, discovery)),
        Err(OAuthError::DiscoveryFailed(_)) => Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::NoOAuthMetadata,
            oauth: None,
        }),
        Err(error) => Err(error),
    }
}

fn oauth_detected(bare_url: String, discovery: OAuthDiscovery) -> McpAuthDetection {
    let resource = if discovery.resource.trim().is_empty() {
        canonical_resource_uri(&bare_url).unwrap_or_else(|_| bare_url.clone())
    } else {
        discovery.resource.clone()
    };
    McpAuthDetection {
        bare_url,
        oauth_discovered: true,
        recommended_mode: McpAuthMode::OAuth,
        reason: DetectionReason::OAuthDiscovered,
        oauth: Some(OAuthDetectionSummary {
            resource,
            scopes: discovery.scopes,
            authorization_endpoint: discovery.metadata.authorization_endpoint,
            token_endpoint: discovery.metadata.token_endpoint,
            registration_endpoint: discovery.metadata.registration_endpoint,
        }),
    }
}
```

- [ ] **Step 5: Run detection tests**

Run:

```bash
devenv shell -- cargo test -p right-mcp detect::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-mcp/src/lib.rs crates/right-mcp/src/detect.rs
devenv shell -- git commit -m "feat(mcp): add URL-first auth detection"
```

---

### Task 4: Add Multi-Header Credential Storage

**Files:**
- Create: `crates/right-db/src/sql/v35_mcp_http_headers.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Modify: `crates/right-mcp/src/credentials.rs`

- [ ] **Step 1: Add the failing migration test**

In `crates/right-db/src/migrations.rs`, near `v33_mcp_servers_has_oauth_resource_column`, add:

```rust
#[test]
fn v35_mcp_http_headers_table() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
        ("nango", "https://api.nango.dev/mcp"),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO mcp_http_headers (server_name, header_name, header_value)
         VALUES (?1, ?2, ?3)",
        ("nango", "connection-id", "conn_123"),
    )
    .unwrap();

    let value: String = conn
        .query_row(
            "SELECT header_value FROM mcp_http_headers WHERE server_name = ?1 AND header_name = ?2",
            ("nango", "connection-id"),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "conn_123");

    conn.execute("DELETE FROM mcp_servers WHERE name = ?1", ["nango"])
        .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mcp_http_headers", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "headers must be deleted with their MCP server");
}
```

- [ ] **Step 2: Run migration test and verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-db migrations::tests::v35_mcp_http_headers_table
```

Expected: FAIL with `no such table: mcp_http_headers`.

- [ ] **Step 3: Add v35 SQL and register it**

Create `crates/right-db/src/sql/v35_mcp_http_headers.sql`:

```sql
CREATE TABLE IF NOT EXISTS mcp_http_headers (
    server_name  TEXT NOT NULL,
    header_name  TEXT NOT NULL,
    header_value TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (server_name, header_name),
    FOREIGN KEY (server_name) REFERENCES mcp_servers(name) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_mcp_http_headers_server
    ON mcp_http_headers(server_name);
```

In `crates/right-db/src/migrations.rs`, add:

```rust
const V35_SCHEMA: &str = include_str!("sql/v35_mcp_http_headers.sql");
pub const LATEST_SCHEMA_VERSION: u32 = 35;
```

and append:

```rust
Migration {
    version: 35,
    sql: V35_SCHEMA,
    hook: None,
},
```

after the v34 migration entry.

- [ ] **Step 4: Run migration test**

Run:

```bash
devenv shell -- cargo test -p right-db migrations::tests::v35_mcp_http_headers_table
```

Expected: PASS.

- [ ] **Step 5: Add failing credential helper tests**

In `crates/right-mcp/src/credentials.rs`, inside `mod db_tests`, add:

```rust
#[test]
fn db_set_http_headers_replaces_values_and_lists_names_only() {
    let (_dir, conn) = setup_db();
    db_add_server(&conn, "nango", "https://api.nango.dev/mcp").unwrap();

    db_set_http_headers(
        &conn,
        "nango",
        &[
            HttpHeaderSecret::new("Authorization", "Bearer env-secret").unwrap(),
            HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
            HttpHeaderSecret::new("provider-config-key", "github").unwrap(),
        ],
    )
    .unwrap();

    let names = db_list_http_header_names(&conn, "nango").unwrap();
    assert_eq!(names, vec!["Authorization", "connection-id", "provider-config-key"]);

    let secrets = db_list_http_headers(&conn, "nango").unwrap();
    assert_eq!(secrets.len(), 3);
    assert_eq!(secrets[0].name(), "Authorization");
    assert_eq!(secrets[0].value(), "Bearer env-secret");

    db_set_http_headers(
        &conn,
        "nango",
        &[HttpHeaderSecret::new("connection-id", "conn_replaced").unwrap()],
    )
    .unwrap();
    let names = db_list_http_header_names(&conn, "nango").unwrap();
    assert_eq!(names, vec!["connection-id"]);
    let secrets = db_list_http_headers(&conn, "nango").unwrap();
    assert_eq!(secrets[0].value(), "conn_replaced");
}

#[test]
fn db_set_http_headers_rejects_bad_header_name() {
    let (_dir, conn) = setup_db();
    db_add_server(&conn, "nango", "https://api.nango.dev/mcp").unwrap();

    let err = HttpHeaderSecret::new("bad header", "secret").unwrap_err();
    assert!(matches!(err, CredentialError::InvalidAuthHeader(_)));
}

#[test]
fn db_set_http_headers_rejects_missing_server() {
    let (_dir, conn) = setup_db();

    let err = db_set_http_headers(
        &conn,
        "ghost",
        &[HttpHeaderSecret::new("Authorization", "Bearer x").unwrap()],
    )
    .unwrap_err();
    assert!(matches!(err, CredentialError::ServerNotFound(_)));
}
```

- [ ] **Step 6: Run credential tests and verify missing symbols fail**

Run:

```bash
devenv shell -- cargo test -p right-mcp credentials::db_tests::db_set_http_headers_replaces_values_and_lists_names_only
```

Expected: FAIL because `HttpHeaderSecret` and helper functions do not exist.

- [ ] **Step 7: Add header-secret type and helper functions**

In `crates/right-mcp/src/credentials.rs`, add an error variant if absent:

```rust
#[error("invalid auth header: {0}")]
InvalidAuthHeader(String),
```

Add this type near `McpServerEntry`:

```rust
#[derive(Clone, PartialEq, Eq)]
pub struct HttpHeaderSecret {
    name: String,
    value: String,
}

impl std::fmt::Debug for HttpHeaderSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpHeaderSecret")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl HttpHeaderSecret {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, CredentialError> {
        let name = name.into();
        let value = value.into();
        validate_header_name(&name)?;
        if value.is_empty() {
            return Err(CredentialError::InvalidAuthHeader(
                "header value must not be empty".to_string(),
            ));
        }
        Ok(Self { name, value })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

pub fn validate_header_name(name: &str) -> Result<(), CredentialError> {
    http::HeaderName::from_bytes(name.as_bytes())
        .map(|_| ())
        .map_err(|_| CredentialError::InvalidAuthHeader(name.to_string()))
}
```

Add helper functions:

```rust
pub fn db_set_http_headers(
    conn: &Connection,
    name: &str,
    headers: &[HttpHeaderSecret],
) -> Result<(), CredentialError> {
    let exists: Option<String> = conn
        .query_one("SELECT name FROM mcp_servers WHERE name = ?1", [name], |row| row.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }

    conn.with_immediate_transaction(|tx| {
        tx.execute("DELETE FROM mcp_http_headers WHERE server_name = ?1", [name])?;
        for header in headers {
            validate_header_name(header.name())?;
            tx.execute(
                "INSERT INTO mcp_http_headers (server_name, header_name, header_value, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'))",
                params![name, header.name(), header.value()],
            )?;
        }
        tx.execute(
            "UPDATE mcp_servers SET auth_type = 'headers', auth_header = NULL, auth_token = NULL WHERE name = ?1",
            [name],
        )?;
        Ok(())
    })?;
    Ok(())
}

pub fn db_list_http_header_names(
    conn: &Connection,
    name: &str,
) -> Result<Vec<String>, CredentialError> {
    Ok(conn.query_all(
        "SELECT header_name FROM mcp_http_headers WHERE server_name = ?1 ORDER BY header_name",
        [name],
        |row| row.get(0),
    )?)
}

pub fn db_list_http_headers(
    conn: &Connection,
    name: &str,
) -> Result<Vec<HttpHeaderSecret>, CredentialError> {
    let rows: Vec<(String, String)> = conn.query_all(
        "SELECT header_name, header_value FROM mcp_http_headers WHERE server_name = ?1 ORDER BY header_name",
        [name],
        |row| {
            let header_name: String = row.get(0)?;
            let header_value: String = row.get(1)?;
            Ok((header_name, header_value))
        },
    )?;

    rows.into_iter()
        .map(|(header_name, header_value)| HttpHeaderSecret::new(header_name, header_value))
        .collect()
}
```

- [ ] **Step 8: Run credential and migration tests**

Run:

```bash
devenv shell -- cargo test -p right-db migrations::tests::v35_mcp_http_headers_table
devenv shell -- cargo test -p right-mcp credentials::db_tests::db_set_http_headers
```

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
devenv shell -- git add crates/right-db/src/sql/v35_mcp_http_headers.sql crates/right-db/src/migrations.rs crates/right-mcp/src/credentials.rs
devenv shell -- git commit -m "feat(mcp): persist multi-header credentials"
```

---

### Task 5: Inject Multi-Header Auth In Proxy Backends

**Files:**
- Modify: `crates/right-mcp/src/proxy.rs`
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Add failing proxy tests**

In `crates/right-mcp/src/proxy.rs`, inside `mod tests`, add:

```rust
#[test]
fn auth_method_display_redacts_headers() {
    let headers = vec![
        crate::credentials::HttpHeaderSecret::new("Authorization", "Bearer secret").unwrap(),
        crate::credentials::HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
    ];
    let method = AuthMethod::Headers(headers);

    assert_eq!(method.to_string(), "headers");
    let debug = format!("{method:?}");
    assert!(debug.contains("Authorization"));
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("Bearer secret"));
    assert!(!debug.contains("conn_123"));
}

#[tokio::test]
async fn dynamic_auth_headers_injects_multiple_headers() {
    setup_crypto();
    let token = Arc::new(RwLock::new(Some("unused-token".to_string())));
    let client = DynamicAuthClient::new(
        reqwest::Client::new(),
        token,
        AuthMethod::Headers(vec![
            crate::credentials::HttpHeaderSecret::new("Authorization", "Bearer env-secret").unwrap(),
            crate::credentials::HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
            crate::credentials::HttpHeaderSecret::new("provider-config-key", "github").unwrap(),
        ]),
    );

    let (auth, extra) = client.build_auth().await;
    assert_eq!(auth, None);
    assert_eq!(extra.len(), 3);
    let headers = extra
        .into_iter()
        .map(|(name, value)| (name.as_str().to_string(), value.to_str().unwrap().to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(headers["authorization"], "Bearer env-secret");
    assert_eq!(headers["connection-id"], "conn_123");
    assert_eq!(headers["provider-config-key"], "github");
}
```

- [ ] **Step 2: Run proxy tests and verify missing variant fails**

Run:

```bash
devenv shell -- cargo test -p right-mcp proxy::tests::dynamic_auth_headers_injects_multiple_headers
```

Expected: FAIL because `AuthMethod::Headers` does not exist.

- [ ] **Step 3: Add `Headers` auth method**

In `crates/right-mcp/src/proxy.rs`, replace the `AuthMethod` derive and enum with:

```rust
#[derive(Clone, Default, PartialEq, Eq)]
pub enum AuthMethod {
    #[default]
    Bearer,
    Header(String),
    Headers(Vec<crate::credentials::HttpHeaderSecret>),
    QueryString,
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer => f.write_str("Bearer"),
            Self::Header(name) => f.debug_tuple("Header").field(name).finish(),
            Self::Headers(headers) => f.debug_tuple("Headers").field(headers).finish(),
            Self::QueryString => f.write_str("QueryString"),
        }
    }
}
```

Update `Display`:

```rust
Self::Headers(_) => f.write_str("headers"),
```

Add a constructor helper:

```rust
pub fn from_db_with_headers(
    auth_type: Option<&str>,
    auth_header: Option<&str>,
    headers: Vec<crate::credentials::HttpHeaderSecret>,
) -> Self {
    match auth_type {
        Some("headers") => Self::Headers(headers),
        _ => Self::from_db(auth_type, auth_header),
    }
}
```

Update `build_auth`:

```rust
AuthMethod::Headers(headers) => {
    let mut extra = Vec::new();
    for header in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name().as_bytes()),
            HeaderValue::from_str(header.value()),
        ) {
            extra.push((name, value));
        }
    }
    (None, extra)
}
```

- [ ] **Step 4: Update Aggregator startup restore**

In `crates/right/src/main.rs`, inside the loop over `db_list_servers`, load headers before constructing `auth_method`:

```rust
let headers = if s.auth_type.as_deref() == Some("headers") {
    right_mcp::credentials::db_list_http_headers(&conn, &s.name).unwrap_or_else(|e| {
        tracing::warn!(
            agent = agent_name.as_str(),
            server = %s.name,
            "failed to load MCP HTTP headers: {e:#}"
        );
        Vec::new()
    })
} else {
    Vec::new()
};
let auth_method = right_mcp::proxy::AuthMethod::from_db_with_headers(
    s.auth_type.as_deref(),
    s.auth_header.as_deref(),
    headers,
);
```

Use this instead of the current `AuthMethod::from_db(...)` call.

- [ ] **Step 5: Run proxy tests and check startup compile**

Run:

```bash
devenv shell -- cargo test -p right-mcp proxy::tests::auth_method
devenv shell -- cargo test -p right-mcp proxy::tests::dynamic_auth_headers_injects_multiple_headers
devenv shell -- cargo check -p right
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-mcp/src/proxy.rs crates/right/src/main.rs
devenv shell -- git commit -m "feat(mcp): inject multiple HTTP auth headers"
```

---

### Task 6: Extend Internal MCP API For Headers And Redaction

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs`
- Modify: `crates/right/src/internal_api.rs`

- [ ] **Step 1: Add failing internal API tests**

In `crates/right/src/internal_api.rs`, inside `mod tests`, add:

```rust
#[tokio::test]
async fn mcp_add_headers_auth_redacts_values_in_list() {
    let tmp = tempfile::tempdir().unwrap();
    let app = make_test_router(tmp.path());

    let (status, body) = send_json(
        app.clone(),
        "/mcp-add",
        serde_json::json!({
            "agent": "test-agent",
            "name": "nango",
            "url": "https://api.nango.dev/mcp",
            "auth_type": "headers",
            "headers": [
                { "name": "Authorization", "value": "Bearer env-secret" },
                { "name": "connection-id", "value": "conn_123" },
                { "name": "provider-config-key", "value": "github" }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");

    let (status, body) = send_json(
        app,
        "/mcp-list",
        serde_json::json!({ "agent": "test-agent" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    let servers = body["servers"].as_array().unwrap();
    let nango = servers
        .iter()
        .find(|server| server["name"] == "nango")
        .expect("nango listed");
    assert_eq!(nango["auth_type"], "headers");
    assert_eq!(
        nango["header_names"],
        serde_json::json!(["Authorization", "connection-id", "provider-config-key"])
    );
    assert!(
        !body.to_string().contains("env-secret"),
        "list response must not expose header values: {body}"
    );
}

#[tokio::test]
async fn mcp_set_headers_replaces_existing_header_names() {
    let tmp = tempfile::tempdir().unwrap();
    let app = make_test_router(tmp.path());

    let (status, body) = send_json(
        app.clone(),
        "/mcp-add",
        serde_json::json!({
            "agent": "test-agent",
            "name": "nango",
            "url": "https://api.nango.dev/mcp",
            "auth_type": "headers",
            "headers": [
                { "name": "Authorization", "value": "Bearer old" },
                { "name": "connection-id", "value": "old_conn" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    let (status, body) = send_json(
        app.clone(),
        "/mcp-set-headers",
        serde_json::json!({
            "agent": "test-agent",
            "name": "nango",
            "headers": [
                { "name": "connection-id", "value": "new_conn" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    let (status, body) = send_json(app, "/mcp-list", serde_json::json!({ "agent": "test-agent" })).await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let nango = body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["name"] == "nango")
        .unwrap();
    assert_eq!(nango["header_names"], serde_json::json!(["connection-id"]));
    assert!(!body.to_string().contains("new_conn"));
}
```

- [ ] **Step 2: Run tests and verify route/field failure**

Run:

```bash
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_add_headers_auth_redacts_values_in_list
```

Expected: FAIL because `headers` request field and list `header_names` are unsupported.

- [ ] **Step 3: Add internal client DTOs**

In `crates/right-mcp/src/internal_client.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpHeaderInput {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSetHeadersRequest {
    pub agent: String,
    pub name: String,
    pub headers: Vec<HttpHeaderInput>,
}

#[derive(Debug, Deserialize)]
pub struct McpSetHeadersResponse {
    pub ok: bool,
}
```

Extend `mcp_add` or add a new method. Prefer adding a struct-based method while leaving the old method for compatibility:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct McpAddRequest<'a> {
    pub agent: &'a str,
    pub name: &'a str,
    pub url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpHeaderInput>,
}

pub async fn mcp_add_request(
    &self,
    request: &McpAddRequest<'_>,
) -> Result<McpAddResponse, InternalClientError> {
    self.post("/mcp-add", request).await
}

pub async fn mcp_set_headers(
    &self,
    request: &McpSetHeadersRequest,
) -> Result<McpSetHeadersResponse, InternalClientError> {
    self.post("/mcp-set-headers", request).await
}
```

Update `McpServerStatus`:

```rust
#[serde(default)]
pub header_names: Vec<String>,
```

- [ ] **Step 4: Extend Aggregator request/response types and router**

In `crates/right/src/internal_api.rs`, add to `McpAddRequest`:

```rust
#[serde(default)]
pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
```

Add:

```rust
#[derive(Deserialize)]
pub(crate) struct McpSetHeadersRequest {
    pub agent: String,
    pub name: String,
    #[serde(default)]
    pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
}

#[derive(Serialize)]
pub(crate) struct McpSetHeadersResponse {
    pub ok: bool,
}
```

Add route:

```rust
.route("/mcp-set-headers", post(handle_mcp_set_headers))
```

Extend `McpServerStatus`:

```rust
#[serde(skip_serializing_if = "Vec::is_empty")]
pub header_names: Vec<String>,
```

- [ ] **Step 5: Implement header conversion helper**

In `crates/right/src/internal_api.rs`, add:

```rust
fn header_inputs_to_secrets(
    headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
) -> Result<Vec<credentials::HttpHeaderSecret>, CredentialError> {
    headers
        .into_iter()
        .map(|header| credentials::HttpHeaderSecret::new(header.name, header.value))
        .collect()
}
```

- [ ] **Step 6: Update `handle_mcp_add` for `headers` auth**

Before creating `auth_method`, convert headers:

```rust
let header_secrets = match header_inputs_to_secrets(req.headers.clone()) {
    Ok(headers) => headers,
    Err(e) => return validation_error(format!("{e}")).into_response(),
};
let auth_method = AuthMethod::from_db_with_headers(
    req.auth_type.as_deref(),
    req.auth_header.as_deref(),
    header_secrets.clone(),
);
```

Inside the DB persistence block, after `db_add_server`, use:

```rust
if req.auth_type.as_deref() == Some("headers") {
    if let Err(e) = credentials::db_set_http_headers(&conn, &req.name, &header_secrets) {
        return internal_error(format!("db_set_http_headers: {e:#}")).into_response();
    }
} else if let Some(ref auth_type_str) = req.auth_type
    && let Err(e) = credentials::db_set_auth(
        &conn,
        &req.name,
        auth_type_str,
        req.auth_header.as_deref(),
        req.auth_token.as_deref(),
    )
{
    return internal_error(format!("db_set_auth: {e:#}")).into_response();
}
```

- [ ] **Step 7: Implement `handle_mcp_set_headers`**

In `crates/right/src/internal_api.rs`, add:

```rust
async fn handle_mcp_set_headers(
    State(state): State<InternalState>,
    Json(req): Json<McpSetHeadersRequest>,
) -> axum::response::Response {
    if req.name == right_mcp::PROTECTED_MCP_SERVER || req.name == "rightmeta" {
        return validation_error("protected MCP server cannot be modified".to_string()).into_response();
    }

    let header_secrets = match header_inputs_to_secrets(req.headers) {
        Ok(headers) => headers,
        Err(e) => return validation_error(format!("{e}")).into_response(),
    };

    let (conn_arc, proxies_lock) = {
        let Some(registry) = state.dispatcher.agents.get(&req.agent) else {
            return not_found(format!("agent '{}' not found", req.agent)).into_response();
        };
        let conn = match registry.right.get_conn(&req.agent) {
            Ok(c) => c,
            Err(e) => return internal_error(format!("db open: {e:#}")).into_response(),
        };
        (conn, Arc::clone(&registry.proxies))
    };

    {
        let conn = match conn_arc.lock() {
            Ok(c) => c,
            Err(e) => return internal_error(format!("mutex poisoned: {e}")).into_response(),
        };
        if let Err(e) = credentials::db_set_http_headers(&conn, &req.name, &header_secrets) {
            return match e {
                CredentialError::ServerNotFound(_) => not_found(format!("server '{}' not found", req.name)).into_response(),
                _ => internal_error(format!("db_set_http_headers: {e:#}")).into_response(),
            };
        }
    }

    let mut proxies = proxies_lock.write().await;
    let Some(existing) = proxies.get(&req.name) else {
        return not_found(format!("server '{}' not found", req.name)).into_response();
    };
    let replacement = Arc::new(ProxyBackend::new(
        req.name.clone(),
        existing.agent_dir().to_path_buf(),
        existing.url().to_string(),
        Arc::new(tokio::sync::RwLock::new(None)),
        AuthMethod::Headers(header_secrets),
    ));
    proxies.insert(req.name, replacement);

    Json(McpSetHeadersResponse { ok: true }).into_response()
}
```

Add this accessor to `ProxyBackend` in `crates/right-mcp/src/proxy.rs`:

```rust
pub fn agent_dir(&self) -> &std::path::Path {
    &self.agent_dir
}
```

to `crates/right-mcp/src/proxy.rs`.

- [ ] **Step 8: Add header names to `handle_mcp_list`**

When loading DB auth data, also load header names:

```rust
let db_header_names: std::collections::HashMap<String, Vec<String>> = {
    match registry.right.get_conn(&req.agent) {
        Ok(conn_arc) => {
            let conn = conn_arc.lock().unwrap_or_else(|e| e.into_inner());
            credentials::db_list_servers(&conn)
                .unwrap_or_default()
                .into_iter()
                .map(|s| {
                    let names = if s.auth_type.as_deref() == Some("headers") {
                        credentials::db_list_http_header_names(&conn, &s.name).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    (s.name, names)
                })
                .collect()
        }
        Err(_) => std::collections::HashMap::new(),
    }
};
```

Populate `header_names` for `right` as `Vec::new()` and for proxies as:

```rust
header_names: db_header_names.get(name).cloned().unwrap_or_default(),
```

- [ ] **Step 9: Run internal API tests**

Run:

```bash
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_add_headers_auth_redacts_values_in_list
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_set_headers_replaces_existing_header_names
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_list_returns_right_backend
```

Expected: PASS.

- [ ] **Step 10: Commit**

Run:

```bash
devenv shell -- git add crates/right-mcp/src/internal_client.rs crates/right/src/internal_api.rs crates/right-mcp/src/proxy.rs
devenv shell -- git commit -m "feat(mcp): expose multi-header internal API"
```

---

### Task 7: Add Bot Dashboard MCP Routes

**Files:**
- Create: `crates/bot/src/telegram/dashboard/mcp.rs`
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Extend dashboard state**

In `crates/bot/src/telegram/dashboard.rs`, add fields to `DashboardState`:

```rust
pub internal_client: std::sync::Arc<right_mcp::internal_client::InternalClient>,
pub pending_auth: super::oauth_callback::PendingAuthMap,
```

Update `crates/bot/src/lib.rs` dashboard construction:

```rust
internal_client: Arc::clone(&internal_client),
pending_auth: Arc::clone(&pending_auth),
```

Update `test_state` in `dashboard.rs`:

```rust
internal_client: Arc::new(right_mcp::internal_client::InternalClient::new(
    agent_dir.join("missing-internal.sock"),
)),
pending_auth: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
```

- [ ] **Step 2: Create route module skeleton**

Create `crates/bot/src/telegram/dashboard/mcp.rs`:

```rust
use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use super::{DashboardState, authenticate_api, json_error};

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpServersResponse {
    pub agent: String,
    pub servers: Vec<DashboardMcpServer>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpServer {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    pub auth_type: Option<String>,
    pub header_names: Vec<String>,
    pub protected: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpDetectRequest {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpAddRequest {
    pub name: String,
    pub url: String,
    pub mode: right_mcp::detect::McpAuthMode,
    #[serde(default)]
    pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpHeadersRequest {
    #[serde(default)]
    pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpMutationResponse {
    pub ok: bool,
}
```

- [ ] **Step 3: Add failing dashboard route tests**

In `crates/bot/src/telegram/dashboard.rs`, inside tests, add this helper:

```rust
async fn post_json(
    path: &str,
    auth: Option<String>,
    agent_dir: std::path::PathBuf,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let router = super::build_dashboard_router(test_state(agent_dir));
    let mut builder = Request::builder()
        .uri(path)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
    }
    let response = router
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("valid request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body bytes");
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json response")
    };
    (status, value)
}
```

Add tests:

```rust
#[tokio::test]
async fn dashboard_mcp_servers_requires_auth() {
    let temp = tempfile::tempdir().expect("tempdir");
    let status = get(
        "/dashboard/alpha/api/v1/mcp/servers",
        None,
        temp.path().to_path_buf(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_mcp_detect_rejects_bad_url() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (status, body) = post_json(
        "/dashboard/alpha/api/v1/mcp/detect",
        Some(signed_init_data(42)),
        temp.path().to_path_buf(),
        json!({ "url": "not a url" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_url");
}
```

- [ ] **Step 4: Register routes**

In `crates/bot/src/telegram/dashboard.rs`, add:

```rust
mod mcp;
```

Add routes in `build_dashboard_router`:

```rust
.route(
    "/dashboard/{agent}/api/v1/mcp/servers",
    get(mcp::handle_mcp_servers).post(mcp::handle_mcp_add),
)
.route(
    "/dashboard/{agent}/api/v1/mcp/detect",
    axum::routing::post(mcp::handle_mcp_detect),
)
.route(
    "/dashboard/{agent}/api/v1/mcp/servers/{server_name}/headers",
    patch(mcp::handle_mcp_headers),
)
.route(
    "/dashboard/{agent}/api/v1/mcp/servers/{server_name}/oauth/start",
    axum::routing::post(mcp::handle_mcp_oauth_start),
)
.route(
    "/dashboard/{agent}/api/v1/mcp/servers/{server_name}",
    axum::routing::delete(mcp::handle_mcp_remove),
)
```

- [ ] **Step 5: Implement server list and detect routes**

In `mcp.rs`, add:

```rust
pub(crate) async fn handle_mcp_servers(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match state.internal_client.mcp_list(&state.agent_name).await {
        Ok(response) => {
            let servers = response
                .servers
                .into_iter()
                .map(|server| DashboardMcpServer {
                    protected: server.name == right_mcp::PROTECTED_MCP_SERVER || server.name == "rightmeta",
                    name: server.name,
                    url: server.url,
                    status: server.status,
                    tool_count: server.tool_count,
                    auth_type: server.auth_type,
                    header_names: server.header_names,
                })
                .collect();
            Json(DashboardMcpServersResponse {
                agent: state.agent_name,
                servers,
            })
            .into_response()
        }
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "mcp_unavailable",
            Some(&format!("{error:#}")),
        ),
    }
}

pub(crate) async fn handle_mcp_detect(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<DashboardMcpDetectRequest>,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    match right_mcp::detect::detect_mcp_auth(&client, &request.url).await {
        Ok(result) => Json(result).into_response(),
        Err(right_mcp::oauth::OAuthError::DiscoveryFailed(detail))
            if detail.contains("invalid server URL") =>
        {
            json_error(StatusCode::BAD_REQUEST, "invalid_url", Some("invalid MCP URL"))
        }
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "detect_failed",
            Some(&format!("{error:#}")),
        ),
    }
}
```

- [ ] **Step 6: Implement add, headers, and remove routes**

In `mcp.rs`, add:

```rust
pub(crate) async fn handle_mcp_add(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<DashboardMcpAddRequest>,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let (url, auth_type, headers) = match request.mode {
        right_mcp::detect::McpAuthMode::OAuth => (request.url.as_str(), Some("oauth"), Vec::new()),
        right_mcp::detect::McpAuthMode::Headers => (request.url.as_str(), Some("headers"), request.headers),
        right_mcp::detect::McpAuthMode::UrlAsIs => (request.url.as_str(), None, Vec::new()),
    };

    let add = right_mcp::internal_client::McpAddRequest {
        agent: &state.agent_name,
        name: &request.name,
        url,
        auth_type,
        auth_header: None,
        auth_token: None,
        headers,
    };

    match state.internal_client.mcp_add_request(&add).await {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "mcp_add_failed",
            Some(&format!("{error:#}")),
        ),
    }
}

pub(crate) async fn handle_mcp_headers(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<DashboardMcpHeadersRequest>,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    if server_name == right_mcp::PROTECTED_MCP_SERVER || server_name == "rightmeta" {
        return json_error(StatusCode::FORBIDDEN, "protected_mcp", Some("protected MCP server cannot be modified"));
    }

    let update = right_mcp::internal_client::McpSetHeadersRequest {
        agent: state.agent_name.clone(),
        name: server_name,
        headers: request.headers,
    };
    match state.internal_client.mcp_set_headers(&update).await {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "mcp_headers_failed",
            Some(&format!("{error:#}")),
        ),
    }
}

pub(crate) async fn handle_mcp_remove(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    if server_name == right_mcp::PROTECTED_MCP_SERVER || server_name == "rightmeta" {
        return json_error(StatusCode::FORBIDDEN, "protected_mcp", Some("protected MCP server cannot be removed"));
    }

    match state.internal_client.mcp_remove(&state.agent_name, &server_name).await {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "mcp_remove_failed",
            Some(&format!("{error:#}")),
        ),
    }
}
```

- [ ] **Step 7: Run dashboard route tests**

Run:

```bash
devenv shell -- cargo test -p rightclaw-bot --lib telegram::dashboard::tests::dashboard_mcp
```

Expected: PASS for the route validation tests added so far.

- [ ] **Step 8: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/mcp.rs crates/bot/src/lib.rs
devenv shell -- git commit -m "feat(dashboard): add authenticated MCP routes"
```

---

### Task 8: Add Dashboard OAuth Start Route

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/mcp.rs`

- [ ] **Step 1: Add OAuth-start response type**

In `mcp.rs`, add:

```rust
#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpOAuthStartResponse {
    pub auth_url: String,
}
```

- [ ] **Step 2: Add failing test for missing server**

In `crates/bot/src/telegram/dashboard.rs`, add:

```rust
#[tokio::test]
async fn dashboard_mcp_oauth_start_unknown_server_is_bad_gateway_or_not_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (status, body) = post_json(
        "/dashboard/alpha/api/v1/mcp/servers/missing/oauth/start",
        Some(signed_init_data(42)),
        temp.path().to_path_buf(),
        json!({}),
    )
    .await;

    assert!(
        status == StatusCode::BAD_GATEWAY || status == StatusCode::NOT_FOUND,
        "unexpected status {status}: {body}"
    );
    assert_ne!(body.to_string().contains("access_token"), true);
}
```

This test uses the existing missing internal socket in test state. Add a stronger happy-path route test only after a test internal server is introduced.

- [ ] **Step 3: Implement OAuth start**

In `mcp.rs`, implement:

```rust
pub(crate) async fn handle_mcp_oauth_start(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let server_url = match state.internal_client.mcp_list(&state.agent_name).await {
        Ok(list) => list
            .servers
            .into_iter()
            .find(|server| server.name == server_name)
            .and_then(|server| server.url),
        Err(error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "mcp_list_failed",
                Some(&format!("{error:#}")),
            );
        }
    };
    let Some(server_url) = server_url else {
        return json_error(StatusCode::NOT_FOUND, "not_found", Some("MCP server not found"));
    };

    let global_config = match right_config::read_global_config(&state.home) {
        Ok(config) => config,
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_failed",
                Some(&format!("{error:#}")),
            );
        }
    };
    if global_config.tunnel.hostname.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "tunnel_missing",
            Some("tunnel hostname is not configured"),
        );
    }

    let http_client = reqwest::Client::new();
    let discovery = match right_mcp::oauth::discover_oauth(&http_client, &server_url).await {
        Ok(discovery) => discovery,
        Err(error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "oauth_discovery_failed",
                Some(&format!("{error:#}")),
            );
        }
    };
    let scopes = discovery.scopes;
    let scope_param = right_mcp::oauth::scope_param(&scopes);
    let scope_refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let redirect_uri = format!(
        "https://{}/oauth/{}/callback",
        global_config.tunnel.hostname,
        state.agent_name
    );
    let (client_id, client_secret) = match right_mcp::oauth::register_client_or_fallback(
        &http_client,
        &discovery.metadata,
        None,
        &redirect_uri,
        &scope_refs,
    )
    .await
    {
        Ok(pair) => pair,
        Err(error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "client_registration_failed",
                Some(&format!("{error:#}")),
            );
        }
    };

    let (code_verifier, code_challenge) = right_mcp::oauth::generate_pkce();
    let oauth_state = right_mcp::oauth::generate_state();
    state.pending_auth.lock().await.insert(
        oauth_state.clone(),
        right_mcp::oauth::PendingAuth {
            server_name: server_name.clone(),
            server_url,
            resource: discovery.resource.clone(),
            code_verifier,
            state: oauth_state.clone(),
            token_endpoint: discovery.metadata.token_endpoint.clone(),
            client_id: client_id.clone(),
            client_secret,
            redirect_uri: redirect_uri.clone(),
            created_at: std::time::Instant::now(),
        },
    );

    let auth_url = right_mcp::oauth::build_auth_url(
        &discovery.metadata,
        &client_id,
        &redirect_uri,
        &oauth_state,
        &code_challenge,
        &discovery.resource,
        scope_param.as_deref(),
    );

    Json(DashboardMcpOAuthStartResponse { auth_url }).into_response()
}
```

- [ ] **Step 4: Run dashboard tests**

Run:

```bash
devenv shell -- cargo test -p rightclaw-bot --lib telegram::dashboard::tests::dashboard_mcp_oauth_start
devenv shell -- cargo test -p rightclaw-bot --lib telegram::oauth_callback::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard/mcp.rs crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(dashboard): start MCP OAuth from web UI"
```

---

### Task 9: Replace Telegram MCP Subcommands With Dashboard Deep Link

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Add formatting helper test**

In `crates/bot/src/telegram/handler.rs`, add a pure helper:

```rust
fn dashboard_mcp_button_label() -> &'static str {
    "Open MCP dashboard"
}
```

Add test:

```rust
#[test]
fn dashboard_mcp_button_label_names_destination() {
    assert_eq!(dashboard_mcp_button_label(), "Open MCP dashboard");
}
```

- [ ] **Step 2: Run helper test**

Run:

```bash
devenv shell -- cargo test -p rightclaw-bot --lib telegram::handler::tests::dashboard_mcp_button_label_names_destination
```

Expected: PASS.

- [ ] **Step 3: Simplify `handle_mcp`**

Replace the body after the private chat check with logic equivalent to:

```rust
let global_config = right_config::read_global_config(&home.0)
    .map_err(|e| to_request_err(format!("mcp dashboard: read config.yaml: {e:#}")))?;
let agent_name = agent_dir
    .0
    .file_name()
    .and_then(|name| name.to_str())
    .ok_or_else(|| {
        to_request_err(format!(
            "mcp dashboard: invalid agent directory name: {}",
            agent_dir.0.display()
        ))
    })?;
let mut url = super::dashboard::dashboard_url(&global_config.tunnel.hostname, agent_name)
    .map_err(|e| to_request_err(format!("mcp dashboard: invalid URL: {e:#}")))?;
url.set_query(Some("view=mcp"));

let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::web_app(
    dashboard_mcp_button_label(),
    teloxide::types::WebAppInfo { url },
)]]);

let mut send = bot
    .send_message(msg.chat.id, "MCP")
    .reply_markup(keyboard);
let eff_thread_id = effective_thread_id(&msg);
if eff_thread_id != 0 {
    send = send.message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(
        eff_thread_id as i32,
    )));
}
send.await?;
Ok(())
```

Keep the current `handle_mcp` signature so dispatch wiring stays local to this
task. Rename arguments that the simplified body no longer uses with a leading
underscore:

```rust
pub async fn handle_mcp(
    bot: BotType,
    msg: Message,
    _args: String,
    agent_dir: Arc<AgentDir>,
    _pending_auth: PendingAuthMap,
    home: Arc<RightHome>,
    _internal: Arc<InternalApi>,
    _pending_token_slot: Arc<PendingTokenSlot>,
    _pending_auth_choice_slot: Arc<PendingMcpAuthChoiceSlot>,
    _ssh_config: Arc<SshConfigPath>,
    _settings: Arc<AgentSettings>,
) -> ResponseResult<()> {
```

Do not remove unrelated handler code in the same commit.

- [ ] **Step 4: Remove Telegram MCP callback prompt wiring if now unused**

After the simplified command compiles, remove these now-unused Telegram MCP
prompt pieces when the compiler reports them as unused:

- `handle_mcp_auth_choice_callback`
- `PendingMcpAuthChoiceSlot`
- `PendingMcpAuthChoiceRequest`
- `PendingMcpAuthChoiceTake`
- `parse_auth_choice_callback_data`
- pending MCP token interception that only served `/mcp add`

Do not delete unrelated dead code.

Expected affected files if cleanup is necessary:

- `crates/bot/src/telegram/handler.rs`
- `crates/bot/src/telegram/mcp_auth_choice.rs`
- `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 5: Run bot handler tests**

Run:

```bash
devenv shell -- cargo test -p rightclaw-bot --lib telegram::handler::tests::dashboard_mcp_button_label_names_destination
devenv shell -- cargo check -p rightclaw-bot
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/mcp_auth_choice.rs
devenv shell -- git commit -m "feat(bot): open dashboard from mcp command"
```

When staging, include only files that actually changed.

---

### Task 10: Add Frontend MCP API Types And Deep-Link State

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/api.ts`
- Modify: `crates/right-dashboard/frontend/src/App.vue`

- [ ] **Step 1: Add TypeScript types**

In `types.ts`, add:

```ts
export type McpAuthMode = 'oauth' | 'headers' | 'url_as_is'

export interface McpServerSummary {
  name: string
  url: string | null
  status: string
  tool_count: number
  auth_type: string | null
  header_names: string[]
  protected: boolean
}

export interface McpServersResponse {
  agent: string
  servers: McpServerSummary[]
}

export interface McpDetectRequest {
  url: string
}

export interface McpDetectResponse {
  bare_url: string
  oauth_discovered: boolean
  recommended_mode: McpAuthMode
  reason: string
  oauth: {
    resource: string
    scopes: string[]
    authorization_endpoint: string
    token_endpoint: string
    registration_endpoint: string | null
  } | null
}

export interface McpHeaderInput {
  name: string
  value: string
}

export interface McpAddRequest {
  name: string
  url: string
  mode: McpAuthMode
  headers: McpHeaderInput[]
}

export interface McpHeadersRequest {
  headers: McpHeaderInput[]
}

export interface McpMutationResponse {
  ok: boolean
}

export interface McpOAuthStartResponse {
  auth_url: string
}
```

- [ ] **Step 2: Add API functions**

In `api.ts`, import the new types and add:

```ts
export function mcpServers(): Promise<McpServersResponse> {
  return requestJson<McpServersResponse>('api/v1/mcp/servers')
}

export function mcpDetect(url: string): Promise<McpDetectResponse> {
  const body: McpDetectRequest = { url }
  return requestJson<McpDetectResponse>('api/v1/mcp/detect', {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function mcpAdd(request: McpAddRequest): Promise<McpMutationResponse> {
  return requestJson<McpMutationResponse>('api/v1/mcp/servers', {
    method: 'POST',
    body: JSON.stringify(request),
  })
}

export function mcpSetHeaders(serverName: string, headers: McpHeaderInput[]): Promise<McpMutationResponse> {
  const body: McpHeadersRequest = { headers }
  return requestJson<McpMutationResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}/headers`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  })
}

export function mcpStartOAuth(serverName: string): Promise<McpOAuthStartResponse> {
  return requestJson<McpOAuthStartResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}/oauth/start`, {
    method: 'POST',
    body: JSON.stringify({}),
  })
}

export function mcpRemove(serverName: string): Promise<McpMutationResponse> {
  return requestJson<McpMutationResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}`, {
    method: 'DELETE',
  })
}
```

- [ ] **Step 3: Add deep-link helper and test**

In `crates/right-dashboard/frontend/src/format.ts`, add this pure helper:

```ts
export function initialDashboardTabFromLocation(search: string, hash: string): string {
  const params = new URLSearchParams(search)
  const queryView = params.get('view')
  if (queryView !== null && queryView.length > 0) {
    return queryView
  }
  const hashView = hash.replace(/^#/, '')
  return hashView.length > 0 ? hashView : 'overview'
}
```

In `crates/right-dashboard/frontend/src/telegram.test.ts`, add Vitest coverage:

```ts
import { describe, expect, it } from 'vitest'
import { initialDashboardTabFromLocation } from './format'

describe('initialDashboardTabFromLocation', () => {
  it('prefers query view', () => {
    expect(initialDashboardTabFromLocation('?view=mcp', '')).toBe('mcp')
  })

  it('uses hash view', () => {
    expect(initialDashboardTabFromLocation('', '#mcp')).toBe('mcp')
  })

  it('defaults to overview', () => {
    expect(initialDashboardTabFromLocation('', '')).toBe('overview')
  })
})
```

- [ ] **Step 4: Wire `mcp` tab in `App.vue`**

Update `DashboardTab`:

```ts
type DashboardTab = 'overview' | 'activity' | 'knowledge' | 'usage' | 'identity' | 'health' | 'mcp'
```

Initialize:

```ts
const activeTab = ref<DashboardTab>(normalizeInitialTab(initialDashboardTabFromLocation(window.location.search, window.location.hash)))
```

Add helper:

```ts
function normalizeInitialTab(tab: string): DashboardTab {
  return isDashboardTab(tab) ? tab : 'overview'
}
```

Update `isDashboardTab`:

```ts
return ['overview', 'activity', 'knowledge', 'usage', 'identity', 'health', 'mcp'].includes(tab)
```

Add tab entry:

```ts
{ key: 'mcp', label: 'MCP', enabled: true },
```

- [ ] **Step 5: Run frontend tests/typecheck**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend test
devenv shell -- npm --prefix crates/right-dashboard/frontend run typecheck
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/frontend/src/format.ts crates/right-dashboard/frontend/src/telegram.test.ts
devenv shell -- git commit -m "feat(dashboard): add MCP API bindings"
```

Only add files that changed.

---

### Task 11: Build The MCP Dashboard View

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/SecretInput.vue`
- Create: `crates/right-dashboard/frontend/src/views/McpView.vue`
- Modify: `crates/right-dashboard/frontend/src/App.vue`

- [ ] **Step 1: Create masked secret input**

Create `SecretInput.vue`:

```vue
<script setup lang="ts">
import { computed, ref } from 'vue'

const props = defineProps<{
  modelValue: string
  placeholder?: string
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const revealed = ref(false)
const inputType = computed(() => (revealed.value ? 'text' : 'password'))

function update(event: Event): void {
  const target = event.target as HTMLInputElement
  emit('update:modelValue', target.value)
}
</script>

<template>
  <div class="secret-input">
    <input
      class="text-input"
      :type="inputType"
      :value="modelValue"
      :placeholder="placeholder"
      :disabled="disabled"
      autocomplete="off"
      @input="update"
    >
    <button
      class="icon-button"
      type="button"
      :aria-label="revealed ? 'Hide value' : 'Show value'"
      :disabled="disabled"
      @click="revealed = !revealed"
    >
      {{ revealed ? 'Hide' : 'Show' }}
    </button>
  </div>
</template>

<style scoped>
.secret-input {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 6px;
}

.text-input {
  width: 100%;
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
}

.icon-button {
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
}
</style>
```

Use text labels for the first pass. Do not add a frontend dependency for icons.

- [ ] **Step 2: Create `McpView.vue` skeleton**

Create `McpView.vue`:

```vue
<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  mcpAdd,
  mcpDetect,
  mcpRemove,
  mcpServers,
  mcpSetHeaders,
  mcpStartOAuth,
} from '../api'
import type {
  McpAuthMode,
  McpDetectResponse,
  McpHeaderInput,
  McpServerSummary,
  McpServersResponse,
} from '../types'
import SecretInput from '../components/SecretInput.vue'

const servers = ref<McpServersResponse | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
const addOpen = ref(false)
const name = ref('')
const url = ref('')
const detection = ref<McpDetectResponse | null>(null)
const selectedMode = ref<McpAuthMode>('headers')
const headerRows = ref<McpHeaderInput[]>([{ name: 'Authorization', value: '' }])
const busyAction = ref<string | null>(null)

const recommendationLabel = computed(() => {
  if (detection.value === null) {
    return ''
  }
  if (detection.value.recommended_mode === 'oauth') {
    return 'OAuth'
  }
  if (detection.value.recommended_mode === 'url_as_is') {
    return 'URL as-is'
  }
  return 'Headers'
})

onMounted(() => {
  void refresh()
})

async function refresh(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    servers.value = await mcpServers()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'MCP unavailable'
  } finally {
    loading.value = false
  }
}

async function detect(): Promise<void> {
  error.value = null
  detection.value = await mcpDetect(url.value)
  selectedMode.value = detection.value.recommended_mode
}

function addHeaderRow(): void {
  headerRows.value = [...headerRows.value, { name: '', value: '' }]
}

function removeHeaderRow(index: number): void {
  headerRows.value = headerRows.value.filter((_, rowIndex) => rowIndex !== index)
}

async function saveServer(): Promise<void> {
  busyAction.value = 'add'
  error.value = null
  try {
    await mcpAdd({
      name: name.value,
      url: selectedMode.value === 'url_as_is' ? url.value : detection.value?.bare_url ?? url.value,
      mode: selectedMode.value,
      headers: selectedMode.value === 'headers' ? nonEmptyHeaders() : [],
    })
    resetAdd()
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to add MCP server'
  } finally {
    busyAction.value = null
  }
}

async function replaceHeaders(server: McpServerSummary): Promise<void> {
  busyAction.value = `headers:${server.name}`
  error.value = null
  try {
    await mcpSetHeaders(server.name, nonEmptyHeaders())
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to update headers'
  } finally {
    busyAction.value = null
  }
}

async function startOAuth(server: McpServerSummary): Promise<void> {
  busyAction.value = `oauth:${server.name}`
  error.value = null
  try {
    const response = await mcpStartOAuth(server.name)
    window.Telegram?.WebApp?.openLink?.(response.auth_url)
    if (!window.Telegram?.WebApp?.openLink) {
      window.location.href = response.auth_url
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to start OAuth'
  } finally {
    busyAction.value = null
  }
}

async function removeServer(server: McpServerSummary): Promise<void> {
  if (server.protected) {
    return
  }
  busyAction.value = `remove:${server.name}`
  error.value = null
  try {
    await mcpRemove(server.name)
    await refresh()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to remove MCP server'
  } finally {
    busyAction.value = null
  }
}

function nonEmptyHeaders(): McpHeaderInput[] {
  return headerRows.value
    .map((header) => ({ name: header.name.trim(), value: header.value }))
    .filter((header) => header.name.length > 0 && header.value.length > 0)
}

function resetAdd(): void {
  addOpen.value = false
  name.value = ''
  url.value = ''
  detection.value = null
  selectedMode.value = 'headers'
  headerRows.value = [{ name: 'Authorization', value: '' }]
}
</script>
```

- [ ] **Step 3: Add template**

In `McpView.vue`, add:

```vue
<template>
  <section class="panel">
    <div class="panel-head">
      <div>
        <h2>MCP</h2>
        <p class="muted-line">External servers</p>
      </div>
      <button class="tool-button" type="button" @click="addOpen = !addOpen">
        Add
      </button>
    </div>

    <section v-if="addOpen" class="section">
      <div class="form-grid">
        <label>
          <span class="label">Name</span>
          <input v-model="name" class="text-input" autocomplete="off">
        </label>
        <label>
          <span class="label">URL</span>
          <input v-model="url" class="text-input" autocomplete="off">
        </label>
      </div>
      <div class="button-row">
        <button class="tool-button" type="button" :disabled="!url" @click="detect">Detect</button>
      </div>

      <div v-if="detection" class="notice inline">
        <strong>Recommended: {{ recommendationLabel }}</strong>
        <span v-if="!detection.oauth_discovered">No OAuth metadata found.</span>
      </div>

      <div v-if="detection" class="segmented">
        <button class="segment-button" :class="{ active: selectedMode === 'oauth' }" type="button" @click="selectedMode = 'oauth'">OAuth</button>
        <button class="segment-button" :class="{ active: selectedMode === 'headers' }" type="button" @click="selectedMode = 'headers'">Headers</button>
        <button class="segment-button" :class="{ active: selectedMode === 'url_as_is' }" type="button" @click="selectedMode = 'url_as_is'">URL as-is</button>
      </div>

      <div v-if="selectedMode === 'headers'" class="header-editor">
        <div v-for="(header, index) in headerRows" :key="index" class="header-row">
          <input v-model="header.name" class="text-input" placeholder="Header name" autocomplete="off">
          <SecretInput v-model="header.value" placeholder="Header value" />
          <button class="tool-button" type="button" @click="removeHeaderRow(index)">Remove</button>
        </div>
        <button class="tool-button" type="button" @click="addHeaderRow">Add header</button>
      </div>

      <div class="button-row">
        <button class="tool-button" type="button" :disabled="!name || !url || busyAction === 'add'" @click="saveServer">
          Save
        </button>
        <button class="tool-button" type="button" @click="resetAdd">Cancel</button>
      </div>
    </section>

    <p v-if="error" class="notice inline">{{ error }}</p>
    <p v-if="loading" class="muted-line">Loading</p>

    <div v-if="servers" class="data-list">
      <article v-for="server in servers.servers" :key="server.name" class="data-row">
        <div class="row-main">
          <strong>{{ server.name }}</strong>
          <small>{{ server.url ?? 'built-in' }}</small>
          <small v-if="server.header_names.length">Headers: {{ server.header_names.join(', ') }}</small>
        </div>
        <div class="row-side">
          <span class="status-pill" :class="{ ok: server.status === 'connected', bad: server.status === 'needs_auth' || server.status === 'unreachable' }">
            {{ server.status }}
          </span>
          <small>{{ server.tool_count }} tools</small>
          <small>{{ server.auth_type ?? 'built-in' }}</small>
        </div>
        <div class="button-row">
          <button v-if="server.auth_type === 'oauth' || server.status === 'needs_auth'" class="tool-button" type="button" @click="startOAuth(server)">
            Authenticate
          </button>
          <button v-if="!server.protected" class="tool-button" type="button" @click="replaceHeaders(server)">
            Save headers
          </button>
          <button v-if="!server.protected" class="tool-button" type="button" @click="removeServer(server)">
            Remove
          </button>
        </div>
      </article>
    </div>
  </section>
</template>
```

- [ ] **Step 4: Add local styles**

Append scoped styles in `McpView.vue`:

```vue
<style scoped>
.form-grid,
.header-editor,
.button-row,
.data-list {
  display: grid;
  gap: 8px;
}

.form-grid {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.header-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr) auto;
  gap: 6px;
  align-items: center;
}

.text-input {
  width: 100%;
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
}

@media (max-width: 680px) {
  .form-grid,
  .header-row {
    grid-template-columns: 1fr;
  }
}
</style>
```

- [ ] **Step 5: Wire view in `App.vue`**

Import:

```ts
import McpView from './views/McpView.vue'
```

Render before Health fallback:

```vue
<McpView v-else-if="activeTab === 'mcp'" />
```

- [ ] **Step 6: Run frontend verification**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend test
devenv shell -- npm --prefix crates/right-dashboard/frontend run typecheck
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS and dashboard generated assets update under `crates/right-dashboard/static/dashboard/`.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/components/SecretInput.vue crates/right-dashboard/frontend/src/views/McpView.vue crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "feat(dashboard): add MCP management view"
```

---

### Task 12: Update Docs And User-Facing References

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify: `PROMPT_SYSTEM.md` if it references removed `/mcp` subcommands
- Modify: other files found by the search below

- [ ] **Step 1: Find stale `/mcp` command references**

Run:

```bash
devenv shell -- rg -n \"/mcp (add|auth|remove|list)|MCP Auth Types|HeaderName: token|Run <code>/mcp auth\" ARCHITECTURE.md PROMPT_SYSTEM.md docs crates
```

Expected: references are listed. Only update current docs/prompts/source comments that describe user-facing behavior. Historical plan/spec files can remain unchanged.

- [ ] **Step 2: Update `ARCHITECTURE.md` MCP auth section**

Replace the current auth type intro with:

```markdown
Dashboard MCP management runs URL-first detection, then asks the user to choose
`OAuth`, `Headers`, or `URL as-is`. The heuristic is a recommendation; the
user's dashboard choice is authoritative. Telegram `/mcp` opens the dashboard
MCP view and has no management subcommands.
```

Update the auth table rows to include:

```markdown
| `headers` | Multiple configured HTTP headers | User chooses `Headers`; values are write-only and redacted from list/detail APIs |
```

Update the security rule to say management is through authenticated Telegram
Mini App dashboard routes routed through the internal Unix socket API.

- [ ] **Step 3: Update `docs/architecture/mcp.md`**

Replace the "MCP Auth Choice Flow" section with a dashboard version that states:

```markdown
`/mcp` opens the Telegram Mini App dashboard on the MCP view. The add flow is
URL-first: the dashboard collects server name + URL, runs detection, then shows
OAuth, Headers, and URL as-is choices. Detection is advisory.

No upstream MCP server is registered until the user saves a chosen mode.
Header values are write-only secrets; list APIs return names only.
```

Update the internal REST API list to include `/mcp-set-headers`.

- [ ] **Step 4: Update `docs/architecture/lifecycle.md`**

In the bot UDS server bullet, add that dashboard serves authenticated MCP APIs.
In the command scope bullet, state `/mcp` opens the dashboard MCP view.

- [ ] **Step 5: Run docs reference search again**

Run:

```bash
devenv shell -- rg -n \"/mcp (add|auth|remove|list)|Run <code>/mcp auth\" ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture crates
```

Expected: no current user-facing references instruct users to use removed Telegram subcommands. Historical `docs/superpowers/` files may still match if included accidentally; do not edit historical plans.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/mcp.md docs/architecture/lifecycle.md PROMPT_SYSTEM.md
devenv shell -- git commit -m "docs(mcp): document dashboard control surface"
```

Only add `PROMPT_SYSTEM.md` if it changed.

---

### Task 13: Integration Checks And Final Verification

**Files:**
- Verify all touched files.

- [ ] **Step 1: Run targeted Rust test suites**

Run:

```bash
devenv shell -- cargo test -p right-db migrations::tests::v35_mcp_http_headers_table
devenv shell -- cargo test -p right-mcp oauth::tests::discover_as_skips_html_protected_resource_response
devenv shell -- cargo test -p right-mcp detect::tests
devenv shell -- cargo test -p right-mcp credentials::db_tests::db_set_http_headers
devenv shell -- cargo test -p right-mcp proxy::tests::dynamic_auth_headers_injects_multiple_headers
devenv shell -- cargo test -p right --lib internal_api::tests::mcp_add_headers_auth_redacts_values_in_list
devenv shell -- cargo test -p rightclaw-bot --lib telegram::dashboard::tests::dashboard_mcp
```

Expected: PASS.

- [ ] **Step 2: Run frontend checks**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend test
devenv shell -- npm --prefix crates/right-dashboard/frontend run typecheck
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS. Generated dashboard assets are updated and staged if changed.

- [ ] **Step 3: Run formatting and package checks**

Run:

```bash
devenv shell -- cargo fmt --check
devenv shell -- cargo check --workspace
```

Expected: PASS.

- [ ] **Step 4: Run final mandatory workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If an ignored external integration test is listed as ignored, that is acceptable. Any non-ignored failure must be investigated before completion.

- [ ] **Step 5: Final docs/assets commit**

Check for generated asset or formatting changes:

```bash
devenv shell -- git status --short
```

When `crates/right-dashboard/static/dashboard/` is dirty, commit those assets:

```bash
devenv shell -- git add crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "build(dashboard): refresh MCP assets"
```

When no generated assets changed, record that result in the final handoff and
do not create an empty commit.

- [ ] **Step 6: Final status**

Run:

```bash
devenv shell -- git status --short
```

Expected: clean worktree, except any explicitly pre-existing unrelated user changes from the starting worktree.
