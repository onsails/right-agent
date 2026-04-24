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

fn default_show_thinking() -> bool {
    true
}

/// Network access policy for sandbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
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
    /// Explicit sandbox name. When set, overrides the deterministic
    /// `rightclaw-{agent_name}` default. Written by migration/restore flows.
    #[serde(default)]
    pub name: Option<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Openshell,
            policy_file: Some(std::path::PathBuf::from("policy.yaml")),
            name: None,
        }
    }
}

/// Memory provider for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryProvider {
    /// File-based memory (MEMORY.md) — default.
    #[default]
    File,
    /// Hindsight Cloud API.
    Hindsight,
}

/// Recall budget level (maps to Hindsight API budget parameter).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallBudget {
    Low,
    #[default]
    Mid,
    High,
}

impl std::fmt::Display for RecallBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecallBudget::Low => write!(f, "low"),
            RecallBudget::Mid => write!(f, "mid"),
            RecallBudget::High => write!(f, "high"),
        }
    }
}

fn default_recall_max_tokens() -> u32 {
    4096
}

/// Memory configuration for an agent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MemoryConfig {
    /// Which memory backend to use.
    #[serde(default)]
    pub provider: MemoryProvider,
    /// Hindsight API key (required when provider=hindsight).
    pub api_key: Option<String>,
    /// Memory bank ID (defaults to agent name).
    pub bank_id: Option<String>,
    /// Recall budget level.
    #[serde(default)]
    pub recall_budget: RecallBudget,
    /// Maximum tokens for recall results.
    #[serde(default = "default_recall_max_tokens")]
    pub recall_max_tokens: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: MemoryProvider::default(),
            api_key: None,
            bank_id: None,
            recall_budget: RecallBudget::default(),
            recall_max_tokens: default_recall_max_tokens(),
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

    /// **DEPRECATED** — source of truth moved to `allowlist.yaml`. Retained
    /// for backward-compatible parsing and one-time migration. On first bot
    /// startup after upgrade, `load_or_migrate_allowlist` seeds `allowlist.yaml`
    /// from this field via `migrate_from_legacy`. Subsequent startups ignore
    /// the field and emit a WARN when it's still populated alongside a present
    /// `allowlist.yaml`. See §Migration in the group-chat spec.
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

    /// Show live thinking indicator in Telegram during CC execution.
    #[serde(default = "default_show_thinking")]
    pub show_thinking: bool,

    /// Memory configuration (optional — defaults to file-based MEMORY.md).
    #[serde(default)]
    pub memory: Option<MemoryConfig>,

    /// Speech-to-text configuration.
    #[serde(default)]
    pub stt: SttConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            restart: RestartPolicy::default(),
            max_restarts: default_max_restarts(),
            backoff_seconds: default_backoff_seconds(),
            network_policy: NetworkPolicy::default(),
            model: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: Vec::new(),
            env: HashMap::new(),
            secret: None,
            attachments: AttachmentsConfig::default(),
            show_thinking: default_show_thinking(),
            memory: None,
            stt: SttConfig::default(),
        }
    }
}

impl AgentConfig {
    /// Whether this agent runs in an OpenShell sandbox (default: true).
    pub fn is_sandboxed(&self) -> bool {
        *self.sandbox_mode() == SandboxMode::Openshell
    }

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

/// Whisper model size for speech-to-text transcription.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
    #[serde(rename = "large-v3")]
    LargeV3,
}

impl WhisperModel {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny.bin",
            Self::Base => "ggml-base.bin",
            Self::Small => "ggml-small.bin",
            Self::Medium => "ggml-medium.bin",
            Self::LargeV3 => "ggml-large-v3.bin",
        }
    }

    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            Self::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            Self::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            Self::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            Self::LargeV3 => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
            }
        }
    }

    pub fn approx_size_mb(&self) -> u64 {
        match self {
            Self::Tiny => 75,
            Self::Base => 150,
            Self::Small => 470,
            Self::Medium => 1500,
            Self::LargeV3 => 3100,
        }
    }

    /// Kebab-case YAML string for this model — mirrors serde's rename_all output.
    pub fn yaml_str(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::LargeV3 => "large-v3",
        }
    }
}

/// Speech-to-text configuration for an agent.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttConfig {
    #[serde(default)] // bool::default() == false
    pub enabled: bool,
    #[serde(default)]
    pub model: WhisperModel,
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

impl AgentDef {
    /// Effective sandbox mode — defaults to Openshell when `config` or `sandbox` section is absent.
    pub fn sandbox_mode(&self) -> &SandboxMode {
        self.config
            .as_ref()
            .map(|c| c.sandbox_mode())
            .unwrap_or(&SandboxMode::Openshell)
    }
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
    fn allowed_chat_ids_still_parses_for_migration() {
        let yaml = "allowed_chat_ids:\n  - 42\n  - -100\n";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.allowed_chat_ids, vec![42, -100]);
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
        assert!(
            result.is_err(),
            "old SandboxOverrides fields must be rejected"
        );
    }

    #[test]
    fn agent_config_without_sandbox_defaults_mode_openshell() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        // sandbox is None — effective mode should be openshell (tested via helper)
        assert!(config.sandbox.is_none());
    }

    #[test]
    fn sandbox_config_with_name() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: "rightclaw-brain-20260415-1430"
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sb = config.sandbox.unwrap();
        assert_eq!(sb.name.as_deref(), Some("rightclaw-brain-20260415-1430"));
    }

    #[test]
    fn sandbox_config_without_name_is_none() {
        let yaml = r#"
sandbox:
  mode: openshell
  policy_file: policy.yaml
"#;
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sb = config.sandbox.unwrap();
        assert!(sb.name.is_none());
    }
}

#[cfg(test)]
mod stt_config_tests {
    use super::*;

    #[test]
    fn stt_config_defaults_when_missing() {
        let yaml = "";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(
            !cfg.stt.enabled,
            "default must be false to grandfather existing agents"
        );
        assert_eq!(cfg.stt.model, WhisperModel::Small);
    }

    #[test]
    fn pre_existing_yaml_without_stt_block_defaults_to_disabled() {
        // Simulates an agent.yaml from before the STT feature shipped:
        // it has other fields but no stt: block.
        let yaml = "telegram_token: \"x\"\nmodel: sonnet\n";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(
            !cfg.stt.enabled,
            "existing agents without stt: block must NOT be silently enabled"
        );
    }

    #[test]
    fn stt_config_explicit_yaml_roundtrip() {
        let yaml = "\
stt:
  enabled: false
  model: tiny
";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(!cfg.stt.enabled);
        assert_eq!(cfg.stt.model, WhisperModel::Tiny);
    }

    #[test]
    fn stt_config_large_v3_kebab_case() {
        let yaml = "\
stt:
  model: large-v3
";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(cfg.stt.model, WhisperModel::LargeV3);
    }

    #[test]
    fn stt_config_invalid_model_errors() {
        let yaml = "\
stt:
  model: huge
";
        let result: Result<AgentConfig, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn whisper_model_filename() {
        assert_eq!(WhisperModel::Tiny.filename(), "ggml-tiny.bin");
        assert_eq!(WhisperModel::Base.filename(), "ggml-base.bin");
        assert_eq!(WhisperModel::Small.filename(), "ggml-small.bin");
        assert_eq!(WhisperModel::Medium.filename(), "ggml-medium.bin");
        assert_eq!(WhisperModel::LargeV3.filename(), "ggml-large-v3.bin");
    }

    #[test]
    fn whisper_model_download_url_is_huggingface() {
        let url = WhisperModel::Small.download_url();
        assert!(url.starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
        assert!(url.ends_with("ggml-small.bin"));
    }

    #[test]
    fn whisper_model_approx_size_mb_is_sane() {
        assert!(WhisperModel::Tiny.approx_size_mb() < 100);
        assert!(WhisperModel::Small.approx_size_mb() < 600);
        assert!(WhisperModel::LargeV3.approx_size_mb() > 2000);
    }
}
