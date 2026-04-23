use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::runtime::pc_client::PC_PORT;

/// Persistent state written during `rightclaw up`, read during `rightclaw down`.
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

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
