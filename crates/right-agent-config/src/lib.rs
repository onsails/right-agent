//! Agent configuration DTOs and filesystem-discovered agent definitions.

#![warn(unreachable_pub)]

use std::collections::HashMap;
use std::path::PathBuf;

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

fn default_prefilter_enabled() -> bool {
    true
}
fn default_prefilter_model() -> Option<String> {
    Some("claude-haiku-4-5-20251001".to_owned())
}
fn default_probe_writer_enabled() -> bool {
    true
}
fn default_curator_enabled() -> bool {
    true
}
fn default_curator_interval_hours() -> u32 {
    168
}
fn default_curator_min_idle_hours() -> u32 {
    2
}
fn default_curator_stale_after_days() -> u32 {
    30
}
fn default_curator_archive_after_days() -> u32 {
    90
}
fn default_curator_paused() -> bool {
    false
}
fn default_curator_cost_spike_k() -> f64 {
    3.0
}
fn default_curator_cost_spike_baseline_days() -> u32 {
    14
}
fn default_curator_cost_spike_min_floor_usd() -> f64 {
    0.05
}
fn default_curator_skill_change_threshold() -> u32 {
    3
}
fn default_curator_min_cooldown_hours() -> u32 {
    12
}
fn default_curator_circuit_failure_threshold() -> u32 {
    3
}
fn default_curator_circuit_cooldown_hours() -> u32 {
    24
}
fn default_baseline_window_days() -> u32 {
    14
}
fn default_baseline_min_sample() -> u32 {
    20
}
fn default_max_daily_budget_usd_v2() -> f64 {
    1.00
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

fn deserialize_positive_finite_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("value must be finite and > 0.0"))
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

/// Rejection message for the removed sandboxless mode. Emitted verbatim when an
/// agent.yaml still carries `sandbox: mode: none`.
const SANDBOXLESS_REMOVED: &str = "`sandbox.mode: none` is no longer supported — every agent runs inside a microsandbox VM. \
Run `right agent migrate-sandbox <agent>` to move this agent's host files into a sandbox, \
then delete the `mode:` line from agent.yaml.";

/// Rejection message for an agent that still lives in an OpenShell sandbox.
/// Emitted verbatim when an agent.yaml still carries `sandbox: mode: openshell`
/// — the marker every pre-microsandbox `right agent init` wrote and only
/// `right agent migrate-sandbox` removes. Starting such an agent would attach
/// it to an *empty* microsandbox VM while its real home still sits in the
/// OpenShell sandbox, so it must fail before anything runs. Public so the CLI
/// can assert the command it advertises actually exists.
pub const OPENSHELL_UNMIGRATED: &str = "`sandbox.mode: openshell` means this agent's files still live in an OpenShell sandbox, which Right no longer runs. \
Run `right agent migrate-sandbox <agent>` to move them into a microsandbox VM — it rewrites this agent.yaml for you. \
Until then the agent cannot start: attaching it to a fresh sandbox would hand it an empty home.";

/// Provider type slug. Built-in slugs are validated against
/// `right_providers::catalog` at API boundaries; `claude` is reserved for the
/// in-sandbox Claude Code login flow and is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ProviderType {
    /// `"generic"` — custom user-defined provider.
    #[serde(deserialize_with = "deserialize_generic_marker")]
    Generic,
    /// Built-in slug like `"anthropic"`, `"github"`, etc.
    BuiltIn(String),
}

fn deserialize_generic_marker<'de, D: serde::Deserializer<'de>>(d: D) -> Result<(), D::Error> {
    let s: String = serde::Deserialize::deserialize(d)?;
    if s == "generic" {
        Ok(())
    } else {
        Err(serde::de::Error::custom("expected \"generic\""))
    }
}

impl serde::Serialize for ProviderType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ProviderType::Generic => s.serialize_str("generic"),
            ProviderType::BuiltIn(slug) => s.serialize_str(slug),
        }
    }
}

/// Generic-only fields. Multi-host; the agent writes the auth header itself
/// around the injected placeholder, so no header/scheme field exists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "GenericProviderRaw")]
pub struct GenericProvider {
    pub env_var: String,
    pub upstream_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_path_prefix: Option<String>,
}

#[derive(Deserialize)]
struct GenericProviderRaw {
    env_var: String,
    #[serde(default)]
    upstream_host: Option<String>,
    #[serde(default)]
    upstream_hosts: Option<Vec<String>>,
    #[serde(default)]
    upstream_path_prefix: Option<String>,
    #[serde(default, rename = "header_name")]
    _legacy_header_name: Option<String>,
}

impl TryFrom<GenericProviderRaw> for GenericProvider {
    type Error = String;

    fn try_from(r: GenericProviderRaw) -> Result<Self, String> {
        let mut hosts: Vec<String> = Vec::new();
        if let Some(h) = r.upstream_host {
            let h = h.trim();
            if !h.is_empty() {
                hosts.push(h.to_string());
            }
        }
        if let Some(extra) = r.upstream_hosts {
            hosts.extend(extra.into_iter().filter_map(|h| {
                let h = h.trim();
                if h.is_empty() {
                    None
                } else {
                    Some(h.to_string())
                }
            }));
        }
        let mut seen = std::collections::HashSet::new();
        hosts.retain(|h| seen.insert(h.clone()));
        if hosts.is_empty() {
            return Err("generic provider requires at least one upstream host".into());
        }
        Ok(GenericProvider {
            env_var: r.env_var,
            upstream_hosts: hosts,
            upstream_path_prefix: r.upstream_path_prefix,
        })
    }
}

/// One provider attached to an agent's sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<GenericProvider>,
}

/// Per-agent sandbox configuration in agent.yaml.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(try_from = "SandboxConfigRaw")]
pub struct SandboxConfig {
    /// Explicit sandbox name. When set, overrides the deterministic
    /// `right_sandbox::sandbox_name(agent)` derivation (`right-{agent_name}`).
    /// New agents (created via `right agent init`) get the derived name
    /// written here explicitly.
    pub name: Option<String>,
    /// Providers attached to this sandbox. Empty by default. Per-agent source of truth.
    pub providers: Vec<ProviderEntry>,
}

/// Wire shape of `sandbox:`. It still carries the two retired keys so an
/// agent.yaml written before the microsandbox migration keeps *loading* far
/// enough to be diagnosed: every `mode:` value is now rejected — `none` as
/// removed sandboxless mode ([`SANDBOXLESS_REMOVED`]), `openshell` as an
/// unmigrated agent ([`OPENSHELL_UNMIGRATED`]) — and `policy_file` is inert
/// now that OpenShell policy files are gone. Both keys are dropped one release
/// from now.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxConfigRaw {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default, rename = "policy_file")]
    _policy_file: Option<PathBuf>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    providers: Vec<ProviderEntry>,
}

impl TryFrom<SandboxConfigRaw> for SandboxConfig {
    type Error = String;

    fn try_from(raw: SandboxConfigRaw) -> Result<Self, Self::Error> {
        match raw.mode.as_deref() {
            None => {}
            Some("none") => return Err(SANDBOXLESS_REMOVED.to_owned()),
            Some("openshell") => return Err(OPENSHELL_UNMIGRATED.to_owned()),
            Some(other) => {
                return Err(format!(
                    "invalid sandbox mode: '{other}'. The `mode:` key is retired; delete it."
                ));
            }
        }
        Ok(SandboxConfig {
            name: raw.name,
            providers: raw.providers,
        })
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

/// Curator execution mode. `Apply` (default) writes consolidations to disk and
/// the lifecycle DB; `ReportOnly` runs a read-only LLM pass that proposes
/// consolidations without writing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CuratorMode {
    #[default]
    Apply,
    ReportOnly,
}

/// Learning-loop configuration (probe-writer + curator pipeline).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct LearningConfig {
    /// Haiku classifier before probe-writer spawn.
    #[serde(default = "default_prefilter_enabled")]
    pub prefilter_enabled: bool,
    #[serde(default = "default_prefilter_model")]
    pub prefilter_model: Option<String>,

    /// Probe-writer fork after each foreground turn.
    #[serde(default = "default_probe_writer_enabled")]
    pub probe_writer_enabled: bool,
    /// Inherit AgentConfig.model when None.
    pub probe_writer_model: Option<String>,

    /// Periodic skill curator.
    #[serde(default = "default_curator_enabled")]
    pub curator_enabled: bool,
    pub curator_model: Option<String>,
    #[serde(
        default = "default_curator_interval_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_interval_hours: u32,
    #[serde(
        default = "default_curator_min_idle_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_min_idle_hours: u32,
    #[serde(
        default = "default_curator_stale_after_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_stale_after_days: u32,
    #[serde(
        default = "default_curator_archive_after_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_archive_after_days: u32,
    #[serde(default = "default_curator_paused")]
    pub curator_paused: bool,

    /// Multiplier on 14-day P50 probe-writer cost; ≥ k * P50 in last 24h
    /// triggers an early curator run.
    #[serde(
        default = "default_curator_cost_spike_k",
        deserialize_with = "deserialize_positive_finite_f64"
    )]
    pub curator_cost_spike_k: f64,
    #[serde(
        default = "default_curator_cost_spike_baseline_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_cost_spike_baseline_days: u32,
    /// Absolute floor on 24h probe-writer spend below which the cost-spike
    /// trigger never fires — protects low-activity agents.
    #[serde(
        default = "default_curator_cost_spike_min_floor_usd",
        deserialize_with = "deserialize_positive_finite_f64"
    )]
    pub curator_cost_spike_min_floor_usd: f64,
    /// Skills created/patched since last curator run; ≥ threshold triggers a
    /// run.
    #[serde(
        default = "default_curator_skill_change_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_skill_change_threshold: u32,
    /// Hard cooldown between curator runs — gates every trigger including the
    /// 168h fallback.
    #[serde(
        default = "default_curator_min_cooldown_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_min_cooldown_hours: u32,
    /// Consecutive failed curator passes before the circuit opens.
    #[serde(
        default = "default_curator_circuit_failure_threshold",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_circuit_failure_threshold: u32,
    /// Fixed cooldown (hours) the circuit stays open once tripped.
    #[serde(
        default = "default_curator_circuit_cooldown_hours",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub curator_circuit_cooldown_hours: u32,
    /// `apply` (write) or `report_only` (propose without writing).
    #[serde(default)]
    pub curator_mode: CuratorMode,

    /// Window for prefilter per-agent turn baselines.
    #[serde(
        default = "default_baseline_window_days",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub baseline_window_days: u32,
    /// Minimum sample size in window for prefilter baselines to be considered
    /// sufficient. Below this, the prompt notes "baseline insufficient."
    #[serde(
        default = "default_baseline_min_sample",
        deserialize_with = "deserialize_positive_u32"
    )]
    pub baseline_min_sample: u32,

    /// Daily $ budget shared by probe-writer and curator.
    #[serde(
        default = "default_max_daily_budget_usd_v2",
        deserialize_with = "deserialize_positive_finite_f64_max_daily"
    )]
    pub max_daily_budget_usd: f64,

    /// Deprecated fields kept for forward compat (silently ignored).
    /// `serde(default)` on the struct accepts their presence without error.
    #[serde(default)]
    pub fork_probe_enabled: Option<bool>,
    #[serde(default)]
    pub fork_probe_model: Option<String>,
    #[serde(default, rename = "probe_model")]
    pub legacy_probe_model: Option<String>,
    #[serde(default)]
    pub background_review_enabled: Option<bool>,
    #[serde(default)]
    pub episode_selector_model: Option<String>,
    #[serde(default)]
    pub episode_selector_max_budget_usd: Option<f64>,
    #[serde(default)]
    pub episode_settle_seconds: Option<u64>,
    #[serde(default)]
    pub circuit_failure_threshold: Option<u32>,
    #[serde(default)]
    pub circuit_cooldown_minutes: Option<u32>,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            prefilter_enabled: default_prefilter_enabled(),
            prefilter_model: default_prefilter_model(),
            probe_writer_enabled: default_probe_writer_enabled(),
            probe_writer_model: None,
            curator_enabled: default_curator_enabled(),
            curator_model: None,
            curator_interval_hours: default_curator_interval_hours(),
            curator_min_idle_hours: default_curator_min_idle_hours(),
            curator_stale_after_days: default_curator_stale_after_days(),
            curator_archive_after_days: default_curator_archive_after_days(),
            curator_paused: default_curator_paused(),
            curator_cost_spike_k: default_curator_cost_spike_k(),
            curator_cost_spike_baseline_days: default_curator_cost_spike_baseline_days(),
            curator_cost_spike_min_floor_usd: default_curator_cost_spike_min_floor_usd(),
            curator_skill_change_threshold: default_curator_skill_change_threshold(),
            curator_min_cooldown_hours: default_curator_min_cooldown_hours(),
            curator_circuit_failure_threshold: default_curator_circuit_failure_threshold(),
            curator_circuit_cooldown_hours: default_curator_circuit_cooldown_hours(),
            curator_mode: CuratorMode::default(),
            baseline_window_days: default_baseline_window_days(),
            baseline_min_sample: default_baseline_min_sample(),
            max_daily_budget_usd: default_max_daily_budget_usd_v2(),
            fork_probe_enabled: None,
            fork_probe_model: None,
            legacy_probe_model: None,
            background_review_enabled: None,
            episode_selector_model: None,
            episode_selector_max_budget_usd: None,
            episode_settle_seconds: None,
            circuit_failure_threshold: None,
            circuit_cooldown_minutes: None,
        }
    }
}

impl LearningConfig {
    /// Emit one warn at load-time if a deprecated field is set in agent.yaml.
    pub fn warn_on_deprecated(&self, agent_name: &str) {
        let pairs: [(&str, bool); 9] = [
            ("fork_probe_enabled", self.fork_probe_enabled.is_some()),
            ("fork_probe_model", self.fork_probe_model.is_some()),
            ("probe_model", self.legacy_probe_model.is_some()),
            (
                "background_review_enabled",
                self.background_review_enabled.is_some(),
            ),
            (
                "episode_selector_model",
                self.episode_selector_model.is_some(),
            ),
            (
                "episode_selector_max_budget_usd",
                self.episode_selector_max_budget_usd.is_some(),
            ),
            (
                "episode_settle_seconds",
                self.episode_settle_seconds.is_some(),
            ),
            (
                "circuit_failure_threshold",
                self.circuit_failure_threshold.is_some(),
            ),
            (
                "circuit_cooldown_minutes",
                self.circuit_cooldown_minutes.is_some(),
            ),
        ];
        for (field, present) in pairs {
            if present {
                tracing::warn!(
                    agent = %agent_name,
                    field,
                    "agent.yaml: `{field}` is deprecated and ignored. See \
                     docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md."
                );
            }
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
    /// Declared `sandbox.providers`, or an empty slice when the `sandbox`
    /// section is absent. This is the agent-local list of provider names the
    /// store resolves into the sandbox's secret bindings.
    pub fn providers(&self) -> &[ProviderEntry] {
        self.sandbox
            .as_ref()
            .map(|s| s.providers.as_slice())
            .unwrap_or(&[])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_mode_none_is_rejected_with_migration_help() {
        let error = serde_saphyr::from_str::<AgentConfig>("sandbox: { mode: none }").unwrap_err();

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("no longer supported"),
            "unexpected error: {rendered}"
        );
        assert!(
            rendered.contains("right agent migrate-sandbox"),
            "error must name the migration path: {rendered}"
        );
    }

    #[test]
    fn sandbox_mode_openshell_is_rejected_as_unmigrated() {
        let error = serde_saphyr::from_str::<AgentConfig>(
            "sandbox:\n  mode: openshell\n  name: right-alpha\n",
        )
        .unwrap_err();

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("still live in an OpenShell sandbox"),
            "unexpected error: {rendered}"
        );
        assert!(
            rendered.contains("right agent migrate-sandbox"),
            "error must name the migration command: {rendered}"
        );
    }

    #[test]
    fn sandbox_policy_file_is_accepted_and_ignored() {
        let config: AgentConfig =
            serde_saphyr::from_str("sandbox:\n  policy_file: custom-policy.yaml\n").unwrap();

        let sandbox = config.sandbox.expect("sandbox section");
        assert_eq!(sandbox.name, None);
        assert!(sandbox.providers.is_empty());
    }

    #[test]
    fn sandbox_unknown_mode_is_rejected() {
        let error = serde_saphyr::from_str::<AgentConfig>("sandbox: { mode: bogus }").unwrap_err();

        assert!(
            format!("{error:#}").contains("invalid sandbox mode: 'bogus'"),
            "unexpected error: {error:#}"
        );
    }

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
    fn learning_config_defaults_use_new_fields() {
        let cfg = LearningConfig::default();
        assert!(cfg.prefilter_enabled);
        assert_eq!(
            cfg.prefilter_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert!(cfg.probe_writer_enabled);
        assert!(cfg.probe_writer_model.is_none());
        assert!(cfg.curator_enabled);
        assert!(cfg.curator_model.is_none());
        assert_eq!(cfg.curator_interval_hours, 168);
        assert_eq!(cfg.curator_min_idle_hours, 2);
        assert_eq!(cfg.curator_stale_after_days, 30);
        assert_eq!(cfg.curator_archive_after_days, 90);
        assert!(!cfg.curator_paused);
        assert_eq!(cfg.max_daily_budget_usd, 1.00);
    }

    #[test]
    fn default_learning_has_new_curator_trigger_fields() {
        let cfg = LearningConfig::default();
        assert!((cfg.curator_cost_spike_k - 3.0).abs() < 1e-9);
        assert_eq!(cfg.curator_cost_spike_baseline_days, 14);
        assert!((cfg.curator_cost_spike_min_floor_usd - 0.05).abs() < 1e-9);
        assert_eq!(cfg.curator_skill_change_threshold, 3);
        assert_eq!(cfg.curator_min_cooldown_hours, 12);
        assert_eq!(cfg.baseline_window_days, 14);
        assert_eq!(cfg.baseline_min_sample, 20);
        assert_eq!(cfg.curator_circuit_failure_threshold, 3);
        assert_eq!(cfg.curator_circuit_cooldown_hours, 24);
        assert_eq!(cfg.curator_mode, CuratorMode::Apply);
    }

    #[test]
    fn learning_yaml_accepts_missing_new_fields_via_defaults() {
        let yaml = "prefilter_enabled: true";
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(cfg.curator_skill_change_threshold, 3);
        assert!((cfg.curator_cost_spike_k - 3.0).abs() < 1e-9);
    }

    #[test]
    fn learning_config_deprecated_fields_are_ignored() {
        let yaml = r#"
fork_probe_enabled: true
fork_probe_model: claude-opus-4-7
background_review_enabled: true
episode_settle_seconds: 60
circuit_failure_threshold: 5
circuit_cooldown_minutes: 60
episode_selector_max_budget_usd: 0.10
episode_selector_model: claude-haiku-4-5
max_daily_budget_usd: 2.50
prefilter_enabled: false
"#;
        let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(cfg.max_daily_budget_usd, 2.50);
        assert!(!cfg.prefilter_enabled);
        assert!(
            cfg.probe_writer_enabled,
            "probe_writer_enabled defaults to true"
        );
    }

    #[test]
    fn sandbox_providers_parses_built_in_entry() {
        let yaml = "sandbox:\n  providers:\n    - name: foo-anthropic\n      type: anthropic\n";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let sandbox = cfg.sandbox.unwrap();
        assert_eq!(sandbox.providers.len(), 1);
        let entry = &sandbox.providers[0];
        assert_eq!(entry.name, "foo-anthropic");
        assert_eq!(entry.type_, ProviderType::BuiltIn("anthropic".into()));
        assert!(entry.label.is_none());
        assert!(entry.generic.is_none());
    }

    #[test]
    fn sandbox_providers_parses_generic_entry() {
        let yaml = "sandbox:\n  providers:\n    - name: foo-acme\n      type: generic\n      label: acme\n      generic:\n        env_var: ACME_TOKEN\n        header_name: X-Acme-Token\n        upstream_host: api.acme.com\n        upstream_path_prefix: /v1\n";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        let entry = &cfg.sandbox.unwrap().providers[0];
        assert_eq!(entry.type_, ProviderType::Generic);
        assert_eq!(entry.label.as_deref(), Some("acme"));
        let g = entry.generic.as_ref().unwrap();
        assert_eq!(g.env_var, "ACME_TOKEN");
        assert_eq!(g.upstream_hosts, vec!["api.acme.com".to_string()]);
        assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
    }

    #[test]
    fn generic_provider_deserializes_legacy_single_host_and_ignores_header_name() {
        let yaml = "env_var: ACME_TOKEN\nheader_name: X-Acme-Token\nupstream_host: api.acme.com\nupstream_path_prefix: /v1\n";
        let g: GenericProvider = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(g.env_var, "ACME_TOKEN");
        assert_eq!(g.upstream_hosts, vec!["api.acme.com".to_string()]);
        assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
    }

    #[test]
    fn generic_provider_deserializes_multi_host_and_dedups() {
        let yaml =
            "env_var: FAL_KEY\nupstream_hosts:\n  - fal.run\n  - queue.fal.run\n  - fal.run\n";
        let g: GenericProvider = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            g.upstream_hosts,
            vec!["fal.run".to_string(), "queue.fal.run".to_string()]
        );
    }

    #[test]
    fn generic_provider_trims_hosts_before_deduping() {
        let yaml = "env_var: FAL_KEY\nupstream_host: ' fal.run '\nupstream_hosts:\n  - ' '\n  - 'queue.fal.run '\n  - ' fal.run'\n  - ''\n";
        let g: GenericProvider = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            g.upstream_hosts,
            vec!["fal.run".to_string(), "queue.fal.run".to_string()]
        );
    }

    #[test]
    fn generic_provider_merges_legacy_and_new_host_fields() {
        let yaml = "env_var: K\nupstream_host: a.example.com\nupstream_hosts:\n  - b.example.com\n";
        let g: GenericProvider = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(
            g.upstream_hosts,
            vec!["a.example.com".to_string(), "b.example.com".to_string()]
        );
    }

    #[test]
    fn generic_provider_rejects_zero_hosts() {
        let yaml = "env_var: K\n";
        assert!(serde_saphyr::from_str::<GenericProvider>(yaml).is_err());
    }

    #[test]
    fn generic_provider_roundtrips_to_upstream_hosts() {
        let g = GenericProvider {
            env_var: "K".into(),
            upstream_hosts: vec!["a.example.com".into()],
            upstream_path_prefix: None,
        };
        let s = serde_saphyr::to_string(&g).unwrap();
        let roundtripped: GenericProvider = serde_saphyr::from_str(&s).unwrap();
        assert_eq!(roundtripped, g);
        assert!(s.contains("upstream_hosts"));
        assert!(!s.contains("header_name"));
    }

    #[test]
    fn sandbox_providers_defaults_to_empty() {
        let yaml = "sandbox: { name: right-alpha }";
        let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert!(cfg.sandbox.unwrap().providers.is_empty());
    }

    #[test]
    fn provider_entry_ignores_legacy_shared_from_key() {
        // Ownership lives in providers.db since stage 3; an agent.yaml written
        // before that still carries `shared_from:` and must keep loading.
        let entry: ProviderEntry =
            serde_saphyr::from_str("name: fal-a1b2c3\ntype: right-fal\nshared_from: riskoff\n")
                .unwrap();
        assert_eq!(entry.name, "fal-a1b2c3");

        let s = serde_saphyr::to_string(&entry).unwrap();
        assert!(
            !s.contains("shared_from"),
            "shared_from must never be emitted; got: {s}"
        );
    }
}
