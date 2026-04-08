use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// Restart policy for an agent process.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    #[default]
    Always,
}

fn default_max_restarts() -> u32 {
    3
}

fn default_backoff_seconds() -> u32 {
    3
}

/// Network access policy for sandbox.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// Only allow Anthropic/Claude domains.
    Restrictive,
    /// Allow all outbound HTTPS (default for backwards compat).
    #[default]
    Permissive,
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

    /// Network access policy: restrictive (Anthropic only) or permissive (all HTTPS).
    #[serde(default)]
    pub network_policy: NetworkPolicy,

    /// Claude model to use (e.g. "sonnet", "opus", "haiku")
    pub model: Option<String>,

    /// Per-agent sandbox overrides from `sandbox:` section.
    #[serde(default)]
    pub sandbox: Option<SandboxOverrides>,

    /// Inline Telegram bot token.
    #[serde(default)]
    pub telegram_token: Option<String>,

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

    /// Persistent per-agent secret for deriving Bearer tokens.
    /// Base64url-encoded, 43 characters. Auto-generated if absent.
    #[serde(default)]
    pub secret: Option<String>,

    /// Attachment handling configuration.
    #[serde(default)]
    pub attachments: AttachmentsConfig,
}

/// Configuration for attachment handling.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentsConfig {
    /// How long to keep inbox/outbox files before cleanup (days).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for AttachmentsConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
        }
    }
}

fn default_retention_days() -> u32 {
    7
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
    fn agent_config_telegram_token_field() {
        let yaml = r#"telegram_token: "123:abc""#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token.as_deref(), Some("123:abc"));
    }

    #[test]
    fn agent_config_without_telegram_defaults_to_none() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.telegram_token, None);
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

    #[test]
    fn network_policy_defaults_to_permissive() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }

    #[test]
    fn network_policy_deserializes_restrictive() {
        let yaml = "network_policy: restrictive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Restrictive);
    }

    #[test]
    fn network_policy_deserializes_permissive() {
        let yaml = "network_policy: permissive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }

    #[test]
    fn agent_config_with_attachments_section() {
        let yaml = r#"
attachments:
  retention_days: 14
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.attachments.retention_days, 14);
    }

    #[test]
    fn agent_config_default_attachments() {
        let yaml = "";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.attachments.retention_days, 7);
    }
}
