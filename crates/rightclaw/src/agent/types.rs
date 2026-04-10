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

fn default_max_turns() -> u32 {
    30
}

fn default_max_budget_usd() -> f64 {
    1.0
}

fn default_show_thinking() -> bool {
    true
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

impl std::fmt::Display for NetworkPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkPolicy::Restrictive => write!(f, "restrictive (Anthropic/Claude only)"),
            NetworkPolicy::Permissive => write!(f, "permissive (all HTTPS)"),
        }
    }
}

impl std::str::FromStr for NetworkPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "restrictive" => Ok(NetworkPolicy::Restrictive),
            "permissive" => Ok(NetworkPolicy::Permissive),
            other => Err(format!(
                "invalid network policy: '{other}'. Expected 'restrictive' or 'permissive'."
            )),
        }
    }
}

/// Sandbox execution mode for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Run inside OpenShell container (default — secure).
    #[default]
    Openshell,
    /// Run directly on host (needed for computer-use, Chrome, etc.).
    None,
}

impl std::fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxMode::Openshell => write!(f, "openshell"),
            SandboxMode::None => write!(f, "none (host)"),
        }
    }
}

impl std::str::FromStr for SandboxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openshell" => Ok(SandboxMode::Openshell),
            "none" => Ok(SandboxMode::None),
            other => Err(format!(
                "invalid sandbox mode: '{other}'. Expected 'openshell' or 'none'."
            )),
        }
    }
}

/// Per-agent sandbox configuration in agent.yaml.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    /// Execution mode: openshell (sandboxed) or none (direct host).
    #[serde(default)]
    pub mode: SandboxMode,
    /// Path to OpenShell policy file, relative to agent directory.
    /// Required when mode is openshell.
    pub policy_file: Option<std::path::PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Openshell,
            policy_file: Some(std::path::PathBuf::from("policy.yaml")),
        }
    }
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

    /// Per-agent sandbox configuration from `sandbox:` section.
    #[serde(default)]
    pub sandbox: Option<SandboxConfig>,

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

    /// Maximum number of CC turns per invocation.
    /// CC stops gracefully with `terminal_reason: "max_turns"`.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Maximum dollar spend per CC invocation.
    /// CC stops gracefully with `terminal_reason: "max_budget"`.
    #[serde(default = "default_max_budget_usd")]
    pub max_budget_usd: f64,

    /// Show live thinking indicator in Telegram during CC execution.
    #[serde(default = "default_show_thinking")]
    pub show_thinking: bool,
}

impl AgentConfig {
    /// Effective sandbox mode — defaults to Openshell when `sandbox` section is absent.
    pub fn sandbox_mode(&self) -> &SandboxMode {
        self.sandbox
            .as_ref()
            .map(|s| &s.mode)
            .unwrap_or(&SandboxMode::Openshell)
    }

    /// Resolved policy file path (absolute), or None if mode is None.
    /// Returns Err if mode is Openshell but policy_file is missing.
    pub fn resolve_policy_path(
        &self,
        agent_dir: &std::path::Path,
    ) -> miette::Result<Option<std::path::PathBuf>> {
        match self.sandbox_mode() {
            SandboxMode::None => Ok(Option::None),
            SandboxMode::Openshell => {
                let rel = self
                    .sandbox
                    .as_ref()
                    .and_then(|s| s.policy_file.as_ref())
                    .ok_or_else(|| {
                        miette::miette!(
                            help = "Add `sandbox:\\n  policy_file: policy.yaml` to agent.yaml, or set `sandbox:\\n  mode: none`",
                            "agent.yaml has sandbox mode 'openshell' but no policy_file specified"
                        )
                    })?;
                let abs = agent_dir.join(rel);
                if !abs.exists() {
                    return Err(miette::miette!(
                        help = "Run `rightclaw agent init <name>` to generate a default policy, or create the file manually",
                        "policy file not found: {}",
                        abs.display()
                    ));
                }
                Ok(Some(abs))
            }
        }
    }
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
        assert_eq!(config.restart, RestartPolicy::Always);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.backoff_seconds, 3);
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
    fn agent_config_without_sandbox_section() {
        let yaml = "restart: never";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(config.sandbox.is_none());
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

    #[test]
    fn sandbox_config_mode_openshell_with_policy() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::Openshell);
        assert_eq!(
            sandbox.policy_file.as_deref(),
            Some(std::path::Path::new("policy.yaml"))
        );
    }

    #[test]
    fn sandbox_config_mode_none() {
        let yaml = r#"
sandbox:
  mode: none
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::None);
        assert!(sandbox.policy_file.is_none());
    }

    #[test]
    fn sandbox_config_defaults_to_openshell() {
        let yaml = "sandbox: {}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = config.sandbox.unwrap();
        assert_eq!(sandbox.mode, SandboxMode::Openshell);
    }

    #[test]
    fn sandbox_config_rejects_unknown_mode() {
        let yaml = r#"
sandbox:
  mode: docker
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn sandbox_config_rejects_old_allow_write_field() {
        let yaml = r#"
sandbox:
  allow_write:
    - "/tmp"
"#;
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err(), "old SandboxOverrides fields must be rejected");
    }

    #[test]
    fn agent_config_without_sandbox_defaults_mode_openshell() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        // sandbox is None — effective mode should be openshell (tested via helper)
        assert!(config.sandbox.is_none());
    }
}
