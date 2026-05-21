//! Agent configuration DTOs and filesystem-discovered agent definitions.

#![warn(unreachable_pub)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whisper model size for speech-to-text transcription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
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

fn default_episode_settle_seconds() -> u64 {
    90
}

fn default_max_daily_budget_usd() -> f64 {
    5.00
}

fn default_circuit_failure_threshold() -> u32 {
    5
}

fn default_circuit_cooldown_minutes() -> u32 {
    60
}

fn default_fork_probe_enabled() -> bool {
    true
}

fn default_background_review_enabled() -> bool {
    false
}

fn deserialize_positive_finite_f64_max_daily<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "max_daily_budget_usd must be finite and > 0.0",
        ))
    }
}

fn deserialize_positive_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > 0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "episode_settle_seconds must be greater than 0",
        ))
    }
}

fn deserialize_positive_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if value > 0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("value must be greater than 0"))
    }
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
    /// Run inside OpenShell container (default: secure).
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
    pub policy_file: Option<PathBuf>,
    /// Explicit sandbox name. When set, overrides the deterministic
    /// `rightclaw-{agent_name}` fallback (kept for backward compatibility
    /// with agents created before the right-agent rename). New agents
    /// (created via `right agent init`) get `right-{agent_name}` written
    /// here explicitly.
    #[serde(default)]
    pub name: Option<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Openshell,
            policy_file: Some(PathBuf::from("policy.yaml")),
            name: None,
        }
    }
}

/// Memory provider for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryProvider {
    /// File-based memory (MEMORY.md) - default.
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

/// Learning-review configuration for an agent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    /// Optional selector model. None means inherit the agent model.
    pub episode_selector_model: Option<String>,

    /// Soft-deprecated. Kept for backward-compatibility with existing
    /// agent.yaml files. Not read by any code. A warn-log is emitted at
    /// agent load time when present (see `warn_on_deprecated`). Slated for
    /// removal in a future release.
    pub episode_selector_max_budget_usd: Option<f64>,

    /// Delay after seed evidence before selecting the episode boundary.
    #[serde(
        default = "default_episode_settle_seconds",
        deserialize_with = "deserialize_positive_u64"
    )]
    pub episode_settle_seconds: u64,

    /// Daily $ budget across all learning invocations.
    #[serde(
        default = "default_max_daily_budget_usd",
        deserialize_with = "deserialize_positive_finite_f64_max_daily"
    )]
    pub max_daily_budget_usd: f64,

    /// Consecutive failures that trip the circuit breaker.
    #[serde(
        default = "default_circuit_failure_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_failure_threshold: u32,

    /// How long the circuit stays open after tripping (minutes).
    #[serde(
        default = "default_circuit_cooldown_minutes",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub circuit_cooldown_minutes: u32,

    /// Model used by the fork-probe. `None` = inherit `AgentConfig.model`.
    pub probe_model: Option<String>,

    /// Master switch for the fork-probe. Defaults `true`; set `false` to
    /// disable post-turn signal-classification probes for an agent.
    #[serde(default = "default_fork_probe_enabled")]
    pub fork_probe_enabled: bool,

    /// Deprecated Stage 2 background pipeline opt-in. Defaults `false`.
    /// When `true`, the legacy `DrainScheduler` + selector + reviewer
    /// run alongside fork-probe; daily budget covers both.
    #[serde(default = "default_background_review_enabled")]
    pub background_review_enabled: bool,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            episode_selector_model: None,
            episode_selector_max_budget_usd: None,
            episode_settle_seconds: default_episode_settle_seconds(),
            max_daily_budget_usd: default_max_daily_budget_usd(),
            circuit_failure_threshold: default_circuit_failure_threshold(),
            circuit_cooldown_minutes: default_circuit_cooldown_minutes(),
            probe_model: None,
            fork_probe_enabled: default_fork_probe_enabled(),
            background_review_enabled: default_background_review_enabled(),
        }
    }
}

impl LearningConfig {
    /// Emit a deprecation warning if the obsolete `episode_selector_max_budget_usd`
    /// field is set in agent.yaml. Call once at agent load time.
    pub fn warn_on_deprecated(&self, agent_name: &str) {
        if let Some(value) = self.episode_selector_max_budget_usd {
            tracing::warn!(
                agent = %agent_name,
                value,
                "agent.yaml: `episode_selector_max_budget_usd` is deprecated and ignored; \
                 use `max_daily_budget_usd` instead. The deprecated field will be removed \
                 in a future release."
            );
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

    /// When `Some`, controls whether `claude -p` runs with --debug --debug-file=...
    /// Hot-reloadable via `/debug` Telegram command. `None` falls back to the
    /// `right bot --debug` CLI flag at boot time.
    #[serde(default)]
    pub debug: Option<bool>,

    /// Per-agent sandbox configuration from `sandbox:` section.
    #[serde(default)]
    pub sandbox: Option<SandboxConfig>,

    /// Inline Telegram bot token.
    #[serde(default)]
    pub telegram_token: Option<String>,

    /// Deprecated: source of truth moved to `allowlist.yaml`. Retained
    /// for backward-compatible parsing and one-time migration. On first bot
    /// startup after upgrade, `load_or_migrate_allowlist` seeds `allowlist.yaml`
    /// from this field via `migrate_from_legacy`. Subsequent startups ignore
    /// the field and emit a WARN when it's still populated alongside a present
    /// `allowlist.yaml`.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,

    /// Per-agent environment variables injected into the shell wrapper before `exec claude`.
    /// Values are stored as-is (plaintext). Single-quoted in the generated wrapper; no
    /// shell expansion, no host variable forwarding.
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

    /// Learning-review configuration.
    #[serde(default)]
    pub learning: LearningConfig,

    /// Memory configuration (optional; defaults to file-based MEMORY.md).
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
            debug: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: Vec::new(),
            env: HashMap::new(),
            secret: None,
            attachments: AttachmentsConfig::default(),
            show_thinking: default_show_thinking(),
            learning: LearningConfig::default(),
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

    /// Effective sandbox mode; defaults to Openshell when `sandbox` section is absent.
    pub fn sandbox_mode(&self) -> &SandboxMode {
        self.sandbox
            .as_ref()
            .map(|s| &s.mode)
            .unwrap_or(&SandboxMode::Openshell)
    }

    /// Resolved policy file path (absolute), or None if mode is None.
    /// Returns Err if mode is Openshell but policy_file is missing.
    pub fn resolve_policy_path(&self, agent_dir: &Path) -> miette::Result<Option<PathBuf>> {
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
                        help = "Run `right agent init <name>` to generate a default policy, or create the file manually",
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

/// Speech-to-text configuration for an agent.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttConfig {
    #[serde(default)]
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
    /// Path to TOOLS.md if present.
    pub tools_path: Option<PathBuf>,
    /// Path to BOOTSTRAP.md if present.
    pub bootstrap_path: Option<PathBuf>,
    /// Path to HEARTBEAT.md if present.
    pub heartbeat_path: Option<PathBuf>,
}

impl AgentDef {
    /// Effective sandbox mode; defaults to Openshell when `config` or `sandbox` section is absent.
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
    fn agent_config_debug_field_defaults_to_none() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, None);
    }

    #[test]
    fn agent_config_debug_true_parses() {
        let yaml = "debug: true";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, Some(true));
    }

    #[test]
    fn agent_config_debug_false_parses() {
        let yaml = "debug: false";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, Some(false));
    }

    #[test]
    fn learning_config_defaults_set_fork_probe_on_and_background_off() {
        let cfg = LearningConfig::default();
        assert!(cfg.fork_probe_enabled, "fork_probe must default ON");
        assert!(
            !cfg.background_review_enabled,
            "background_review must default OFF"
        );
        assert!(
            cfg.probe_model.is_none(),
            "probe_model must default to None (inherit agent.model)"
        );
    }

    #[test]
    fn learning_config_deserialises_minimal_yaml_with_new_defaults() {
        let yaml = "{}";
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(cfg.fork_probe_enabled);
        assert!(!cfg.background_review_enabled);
        assert!(cfg.probe_model.is_none());
    }

    #[test]
    fn learning_config_deserialises_pre_v27_yaml_without_new_fields() {
        let yaml = "
episode_selector_model: claude-sonnet-4-6
episode_settle_seconds: 60
max_daily_budget_usd: 2.50
circuit_failure_threshold: 3
circuit_cooldown_minutes: 30
";
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            cfg.episode_selector_model.as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert!(cfg.fork_probe_enabled);
        assert!(!cfg.background_review_enabled);
        assert_eq!(cfg.max_daily_budget_usd, 2.50);
    }

    #[test]
    fn learning_config_accepts_probe_model_override() {
        let yaml = "probe_model: claude-haiku-4-5-20251001\n";
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            cfg.probe_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn learning_config_accepts_background_review_opt_in() {
        let yaml = "background_review_enabled: true\n";
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(cfg.background_review_enabled);
    }
}
