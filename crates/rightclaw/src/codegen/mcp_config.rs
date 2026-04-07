use std::path::Path;

/// Merge the `right` MCP server entry into an agent's `mcp.json`.
///
/// - If `mcp.json` exists, reads it, parses as JSON object, injects/updates
///   `mcpServers.right` key, writes back.
/// - If `mcp.json` does not exist, creates it with just the right entry.
/// - Preserves all other keys in the existing JSON (non-destructive merge per D-05).
/// - `binary` is written verbatim into the `command` field — pass `current_exe()` result
///   so agents can always find the rightclaw binary regardless of PATH.
/// - `agent_name` is injected as `RC_AGENT_NAME` in the env section (D-04).
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    let mut root: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)
            .map_err(|e| miette::miette!("failed to read mcp.json: {e:#}"))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse mcp.json: {e:#}"))?
    } else {
        serde_json::json!({})
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| miette::miette!("mcp.json root is not a JSON object"))?;

    // Ensure mcpServers key exists as object
    if !obj.contains_key("mcpServers") {
        obj.insert("mcpServers".to_string(), serde_json::json!({}));
    }

    let servers = obj
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| miette::miette!("mcp.json mcpServers is not a JSON object"))?;

    // Insert/update the right entry (per D-05)
    servers.insert(
        "right".to_string(),
        serde_json::json!({
            "command": binary.to_string_lossy(),
            "args": ["memory-server"],
            "env": {
                "RC_AGENT_NAME": agent_name,
                "RC_RIGHTCLAW_HOME": rightclaw_home.to_string_lossy().as_ref()
            }
        }),
    );

    // Remove stale "rightmemory" entry from pre-rename agents.
    servers.remove("rightmemory");

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))?;
    std::fs::write(&mcp_path, output)
        .map_err(|e| miette::miette!("failed to write mcp.json: {e:#}"))?;

    Ok(())
}

/// Generate `mcp.json` with right as HTTP MCP server entry.
///
/// Used when agents run inside OpenShell sandbox and connect to host right MCP server via HTTP.
pub fn generate_mcp_config_http(
    agent_path: &Path,
    _agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
) -> miette::Result<()> {
    let mcp_path = agent_path.join("mcp.json");

    let mut root: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)
            .map_err(|e| miette::miette!("failed to read mcp.json: {e:#}"))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse mcp.json: {e:#}"))?
    } else {
        serde_json::json!({})
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
        .ok_or_else(|| miette::miette!("mcpServers is not a JSON object"))?;

    servers.insert(
        "right".to_string(),
        serde_json::json!({
            "type": "http",
            "url": right_mcp_url,
            "headers": {
                "Authorization": format!("Bearer {bearer_token}")
            }
        }),
    );

    // Remove stale "rightmemory" entry from pre-rename agents.
    servers.remove("rightmemory");

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize mcp.json: {e:#}"))?;
    std::fs::write(&mcp_path, output)
        .map_err(|e| miette::miette!("failed to write mcp.json: {e:#}"))?;

    Ok(())
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
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
            "right command should be 'rightclaw'"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["args"][0],
            "memory-server",
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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed["mcpServers"]["other"].is_object(),
            "existing 'other' server must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["otherService"], true,
            "'otherService' key must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
            "stale command should be replaced"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["args"][0],
            "memory-server",
            "stale args should be replaced"
        );
    }

    #[test]
    fn idempotent_on_repeated_calls() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        // Valid JSON with single right entry
        assert!(parsed.is_object(), "result must be valid JSON object");
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
            "right must be present after two calls"
        );
        // Ensure only one right key (no duplication)
        let servers = parsed["mcpServers"].as_object().unwrap();
        let count = servers
            .keys()
            .filter(|k| k.as_str() == "right")
            .count();
        assert_eq!(count, 1, "right should appear exactly once");
    }

    #[test]
    fn creates_mcp_servers_key_if_missing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("mcp.json"), r#"{"telegram": true}"#).unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["telegram"], true,
            "'telegram' key must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "rightclaw",
            "mcpServers.right must be added"
        );
    }

    #[test]
    fn uses_provided_binary_path() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("/usr/local/bin/rightclaw"), "test-agent", Path::new("/home/user")).unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["command"],
            "/usr/local/bin/rightclaw",
            "command must be the absolute path passed in, not a hardcoded name"
        );
    }

    #[test]
    fn mcp_config_env_contains_agent_name() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent", Path::new("/home/user")).unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["env"]["RC_AGENT_NAME"],
            "myagent",
            "RC_AGENT_NAME must be injected into env"
        );
    }

    #[test]
    fn mcp_config_env_contains_rightclaw_home() {
        let dir = tempdir().unwrap();
        let home = Path::new("/home/user/.rightclaw");
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent", home).unwrap();
        let content = std::fs::read_to_string(dir.path().join("mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["right"]["env"]["RC_RIGHTCLAW_HOME"],
            "/home/user/.rightclaw",
            "RC_RIGHTCLAW_HOME must be injected into env"
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
            "http://host.docker.internal:8100/mcp",
            token,
        )
        .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap())
                .unwrap();
        assert_eq!(content["mcpServers"]["right"]["type"], "http");
        assert_eq!(
            content["mcpServers"]["right"]["url"],
            "http://host.docker.internal:8100/mcp"
        );
        assert_eq!(
            content["mcpServers"]["right"]["headers"]["Authorization"],
            "Bearer test-bearer-token-abc123"
        );
    }

    #[test]
    fn http_preserves_existing_servers() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers":{"notion":{"type":"http","url":"https://mcp.notion.com/mcp"}}}"#,
        )
        .unwrap();
        generate_mcp_config_http(dir.path(), "brain", "http://localhost:8100/mcp", "tok")
            .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["notion"]["url"],
            "https://mcp.notion.com/mcp"
        );
        assert_eq!(content["mcpServers"]["right"]["type"], "http");
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
