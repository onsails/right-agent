use std::path::Path;

use crate::mcp::credentials::{read_credential, CredentialError};

#[cfg(test)]
use crate::mcp::credentials::{write_credential, CredentialToken};

/// Auth state for an MCP server's OAuth token.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    Present,
    Missing,
    Expired,
}

impl std::fmt::Display for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthState::Present => write!(f, "present"),
            AuthState::Missing => write!(f, "auth required"),
            AuthState::Expired => write!(f, "expired"),
        }
    }
}

/// Auth status for a single MCP OAuth server entry.
pub struct ServerStatus {
    pub name: String,
    pub url: String,
    pub state: AuthState,
}

/// Return auth status for all OAuth-candidate servers in an agent's .mcp.json.
///
/// OAuth candidates = entries with a "url" field (HTTP/SSE transport).
/// Stdio entries (command+args only, e.g. rightmemory) are silently skipped.
/// Returns Ok(vec![]) when .mcp.json does not exist.
///
/// `credentials_path` = host ~/.claude/.credentials.json
pub fn mcp_auth_status(
    agent_mcp_path: &Path,
    credentials_path: &Path,
) -> Result<Vec<ServerStatus>, CredentialError> {
    if !agent_mcp_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(agent_mcp_path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;

    let servers = match root.get("mcpServers").and_then(|v| v.as_object()) {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut results: Vec<ServerStatus> = Vec::new();

    for (name, entry) in servers {
        let url = match entry.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => continue, // stdio server — skip silently
        };

        let token_opt = read_credential(credentials_path, name, &url)?;

        let state = match token_opt {
            None => AuthState::Missing,
            Some(token) => {
                if token.expires_at > 0 && token.expires_at < now_unix {
                    AuthState::Expired
                } else {
                    AuthState::Present
                }
            }
        };

        results.push(ServerStatus { name: name.clone(), url, state });
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_mcp_json(servers: &[(&str, Option<&str>)]) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (name, url_opt) in servers {
            let mut entry = serde_json::Map::new();
            if let Some(url) = url_opt {
                entry.insert("url".to_string(), serde_json::Value::String(url.to_string()));
            } else {
                // stdio server: has command+args but no url
                entry.insert(
                    "command".to_string(),
                    serde_json::Value::String("some-binary".to_string()),
                );
            }
            map.insert(name.to_string(), serde_json::Value::Object(entry));
        }
        serde_json::json!({ "mcpServers": map })
    }

    fn write_mcp_json(dir: &std::path::Path, servers: &[(&str, Option<&str>)]) -> std::path::PathBuf {
        let path = dir.join(".mcp.json");
        let v = make_mcp_json(servers);
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        path
    }

    fn token_with_expiry(expires_at: u64) -> CredentialToken {
        CredentialToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at,
            client_id: None,
            client_secret: None,
        }
    }

    // Test 1: expires_at = 0 → Present (non-expiring, Linear case)
    #[test]
    fn expires_at_zero_is_present() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("linear", Some("https://mcp.linear.app/mcp"))]);
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "linear", "https://mcp.linear.app/mcp", &token_with_expiry(0)).unwrap();

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Present);
    }

    // Test 2: expires_at far future → Present
    #[test]
    fn expires_at_far_future_is_present() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &token_with_expiry(9_999_999_999)).unwrap();

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Present);
    }

    // Test 3: expires_at = 1 (far past) → Expired
    #[test]
    fn expires_at_past_is_expired() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &token_with_expiry(1)).unwrap();

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Expired);
    }

    // Test 4: server key absent from credentials file → Missing
    #[test]
    fn absent_key_is_missing() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);
        let creds = dir.path().join(".credentials.json");

        // Write credentials for a different server — notion key absent
        write_credential(&creds, "linear", "https://mcp.linear.app/mcp", &token_with_expiry(0)).unwrap();

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Missing);
    }

    // Test 5: credentials file absent → Missing (not error)
    #[test]
    fn absent_credentials_file_is_missing() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);
        let creds = dir.path().join("nonexistent.json");

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Missing);
    }

    // Test 6: .mcp.json absent → Ok(vec![]) (not error)
    #[test]
    fn absent_mcp_json_returns_empty_vec() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join("nonexistent.mcp.json");
        let creds = dir.path().join(".credentials.json");

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert!(result.is_empty());
    }

    // Test 7: .mcp.json entry without url field (stdio) → not returned in Vec
    #[test]
    fn stdio_server_without_url_is_skipped() {
        let dir = tempdir().unwrap();
        // rightmemory is stdio (no url field)
        let mcp_path = write_mcp_json(dir.path(), &[("rightmemory", None)]);
        let creds = dir.path().join(".credentials.json");

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert!(result.is_empty(), "stdio server must not appear in results");
    }

    // Test 8: .mcp.json entry with url field → included in Vec
    #[test]
    fn http_server_with_url_is_included() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);
        let creds = dir.path().join("nonexistent.json");

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "notion");
    }

    // Test 9: results sorted by server name (deterministic)
    #[test]
    fn results_are_sorted_by_name() {
        let dir = tempdir().unwrap();
        let mcp_path = write_mcp_json(
            dir.path(),
            &[
                ("zebra", Some("https://zebra.example.com/mcp")),
                ("apple", Some("https://apple.example.com/mcp")),
                ("mango", Some("https://mango.example.com/mcp")),
            ],
        );
        let creds = dir.path().join("nonexistent.json");

        let result = mcp_auth_status(&mcp_path, &creds).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "apple");
        assert_eq!(result[1].name, "mango");
        assert_eq!(result[2].name, "zebra");
    }
}
