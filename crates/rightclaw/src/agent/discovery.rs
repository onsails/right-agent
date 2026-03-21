use std::path::{Path, PathBuf};

use crate::agent::types::{AgentConfig, AgentDef};
use crate::error::AgentError;

/// Validate that an agent directory name contains only allowed characters.
///
/// Names must be non-empty, start with an alphanumeric character, and contain
/// only alphanumeric characters, hyphens, or underscores.
pub fn validate_agent_name(name: &str) -> Result<(), AgentError> {
    if name.is_empty() {
        return Err(AgentError::InvalidName {
            name: name.to_string(),
        });
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(AgentError::InvalidName {
            name: name.to_string(),
        });
    }

    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(AgentError::InvalidName {
                name: name.to_string(),
            });
        }
    }

    Ok(())
}

/// Parse `agent.yaml` from an agent directory if it exists.
///
/// Returns `Ok(None)` if no `agent.yaml` is present.
/// Returns `Err` if the file exists but contains invalid YAML or unknown fields.
pub fn parse_agent_config(agent_dir: &Path) -> miette::Result<Option<AgentConfig>> {
    let config_path = agent_dir.join("agent.yaml");
    if !config_path.exists() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&config_path).map_err(|e| AgentError::IoError {
        path: config_path.display().to_string(),
        source: e,
    })?;

    let config: AgentConfig =
        serde_saphyr::from_str(&contents).map_err(|e| AgentError::InvalidConfig {
            name: agent_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
            reason: format!("{e}"),
        })?;

    Ok(Some(config))
}

/// Check if an optional file exists in the agent directory.
/// Returns `Some(path)` if it exists, `None` otherwise.
fn optional_file(agent_dir: &Path, filename: &str) -> Option<PathBuf> {
    let path = agent_dir.join(filename);
    path.exists().then_some(path)
}

/// Discover all valid agents in the given agents directory.
///
/// Scans `agents_dir` for subdirectories, validates each as an agent definition.
/// Agents are sorted by name for deterministic ordering.
///
/// # Errors
///
/// Returns an error if:
/// - The agents directory cannot be read
/// - Any agent directory has an invalid name
/// - Any agent has `IDENTITY.md` but is missing `policy.yaml`
/// - Any agent has an invalid `agent.yaml`
pub fn discover_agents(agents_dir: &Path) -> miette::Result<Vec<AgentDef>> {
    let entries = std::fs::read_dir(agents_dir).map_err(|e| AgentError::IoError {
        path: agents_dir.display().to_string(),
        source: e,
    })?;

    let mut agents = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| AgentError::IoError {
            path: agents_dir.display().to_string(),
            source: e,
        })?;

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        validate_agent_name(&name)?;

        let identity_path = path.join("IDENTITY.md");
        if !identity_path.exists() {
            tracing::warn!(agent = %name, "Skipping directory without IDENTITY.md");
            continue;
        }

        let policy_path = path.join("policy.yaml");
        if !policy_path.exists() {
            return Err(AgentError::MissingRequiredFile {
                name: name.clone(),
                file: "policy.yaml".to_string(),
            }
            .into());
        }

        let config = parse_agent_config(&path)?;

        let agent = AgentDef {
            name,
            identity_path,
            policy_path,
            config,
            mcp_config_path: optional_file(&path, ".mcp.json"),
            soul_path: optional_file(&path, "SOUL.md"),
            user_path: optional_file(&path, "USER.md"),
            memory_path: optional_file(&path, "MEMORY.md"),
            agents_path: optional_file(&path, "AGENTS.md"),
            tools_path: optional_file(&path, "TOOLS.md"),
            bootstrap_path: optional_file(&path, "BOOTSTRAP.md"),
            heartbeat_path: optional_file(&path, "HEARTBEAT.md"),
            path,
        };

        agents.push(agent);
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
