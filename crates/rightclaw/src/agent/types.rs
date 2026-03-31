use std::collections::HashMap;
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

/// Per-agent sandbox overrides defined in agent.yaml.
///
/// All arrays MERGE with generated defaults (D-08).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxOverrides {
    /// Additional paths to allow writing (appended to defaults).
    #[serde(default)]
    pub allow_write: Vec<String>,

    /// Additional paths to allow reading (appended to defaults, D-09b).
    #[serde(default)]
    pub allow_read: Vec<String>,

    /// Additional domains to allow (appended to defaults).
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Commands to exclude from sandbox (appended to defaults).
    #[serde(default)]
    pub excluded_commands: Vec<String>,
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

    /// Claude model to use (e.g. "sonnet", "opus", "haiku")
    pub model: Option<String>,

    /// Per-agent sandbox overrides from `sandbox:` section.
    #[serde(default)]
    pub sandbox: Option<SandboxOverrides>,

    /// Path to file containing the Telegram bot token, relative to the agent directory.
    /// Takes precedence over `telegram_token` if both are set.
    #[serde(default)]
    pub telegram_token_file: Option<String>,

    /// Inline Telegram bot token. Fallback if `telegram_token_file` is not set.
    /// Prefer `telegram_token_file` to avoid committing secrets into agent.yaml.
    #[serde(default)]
    pub telegram_token: Option<String>,

    /// Numeric Telegram user ID for access.json pre-pairing.
    /// If absent, access.json is not written; user must pair interactively.
    #[serde(default)]
    pub telegram_user_id: Option<String>,

    /// Telegram chat IDs permitted to interact with this agent's bot.
    /// Empty vec = block all incoming messages (secure default). Bot emits
    /// `tracing::warn!` at startup when empty — see D-05.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,

    /// Per-agent environment variables injected into the shell wrapper before `exec claude`.
    /// Values are stored as-is (plaintext). Single-quoted in the generated wrapper — no
    /// shell expansion, no host variable forwarding. See D-01 in phase 11 context.
    #[serde(default)]
    pub env: HashMap<String, String>,
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
    /// Parsed agent.yaml if present.
    pub config: Option<AgentConfig>,
    /// Path to SOUL.md if present.
    pub soul_path: Option<PathBuf>,
    /// Path to USER.md if present.
    pub user_path: Option<PathBuf>,
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
    fn agent_config_telegram_token_file_field() {
        let yaml = r#"telegram_token_file: ".telegram.env""#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token_file.as_deref(), Some(".telegram.env"));
        assert_eq!(config.telegram_token, None);
        assert_eq!(config.telegram_user_id, None);
    }

    #[test]
    fn agent_config_telegram_token_field() {
        let yaml = r#"telegram_token: "123:abc""#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token.as_deref(), Some("123:abc"));
        assert_eq!(config.telegram_token_file, None);
        assert_eq!(config.telegram_user_id, None);
    }

    #[test]
    fn agent_config_telegram_user_id_field() {
        let yaml = r#"telegram_user_id: "987654321""#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_user_id.as_deref(), Some("987654321"));
        assert_eq!(config.telegram_token_file, None);
        assert_eq!(config.telegram_token, None);
    }

    #[test]
    fn agent_config_without_telegram_defaults_to_none() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token_file, None);
        assert_eq!(config.telegram_token, None);
        assert_eq!(config.telegram_user_id, None);
    }

    #[test]
    fn agent_config_all_telegram_fields_together() {
        let yaml = r#"
telegram_token_file: ".telegram.env"
telegram_token: "123:abc"
telegram_user_id: "987654321"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token_file.as_deref(), Some(".telegram.env"));
        assert_eq!(config.telegram_token.as_deref(), Some("123:abc"));
        assert_eq!(config.telegram_user_id.as_deref(), Some("987654321"));
    }

    #[test]
    fn agent_config_deserializes_full_yaml() {
        let yaml = r#"
restart: always
max_restarts: 10
backoff_seconds: 30
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::Always);
        assert_eq!(config.max_restarts, 10);
        assert_eq!(config.backoff_seconds, 30);
    }

    #[test]
    fn agent_config_deserializes_minimal_yaml_with_defaults() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.restart, RestartPolicy::OnFailure);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.backoff_seconds, 5);
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

    #[test]
    fn agent_config_with_sandbox_overrides() {
        let yaml = r#"
restart: on_failure
sandbox:
  allow_write:
    - "/tmp/builds"
  allow_read:
    - "/data/shared"
  allowed_domains:
    - "registry.npmjs.org"
  excluded_commands:
    - "docker"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.allow_write, vec!["/tmp/builds"]);
        assert_eq!(sandbox.allow_read, vec!["/data/shared"]);
        assert_eq!(sandbox.allowed_domains, vec!["registry.npmjs.org"]);
        assert_eq!(sandbox.excluded_commands, vec!["docker"]);
    }

    #[test]
    fn sandbox_overrides_deserializes_allow_read() {
        let yaml = r#"
sandbox:
  allow_read:
    - "/data/shared"
    - "/mnt/datasets"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.allow_read, vec!["/data/shared", "/mnt/datasets"]);
    }

    #[test]
    fn sandbox_overrides_allow_read_defaults_empty() {
        let yaml = "sandbox: {}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert!(sandbox.allow_read.is_empty(), "allow_read should default to empty vec");
    }

    #[test]
    fn agent_config_without_sandbox_section() {
        let yaml = "restart: never";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(config.sandbox.is_none());
    }

    #[test]
    fn sandbox_overrides_empty_section() {
        let yaml = "sandbox: {}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert!(sandbox.allow_write.is_empty());
        assert!(sandbox.allow_read.is_empty());
        assert!(sandbox.allowed_domains.is_empty());
        assert!(sandbox.excluded_commands.is_empty());
    }

    #[test]
    fn sandbox_overrides_rejects_unknown_fields() {
        let yaml = r#"
sandbox:
  allow_write:
    - "/tmp"
  unknown_field: "bad"
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
    fn agent_config_allowed_chat_ids_deserializes_list() {
        let yaml = "allowed_chat_ids:\n  - 123456789\n  - -1001234567890";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            config.allowed_chat_ids,
            vec![123456789_i64, -1001234567890_i64]
        );
    }

    #[test]
    fn agent_config_allowed_chat_ids_defaults_to_empty() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(config.allowed_chat_ids.is_empty());
    }

    #[test]
    fn agent_config_allowed_chat_ids_absent_does_not_reject() {
        let yaml = "restart: never\nmax_restarts: 5";
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_ok());
        assert!(result.unwrap().allowed_chat_ids.is_empty());
    }

    #[test]
    fn agent_config_allowed_chat_ids_negative_values() {
        let yaml = "allowed_chat_ids:\n  - -100";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.allowed_chat_ids, vec![-100_i64]);
    }
}
