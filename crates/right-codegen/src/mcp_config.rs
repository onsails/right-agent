use std::path::Path;

/// Merge the `right` MCP server entry into an agent's `mcp.json`.
///
/// - If `mcp.json` exists, reads it, parses as JSON object, injects/updates
///   `mcpServers.right` key, writes back.
/// - If `mcp.json` does not exist, creates it with just the right entry.
/// - Preserves all other keys in the existing JSON (non-destructive merge per D-05).
/// - `binary` is written verbatim into the `command` field — pass `current_exe()` result
///   so agents can always find the right binary regardless of PATH.
/// - `agent_name` is injected as `RC_AGENT_NAME` in the env section (D-04).
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    right_home: &Path,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    crate::contract::write_merged_rmw(&mcp_path, |existing| {
        let mut root: serde_json::Value = match existing {
            Some(content) => serde_json::from_str(content)
                .map_err(|e| miette::miette!("failed to parse mcp.json: {e:#}"))?,
            None => serde_json::json!({}),
        };

        let obj = root
            .as_object_mut()
            .ok_or_else(|| miette::miette!("mcp.json root is not a JSON object"))?;

        if !obj.contains_key("mcpServers") {
            obj.insert("mcpServers".to_string(), serde_json::json!({}));
        }

        let servers = obj
            .get_mut("mcpServers")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| miette::miette!("mcp.json mcpServers is not a JSON object"))?;

        servers.insert(
            "right".to_string(),
            serde_json::json!({
                "command": binary.to_string_lossy(),
                "args": ["memory-server"],
                "env": {
                    "RC_AGENT_NAME": agent_name,
                    "RC_RIGHT_HOME": right_home.to_string_lossy().as_ref()
                }
            }),
        );

        servers.remove("rightmemory");

        serde_json::to_string_pretty(&root)
            .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))
    })
}

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

/// Generate a random 32-byte Bearer token, base64url-encoded (no padding).
pub fn generate_agent_token() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "right command should be 'right'"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["args"][0], "memory-server",
            "right args[0] should be 'memory-server'"
        );
    }

    #[test]
    fn merges_into_existing_mcp_json() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            r#"{"mcpServers":{"other":{"command":"other-tool"}}}"#,
        )
        .unwrap();

        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed["mcpServers"]["other"].is_object(),
            "existing 'other' server must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "right server must be added"
        );
    }

    #[test]
    fn preserves_unknown_top_level_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            r#"{"otherService": true, "mcpServers":{}}"#,
        )
        .unwrap();

        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["otherService"], true,
            "'otherService' key must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "right must be present"
        );
    }

    #[test]
    fn overwrites_stale_right_entry() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            r#"{"mcpServers":{"right":{"command":"old-binary","args":["old-arg"]}}}"#,
        )
        .unwrap();

        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "stale command should be replaced"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["args"][0], "memory-server",
            "stale args should be replaced"
        );
    }

    #[test]
    fn idempotent_on_repeated_calls() {
        let dir = tempdir().unwrap();
        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();
        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        // Valid JSON with single right entry
        assert!(parsed.is_object(), "result must be valid JSON object");
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "right must be present after two calls"
        );
        // Ensure only one right key (no duplication)
        let servers = parsed["mcpServers"].as_object().unwrap();
        let count = servers.keys().filter(|k| k.as_str() == "right").count();
        assert_eq!(count, 1, "right should appear exactly once");
    }

    #[test]
    fn creates_mcp_servers_key_if_missing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("mcp.json"), r#"{"telegram": true}"#).unwrap();

        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["telegram"], true, "'telegram' key must be preserved");
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "right",
            "mcpServers.right must be added"
        );
    }

    #[test]
    fn uses_provided_binary_path() {
        let dir = tempdir().unwrap();
        generate_mcp_config(
            dir.path(),
            Path::new("/usr/local/bin/right"),
            "test-agent",
            Path::new("/home/user"),
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"], "/usr/local/bin/right",
            "command must be the absolute path passed in, not a hardcoded name"
        );
    }

    #[test]
    fn mcp_config_env_contains_agent_name() {
        let dir = tempdir().unwrap();
        generate_mcp_config(
            dir.path(),
            Path::new("right"),
            "myagent",
            Path::new("/home/user"),
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["env"]["RC_AGENT_NAME"], "myagent",
            "RC_AGENT_NAME must be injected into env"
        );
    }

    #[test]
    fn mcp_config_env_contains_right_home() {
        let dir = tempdir().unwrap();
        let home = Path::new("/home/user/.right");
        generate_mcp_config(dir.path(), Path::new("right"), "myagent", home).unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["env"]["RC_RIGHT_HOME"], "/home/user/.right",
            "RC_RIGHT_HOME must be injected into env"
        );
    }

    // --- HTTP right MCP server tests (OpenShell sandbox mode) ---

    #[test]
    fn generates_http_right_entry() {
        let dir = tempdir().unwrap();
        let token = "test-bearer-token-abc123";
        generate_mcp_config_http(
            dir.path(),
            "brain",
            "http://host.openshell.internal:8100/mcp",
            token,
        )
        .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap())
                .unwrap();
        assert_eq!(content["mcpServers"]["right"]["type"], "http");
        assert_eq!(
            content["mcpServers"]["right"]["url"],
            "http://host.openshell.internal:8100/mcp"
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
            "http://host.openshell.internal:8100/mcp",
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
            content["mcpServers"]["right"]["url"], "http://host.openshell.internal:8100/mcp",
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

    #[test]
    fn generate_agent_token_is_32_bytes_base64url() {
        let token = generate_agent_token();
        // 32 bytes -> 43 chars in base64url no-pad
        assert_eq!(token.len(), 43);
        // Should be different each time
        let token2 = generate_agent_token();
        assert_ne!(token, token2);
    }
}
