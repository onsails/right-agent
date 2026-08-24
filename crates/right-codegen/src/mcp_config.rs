use std::path::Path;

/// Generate `mcp.json` with right as the sole HTTP MCP server entry.
///
/// Writes from scratch — any existing entries (external servers, stale configs) are
/// stripped. External MCP servers are managed by the Aggregator's SQLite registry and
/// proxied through the `right` server. Keeping them in `.mcp.json` would cause Claude
/// to connect directly, bypassing the Aggregator.
pub fn generate_mcp_config_http(
    agent_path: &Path,
    _agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    let root = serde_json::json!({
        "mcpServers": {
            "right": {
                "type": "http",
                "url": right_mcp_url,
                "headers": {
                    "Authorization": format!("Bearer {bearer_token}")
                }
            }
        }
    });

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))?;
    crate::contract::write_regenerated(&mcp_path, &output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- HTTP right MCP server tests (sandboxed aggregator mode) ---

    #[test]
    fn generates_http_right_entry() {
        let dir = tempdir().unwrap();
        let token = "test-bearer-token-abc123";
        generate_mcp_config_http(
            dir.path(),
            "brain",
            "http://host.microsandbox.internal:8100/mcp",
            token,
        )
        .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap())
                .unwrap();
        assert_eq!(content["mcpServers"]["right"]["type"], "http");
        assert_eq!(
            content["mcpServers"]["right"]["url"],
            "http://host.microsandbox.internal:8100/mcp"
        );
        assert_eq!(
            content["mcpServers"]["right"]["headers"]["Authorization"],
            "Bearer test-bearer-token-abc123"
        );
    }

    #[test]
    fn http_strips_stale_external_entries() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers":{"notion":{"type":"http","url":"https://mcp.notion.com/mcp"},"rightmemory":{"command":"old"}}}"#,
        )
        .unwrap();
        generate_mcp_config_http(dir.path(), "brain", "http://localhost:8100/mcp", "tok").unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
        let servers = content["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1, "only 'right' should remain");
        assert!(
            content["mcpServers"]["notion"].is_null(),
            "stale 'notion' entry must be stripped"
        );
        assert!(
            content["mcpServers"]["rightmemory"].is_null(),
            "stale 'rightmemory' entry must be stripped"
        );
        assert_eq!(content["mcpServers"]["right"]["type"], "http");
    }

    #[test]
    fn http_overwrites_external_entries_on_existing_file() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join("mcp.json");
        // Pre-populate with multiple external servers and stale right entry
        std::fs::write(
            &mcp_path,
            r#"{
                "mcpServers": {
                    "notion": {"type": "http", "url": "https://mcp.notion.com/mcp"},
                    "github": {"type": "http", "url": "https://mcp.github.com/mcp"},
                    "right": {"type": "http", "url": "http://old-url/mcp", "headers": {"Authorization": "Bearer old-token"}}
                },
                "otherKey": true
            }"#,
        )
        .unwrap();
        generate_mcp_config_http(
            dir.path(),
            "brain",
            "http://host.microsandbox.internal:8100/mcp",
            "new-token",
        )
        .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
        let servers = content["mcpServers"].as_object().unwrap();
        assert_eq!(
            servers.len(),
            1,
            "only 'right' should remain after overwrite"
        );
        assert_eq!(
            content["mcpServers"]["right"]["url"], "http://host.microsandbox.internal:8100/mcp",
            "right URL must be updated"
        );
        assert_eq!(
            content["mcpServers"]["right"]["headers"]["Authorization"], "Bearer new-token",
            "right token must be updated"
        );
        // Top-level keys from old file should NOT survive (written from scratch)
        assert!(
            content["otherKey"].is_null(),
            "old top-level keys must not survive from-scratch write"
        );
    }
}
