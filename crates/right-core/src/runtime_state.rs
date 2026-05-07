use std::path::Path;

use serde::{Deserialize, Serialize};

/// Default TCP port for the process-compose API.
pub const PC_PORT: u16 = 18927;

/// Default TCP port for the right-mcp-server HTTP transport.
pub const MCP_HTTP_PORT: u16 = 8100;

/// Persistent state written during `right up`, read during `right down`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeState {
    pub agents: Vec<AgentState>,
    pub socket_path: String,
    pub started_at: String,
    /// TCP port the running process-compose instance listens on.
    ///
    /// Defaults to `PC_PORT` so older state files (pre-isolation fix) deserialize
    /// cleanly and still point callers at the current process-compose.
    #[serde(default = "default_pc_port")]
    pub pc_port: u16,
    /// Bearer token for the process-compose REST API (`PC_API_TOKEN`).
    ///
    /// When set, process-compose rejects unauthenticated requests — prevents
    /// stray HTTP hits from tests or other tools from stopping production bots.
    #[serde(default)]
    pub pc_api_token: Option<String>,
}

fn default_pc_port() -> u16 {
    PC_PORT
}

/// Tracks a single agent in the runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub name: String,
}

/// Write runtime state to a JSON file.
pub fn write_state(state: &RuntimeState, path: &Path) -> miette::Result<()> {
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| miette::miette!("failed to serialize runtime state: {e:#}"))?;
    std::fs::write(path, json).map_err(|e| {
        miette::miette!("failed to write runtime state to {}: {e:#}", path.display())
    })?;
    Ok(())
}

/// Read runtime state from a JSON file.
pub fn read_state(path: &Path) -> miette::Result<RuntimeState> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        miette::miette!(
            "failed to read runtime state from {}: {e:#}",
            path.display()
        )
    })?;
    let state: RuntimeState = serde_json::from_str(&contents)
        .map_err(|e| miette::miette!("failed to parse runtime state: {e:#}"))?;
    Ok(state)
}

/// Generate a random 32-byte URL-safe base64 token for process-compose API auth.
pub fn generate_pc_api_token() -> String {
    use base64::Engine as _;
    use rand::RngExt as _;
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_state_json_roundtrip() {
        let state = RuntimeState {
            agents: vec![
                AgentState {
                    name: "agent1".to_string(),
                },
                AgentState {
                    name: "agent2".to_string(),
                },
            ],
            socket_path: "/tmp/pc.sock".to_string(),
            started_at: "2026-03-22T12:00:00Z".to_string(),
            pc_port: PC_PORT,
            pc_api_token: None,
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let deserialized: RuntimeState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deserialized);
    }

    #[test]
    fn write_state_and_read_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let state = RuntimeState {
            agents: vec![AgentState {
                name: "test-agent".to_string(),
            }],
            socket_path: "/run/pc.sock".to_string(),
            started_at: "2026-03-22T16:00:00Z".to_string(),
            pc_port: PC_PORT,
            pc_api_token: None,
        };

        write_state(&state, &path).unwrap();
        let loaded = read_state(&path).unwrap();
        assert_eq!(state, loaded);
    }

    #[test]
    fn read_state_fails_on_missing_file() {
        let result = read_state(std::path::Path::new("/nonexistent/state.json"));
        assert!(result.is_err());
    }

    #[test]
    fn read_state_fails_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json").unwrap();

        let result = read_state(&path);
        assert!(result.is_err());
    }

    /// Verify that state.json files from v1.0 (with extra fields like
    /// sandbox_name, no_sandbox) can still be deserialized by the new
    /// simplified structs. Serde ignores unknown fields by default.
    #[test]
    fn read_state_ignores_extra_fields_from_v1() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1-state.json");
        let v1_json = r#"{
            "agents": [
                {"name": "agent1", "sandbox_name": "rightclaw-agent1"}
            ],
            "socket_path": "/tmp/pc.sock",
            "started_at": "2026-03-22T12:00:00Z",
            "no_sandbox": false
        }"#;
        std::fs::write(&path, v1_json).unwrap();

        let state = read_state(&path).unwrap();
        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.agents[0].name, "agent1");
        assert_eq!(state.pc_port, PC_PORT);
    }

    #[test]
    fn generate_pc_api_token_returns_url_safe_no_pad_token() {
        let token = generate_pc_api_token();
        assert_eq!(token.len(), 43);
        assert!(
            token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
            "token must be URL-safe base64 without padding: {token}",
        );
    }
}
