use std::io::Write as _;
use std::path::Path;

use serde_json::json;
use tempfile::NamedTempFile;

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

/// Read and parse .mcp.json. Returns empty object if file absent.
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

/// Add an HTTP MCP server to `.mcp.json` under `mcpServers.<name>`.
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

/// Remove an HTTP MCP server from `.mcp.json` under `mcpServers.<name>`.
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

/// List all HTTP MCP servers from `.mcp.json`.
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

/// Set a custom header on an HTTP MCP server entry in `.mcp.json`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["type"], "http");
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
    }

    #[test]
    fn add_merges_into_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { "rightmemory": { "type": "http", "url": "http://localhost:8100/mcp" } }
        })).unwrap()).unwrap();
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
        assert_eq!(content["mcpServers"]["rightmemory"]["url"], "http://localhost:8100/mcp");
    }

    #[test]
    fn remove_deletes_named_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
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
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        let err = remove_http_server(&path, "nonexistent").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn list_returns_sorted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
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
        let path = dir.path().join("nonexistent.mcp.json");
        let servers = list_http_servers(&path).unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn set_header_adds_authorization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        set_server_header(&path, "notion", "Authorization", "Bearer tok-abc").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["headers"]["Authorization"], "Bearer tok-abc");
    }

    #[test]
    fn set_header_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
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
