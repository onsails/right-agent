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
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_config_deserializes_full_yaml() {
        let yaml = r#"
restart: always
max_restarts: 10
backoff_seconds: 30
start_prompt: "You are a helpful agent"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
        assert_eq!(config.max_restarts, 10);
        assert_eq!(config.backoff_seconds, 30);
        assert_eq!(
            config.start_prompt.as_deref(),
            Some("You are a helpful agent")
        );
    }

    #[test]
    fn agent_config_deserializes_minimal_yaml_with_defaults() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::OnFailure);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.backoff_seconds, 5);
        assert_eq!(config.start_prompt, None);
    }

    #[test]
    fn agent_config_rejects_unknown_fields() {
        let yaml = r#"
restart: never
unknown_field: "should fail"
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn restart_policy_deserializes_never() {
        let yaml = r#"restart: never"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Never);
    }

    #[test]
    fn restart_policy_deserializes_on_failure() {
        let yaml = r#"restart: on_failure"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::OnFailure);
    }

    #[test]
    fn restart_policy_deserializes_always() {
        let yaml = r#"restart: always"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
    }
}
