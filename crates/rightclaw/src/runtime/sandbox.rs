use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persistent state written during `rightclaw up`, read during `rightclaw down`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeState {
    pub agents: Vec<AgentState>,
    pub socket_path: String,
    pub started_at: String,
}

/// Tracks a single agent's sandbox identity for cleanup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub name: String,
    pub sandbox_name: String,
}

/// Returns the deterministic sandbox name for an agent: `rightclaw-{agent_name}`.
pub fn sandbox_name_for(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}

/// Write runtime state to a JSON file.
pub fn write_state(state: &RuntimeState, path: &Path) -> miette::Result<()> {
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| miette::miette!("failed to serialize runtime state: {e:#}"))?;
    std::fs::write(path, json)
        .map_err(|e| miette::miette!("failed to write runtime state to {}: {e:#}", path.display()))?;
    Ok(())
}

/// Read runtime state from a JSON file.
pub fn read_state(path: &Path) -> miette::Result<RuntimeState> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("failed to read runtime state from {}: {e:#}", path.display()))?;
    let state: RuntimeState = serde_json::from_str(&contents)
        .map_err(|e| miette::miette!("failed to parse runtime state: {e:#}"))?;
    Ok(state)
}

/// Destroy all OpenShell sandboxes for the given agents.
///
/// This is best-effort: individual sandbox deletions may fail (e.g. sandbox
/// already gone) without causing the overall operation to fail. This is the
/// ONE exception to the fail-fast principle -- cleanup must attempt all agents.
pub fn destroy_sandboxes(agents: &[AgentState]) -> miette::Result<()> {
    for agent in agents {
        tracing::info!(sandbox = %agent.sandbox_name, "destroying sandbox");
        let output = std::process::Command::new("openshell")
            .args(["sandbox", "delete", &agent.sandbox_name])
            .output();

        match output {
            Ok(result) if !result.status.success() => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                tracing::warn!(
                    sandbox = %agent.sandbox_name,
                    "sandbox delete failed (may already be gone): {stderr}"
                );
            }
            Err(e) => {
                tracing::warn!(
                    sandbox = %agent.sandbox_name,
                    "failed to run openshell: {e:#}"
                );
            }
            Ok(_) => {
                tracing::info!(sandbox = %agent.sandbox_name, "sandbox destroyed");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
