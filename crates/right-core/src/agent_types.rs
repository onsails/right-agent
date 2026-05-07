use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use crate::stt::WhisperModel;

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
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
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
