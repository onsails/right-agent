use std::io::Write as _;
use std::path::Path;

use rusqlite::Connection;
use serde_json::json;
use tempfile::NamedTempFile;
use url::Url;

/// Reserved server names that cannot be registered.
const RESERVED_NAMES: &[&str] = &["right", "rightmeta"];

/// Error type for credential operations.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error on credentials file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("server '{0}' not found in mcpServers")]
    ServerNotFound(String),
    #[error("credentials file parent directory not found")]
    InvalidPath,
    #[error("atomic write failed: {0}")]
    Persist(#[from] tempfile::PersistError),
    #[error("invalid server name: {0}")]
    InvalidServerName(String),
    #[error("invalid server URL: {0}")]
    InvalidServerUrl(String),
}

/// Atomically write JSON value to path using same-dir NamedTempFile + rename.
pub(crate) fn write_json_atomic(
    path: &Path,
    value: &serde_json::Value,
) -> Result<(), CredentialError> {
    let content = serde_json::to_string_pretty(value)?;
    let dir = path.parent().ok_or(CredentialError::InvalidPath)?;
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.persist(path)?;
    Ok(())
}

/// Read and parse mcp.json. Returns empty object if file absent.
fn read_mcp_json(path: &Path) -> Result<serde_json::Value, CredentialError> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = std::fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;
    Ok(root)
}

/// Ensure `mcpServers` object exists at root, return mutable ref to root.
fn ensure_mcp_servers(root: &mut serde_json::Value) -> Result<(), CredentialError> {
    root.as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    Ok(())
}

/// Add an HTTP MCP server to `mcp.json` under `mcpServers.<name>`.
///
/// Creates the file and structure if absent. Atomic read-modify-write via tempfile.
pub fn add_http_server(
    mcp_json_path: &Path,
    server_name: &str,
    url: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;
    ensure_mcp_servers(&mut root)?;

    root["mcpServers"]
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .insert(
            server_name.to_string(),
            json!({ "type": "http", "url": url }),
        );

    write_json_atomic(mcp_json_path, &root)
}

/// Remove an HTTP MCP server from `mcp.json` under `mcpServers.<name>`.
///
/// Returns `CredentialError::ServerNotFound` if the server entry does not exist.
pub fn remove_http_server(
    mcp_json_path: &Path,
    server_name: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;

    let removed = root
        .get_mut("mcpServers")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.remove(server_name));

    if removed.is_none() {
        return Err(CredentialError::ServerNotFound(server_name.to_string()));
    }

    write_json_atomic(mcp_json_path, &root)
}

/// List all HTTP MCP servers from `mcp.json`.
///
/// Returns vec of `(name, url)` pairs sorted by name. Returns empty vec if file or
/// `mcpServers` is absent.
pub fn list_http_servers(
    mcp_json_path: &Path,
) -> Result<Vec<(String, String)>, CredentialError> {
    let root = read_mcp_json(mcp_json_path)?;

    let servers = match root.get("mcpServers").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    let mut result: Vec<(String, String)> = servers
        .iter()
        .filter_map(|(name, entry)| {
            let url = entry.get("url")?.as_str()?;
            Some((name.clone(), url.to_string()))
        })
        .collect();

    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// Set a custom header on an HTTP MCP server entry in `mcp.json`.
///
/// The server entry must already exist (call `add_http_server` first).
/// Headers are stored under `mcpServers.<name>.headers.<header_name>`.
pub fn set_server_header(
    mcp_json_path: &Path,
    server_name: &str,
    header_name: &str,
    header_value: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;

    let server = root
        .get_mut("mcpServers")
        .and_then(|s| s.get_mut(server_name))
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;

    let headers = server
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .entry("headers")
        .or_insert_with(|| json!({}));

    headers
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .insert(header_name.to_string(), json!(header_value));

    write_json_atomic(mcp_json_path, &root)
}

// ---------------------------------------------------------------------------
// SQLite-based server registry
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` into `CredentialError::Io`.
fn map_db_err(e: rusqlite::Error) -> CredentialError {
    CredentialError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{e:#}")))
}

/// Validate an MCP server name.
///
/// Rejects empty names, reserved names (`right`, `rightmeta`), and names
/// containing `__` (double underscore — reserved for internal namespacing).
pub fn validate_server_name(name: &str) -> Result<(), CredentialError> {
    if name.is_empty() {
        return Err(CredentialError::InvalidServerName(
            "server name must not be empty".to_string(),
        ));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(CredentialError::InvalidServerName(format!(
            "'{name}' is a reserved server name"
        )));
    }
    if name.contains("__") {
        return Err(CredentialError::InvalidServerName(format!(
            "'{name}' must not contain '__'"
        )));
    }
    Ok(())
}

/// Validate an MCP server URL.
///
/// Requires HTTPS scheme. Rejects private/loopback/link-local IP addresses
/// and `localhost`.
pub fn validate_server_url(url_str: &str) -> Result<(), CredentialError> {
    let parsed = Url::parse(url_str)
        .map_err(|e| CredentialError::InvalidServerUrl(format!("invalid URL: {e}")))?;

    if parsed.scheme() != "https" {
        return Err(CredentialError::InvalidServerUrl(format!(
            "only HTTPS URLs are allowed, got '{}'",
            parsed.scheme()
        )));
    }

    let url_host = parsed
        .host()
        .ok_or_else(|| CredentialError::InvalidServerUrl("URL has no host".to_string()))?;

    match url_host {
        url::Host::Domain(domain) => {
            if domain == "localhost" {
                return Err(CredentialError::InvalidServerUrl(
                    "localhost is not allowed".to_string(),
                ));
            }
        }
        url::Host::Ipv4(v4) => {
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() {
                return Err(CredentialError::InvalidServerUrl(format!(
                    "private/loopback IP address '{v4}' is not allowed"
                )));
            }
        }
        url::Host::Ipv6(v6) => {
            if v6.is_loopback() {
                return Err(CredentialError::InvalidServerUrl(format!(
                    "loopback IP address '{v6}' is not allowed"
                )));
            }
        }
    }

    Ok(())
}

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

/// Register (or update) an external MCP server in the SQLite registry.
pub fn db_add_server(
    conn: &Connection,
    name: &str,
    url: &str,
) -> Result<(), CredentialError> {
    validate_server_name(name)?;
    validate_server_url(url)?;

    conn.execute(
        "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET url = excluded.url",
        rusqlite::params![name, url],
    )
    .map_err(map_db_err)?;

    Ok(())
}

/// Remove an external MCP server from the SQLite registry.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub fn db_remove_server(conn: &Connection, name: &str) -> Result<(), CredentialError> {
    let rows = conn
        .execute(
            "DELETE FROM mcp_servers WHERE name = ?1",
            rusqlite::params![name],
        )
        .map_err(map_db_err)?;

    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// Update the instructions for an external MCP server in the SQLite registry.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
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
        .map_err(map_db_err)?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// List all registered external MCP servers, sorted by name.
pub fn db_list_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, url, instructions, auth_type, auth_header, auth_token, \
             refresh_token, token_endpoint, client_id, client_secret, expires_at \
             FROM mcp_servers ORDER BY name",
        )
        .map_err(map_db_err)?;

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
        .map_err(map_db_err)?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(map_db_err)?);
    }
    Ok(result)
}

/// Update auth fields for an MCP server.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub fn db_set_auth(
    conn: &Connection,
    name: &str,
    auth_type: &str,
    auth_header: Option<&str>,
    auth_token: Option<&str>,
) -> Result<(), CredentialError> {
    let changed = conn
        .execute(
            "UPDATE mcp_servers SET auth_type = ?1, auth_header = ?2, auth_token = ?3 WHERE name = ?4",
            rusqlite::params![auth_type, auth_header, auth_token, name],
        )
        .map_err(map_db_err)?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// Set full OAuth state for an MCP server.
///
/// Sets `auth_type` to `"oauth"`. Returns `CredentialError::ServerNotFound` if
/// no matching row exists.
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
    let changed = conn
        .execute(
            "UPDATE mcp_servers SET auth_type = 'oauth', auth_token = ?1, refresh_token = ?2, \
             token_endpoint = ?3, client_id = ?4, client_secret = ?5, expires_at = ?6 \
             WHERE name = ?7",
            rusqlite::params![
                access_token,
                refresh_token,
                token_endpoint,
                client_id,
                client_secret,
                expires_at,
                name
            ],
        )
        .map_err(map_db_err)?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// Update just the access token and expiry for an OAuth MCP server (used by
/// the refresh scheduler).
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub fn db_update_oauth_token(
    conn: &Connection,
    name: &str,
    access_token: &str,
    expires_at: &str,
) -> Result<(), CredentialError> {
    let changed = conn
        .execute(
            "UPDATE mcp_servers SET auth_token = ?1, expires_at = ?2 WHERE name = ?3",
            rusqlite::params![access_token, expires_at, name],
        )
        .map_err(map_db_err)?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// List OAuth servers that have a refresh token (candidates for token refresh).
pub fn db_list_oauth_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, url, instructions, auth_type, auth_header, auth_token, \
             refresh_token, token_endpoint, client_id, client_secret, expires_at \
             FROM mcp_servers \
             WHERE auth_type = 'oauth' AND refresh_token IS NOT NULL \
             ORDER BY name",
        )
        .map_err(map_db_err)?;

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
        .map_err(map_db_err)?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(map_db_err)?);
    }
    Ok(result)
}

/// Redact query parameters from a URL.
///
/// If the URL contains a `?`, returns `scheme://host/path?<redacted>`.
/// Otherwise returns the URL as-is.
pub fn redact_url(url: &str) -> String {
    match url.find('?') {
        Some(idx) => format!("{}?<redacted>", &url[..idx]),
        None => url.to_string(),
    }
}

/// Check whether a URL is a valid public HTTPS URL (not localhost/private IP).
pub fn is_public_url(url: &str) -> bool {
    validate_server_url(url).is_ok()
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::memory::migrations::MIGRATIONS;

    fn setup_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn add_and_list_servers() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_add_server(&conn, "linear", "https://mcp.linear.app/mcp").unwrap();

        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "linear");
        assert_eq!(servers[0].url, "https://mcp.linear.app/mcp");
        assert_eq!(servers[1].name, "notion");
        assert_eq!(servers[1].url, "https://mcp.notion.com/mcp");
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
        let err = db_remove_server(&conn, "ghost").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn upsert_server() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://old.notion.com/mcp").unwrap();
        db_add_server(&conn, "notion", "https://new.notion.com/mcp").unwrap();

        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].url, "https://new.notion.com/mcp");
    }

    #[test]
    fn validate_server_name_valid() {
        validate_server_name("notion").unwrap();
        validate_server_name("my-server").unwrap();
        validate_server_name("server_one").unwrap();
    }

    #[test]
    fn validate_server_name_reserved() {
        assert!(matches!(
            validate_server_name("right"),
            Err(CredentialError::InvalidServerName(_))
        ));
        assert!(matches!(
            validate_server_name("rightmeta"),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[test]
    fn validate_server_name_double_underscore() {
        assert!(matches!(
            validate_server_name("my__server"),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[test]
    fn validate_server_name_empty() {
        assert!(matches!(
            validate_server_name(""),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[test]
    fn validate_server_url_https_ok() {
        validate_server_url("https://mcp.notion.com/mcp").unwrap();
    }

    #[test]
    fn validate_server_url_http_rejected() {
        assert!(matches!(
            validate_server_url("http://mcp.notion.com/mcp"),
            Err(CredentialError::InvalidServerUrl(_))
        ));
    }

    #[test]
    fn validate_server_url_private_ips_rejected() {
        // RFC1918
        assert!(validate_server_url("https://192.168.1.1/mcp").is_err());
        assert!(validate_server_url("https://10.0.0.1/mcp").is_err());
        assert!(validate_server_url("https://172.16.0.1/mcp").is_err());
        // Loopback
        assert!(validate_server_url("https://127.0.0.1/mcp").is_err());
        // Link-local
        assert!(validate_server_url("https://169.254.1.1/mcp").is_err());
        // localhost
        assert!(validate_server_url("https://localhost/mcp").is_err());
        // IPv6 loopback
        assert!(validate_server_url("https://[::1]/mcp").is_err());
    }

    #[test]
    fn update_and_list_instructions() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert!(servers[0].instructions.is_none());

        db_update_instructions(&conn, "notion", Some("Use Notion tools")).unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers[0].instructions.as_deref(), Some("Use Notion tools"));

        db_update_instructions(&conn, "notion", None).unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert!(servers[0].instructions.is_none());
    }

    #[test]
    fn upsert_preserves_instructions() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://old.notion.com/mcp").unwrap();
        db_update_instructions(&conn, "notion", Some("Notion instructions")).unwrap();

        db_add_server(&conn, "notion", "https://new.notion.com/mcp").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers[0].url, "https://new.notion.com/mcp");
        assert_eq!(
            servers[0].instructions.as_deref(),
            Some("Notion instructions")
        );
    }

    #[test]
    fn update_instructions_nonexistent_server() {
        let conn = setup_db();
        let err = db_update_instructions(&conn, "ghost", Some("instructions")).unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn db_add_server_with_auth() {
        let conn = setup_db();
        db_add_server(&conn, "test", "https://example.com/mcp").unwrap();
        db_set_auth(&conn, "test", "bearer", None, Some("sk-123")).unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].auth_type.as_deref(), Some("bearer"));
        assert_eq!(servers[0].auth_token.as_deref(), Some("sk-123"));
    }

    #[test]
    fn db_set_auth_nonexistent_server() {
        let conn = setup_db();
        let err = db_set_auth(&conn, "ghost", "bearer", None, Some("tok")).unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn db_set_oauth_state_test() {
        let conn = setup_db();
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
    }

    #[test]
    fn db_update_oauth_token_test() {
        let conn = setup_db();
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "old",
            Some("rt"),
            "https://ex.com/token",
            "c",
            None,
            "2026-04-13T12:00:00Z",
        )
        .unwrap();
        db_update_oauth_token(&conn, "notion", "new-tok", "2026-04-13T13:00:00Z").unwrap();
        let servers = db_list_servers(&conn).unwrap();
        assert_eq!(servers[0].auth_token.as_deref(), Some("new-tok"));
        assert_eq!(
            servers[0].expires_at.as_deref(),
            Some("2026-04-13T13:00:00Z")
        );
    }

    #[test]
    fn db_list_oauth_servers_test() {
        let conn = setup_db();
        db_add_server(&conn, "oauth-srv", "https://a.com/mcp").unwrap();
        db_set_oauth_state(
            &conn,
            "oauth-srv",
            "tok",
            Some("rt"),
            "https://a.com/token",
            "c",
            None,
            "2026-04-13T12:00:00Z",
        )
        .unwrap();
        db_add_server(&conn, "bearer-srv", "https://b.com/mcp").unwrap();
        db_set_auth(&conn, "bearer-srv", "bearer", None, Some("key")).unwrap();
        let oauth = db_list_oauth_servers(&conn).unwrap();
        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].name, "oauth-srv");
    }

    #[test]
    fn redact_url_strips_query() {
        assert_eq!(
            redact_url("https://example.com/mcp?key=secret&foo=bar"),
            "https://example.com/mcp?<redacted>"
        );
        assert_eq!(
            redact_url("https://example.com/mcp"),
            "https://example.com/mcp"
        );
    }

    #[test]
    fn is_public_url_test() {
        assert!(is_public_url("https://mcp.notion.com/mcp"));
        assert!(!is_public_url("https://localhost/mcp"));
        assert!(!is_public_url("https://192.168.1.1/mcp"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["type"], "http");
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
    }

    #[test]
    fn add_merges_into_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { "right": { "type": "http", "url": "http://localhost:8100/mcp" } }
        })).unwrap()).unwrap();
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
        assert_eq!(content["mcpServers"]["right"]["url"], "http://localhost:8100/mcp");
    }

    #[test]
    fn remove_deletes_named_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        add_http_server(&path, "linear", "https://mcp.linear.app/mcp").unwrap();
        remove_http_server(&path, "notion").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content["mcpServers"]["notion"].is_null());
        assert_eq!(content["mcpServers"]["linear"]["url"], "https://mcp.linear.app/mcp");
    }

    #[test]
    fn remove_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let err = remove_http_server(&path, "nonexistent").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn list_returns_sorted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "zebra", "https://zebra.example.com/mcp").unwrap();
        add_http_server(&path, "apple", "https://apple.example.com/mcp").unwrap();
        let servers = list_http_servers(&path).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].0, "apple");
        assert_eq!(servers[1].0, "zebra");
    }

    #[test]
    fn list_returns_empty_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent-mcp.json");
        let servers = list_http_servers(&path).unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn set_header_adds_authorization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        set_server_header(&path, "notion", "Authorization", "Bearer tok-abc").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["headers"]["Authorization"], "Bearer tok-abc");
    }

    #[test]
    fn set_header_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "{}").unwrap();
        let err = set_server_header(&path, "ghost", "Authorization", "Bearer x").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn atomic_write_works() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = serde_json::json!({"key": "value"});
        write_json_atomic(&path, &value).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["key"], "value");
    }
}
