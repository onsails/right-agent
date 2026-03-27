use std::path::Path;

/// Merge the `rightmemory` MCP server entry into an agent's `.mcp.json`.
///
/// - If `.mcp.json` exists, reads it, parses as JSON object, injects/updates
///   `mcpServers.rightmemory` key, writes back.
/// - If `.mcp.json` does not exist, creates it with just the rightmemory entry.
/// - Preserves all other keys in the existing JSON (non-destructive merge per D-05).
/// - `binary` is written verbatim into the `command` field — pass `current_exe()` result
///   so agents can always find the rightclaw binary regardless of PATH.
pub fn generate_mcp_config(agent_path: &Path, binary: &Path) -> miette::Result<()> {
    let mcp_path = agent_path.join(".mcp.json");

    let mut root: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)
            .map_err(|e| miette::miette!("failed to read .mcp.json: {e:#}"))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse .mcp.json: {e:#}"))?
    } else {
        serde_json::json!({})
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| miette::miette!(".mcp.json root is not a JSON object"))?;

    // Ensure mcpServers key exists as object
    if !obj.contains_key("mcpServers") {
        obj.insert("mcpServers".to_string(), serde_json::json!({}));
    }

    let servers = obj
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| miette::miette!(".mcp.json mcpServers is not a JSON object"))?;

    // Insert/update the rightmemory entry (per D-05)
    servers.insert(
        "rightmemory".to_string(),
        serde_json::json!({
            "command": binary.to_string_lossy(),
            "args": ["memory-server"],
            "env": {}
        }),
    );

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize .mcp.json: {e:#}"))?;
    std::fs::write(&mcp_path, output)
        .map_err(|e| miette::miette!("failed to write .mcp.json: {e:#}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "rightmemory command should be 'rightclaw'"
        );
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["args"][0],
            "memory-server",
            "rightmemory args[0] should be 'memory-server'"
        );
    }

    #[test]
    fn merges_into_existing_mcp_json() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"other":{"command":"other-tool"}}}"#,
        )
        .unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed["mcpServers"]["other"].is_object(),
            "existing 'other' server must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "rightmemory server must be added"
        );
    }

    #[test]
    fn preserves_non_mcp_servers_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"telegram": true, "mcpServers":{}}"#,
        )
        .unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["telegram"], true,
            "'telegram' key must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "rightmemory must be present"
        );
    }

    #[test]
    fn overwrites_stale_rightmemory_entry() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"rightmemory":{"command":"old-binary","args":["old-arg"]}}}"#,
        )
        .unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "stale command should be replaced"
        );
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["args"][0],
            "memory-server",
            "stale args should be replaced"
        );
    }

    #[test]
    fn idempotent_on_repeated_calls() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        // Valid JSON with single rightmemory entry
        assert!(parsed.is_object(), "result must be valid JSON object");
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "rightmemory must be present after two calls"
        );
        // Ensure only one rightmemory key (no duplication)
        let servers = parsed["mcpServers"].as_object().unwrap();
        let count = servers
            .keys()
            .filter(|k| k.as_str() == "rightmemory")
            .count();
        assert_eq!(count, 1, "rightmemory should appear exactly once");
    }

    #[test]
    fn creates_mcp_servers_key_if_missing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".mcp.json"), r#"{"telegram": true}"#).unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["telegram"], true,
            "'telegram' key must be preserved"
        );
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "rightclaw",
            "mcpServers.rightmemory must be added"
        );
    }

    #[test]
    fn uses_provided_binary_path() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("/usr/local/bin/rightclaw")).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["command"],
            "/usr/local/bin/rightclaw",
            "command must be the absolute path passed in, not a hardcoded name"
        );
    }

    #[test]
    fn mcp_config_env_contains_agent_name() {
        let dir = tempdir().unwrap();
        // NOTE: uses current 2-arg signature — will be updated to 3-arg in Task 2
        generate_mcp_config(dir.path(), Path::new("rightclaw")).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["env"]["RC_AGENT_NAME"],
            "myagent",
            "RC_AGENT_NAME must be injected into env"
        );
    }
}
