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

/// Read and parse .claude.json. Returns empty object if file absent.
fn read_claude_json(path: &Path) -> Result<serde_json::Value, CredentialError> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = std::fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;
    Ok(root)
}

/// Add an HTTP MCP server to .claude.json under `projects.<agent_path_key>.mcpServers.<name>`.
///
/// Creates the entire structure if absent. Atomic read-modify-write via tempfile.
/// CC reads this at startup and handles OAuth natively for `type: "http"` entries.
pub fn add_http_server_to_claude_json(
    claude_json_path: &Path,
    agent_path_key: &str,
    server_name: &str,
    url: &str,
) -> Result<(), CredentialError> {
    let mut root = read_claude_json(claude_json_path)?;

    let root_obj = root
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?;

    let projects = root_obj
        .entry("projects")
        .or_insert_with(|| json!({}));

    let project = projects
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .entry(agent_path_key)
        .or_insert_with(|| json!({}));

    let mcp_servers = project
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));

    mcp_servers
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .insert(
            server_name.to_string(),
            json!({ "type": "http", "url": url }),
        );

    write_json_atomic(claude_json_path, &root)
}

/// Remove an HTTP MCP server from .claude.json under `projects.<agent_path_key>.mcpServers.<name>`.
///
/// Returns `CredentialError::ServerNotFound` if the server entry does not exist.
pub fn remove_http_server_from_claude_json(
    claude_json_path: &Path,
    agent_path_key: &str,
    server_name: &str,
) -> Result<(), CredentialError> {
    let mut root = read_claude_json(claude_json_path)?;

    let removed = root
        .get_mut("projects")
        .and_then(|p| p.get_mut(agent_path_key))
        .and_then(|proj| proj.get_mut("mcpServers"))
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.remove(server_name));

    if removed.is_none() {
        return Err(CredentialError::ServerNotFound(server_name.to_string()));
    }

    write_json_atomic(claude_json_path, &root)
}

/// List all HTTP MCP servers from .claude.json for the given project key.
///
/// Returns vec of (name, url) pairs. Returns empty vec if file/project/mcpServers absent.
pub fn list_http_servers_from_claude_json(
    claude_json_path: &Path,
    agent_path_key: &str,
) -> Result<Vec<(String, String)>, CredentialError> {
    let root = read_claude_json(claude_json_path)?;

    let servers = match root
        .get("projects")
        .and_then(|p| p.get(agent_path_key))
        .and_then(|proj| proj.get("mcpServers"))
        .and_then(|s| s.as_object())
    {
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

/// Set custom headers on an HTTP MCP server entry in .claude.json.
///
/// The server entry must already exist (call `add_http_server_to_claude_json` first).
/// Headers are stored under `mcpServers.<name>.headers.<header_name>`.
/// This is the CC convention for attaching Bearer tokens to HTTP MCP servers.
pub fn set_server_header(
    claude_json_path: &Path,
    agent_path_key: &str,
    server_name: &str,
    header_name: &str,
    header_value: &str,
) -> Result<(), CredentialError> {
    let mut root = read_claude_json(claude_json_path)?;

    let server = root
        .get_mut("projects")
        .and_then(|p| p.get_mut(agent_path_key))
        .and_then(|proj| proj.get_mut("mcpServers"))
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

    write_json_atomic(claude_json_path, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- add_http_server_to_claude_json tests ---

    #[test]
    fn add_creates_claude_json_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");

        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["type"],
            "http"
        );
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
    }

    #[test]
    fn add_merges_into_existing_claude_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        // Pre-populate with existing data
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "hasCompletedOnboarding": true,
                "projects": {
                    "/agents/bot": {
                        "hasTrustDialogAccepted": true,
                        "mcpServers": {
                            "linear": { "type": "http", "url": "https://mcp.linear.app/mcp" }
                        }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // New server added
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
        // Existing server preserved
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["linear"]["url"],
            "https://mcp.linear.app/mcp"
        );
        // Other fields preserved
        assert_eq!(content["hasCompletedOnboarding"], true);
        assert_eq!(
            content["projects"]["/agents/bot"]["hasTrustDialogAccepted"],
            true
        );
    }

    // --- remove_http_server_from_claude_json tests ---

    #[test]
    fn remove_deletes_named_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();
        add_http_server_to_claude_json(&path, "/agents/bot", "linear", "https://mcp.linear.app/mcp")
            .unwrap();

        remove_http_server_from_claude_json(&path, "/agents/bot", "notion").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content["projects"]["/agents/bot"]["mcpServers"]["notion"].is_null());
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["linear"]["url"],
            "https://mcp.linear.app/mcp"
        );
    }

    #[test]
    fn remove_returns_server_not_found_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();

        let err = remove_http_server_from_claude_json(&path, "/agents/bot", "nonexistent")
            .unwrap_err();
        assert!(
            matches!(err, CredentialError::ServerNotFound(_)),
            "expected ServerNotFound, got: {err:?}"
        );
    }

    // --- list_http_servers_from_claude_json tests ---

    #[test]
    fn list_returns_servers_sorted_by_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        add_http_server_to_claude_json(&path, "/agents/bot", "zebra", "https://zebra.example.com/mcp")
            .unwrap();
        add_http_server_to_claude_json(&path, "/agents/bot", "apple", "https://apple.example.com/mcp")
            .unwrap();

        let servers = list_http_servers_from_claude_json(&path, "/agents/bot").unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].0, "apple");
        assert_eq!(servers[1].0, "zebra");
    }

    #[test]
    fn list_returns_empty_when_file_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.claude.json");
        let servers = list_http_servers_from_claude_json(&path, "/agents/bot").unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn list_returns_empty_when_no_servers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, "{}").unwrap();
        let servers = list_http_servers_from_claude_json(&path, "/agents/bot").unwrap();
        assert!(servers.is_empty());
    }

    // --- set_server_header tests ---

    #[test]
    fn set_header_adds_authorization_to_existing_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();

        set_server_header(&path, "/agents/bot", "notion", "Authorization", "Bearer tok-abc")
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer tok-abc"
        );
        // URL preserved
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
    }

    #[test]
    fn set_header_returns_server_not_found_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, "{}").unwrap();

        let err = set_server_header(&path, "/agents/bot", "ghost", "Authorization", "Bearer x")
            .unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    #[test]
    fn set_header_overwrites_existing_header() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        add_http_server_to_claude_json(&path, "/agents/bot", "notion", "https://mcp.notion.com/mcp")
            .unwrap();

        set_server_header(&path, "/agents/bot", "notion", "Authorization", "Bearer old").unwrap();
        set_server_header(&path, "/agents/bot", "notion", "Authorization", "Bearer new").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["projects"]["/agents/bot"]["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer new"
        );
    }

    // --- Atomic write test ---

    #[test]
    fn write_is_atomic_via_tempfile_persist() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = json!({"key": "value"});
        write_json_atomic(&path, &value).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["key"], "value");
    }
}
