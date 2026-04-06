use std::path::Path;

use crate::config::ChromeConfig;

/// Merge the `rightmemory` MCP server entry into an agent's `.mcp.json`.
///
/// - If `.mcp.json` exists, reads it, parses as JSON object, injects/updates
///   `mcpServers.rightmemory` key, writes back.
/// - If `.mcp.json` does not exist, creates it with just the rightmemory entry.
/// - Preserves all other keys in the existing JSON (non-destructive merge per D-05).
/// - `binary` is written verbatim into the `command` field — pass `current_exe()` result
///   so agents can always find the rightclaw binary regardless of PATH.
/// - `agent_name` is injected as `RC_AGENT_NAME` in the env section (D-04).
/// - When `chrome_config` is Some, injects a `chrome-devtools` MCP entry (INJECT-01, INJECT-02).
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
    chrome_config: Option<&ChromeConfig>,
) -> miette::Result<()> {
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
            "env": {
                "RC_AGENT_NAME": agent_name,
                "RC_RIGHTCLAW_HOME": rightclaw_home.to_string_lossy().as_ref()
            }
        }),
    );

    // Inject chrome-devtools MCP entry when Chrome is configured (per D-07, INJECT-01, INJECT-02).
    if let Some(chrome) = chrome_config {
        let profile_dir = agent_path.join(".chrome-profile");
        servers.insert(
            "chrome-devtools".to_string(),
            serde_json::json!({
                "command": chrome.mcp_binary_path.to_string_lossy(),
                "args": [
                    "--executablePath", chrome.chrome_path.to_string_lossy().as_ref(),
                    "--headless",
                    "--isolated",
                    "--no-sandbox",
                    "--userDataDir", profile_dir.to_string_lossy().as_ref()
                ]
            }),
        );
    }

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
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

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
    fn preserves_unknown_top_level_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"otherService": true, "mcpServers":{}}"#,
        )
        .unwrap();

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["otherService"], true,
            "'otherService' key must be preserved"
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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

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
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

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

        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();

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
        generate_mcp_config(dir.path(), Path::new("/usr/local/bin/rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();
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
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent", Path::new("/home/user/.rightclaw"), None).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["env"]["RC_AGENT_NAME"],
            "myagent",
            "RC_AGENT_NAME must be injected into env"
        );
    }

    #[test]
    fn mcp_config_env_contains_rightclaw_home() {
        let dir = tempdir().unwrap();
        let home = Path::new("/home/user/.rightclaw");
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "myagent", home, None).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["rightmemory"]["env"]["RC_RIGHTCLAW_HOME"],
            "/home/user/.rightclaw",
            "RC_RIGHTCLAW_HOME must be injected into env"
        );
    }

    // --- Chrome injection tests (Task 1 TDD) ---

    #[test]
    fn chrome_devtools_injected_when_chrome_config_some() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["chrome-devtools"]["command"],
            "/usr/local/bin/chrome-devtools-mcp",
            "chrome-devtools command must be the mcp_binary_path"
        );
        let args = parsed["mcpServers"]["chrome-devtools"]["args"].as_array().unwrap();
        let args_strs: Vec<&str> = args.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(args_strs.contains(&"--executablePath"), "args must contain --executablePath");
        assert!(args_strs.contains(&"/usr/bin/chrome"), "args must contain chrome path");
        assert!(args_strs.contains(&"--headless"), "args must contain --headless");
        assert!(args_strs.contains(&"--isolated"), "args must contain --isolated");
        assert!(args_strs.contains(&"--no-sandbox"), "args must contain --no-sandbox");
        assert!(args_strs.contains(&"--userDataDir"), "args must contain --userDataDir");
        assert!(
            args_strs.iter().any(|s| s.ends_with(".chrome-profile")),
            "args must contain path ending in .chrome-profile"
        );
    }

    #[test]
    fn chrome_devtools_not_injected_when_none() {
        let dir = tempdir().unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), None).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            parsed["mcpServers"]["chrome-devtools"].is_null(),
            "chrome-devtools must be absent when chrome_config is None"
        );
    }

    #[test]
    fn chrome_devtools_uses_absolute_binary_path_not_npx() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let command = parsed["mcpServers"]["chrome-devtools"]["command"].as_str().unwrap();
        assert!(!command.contains("npx"), "command must NOT contain 'npx'");
        assert_eq!(command, "/usr/local/bin/chrome-devtools-mcp", "command must be the exact mcp_binary_path");
    }

    #[test]
    fn chrome_devtools_user_data_dir_is_agent_dir_chrome_profile() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        let agent_path = dir.path();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(agent_path, Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(agent_path.join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let args = parsed["mcpServers"]["chrome-devtools"]["args"].as_array().unwrap();
        let args_strs: Vec<&str> = args.iter().map(|v| v.as_str().unwrap()).collect();
        // Find the value after --userDataDir
        let idx = args_strs.iter().position(|&s| s == "--userDataDir").expect("--userDataDir must be in args");
        let user_data_dir = args_strs[idx + 1];
        let expected = format!("{}/.chrome-profile", agent_path.display());
        assert_eq!(user_data_dir, expected, "userDataDir must be agent_path/.chrome-profile");
    }

    #[test]
    fn chrome_devtools_coexists_with_rightmemory() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed["mcpServers"]["rightmemory"].is_object(), "rightmemory must be present");
        assert!(parsed["mcpServers"]["chrome-devtools"].is_object(), "chrome-devtools must be present");
    }

    #[test]
    fn chrome_devtools_idempotent() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let servers = parsed["mcpServers"].as_object().unwrap();
        let count = servers.keys().filter(|k| k.as_str() == "chrome-devtools").count();
        assert_eq!(count, 1, "chrome-devtools should appear exactly once after two calls");
    }

    #[test]
    fn chrome_devtools_overwrites_stale_entry() {
        use crate::config::ChromeConfig;
        use std::path::PathBuf;
        let dir = tempdir().unwrap();
        // Pre-write stale entry with wrong command
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"chrome-devtools":{"command":"npx chrome-devtools-mcp","args":[]}}}"#,
        ).unwrap();
        let chrome = ChromeConfig {
            chrome_path: PathBuf::from("/usr/bin/chrome"),
            mcp_binary_path: PathBuf::from("/usr/local/bin/chrome-devtools-mcp"),
        };
        generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/tmp/rc"), Some(&chrome)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["mcpServers"]["chrome-devtools"]["command"],
            "/usr/local/bin/chrome-devtools-mcp",
            "stale command should be replaced with mcp_binary_path"
        );
        assert!(!parsed["mcpServers"]["chrome-devtools"]["command"].as_str().unwrap().contains("npx"),
            "updated command must not contain npx");
    }
}
