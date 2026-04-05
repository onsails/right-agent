use std::path::Path;

use crate::mcp::credentials::CredentialError;

/// Auth state for an MCP server.
///
/// CC manages OAuth natively for HTTP servers — we only track presence/absence
/// of the server entry, not token expiry.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    Present,
    Missing,
}

impl std::fmt::Display for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthState::Present => write!(f, "present"),
            AuthState::Missing => write!(f, "auth required"),
        }
    }
}

/// Source of an MCP server entry.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerSource {
    ClaudeJson,
    McpJson,
}

impl std::fmt::Display for ServerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerSource::ClaudeJson => write!(f, ".claude.json"),
            ServerSource::McpJson => write!(f, ".mcp.json"),
        }
    }
}

/// Status for a single MCP server entry.
pub struct ServerStatus {
    pub name: String,
    pub url: String,
    pub state: AuthState,
    pub source: ServerSource,
}

/// Return status for all MCP servers in an agent directory.
///
/// Combines HTTP servers from .claude.json (type: "http") and URL-bearing
/// servers from .mcp.json. Stdio entries (command+args only) are skipped.
/// Returns Ok(vec![]) when neither file exists.
pub fn mcp_auth_status(
    agent_dir: &Path,
) -> Result<Vec<ServerStatus>, CredentialError> {
    let mut results: Vec<ServerStatus> = Vec::new();

    // 1. HTTP servers from .claude.json
    let claude_json_path = agent_dir.join(".claude.json");
    if claude_json_path.exists() {
        let content = std::fs::read_to_string(&claude_json_path)?;
        let root: serde_json::Value = serde_json::from_str(&content)?;

        // Try the agent's canonicalized path as the project key
        let path_key = agent_dir
            .canonicalize()
            .unwrap_or_else(|_| agent_dir.to_path_buf())
            .display()
            .to_string();

        if let Some(servers) = root
            .get("projects")
            .and_then(|p| p.get(&path_key))
            .and_then(|proj| proj.get("mcpServers"))
            .and_then(|s| s.as_object())
        {
            for (name, entry) in servers {
                if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
                    results.push(ServerStatus {
                        name: name.clone(),
                        url: url.to_string(),
                        state: AuthState::Present, // CC manages auth natively
                        source: ServerSource::ClaudeJson,
                    });
                }
            }
        }
    }

    // 2. URL-bearing servers from .mcp.json (stdio servers skipped)
    let mcp_json_path = agent_dir.join(".mcp.json");
    if mcp_json_path.exists() {
        let content = std::fs::read_to_string(&mcp_json_path)?;
        let root: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(servers) = root.get("mcpServers").and_then(|v| v.as_object()) {
            for (name, entry) in servers {
                let url = match entry.get("url").and_then(|v| v.as_str()) {
                    Some(u) => u.to_string(),
                    None => continue, // stdio server — skip
                };

                // Skip if already listed from .claude.json (avoid duplicates)
                if results.iter().any(|r| r.name == *name) {
                    continue;
                }

                let has_bearer = entry
                    .get("headers")
                    .and_then(|v| v.get("Authorization"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.starts_with("Bearer "));

                results.push(ServerStatus {
                    name: name.clone(),
                    url,
                    state: if has_bearer {
                        AuthState::Present
                    } else {
                        AuthState::Missing
                    },
                    source: ServerSource::McpJson,
                });
            }
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Write .claude.json with HTTP servers under the project key.
    fn write_claude_json_with_servers(
        dir: &std::path::Path,
        servers: &[(&str, &str)],
    ) {
        let path_key = dir
            .canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string();

        let mut mcp_servers = serde_json::Map::new();
        for (name, url) in servers {
            mcp_servers.insert(
                name.to_string(),
                serde_json::json!({ "type": "http", "url": url }),
            );
        }

        let v = serde_json::json!({
            "projects": {
                path_key: {
                    "mcpServers": mcp_servers
                }
            }
        });
        std::fs::write(
            dir.join(".claude.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
    }

    /// Write .mcp.json with servers. url=Some means HTTP server, url=None means stdio.
    fn write_mcp_json(
        dir: &std::path::Path,
        servers: &[(&str, Option<&str>)],
    ) {
        let mut map = serde_json::Map::new();
        for (name, url_opt) in servers {
            let mut entry = serde_json::Map::new();
            if let Some(url) = url_opt {
                entry.insert("url".to_string(), serde_json::Value::String(url.to_string()));
            } else {
                entry.insert(
                    "command".to_string(),
                    serde_json::Value::String("some-binary".to_string()),
                );
            }
            map.insert(name.to_string(), serde_json::Value::Object(entry));
        }
        let v = serde_json::json!({ "mcpServers": map });
        std::fs::write(
            dir.join(".mcp.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn list_http_servers_from_claude_json() {
        let dir = tempdir().unwrap();
        write_claude_json_with_servers(
            dir.path(),
            &[("notion", "https://mcp.notion.com/mcp")],
        );

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "notion");
        assert_eq!(result[0].source, ServerSource::ClaudeJson);
        assert_eq!(result[0].state, AuthState::Present);
    }

    #[test]
    fn list_mcp_json_url_servers() {
        let dir = tempdir().unwrap();
        write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "notion");
        assert_eq!(result[0].source, ServerSource::McpJson);
        assert_eq!(result[0].state, AuthState::Missing); // no bearer header
    }

    #[test]
    fn mcp_json_bearer_present_is_present() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        let v = serde_json::json!({
            "mcpServers": {
                "notion": {
                    "url": "https://mcp.notion.com/mcp",
                    "headers": { "Authorization": "Bearer tok123" }
                }
            }
        });
        std::fs::write(&mcp_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, AuthState::Present);
    }

    #[test]
    fn stdio_server_skipped() {
        let dir = tempdir().unwrap();
        write_mcp_json(dir.path(), &[("rightmemory", None)]);

        let result = mcp_auth_status(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn combined_sources_deduped() {
        let dir = tempdir().unwrap();
        // Same server in both files — .claude.json takes precedence
        write_claude_json_with_servers(
            dir.path(),
            &[("notion", "https://mcp.notion.com/mcp")],
        );
        write_mcp_json(dir.path(), &[("notion", Some("https://mcp.notion.com/mcp"))]);

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, ServerSource::ClaudeJson);
    }

    #[test]
    fn combined_sources_merged() {
        let dir = tempdir().unwrap();
        write_claude_json_with_servers(
            dir.path(),
            &[("notion", "https://mcp.notion.com/mcp")],
        );
        write_mcp_json(dir.path(), &[("linear", Some("https://mcp.linear.app/mcp"))]);

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
        // Sorted by name
        assert_eq!(result[0].name, "linear");
        assert_eq!(result[0].source, ServerSource::McpJson);
        assert_eq!(result[1].name, "notion");
        assert_eq!(result[1].source, ServerSource::ClaudeJson);
    }

    #[test]
    fn absent_files_return_empty_vec() {
        let dir = tempdir().unwrap();
        let result = mcp_auth_status(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn results_are_sorted_by_name() {
        let dir = tempdir().unwrap();
        write_claude_json_with_servers(
            dir.path(),
            &[
                ("zebra", "https://zebra.example.com/mcp"),
                ("apple", "https://apple.example.com/mcp"),
            ],
        );

        let result = mcp_auth_status(dir.path()).unwrap();
        assert_eq!(result[0].name, "apple");
        assert_eq!(result[1].name, "zebra");
    }
}
