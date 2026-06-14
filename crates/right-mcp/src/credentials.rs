use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use http::{HeaderName, HeaderValue};
use right_db::{Connection, DbError, OptionalExtension, params};
use serde_json::json;
use tempfile::NamedTempFile;
use url::Url;

/// Reserved server names that cannot be registered.
const RESERVED_NAMES: &[&str] = &[crate::PROTECTED_MCP_SERVER, crate::RIGHT_META_NAMESPACE];

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
    #[error("invalid auth header: {0}")]
    InvalidAuthHeader(String),
    #[error("invalid auth type: {0}")]
    InvalidAuthType(String),
}

impl From<DbError> for CredentialError {
    fn from(e: DbError) -> Self {
        CredentialError::Io(std::io::Error::other(format!("{e:#}")))
    }
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
pub fn remove_http_server(mcp_json_path: &Path, server_name: &str) -> Result<(), CredentialError> {
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
pub fn list_http_servers(mcp_json_path: &Path) -> Result<Vec<(String, String)>, CredentialError> {
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

/// Secret value for one HTTP header credential.
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
        let name = canonical_header_name(&name)?;
        if value.is_empty() {
            return Err(CredentialError::InvalidAuthHeader(
                "header value must not be empty".to_string(),
            ));
        }
        HeaderValue::from_str(&value)
            .map_err(|_| CredentialError::InvalidAuthHeader("invalid header value".to_string()))?;
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
    canonical_header_name(name).map(drop)
}

fn canonical_header_name(name: &str) -> Result<String, CredentialError> {
    HeaderName::from_bytes(name.as_bytes())
        .map(|name| name.to_string())
        .map_err(|e| CredentialError::InvalidAuthHeader(format!("invalid header name: {e}")))
}

// ---------------------------------------------------------------------------
// SQLite-based server registry
// ---------------------------------------------------------------------------

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

fn parse_url(url_str: &str) -> Result<Url, CredentialError> {
    Url::parse(url_str).map_err(|e| CredentialError::InvalidServerUrl(format!("invalid URL: {e}")))
}

fn is_loopback_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => crate::ssrf::is_localhost_domain(domain),
        url::Host::Ipv4(v4) => v4.is_loopback(),
        url::Host::Ipv6(v6) => {
            v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()) || v6.is_loopback()
        }
    }
}

fn is_private_or_link_local_ipv4(ip: Ipv4Addr) -> bool {
    // link-local + the operator private-LAN families (RFC1918 + CGNAT) from ssrf,
    // so the detection gate agrees with the AllowPrivate connect tier.
    ip.is_link_local() || crate::ssrf::is_user_private_lan(IpAddr::V4(ip))
}

fn is_private_or_link_local_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_or_link_local_ipv4(v4);
    }
    // ULA (fc00::/7) classification shares ssrf's canonical predicate; link-local
    // (fe80::/10) stays on the stdlib check.
    crate::ssrf::is_user_private_lan(IpAddr::V6(ip)) || ip.is_unicast_link_local()
}

fn is_private_or_link_local_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(_) => false,
        url::Host::Ipv4(v4) => is_private_or_link_local_ipv4(*v4),
        url::Host::Ipv6(v6) => is_private_or_link_local_ipv6(*v6),
    }
}

/// Validate an explicitly registered MCP server URL.
///
/// Servers may use HTTP or HTTPS. Operator-supplied private LAN / Tailscale /
/// ULA addresses are allowed (`AllowPrivate` tier). Link-local and
/// cloud-metadata addresses are always rejected; loopback is allowed (operator's
/// own machine).
pub fn validate_server_url(url_str: &str) -> Result<(), CredentialError> {
    let parsed = parse_url(url_str)?;

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(CredentialError::InvalidServerUrl(format!(
            "only HTTP/HTTPS URLs are allowed, got '{}'",
            parsed.scheme()
        )));
    }

    let url_host = parsed
        .host()
        .ok_or_else(|| CredentialError::InvalidServerUrl("URL has no host".to_string()))?;

    // Operator-supplied base URL: allow public + private LAN/Tailscale/ULA +
    // loopback (for local dev MCP servers). Only link-local and cloud-metadata
    // remain blocked (AllowPrivate tier).
    let allowed = match url_host {
        url::Host::Domain(_) => true,
        url::Host::Ipv4(v4) => {
            crate::ssrf::ip_allowed(IpAddr::V4(v4), crate::ssrf::NetworkPolicy::AllowPrivate)
        }
        url::Host::Ipv6(v6) => {
            crate::ssrf::ip_allowed(IpAddr::V6(v6), crate::ssrf::NetworkPolicy::AllowPrivate)
        }
    };
    if !allowed {
        return Err(CredentialError::InvalidServerUrl(format!(
            "address '{}' is not allowed (link-local or cloud-metadata)",
            parsed.host_str().unwrap_or("<unknown>")
        )));
    }

    Ok(())
}

/// Entry returned by `db_list_servers`.
#[derive(Debug, Clone, Default)]
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
    pub oauth_resource: Option<String>,
}

/// Returns true if a server with the given name is registered.
pub async fn db_server_exists(conn: &Connection, name: &str) -> Result<bool, CredentialError> {
    let row: Option<i64> = conn
        .query_one("SELECT 1 FROM mcp_servers WHERE name = ?1", [name], |row| {
            row.get(0)
        })
        .await
        .optional()?;
    Ok(row.is_some())
}

/// Register (or update) an external MCP server in the SQLite registry.
pub async fn db_add_server(
    conn: &Connection,
    name: &str,
    url: &str,
) -> Result<(), CredentialError> {
    validate_server_name(name)?;
    validate_server_url(url)?;

    conn.execute(
        "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET url = excluded.url",
        (name, url),
    )
    .await?;

    Ok(())
}

/// Remove an external MCP server from the SQLite registry.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub async fn db_remove_server(conn: &Connection, name: &str) -> Result<(), CredentialError> {
    let rows = conn
        .execute("DELETE FROM mcp_servers WHERE name = ?1", [name])
        .await?;

    if rows == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// Update the instructions for an external MCP server in the SQLite registry.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub async fn db_update_instructions(
    conn: &Connection,
    name: &str,
    instructions: Option<&str>,
) -> Result<(), CredentialError> {
    let changed = conn
        .execute(
            "UPDATE mcp_servers SET instructions = ?1 WHERE name = ?2",
            (instructions, name),
        )
        .await?;
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// Shared SELECT columns for server queries.
const SERVER_COLUMNS: &str = "name, url, instructions, auth_type, auth_header, auth_token, \
    refresh_token, token_endpoint, client_id, client_secret, expires_at, oauth_resource";

fn server_entry_from_row(row: &right_db::row::Row<'_>) -> Result<McpServerEntry, DbError> {
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
        oauth_resource: row.get(11)?,
    })
}

async fn query_server_entries(
    conn: &Connection,
    sql: &str,
    query_params: impl right_db::params::IntoParams,
) -> Result<Vec<McpServerEntry>, CredentialError> {
    Ok(conn
        .query_all(sql, query_params, server_entry_from_row)
        .await?)
}

/// List all registered external MCP servers, sorted by name.
pub async fn db_list_servers(conn: &Connection) -> Result<Vec<McpServerEntry>, CredentialError> {
    query_server_entries(
        conn,
        &format!("SELECT {SERVER_COLUMNS} FROM mcp_servers ORDER BY name"),
        (),
    )
    .await
}

/// Update auth fields for an MCP server.
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub async fn db_set_auth(
    conn: &Connection,
    name: &str,
    auth_type: &str,
    auth_header: Option<&str>,
    auth_token: Option<&str>,
) -> Result<(), CredentialError> {
    if auth_type == "headers" {
        return Err(CredentialError::InvalidAuthType(
            "'headers' auth must be configured with db_set_http_headers".to_string(),
        ));
    }

    let tx = conn.transaction().await?;
    let result: Result<(), CredentialError> = async {
        let changed = tx
            .execute(
                "UPDATE mcp_servers
             SET auth_type = ?1,
                 auth_header = ?2,
                 auth_token = ?3,
                 refresh_token = NULL,
                 token_endpoint = NULL,
                 client_id = NULL,
                 client_secret = NULL,
                 expires_at = NULL,
                 oauth_resource = NULL
             WHERE name = ?4",
                params![auth_type, auth_header, auth_token, name],
            )
            .await?;
        if changed == 0 {
            return Err(CredentialError::ServerNotFound(name.to_string()));
        }

        if auth_type != "headers" {
            tx.execute(
                "DELETE FROM mcp_http_headers WHERE server_name = ?1",
                [name],
            )
            .await?;
        }

        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(err) => {
            if let Err(rollback_err) = tx.rollback().await {
                tracing::warn!(
                    operation_error = format!("{err:#}"),
                    rollback_error = format!("{rollback_err:#}"),
                    "mcp auth transaction rollback failed; returning original operation error",
                );
            }
            Err(err)
        }
    }
}

/// Clear all stored authentication fields for an MCP server.
///
/// Used when a server is switched back to URL-as-is mode. This must delete
/// multi-header rows as well as legacy single-header and OAuth fields so stale
/// secrets cannot reappear in list output or after restart.
pub async fn db_clear_auth(conn: &Connection, name: &str) -> Result<(), CredentialError> {
    let tx = conn.transaction().await?;
    let result: Result<(), CredentialError> = async {
        let changed = tx
            .execute(
                "UPDATE mcp_servers
             SET auth_type = NULL,
                 auth_header = NULL,
                 auth_token = NULL,
                 refresh_token = NULL,
                 token_endpoint = NULL,
                 client_id = NULL,
                 client_secret = NULL,
                 expires_at = NULL,
                 oauth_resource = NULL
             WHERE name = ?1",
                [name],
            )
            .await?;
        if changed == 0 {
            return Err(CredentialError::ServerNotFound(name.to_string()));
        }

        tx.execute(
            "DELETE FROM mcp_http_headers WHERE server_name = ?1",
            [name],
        )
        .await?;

        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(err) => {
            if let Err(rollback_err) = tx.rollback().await {
                tracing::warn!(
                    operation_error = format!("{err:#}"),
                    rollback_error = format!("{rollback_err:#}"),
                    "mcp auth clear transaction rollback failed; returning original operation error",
                );
            }
            Err(err)
        }
    }
}

/// Replace all stored HTTP header credentials for an MCP server.
///
/// Header values are stored only for proxy injection. Use
/// [`db_list_http_header_names`] for user-facing output.
pub async fn db_set_http_headers(
    conn: &Connection,
    server_name: &str,
    headers: &[HttpHeaderSecret],
) -> Result<(), CredentialError> {
    let tx = conn.transaction().await?;
    let result: Result<(), CredentialError> = async {
        let changed = tx
            .execute(
                "UPDATE mcp_servers
             SET auth_type = 'headers',
                 auth_header = NULL,
                 auth_token = NULL,
                 refresh_token = NULL,
                 token_endpoint = NULL,
                 client_id = NULL,
                 client_secret = NULL,
                 expires_at = NULL,
                 oauth_resource = NULL
             WHERE name = ?1",
                [server_name],
            )
            .await?;
        if changed == 0 {
            return Err(CredentialError::ServerNotFound(server_name.to_string()));
        }

        tx.execute(
            "DELETE FROM mcp_http_headers WHERE server_name = ?1",
            [server_name],
        )
        .await?;
        for header in headers {
            tx.execute(
                "INSERT INTO mcp_http_headers (server_name, header_name, header_value)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(server_name, header_name) DO UPDATE SET
                    header_value = excluded.header_value,
                    updated_at = datetime('now')",
                params![server_name, header.name(), header.value()],
            )
            .await?;
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(err) => {
            if let Err(rollback_err) = tx.rollback().await {
                tracing::warn!(
                    operation_error = format!("{err:#}"),
                    rollback_error = format!("{rollback_err:#}"),
                    "mcp http header transaction rollback failed; returning original operation error",
                );
            }
            Err(err)
        }
    }
}

/// List stored HTTP header names for a server without exposing values.
pub async fn db_list_http_header_names(
    conn: &Connection,
    server_name: &str,
) -> Result<Vec<String>, CredentialError> {
    Ok(conn
        .query_all(
            "SELECT header_name FROM mcp_http_headers WHERE server_name = ?1 ORDER BY header_name",
            [server_name],
            |row| row.get(0),
        )
        .await?)
}

/// List stored HTTP header names for every server in one query.
///
/// Returns `(server_name, header_name)` rows ordered by server, then header.
pub async fn db_list_all_http_header_names(
    conn: &Connection,
) -> Result<Vec<(String, String)>, CredentialError> {
    Ok(conn
        .query_all(
            "SELECT server_name, header_name FROM mcp_http_headers
         ORDER BY server_name, header_name",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .await?)
}

/// List stored HTTP header credentials for proxy injection.
pub async fn db_list_http_headers(
    conn: &Connection,
    server_name: &str,
) -> Result<Vec<HttpHeaderSecret>, CredentialError> {
    let rows: Vec<(String, String)> = conn
        .query_all(
            "SELECT header_name, header_value FROM mcp_http_headers
         WHERE server_name = ?1
         ORDER BY header_name",
            [server_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .await?;

    rows.into_iter()
        .map(|(name, value)| HttpHeaderSecret::new(name, value))
        .collect()
}

/// Set full OAuth state for an MCP server.
///
/// Sets `auth_type` to `"oauth"` and stores the current access token plus
/// refresh metadata. Empty access tokens are stored as NULL. Returns
/// `CredentialError::ServerNotFound` if no matching row exists.
#[allow(clippy::too_many_arguments)]
pub async fn db_set_oauth_state(
    conn: &Connection,
    name: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    expires_at: &str,
    oauth_resource: &str,
) -> Result<(), CredentialError> {
    let auth_token = (!access_token.is_empty()).then_some(access_token);
    let tx = conn.transaction().await?;
    let result: Result<(), CredentialError> = async {
        let changed = tx
            .execute(
                "UPDATE mcp_servers SET auth_type = 'oauth', auth_header = NULL, auth_token = ?1, \
             refresh_token = ?2, token_endpoint = ?3, client_id = ?4, client_secret = ?5, \
             expires_at = ?6, oauth_resource = ?7 WHERE name = ?8",
                params![
                    auth_token,
                    refresh_token,
                    token_endpoint,
                    client_id,
                    client_secret,
                    expires_at,
                    oauth_resource,
                    name
                ],
            )
            .await?;
        if changed == 0 {
            return Err(CredentialError::ServerNotFound(name.to_string()));
        }

        tx.execute(
            "DELETE FROM mcp_http_headers WHERE server_name = ?1",
            [name],
        )
        .await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(err) => {
            if let Err(rollback_err) = tx.rollback().await {
                tracing::warn!(
                    operation_error = format!("{err:#}"),
                    rollback_error = format!("{rollback_err:#}"),
                    "mcp oauth transaction rollback failed; returning original operation error",
                );
            }
            Err(err)
        }
    }
}

/// Update just the access token and expiry for an OAuth MCP server (used by
/// the refresh scheduler).
///
/// Returns `CredentialError::ServerNotFound` if no matching row exists.
pub async fn db_update_oauth_token(
    conn: &Connection,
    name: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: &str,
) -> Result<(), CredentialError> {
    let changed = if let Some(rt) = refresh_token {
        conn.execute(
            "UPDATE mcp_servers SET auth_token = ?1, refresh_token = ?2, expires_at = ?3 WHERE name = ?4",
            (access_token, rt, expires_at, name),
        )
        .await?
    } else {
        conn.execute(
            "UPDATE mcp_servers SET auth_token = ?1, expires_at = ?2 WHERE name = ?3",
            (access_token, expires_at, name),
        )
        .await?
    };
    if changed == 0 {
        return Err(CredentialError::ServerNotFound(name.to_string()));
    }
    Ok(())
}

/// List OAuth servers that have a refresh token (candidates for token refresh).
pub async fn db_list_oauth_servers(
    conn: &Connection,
) -> Result<Vec<McpServerEntry>, CredentialError> {
    query_server_entries(
        conn,
        &format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers \
             WHERE auth_type = 'oauth' AND refresh_token IS NOT NULL \
             ORDER BY name"
        ),
        (),
    )
    .await
}

/// Save an auth token, replacing any existing one.
pub async fn save_auth_token(conn: &Connection, token: &str) -> Result<(), CredentialError> {
    let tx = conn.transaction().await?;
    tx.execute("DELETE FROM auth_tokens", ()).await?;
    tx.execute("INSERT INTO auth_tokens (token) VALUES (?1)", [token])
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Get the stored auth token, if any.
pub async fn get_auth_token(conn: &Connection) -> Result<Option<String>, CredentialError> {
    Ok(conn
        .query_one("SELECT token FROM auth_tokens LIMIT 1", (), |row| {
            row.get(0)
        })
        .await
        .optional()?)
}

/// Delete the stored auth token.
pub async fn delete_auth_token(conn: &Connection) -> Result<(), CredentialError> {
    conn.execute("DELETE FROM auth_tokens", ()).await?;
    Ok(())
}

/// Per-agent platform-notice authentication token. Generated once and stored;
/// stable for the agent's lifetime. Used to make `⟨⟨SYSTEM_NOTICE:<token>⟩⟩`
/// unforgeable by untrusted content the agent reads.
pub async fn get_or_create_notice_token(conn: &Connection) -> Result<String, CredentialError> {
    if let Some(existing) = conn
        .query_one("SELECT token FROM notice_token LIMIT 1", (), |row| {
            row.get(0)
        })
        .await
        .optional()?
    {
        return Ok(existing);
    }
    let token = generate_notice_token();
    let tx = conn.transaction().await?;
    tx.execute("DELETE FROM notice_token", ()).await?;
    tx.execute(
        "INSERT INTO notice_token (token) VALUES (?1)",
        [token.as_str()],
    )
    .await?;
    tx.commit().await?;
    Ok(token)
}

/// 128-bit token as 32 lowercase hex chars.
fn generate_notice_token() -> String {
    use rand::RngExt as _;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

/// Check whether a URL points at localhost or a loopback IP address.
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = parse_url(url) else {
        return false;
    };
    parsed.host().is_some_and(|host| is_loopback_host(&host))
}

/// Check whether a URL is a network-routable HTTP(S) URL (not localhost/private IP).
pub fn is_public_url(url: &str) -> bool {
    let Ok(parsed) = parse_url(url) else {
        return false;
    };

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return false;
    }

    parsed
        .host()
        .is_some_and(|host| !is_loopback_host(&host) && !is_private_or_link_local_host(&host))
}

#[cfg(test)]
#[path = "credentials_auth_token_tests.rs"]
mod auth_token_tests;

#[cfg(test)]
mod db_tests {
    use super::*;

    async fn setup_db() -> (tempfile::TempDir, Connection) {
        right_db::test_support::migrated_connection().await
    }

    #[tokio::test]
    async fn add_and_list_servers() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_add_server(&conn, "linear", "https://mcp.linear.app/mcp")
            .await
            .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "linear");
        assert_eq!(servers[0].url, "https://mcp.linear.app/mcp");
        assert_eq!(servers[1].name, "notion");
        assert_eq!(servers[1].url, "https://mcp.notion.com/mcp");
    }

    #[tokio::test]
    async fn remove_server() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_remove_server(&conn, "notion").await.unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn remove_nonexistent_server() {
        let (_dir, conn) = setup_db().await;
        let err = db_remove_server(&conn, "ghost").await.unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn upsert_server() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://old.notion.com/mcp")
            .await
            .unwrap();
        db_add_server(&conn, "notion", "https://new.notion.com/mcp")
            .await
            .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].url, "https://new.notion.com/mcp");
    }

    #[tokio::test]
    async fn validate_server_name_valid() {
        validate_server_name("notion").unwrap();
        validate_server_name("my-server").unwrap();
        validate_server_name("server_one").unwrap();
    }

    #[tokio::test]
    async fn validate_server_name_reserved() {
        assert!(matches!(
            validate_server_name("right"),
            Err(CredentialError::InvalidServerName(_))
        ));
        assert!(matches!(
            validate_server_name("rightmeta"),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[tokio::test]
    async fn validate_server_name_double_underscore() {
        assert!(matches!(
            validate_server_name("my__server"),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[tokio::test]
    async fn validate_server_name_empty() {
        assert!(matches!(
            validate_server_name(""),
            Err(CredentialError::InvalidServerName(_))
        ));
    }

    #[tokio::test]
    async fn validate_server_url_https_ok() {
        validate_server_url("https://mcp.notion.com/mcp").unwrap();
    }

    #[tokio::test]
    async fn validate_server_url_plain_http_ok() {
        validate_server_url("http://mcp.notion.com/mcp").unwrap();
    }

    #[tokio::test]
    async fn validate_server_url_rejects_non_http_schemes() {
        assert!(matches!(
            validate_server_url("ftp://mcp.notion.com/mcp"),
            Err(CredentialError::InvalidServerUrl(_))
        ));
    }

    #[tokio::test]
    async fn validate_server_url_rejects_link_local_and_metadata() {
        // link-local and cloud-metadata are still rejected
        assert!(validate_server_url("https://169.254.1.1/mcp").is_err());
        assert!(validate_server_url("http://169.254.169.254/mcp").is_err());

        // RFC1918 ranges are allowed (operator LAN base URLs)
        assert!(validate_server_url("https://10.0.0.1/mcp").is_ok());
        assert!(validate_server_url("https://192.168.1.1/mcp").is_ok());
        assert!(validate_server_url("https://172.16.0.1/mcp").is_ok());

        // loopback and localhost are now allowed (local dev MCP servers)
        assert!(validate_server_url("http://127.0.0.1:3333/mcp").is_ok());
        assert!(validate_server_url("http://[::1]:3333/mcp").is_ok());
        assert!(validate_server_url("http://localhost:3333/mcp").is_ok());
        assert!(validate_server_url("https://localhost/mcp").is_ok());
    }

    #[tokio::test]
    async fn validate_server_url_ipv4_mapped_ipv6() {
        // RFC1918 via IPv4-mapped IPv6 are allowed (AllowPrivate tier)
        assert!(validate_server_url("https://[::ffff:192.168.1.1]/mcp").is_ok());
        assert!(validate_server_url("https://[::ffff:10.0.0.1]/mcp").is_ok());
        assert!(validate_server_url("https://[::ffff:172.16.0.1]/mcp").is_ok());
        // link-local via IPv4-mapped IPv6 is still rejected
        assert!(validate_server_url("https://[::ffff:169.254.1.1]/mcp").is_err());
        // loopback via IPv4-mapped IPv6 is now allowed
        assert!(validate_server_url("https://[::ffff:127.0.0.1]/mcp").is_ok());
    }

    #[tokio::test]
    async fn update_and_list_instructions() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert!(servers[0].instructions.is_none());

        db_update_instructions(&conn, "notion", Some("Use Notion tools"))
            .await
            .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers[0].instructions.as_deref(), Some("Use Notion tools"));

        db_update_instructions(&conn, "notion", None).await.unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert!(servers[0].instructions.is_none());
    }

    #[tokio::test]
    async fn upsert_preserves_instructions() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://old.notion.com/mcp")
            .await
            .unwrap();
        db_update_instructions(&conn, "notion", Some("Notion instructions"))
            .await
            .unwrap();

        db_add_server(&conn, "notion", "https://new.notion.com/mcp")
            .await
            .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers[0].url, "https://new.notion.com/mcp");
        assert_eq!(
            servers[0].instructions.as_deref(),
            Some("Notion instructions")
        );
    }

    #[tokio::test]
    async fn update_instructions_nonexistent_server() {
        let (_dir, conn) = setup_db().await;
        let err = db_update_instructions(&conn, "ghost", Some("instructions"))
            .await
            .unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn db_add_server_with_auth() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "test", "https://example.com/mcp")
            .await
            .unwrap();
        db_set_auth(&conn, "test", "bearer", None, Some("sk-123"))
            .await
            .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].auth_type.as_deref(), Some("bearer"));
        assert_eq!(servers[0].auth_token.as_deref(), Some("sk-123"));
    }

    #[tokio::test]
    async fn db_set_auth_nonexistent_server() {
        let (_dir, conn) = setup_db().await;
        let err = db_set_auth(&conn, "ghost", "bearer", None, Some("tok"))
            .await
            .unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn db_set_auth_rejects_raw_headers_auth_type_without_mutating_row() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();

        let err = db_set_auth(&conn, "nango", "headers", Some("X-Api-Key"), Some("secret"))
            .await
            .unwrap_err();
        assert!(matches!(
            &err,
            CredentialError::InvalidAuthType(message) if message.contains("headers")
        ));
        assert!(err.to_string().contains("headers"));
        assert!(
            !err.to_string().contains("secret"),
            "error must not expose credential values"
        );

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert!(server.auth_type.is_none());
        assert!(server.auth_header.is_none());
        assert!(server.auth_token.is_none());

        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM mcp_http_headers WHERE server_name = ?1",
                ["nango"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn db_set_auth_to_bearer_clears_http_headers() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();
        db_set_http_headers(
            &conn,
            "nango",
            &[HttpHeaderSecret::new("authorization", "Bearer env-secret").unwrap()],
        )
        .await
        .unwrap();

        db_set_auth(&conn, "nango", "bearer", None, Some("Bearer legacy"))
            .await
            .unwrap();

        assert!(
            db_list_http_headers(&conn, "nango")
                .await
                .unwrap()
                .is_empty()
        );
        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM mcp_http_headers WHERE server_name = ?1",
                ["nango"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn db_clear_auth_removes_headers_and_oauth_fields() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "nango",
            "access-tok",
            Some("refresh-tok"),
            "https://api.nango.dev/oauth/token",
            "client-id",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://api.nango.dev/mcp",
        )
        .await
        .unwrap();
        db_set_http_headers(
            &conn,
            "nango",
            &[HttpHeaderSecret::new("Authorization", "Bearer secret").unwrap()],
        )
        .await
        .unwrap();

        db_clear_auth(&conn, "nango").await.unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert!(server.auth_type.is_none());
        assert!(server.auth_header.is_none());
        assert!(server.auth_token.is_none());
        assert!(server.refresh_token.is_none());
        assert!(server.token_endpoint.is_none());
        assert!(server.client_id.is_none());
        assert!(server.client_secret.is_none());
        assert!(server.expires_at.is_none());
        assert!(server.oauth_resource.is_none());
        assert!(
            db_list_http_headers(&conn, "nango")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn db_clear_auth_rejects_missing_server() {
        let (_dir, conn) = setup_db().await;
        let err = db_clear_auth(&conn, "missing").await.unwrap_err();

        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn db_set_auth_to_header_clears_http_headers() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();
        db_set_http_headers(
            &conn,
            "nango",
            &[HttpHeaderSecret::new("authorization", "Bearer env-secret").unwrap()],
        )
        .await
        .unwrap();

        db_set_auth(
            &conn,
            "nango",
            "header",
            Some("authorization"),
            Some("Bearer legacy"),
        )
        .await
        .unwrap();

        assert!(
            db_list_http_headers(&conn, "nango")
                .await
                .unwrap()
                .is_empty()
        );
        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM mcp_http_headers WHERE server_name = ?1",
                ["nango"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn db_set_auth_to_bearer_clears_stale_oauth_fields() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_http_headers (server_name, header_name, header_value)
             VALUES (?1, ?2, ?3)",
            params!["notion", "authorization", "Bearer stale"],
        )
        .await
        .unwrap();

        db_set_auth(&conn, "notion", "bearer", None, Some("Bearer legacy"))
            .await
            .unwrap();

        assert!(
            db_list_http_headers(&conn, "notion")
                .await
                .unwrap()
                .is_empty()
        );
        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM mcp_http_headers WHERE server_name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert_eq!(server.auth_type.as_deref(), Some("bearer"));
        assert!(server.auth_header.is_none());
        assert_eq!(server.auth_token.as_deref(), Some("Bearer legacy"));
        assert!(server.refresh_token.is_none());
        assert!(server.token_endpoint.is_none());
        assert!(server.client_id.is_none());
        assert!(server.client_secret.is_none());
        assert!(server.expires_at.is_none());
        assert!(server.oauth_resource.is_none());
    }

    #[test]
    fn http_header_secret_canonicalizes_header_name() {
        let header = HttpHeaderSecret::new("Authorization", "Bearer x").unwrap();

        assert_eq!(header.name(), "authorization");
    }

    #[tokio::test]
    async fn db_set_http_headers_replaces_values_and_lists_names_only() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();
        db_set_auth(
            &conn,
            "nango",
            "bearer",
            Some("Authorization"),
            Some("Bearer legacy"),
        )
        .await
        .unwrap();

        db_set_http_headers(
            &conn,
            "nango",
            &[
                HttpHeaderSecret::new("Authorization", "Bearer env-secret").unwrap(),
                HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
                HttpHeaderSecret::new("provider-config-key", "github").unwrap(),
            ],
        )
        .await
        .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers[0].auth_type.as_deref(), Some("headers"));
        assert!(servers[0].auth_header.is_none());
        assert!(servers[0].auth_token.is_none());

        let names = db_list_http_header_names(&conn, "nango").await.unwrap();
        assert_eq!(
            names,
            vec!["authorization", "connection-id", "provider-config-key"]
        );

        let secrets = db_list_http_headers(&conn, "nango").await.unwrap();
        assert_eq!(secrets.len(), 3);
        assert_eq!(secrets[0].name(), "authorization");
        assert_eq!(secrets[0].value(), "Bearer env-secret");

        db_set_http_headers(
            &conn,
            "nango",
            &[HttpHeaderSecret::new("connection-id", "conn_replaced").unwrap()],
        )
        .await
        .unwrap();
        let names = db_list_http_header_names(&conn, "nango").await.unwrap();
        assert_eq!(names, vec!["connection-id"]);
        let secrets = db_list_http_headers(&conn, "nango").await.unwrap();
        assert_eq!(secrets[0].value(), "conn_replaced");
    }

    #[tokio::test]
    async fn db_set_http_headers_clears_stale_oauth_fields() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();

        db_set_http_headers(
            &conn,
            "notion",
            &[HttpHeaderSecret::new("authorization", "Bearer env-secret").unwrap()],
        )
        .await
        .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert_eq!(server.auth_type.as_deref(), Some("headers"));
        assert!(server.auth_header.is_none());
        assert!(server.auth_token.is_none());
        assert!(server.refresh_token.is_none());
        assert!(server.token_endpoint.is_none());
        assert!(server.client_id.is_none());
        assert!(server.client_secret.is_none());
        assert!(server.expires_at.is_none());
        assert!(server.oauth_resource.is_none());
    }

    #[tokio::test]
    async fn db_set_http_headers_canonicalizes_duplicate_case_names() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "nango", "https://api.nango.dev/mcp")
            .await
            .unwrap();

        db_set_http_headers(
            &conn,
            "nango",
            &[
                HttpHeaderSecret::new("Authorization", "Bearer first").unwrap(),
                HttpHeaderSecret::new("authorization", "Bearer second").unwrap(),
            ],
        )
        .await
        .unwrap();

        let secrets = db_list_http_headers(&conn, "nango").await.unwrap();
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].name(), "authorization");
        assert_eq!(secrets[0].value(), "Bearer second");
    }

    #[test]
    fn db_set_http_headers_rejects_bad_header_name() {
        let err = HttpHeaderSecret::new("bad header", "secret").unwrap_err();
        assert!(matches!(err, CredentialError::InvalidAuthHeader(_)));
    }

    #[test]
    fn db_set_http_headers_rejects_bad_header_value() {
        let err = HttpHeaderSecret::new("authorization", "Bearer good\nInjected: bad").unwrap_err();

        assert!(matches!(err, CredentialError::InvalidAuthHeader(_)));
        assert!(
            !err.to_string().contains("Bearer good"),
            "error must not expose header value"
        );
    }

    #[tokio::test]
    async fn db_set_http_headers_rejects_missing_server() {
        let (_dir, conn) = setup_db().await;

        let err = db_set_http_headers(
            &conn,
            "ghost",
            &[HttpHeaderSecret::new("Authorization", "Bearer x").unwrap()],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn db_set_oauth_state_test() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            None,
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        let s = &servers[0];
        assert_eq!(s.auth_type.as_deref(), Some("oauth"));
        assert!(s.auth_header.is_none());
        assert_eq!(s.auth_token.as_deref(), Some("access-tok"));
        assert_eq!(s.refresh_token.as_deref(), Some("refresh-tok"));
        assert_eq!(
            s.oauth_resource.as_deref(),
            Some("https://mcp.notion.com/mcp")
        );
    }

    #[tokio::test]
    async fn db_set_oauth_state_clears_legacy_single_header_auth() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_auth(&conn, "notion", "header", Some("X-Api-Key"), Some("secret"))
            .await
            .unwrap();

        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert_eq!(server.auth_type.as_deref(), Some("oauth"));
        assert!(server.auth_header.is_none());
        assert_eq!(server.auth_token.as_deref(), Some("access-tok"));
        assert_eq!(server.refresh_token.as_deref(), Some("refresh-tok"));
        assert_eq!(
            server.token_endpoint.as_deref(),
            Some("https://accounts.notion.com/oauth/token")
        );
        assert_eq!(server.client_id.as_deref(), Some("client-123"));
        assert_eq!(server.client_secret.as_deref(), Some("client-secret"));
        assert_eq!(server.expires_at.as_deref(), Some("2026-04-13T12:00:00Z"));
        assert_eq!(
            server.oauth_resource.as_deref(),
            Some("https://mcp.notion.com/mcp")
        );
    }

    #[tokio::test]
    async fn db_set_oauth_state_clears_http_headers() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_http_headers(
            &conn,
            "notion",
            &[HttpHeaderSecret::new("authorization", "Bearer env-secret").unwrap()],
        )
        .await
        .unwrap();

        db_set_oauth_state(
            &conn,
            "notion",
            "access-tok",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();

        assert!(
            db_list_http_headers(&conn, "notion")
                .await
                .unwrap()
                .is_empty()
        );
        let count: i64 = conn
            .query_one(
                "SELECT COUNT(*) FROM mcp_http_headers WHERE server_name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(count, 0);

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert_eq!(server.auth_type.as_deref(), Some("oauth"));
        assert!(server.auth_header.is_none());
        assert_eq!(server.auth_token.as_deref(), Some("access-tok"));
        assert_eq!(server.refresh_token.as_deref(), Some("refresh-tok"));
        assert_eq!(
            server.oauth_resource.as_deref(),
            Some("https://mcp.notion.com/mcp")
        );
    }

    #[tokio::test]
    async fn db_set_oauth_state_empty_access_token_stores_null() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();

        db_set_oauth_state(
            &conn,
            "notion",
            "",
            Some("refresh-tok"),
            "https://accounts.notion.com/oauth/token",
            "client-123",
            Some("client-secret"),
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();

        let servers = db_list_servers(&conn).await.unwrap();
        let server = &servers[0];
        assert_eq!(server.auth_type.as_deref(), Some("oauth"));
        assert!(server.auth_token.is_none());
        assert_eq!(server.refresh_token.as_deref(), Some("refresh-tok"));
    }

    #[tokio::test]
    async fn db_update_oauth_token_test() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "old",
            Some("rt"),
            "https://ex.com/token",
            "c",
            None,
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();
        db_update_oauth_token(
            &conn,
            "notion",
            "new-tok",
            Some("rt2"),
            "2026-04-13T13:00:00Z",
        )
        .await
        .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers[0].auth_token.as_deref(), Some("new-tok"));
        assert_eq!(servers[0].refresh_token.as_deref(), Some("rt2"));
        assert_eq!(
            servers[0].expires_at.as_deref(),
            Some("2026-04-13T13:00:00Z")
        );
    }

    #[tokio::test]
    async fn db_update_oauth_token_keeps_old_refresh_when_none() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "notion", "https://mcp.notion.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "notion",
            "old",
            Some("rt-original"),
            "https://ex.com/token",
            "c",
            None,
            "2026-04-13T12:00:00Z",
            "https://mcp.notion.com/mcp",
        )
        .await
        .unwrap();
        // Pass None — should keep "rt-original"
        db_update_oauth_token(&conn, "notion", "new-tok", None, "2026-04-13T13:00:00Z")
            .await
            .unwrap();
        let servers = db_list_servers(&conn).await.unwrap();
        assert_eq!(servers[0].refresh_token.as_deref(), Some("rt-original"));
    }

    #[tokio::test]
    async fn db_list_oauth_servers_test() {
        let (_dir, conn) = setup_db().await;
        db_add_server(&conn, "oauth-srv", "https://a.com/mcp")
            .await
            .unwrap();
        db_set_oauth_state(
            &conn,
            "oauth-srv",
            "tok",
            Some("rt"),
            "https://a.com/token",
            "c",
            None,
            "2026-04-13T12:00:00Z",
            "https://a.com/mcp",
        )
        .await
        .unwrap();
        db_add_server(&conn, "bearer-srv", "https://b.com/mcp")
            .await
            .unwrap();
        db_set_auth(&conn, "bearer-srv", "bearer", None, Some("key"))
            .await
            .unwrap();
        let oauth = db_list_oauth_servers(&conn).await.unwrap();
        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].name, "oauth-srv");
        assert_eq!(
            oauth[0].oauth_resource.as_deref(),
            Some("https://a.com/mcp")
        );
    }

    #[tokio::test]
    async fn notice_token_is_stable_and_generated_once() {
        let (_dir, conn) = setup_db().await;
        let t1 = get_or_create_notice_token(&conn).await.unwrap();
        let t2 = get_or_create_notice_token(&conn).await.unwrap();
        assert_eq!(t1, t2, "token must be stable across calls");
        assert_eq!(t1.len(), 32, "token is 32 hex chars");
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn redact_url_strips_query() {
        assert_eq!(
            redact_url("https://example.com/mcp?key=secret&foo=bar"),
            "https://example.com/mcp?<redacted>"
        );
        assert_eq!(
            redact_url("https://example.com/mcp"),
            "https://example.com/mcp"
        );
    }

    #[tokio::test]
    async fn is_public_url_accepts_network_routable_http_and_https_urls() {
        assert!(is_public_url("https://mcp.notion.com/mcp"));
        assert!(is_public_url("http://mcp.notion.com/mcp"));
        assert!(!is_public_url("http://localhost:3333/mcp"));
        assert!(!is_public_url("https://localhost/mcp"));
        assert!(!is_public_url("https://192.168.1.1/mcp"));
        // RFC 6598 CGNAT (Tailscale) must be treated as private, not public.
        assert!(!is_public_url("http://100.85.147.49:27123/mcp"));
    }

    #[tokio::test]
    async fn is_public_url_rejects_ipv4_mapped_private_ipv6() {
        assert!(!is_public_url("https://[::ffff:192.168.1.1]/mcp"));
        assert!(!is_public_url("https://[::ffff:10.0.0.1]/mcp"));
        assert!(!is_public_url("https://[::ffff:169.254.1.1]/mcp"));
        assert!(!is_public_url("https://[::ffff:127.0.0.1]/mcp"));
    }

    #[tokio::test]
    async fn is_loopback_url_detects_localhost_and_loopback_ips() {
        assert!(is_loopback_url("http://localhost:3333/mcp"));
        assert!(is_loopback_url("https://127.0.0.1/mcp"));
        assert!(is_loopback_url("http://[::1]:3333/mcp"));
        assert!(!is_loopback_url("https://mcp.notion.com/mcp"));
        assert!(!is_loopback_url("https://192.168.1.1/mcp"));
    }

    #[tokio::test]
    async fn localhost_with_trailing_dot_is_loopback_not_public() {
        assert!(is_loopback_url("http://localhost.:3333/mcp"));
        assert!(!is_public_url("https://localhost./mcp"));
    }

    #[test]
    fn validate_server_url_allows_private_loopback_and_tailscale() {
        validate_server_url("http://openclaw.owl-skate.ts.net:27123/mcp").unwrap();
        validate_server_url("http://100.85.147.49:27123/mcp").unwrap();
        validate_server_url("http://192.168.1.10:8080/mcp").unwrap();
        validate_server_url("http://10.0.0.5/mcp").unwrap();
        validate_server_url("http://127.0.0.1:8080/mcp").unwrap();
        validate_server_url("http://localhost:8080/mcp").unwrap();
    }

    #[test]
    fn validate_server_url_rejects_link_local_metadata_and_bad_scheme() {
        for url in [
            "http://169.254.169.254/latest/meta-data",
            "http://100.100.100.200/latest/meta-data",
            "http://[fd00:ec2::254]/mcp",
            "ftp://example.com/mcp",
        ] {
            assert!(validate_server_url(url).is_err(), "{url} must be rejected");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn add_creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["type"], "http");
        assert_eq!(
            content["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
    }

    #[tokio::test]
    async fn add_merges_into_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": { "right": { "type": "http", "url": "http://localhost:8100/mcp" } }
            }))
            .unwrap(),
        )
        .unwrap();
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
        assert_eq!(
            content["mcpServers"]["right"]["url"],
            "http://localhost:8100/mcp"
        );
    }

    #[tokio::test]
    async fn remove_deletes_named_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        add_http_server(&path, "linear", "https://mcp.linear.app/mcp").unwrap();
        remove_http_server(&path, "notion").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content["mcpServers"]["notion"].is_null());
        assert_eq!(
            content["mcpServers"]["linear"]["url"],
            "https://mcp.linear.app/mcp"
        );
    }

    #[tokio::test]
    async fn remove_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let err = remove_http_server(&path, "nonexistent").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn list_returns_sorted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "zebra", "https://zebra.example.com/mcp").unwrap();
        add_http_server(&path, "apple", "https://apple.example.com/mcp").unwrap();
        let servers = list_http_servers(&path).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].0, "apple");
        assert_eq!(servers[1].0, "zebra");
    }

    #[tokio::test]
    async fn list_returns_empty_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent-mcp.json");
        let servers = list_http_servers(&path).unwrap();
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn set_header_adds_authorization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        set_server_header(&path, "notion", "Authorization", "Bearer tok-abc").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer tok-abc"
        );
    }

    #[tokio::test]
    async fn set_header_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "{}").unwrap();
        let err = set_server_header(&path, "ghost", "Authorization", "Bearer x").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[tokio::test]
    async fn atomic_write_works() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = serde_json::json!({"key": "value"});
        write_json_atomic(&path, &value).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["key"], "value");
    }

    #[test]
    fn is_private_or_link_local_ipv6_matches_ula_and_link_local() {
        let v6 = |s: &str| s.parse::<std::net::Ipv6Addr>().unwrap();
        // ULA fc00::/7
        assert!(is_private_or_link_local_ipv6(v6("fc00::1")));
        assert!(is_private_or_link_local_ipv6(v6("fdff:ffff::1")));
        // link-local fe80::/10
        assert!(is_private_or_link_local_ipv6(v6("fe80::1")));
        // ipv4-mapped private folds through
        assert!(is_private_or_link_local_ipv6(v6("::ffff:10.0.0.1")));
        // public stays public
        assert!(!is_private_or_link_local_ipv6(v6("2001:4860:4860::8888")));
    }
}
