use std::path::PathBuf;

use serde::Deserialize;

/// Restart policy for an agent process.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    #[default]
    OnFailure,
    Always,
}

fn default_max_restarts() -> u32 {
    3
}

fn default_backoff_seconds() -> u32 {
    5
}

/// Parsed `agent.yaml` configuration for a single agent.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default)]
    pub restart: RestartPolicy,

    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,

    #[serde(default = "default_backoff_seconds")]
    pub backoff_seconds: u32,

    pub start_prompt: Option<String>,
}

/// A discovered agent definition from the filesystem.
#[derive(Debug, Clone)]
pub struct AgentDef {
    /// Directory name (validated: alphanumeric, hyphens, underscores).
    pub name: String,
    /// Absolute path to the agent directory.
    pub path: PathBuf,
    /// Path to IDENTITY.md (required).
    pub identity_path: PathBuf,
    /// Path to policy.yaml (required).
    pub policy_path: PathBuf,
    /// Parsed agent.yaml if present.
    pub config: Option<AgentConfig>,
    /// Path to .mcp.json if present.
    pub mcp_config_path: Option<PathBuf>,
    /// Path to SOUL.md if present.
    pub soul_path: Option<PathBuf>,
    /// Path to USER.md if present.
    pub user_path: Option<PathBuf>,
    /// Path to MEMORY.md if present.
    pub memory_path: Option<PathBuf>,
    /// Path to AGENTS.md if present.
    pub agents_path: Option<PathBuf>,
    /// Path to TOOLS.md if present.
    pub tools_path: Option<PathBuf>,
    /// Path to BOOTSTRAP.md if present.
    pub bootstrap_path: Option<PathBuf>,
    /// Path to HEARTBEAT.md if present.
    pub heartbeat_path: Option<PathBuf>,
}
