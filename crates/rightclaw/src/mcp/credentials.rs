use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::Path;
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

/// MCP OAuth token held in memory during refresh grant cycle.
/// Retained for internal use by refresh.rs.
#[derive(Serialize, Deserialize, Clone)]
pub struct CredentialToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    /// Unix timestamp seconds. 0 = non-expiring (e.g. Linear).
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    /// OAuth client_id used to obtain this token -- stored for refresh grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth client_secret (confidential clients only). None for public clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for CredentialToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialToken")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_deref().map(|_| "[REDACTED]"),
            )
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_deref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// OAuth refresh metadata stored in .mcp.json under _rightclaw_oauth per server.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthMetadata {
    pub refresh_token: Option<String>,
    /// Unix timestamp seconds. 0 = non-expiring.
    pub expires_at: u64,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
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

/// Read and parse .mcp.json. Returns empty object with mcpServers if file absent.
fn read_mcp_json(path: &Path) -> Result<serde_json::Value, CredentialError> {
    if !path.exists() {
        return Ok(serde_json::json!({ "mcpServers": {} }));
    }
    let content = std::fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;
    Ok(root)
}

/// Write Bearer token into .mcp.json headers for the named server.
/// Atomic read-modify-write. Preserves all other .mcp.json content.
/// Creates headers object if absent. Server entry must already exist.
pub fn write_bearer_to_mcp_json(
    mcp_json_path: &Path,
    server_name: &str,
    access_token: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;

    let servers = root
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;

    let entry = servers
        .get_mut(server_name)
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;

    // Create headers object if absent
    if !entry.contains_key("headers") {
        entry.insert(
            "headers".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    let headers = entry
        .get_mut("headers")
        .and_then(|v| v.as_object_mut())
        .expect("headers was just created or already existed as object");

    headers.insert(
        "Authorization".to_string(),
        serde_json::Value::String(format!("Bearer {access_token}")),
    );

    write_json_atomic(mcp_json_path, &root)
}

/// Write OAuth refresh metadata into .mcp.json _rightclaw_oauth for the named server.
/// Atomic read-modify-write. Preserves all other .mcp.json content.
pub fn write_oauth_metadata(
    mcp_json_path: &Path,
    server_name: &str,
    metadata: &OAuthMetadata,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;

    let servers = root
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;

    let entry = servers
        .get_mut(server_name)
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;

    let metadata_value = serde_json::to_value(metadata)?;
    entry.insert("_rightclaw_oauth".to_string(), metadata_value);

    write_json_atomic(mcp_json_path, &root)
}

/// Read the Bearer token from .mcp.json headers.Authorization for the named server.
/// Returns None if file/server/header absent. Strips "Bearer " prefix.
pub fn read_bearer_from_mcp_json(
    mcp_json_path: &Path,
    server_name: &str,
) -> Result<Option<String>, CredentialError> {
    if !mcp_json_path.exists() {
        return Ok(None);
    }

    let root = read_mcp_json(mcp_json_path)?;

    let token = root
        .get("mcpServers")
        .and_then(|v| v.get(server_name))
        .and_then(|v| v.get("headers"))
        .and_then(|v| v.get("Authorization"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    Ok(token)
}

/// Read OAuth refresh metadata from .mcp.json _rightclaw_oauth for the named server.
/// Returns None if file/server/metadata absent.
pub fn read_oauth_metadata(
    mcp_json_path: &Path,
    server_name: &str,
) -> Result<Option<OAuthMetadata>, CredentialError> {
    if !mcp_json_path.exists() {
        return Ok(None);
    }

    let root = read_mcp_json(mcp_json_path)?;

    let meta_value = root
        .get("mcpServers")
        .and_then(|v| v.get(server_name))
        .and_then(|v| v.get("_rightclaw_oauth"));

    match meta_value {
        Some(v) => {
            let metadata: OAuthMetadata = serde_json::from_value(v.clone())?;
            Ok(Some(metadata))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper: create .mcp.json with a server entry (url-based).
    fn write_mcp_with_server(dir: &std::path::Path, server_name: &str, url: &str) -> std::path::PathBuf {
        let path = dir.join(".mcp.json");
        let v = serde_json::json!({
            "mcpServers": {
                server_name: {
                    "url": url
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        path
    }

    // --- write_bearer_to_mcp_json tests ---

    #[test]
    fn write_bearer_sets_authorization_header() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        write_bearer_to_mcp_json(&mcp, "notion", "my_access_token").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer my_access_token"
        );
    }

    #[test]
    fn write_bearer_preserves_existing_entries() {
        let dir = tempdir().unwrap();
        let mcp = dir.path().join(".mcp.json");
        let v = serde_json::json!({
            "mcpServers": {
                "notion": { "url": "https://mcp.notion.com/mcp" },
                "linear": { "url": "https://mcp.linear.app/mcp" }
            }
        });
        std::fs::write(&mcp, serde_json::to_string(&v).unwrap()).unwrap();

        write_bearer_to_mcp_json(&mcp, "notion", "tok123").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        // notion has the header
        assert_eq!(
            content["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer tok123"
        );
        // linear is untouched
        assert_eq!(
            content["mcpServers"]["linear"]["url"],
            "https://mcp.linear.app/mcp"
        );
    }

    #[test]
    fn write_bearer_creates_headers_if_absent() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        // Server entry has no headers key
        write_bearer_to_mcp_json(&mcp, "notion", "tok").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        assert!(content["mcpServers"]["notion"]["headers"].is_object());
        assert_eq!(
            content["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer tok"
        );
    }

    #[test]
    fn write_bearer_errors_if_server_not_found() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        let err = write_bearer_to_mcp_json(&mcp, "nonexistent", "tok").unwrap_err();
        assert!(
            matches!(err, CredentialError::ServerNotFound(_)),
            "expected ServerNotFound, got: {err:?}"
        );
    }

    #[test]
    fn write_bearer_errors_if_file_absent() {
        let dir = tempdir().unwrap();
        let mcp = dir.path().join(".mcp.json");
        // File doesn't exist -> read_mcp_json returns empty mcpServers -> ServerNotFound
        let err = write_bearer_to_mcp_json(&mcp, "notion", "tok").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    // --- write_oauth_metadata tests ---

    #[test]
    fn write_oauth_metadata_sets_rightclaw_oauth() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        let meta = OAuthMetadata {
            refresh_token: Some("rt_xxx".to_string()),
            expires_at: 1712345678,
            client_id: Some("cli-abc".to_string()),
            client_secret: None,
        };
        write_oauth_metadata(&mcp, "notion", &meta).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        let oauth = &content["mcpServers"]["notion"]["_rightclaw_oauth"];
        assert_eq!(oauth["refresh_token"], "rt_xxx");
        assert_eq!(oauth["expires_at"], 1712345678);
        assert_eq!(oauth["client_id"], "cli-abc");
        assert!(oauth["client_secret"].is_null());
    }

    // --- read_bearer_from_mcp_json tests ---

    #[test]
    fn read_bearer_returns_token_when_present() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");
        write_bearer_to_mcp_json(&mcp, "notion", "my_token").unwrap();

        let result = read_bearer_from_mcp_json(&mcp, "notion").unwrap();
        assert_eq!(result, Some("my_token".to_string()));
    }

    #[test]
    fn read_bearer_returns_none_when_no_headers() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        let result = read_bearer_from_mcp_json(&mcp, "notion").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_bearer_returns_none_when_no_authorization() {
        let dir = tempdir().unwrap();
        let mcp = dir.path().join(".mcp.json");
        let v = serde_json::json!({
            "mcpServers": {
                "notion": {
                    "url": "https://mcp.notion.com/mcp",
                    "headers": { "X-Custom": "val" }
                }
            }
        });
        std::fs::write(&mcp, serde_json::to_string(&v).unwrap()).unwrap();

        let result = read_bearer_from_mcp_json(&mcp, "notion").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_bearer_returns_none_when_file_absent() {
        let dir = tempdir().unwrap();
        let mcp = dir.path().join(".mcp.json");
        let result = read_bearer_from_mcp_json(&mcp, "notion").unwrap();
        assert!(result.is_none());
    }

    // --- read_oauth_metadata tests ---

    #[test]
    fn read_oauth_metadata_returns_struct_when_present() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");
        let meta = OAuthMetadata {
            refresh_token: Some("rt".to_string()),
            expires_at: 999,
            client_id: Some("c".to_string()),
            client_secret: Some("s".to_string()),
        };
        write_oauth_metadata(&mcp, "notion", &meta).unwrap();

        let result = read_oauth_metadata(&mcp, "notion").unwrap().unwrap();
        assert_eq!(result.refresh_token, Some("rt".to_string()));
        assert_eq!(result.expires_at, 999);
        assert_eq!(result.client_id, Some("c".to_string()));
        assert_eq!(result.client_secret, Some("s".to_string()));
    }

    #[test]
    fn read_oauth_metadata_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        let mcp = write_mcp_with_server(dir.path(), "notion", "https://mcp.notion.com/mcp");

        let result = read_oauth_metadata(&mcp, "notion").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_oauth_metadata_returns_none_when_file_absent() {
        let dir = tempdir().unwrap();
        let mcp = dir.path().join(".mcp.json");
        let result = read_oauth_metadata(&mcp, "notion").unwrap();
        assert!(result.is_none());
    }

    // --- Atomic write test ---

    #[test]
    fn write_is_atomic_via_tempfile_persist() {
        // Verify write_json_atomic creates the file atomically
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = serde_json::json!({"key": "value"});
        write_json_atomic(&path, &value).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["key"], "value");
    }

    // --- CredentialToken retained ---

    #[test]
    fn credential_token_struct_retained() {
        // CredentialToken must still exist for refresh.rs
        let _token = CredentialToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            token_type: None,
            scope: None,
            expires_at: 0,
            client_id: None,
            client_secret: None,
        };
    }

    // --- Verify old functions are gone (compile-time check) ---
    // mcp_oauth_key, write_credential, read_credential, rotate_backups
    // are removed — if someone tries to call them, compilation fails.
}
