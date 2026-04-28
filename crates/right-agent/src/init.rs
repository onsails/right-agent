use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent::types::{MemoryProvider, NetworkPolicy, RecallBudget, SandboxMode, SttConfig};

/// Default recall budget used when the user doesn't override it.
pub const DEFAULT_RECALL_BUDGET: RecallBudget = RecallBudget::Mid;
/// Default recall max tokens used when the user doesn't override it.
pub const DEFAULT_RECALL_MAX_TOKENS: u32 = 4096;

/// Preserved config from a previous agent, used during `--force` re-init.
pub struct InitOverrides {
    pub sandbox_mode: SandboxMode,
    pub network_policy: NetworkPolicy,
    pub telegram_token: Option<String>,
    pub allowed_chat_ids: Vec<i64>,
    pub model: Option<String>,
    pub env: HashMap<String, String>,
    pub memory_provider: MemoryProvider,
    pub memory_api_key: Option<String>,
    pub memory_bank_id: Option<String>,
    pub memory_recall_budget: RecallBudget,
    pub memory_recall_max_tokens: u32,
    pub stt: SttConfig,
}

const DEFAULT_AGENTS: &str = include_str!("../templates/right/agent/AGENTS.md");
const DEFAULT_BOOTSTRAP: &str = include_str!("../templates/right/agent/BOOTSTRAP.md");
const DEFAULT_TOOLS: &str = include_str!("../templates/right/agent/TOOLS.md");
const DEFAULT_AGENT_YAML: &str = include_str!("../templates/right/agent/agent.yaml");

/// Initialize a single agent under `agents_parent_dir/<name>/`.
///
/// Creates the agent directory with template files (AGENTS.md, BOOTSTRAP.md,
/// agent.yaml), installs built-in skills, generates
/// .claude/settings.json, writes network and sandbox config to agent.yaml,
/// optionally generates policy.yaml for openshell mode, and sets up trust entries.
///
/// Returns the absolute path to the created agent directory.
/// Callers are responsible for checking if the directory already exists.
pub fn init_agent(
    agents_parent_dir: &Path,
    name: &str,
    overrides: Option<&InitOverrides>,
) -> miette::Result<PathBuf> {
    // Extract values from overrides with defaults.
    let default_overrides = InitOverrides {
        sandbox_mode: SandboxMode::default(),
        network_policy: NetworkPolicy::default(),
        telegram_token: None,
        allowed_chat_ids: vec![],
        model: None,
        env: HashMap::new(),
        memory_provider: MemoryProvider::File,
        memory_api_key: None,
        memory_bank_id: None,
        memory_recall_budget: DEFAULT_RECALL_BUDGET,
        memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
        stt: SttConfig::default(),
    };
    let ov = overrides.unwrap_or(&default_overrides);

    let agents_dir = agents_parent_dir.join(name);

    std::fs::create_dir_all(&agents_dir).map_err(|e| {
        miette::miette!("Failed to create directory {}: {}", agents_dir.display(), e)
    })?;

    // Create staging directory for OpenShell upload workflow
    let staging_dir = agents_dir.join("staging");
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| miette::miette!("Failed to create staging dir: {e}"))?;

    let files: &[(&str, &str)] = &[
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("TOOLS.md", DEFAULT_TOOLS),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];

    for (filename, content) in files {
        let path = agents_dir.join(filename);
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("Failed to write {}: {}", path.display(), e))?;
    }

    // Install built-in skills into .claude/skills/ (standard Agent Skills path).
    // Claude Code discovers skills from .claude/skills/ relative to cwd.
    crate::codegen::install_builtin_skills(&agents_dir, &ov.memory_provider)?;

    // Resolve host HOME once, before any HOME env manipulation (Phase 8).
    let host_home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Generate .claude/settings.json via codegen (D-17, D-18).
    {
        let settings = crate::codegen::generate_settings()?;
        let claude_dir = agents_dir.join(".claude");
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| miette::miette!("Failed to create {}: {}", claude_dir.display(), e))?;
        std::fs::write(
            claude_dir.join("settings.json"),
            serde_json::to_string_pretty(&settings)
                .map_err(|e| miette::miette!("Failed to serialize settings: {e}"))?,
        )
        .map_err(|e| miette::miette!("Failed to write settings.json: {}", e))?;
    }

    // Append dynamic config to agent.yaml in a single read-modify-write.
    {
        let agent_yaml_path = agents_dir.join("agent.yaml");
        let mut yaml = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("Failed to read agent.yaml: {}", e))?;

        // Network policy.
        let policy_str = match ov.network_policy {
            NetworkPolicy::Restrictive => "restrictive",
            NetworkPolicy::Permissive => "permissive",
        };
        yaml.push_str(&format!("\nnetwork_policy: {policy_str}\n"));

        // Sandbox config.
        match ov.sandbox_mode {
            SandboxMode::Openshell => {
                yaml.push_str(&format!(
                    "\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n  name: right-{name}\n"
                ));
            }
            SandboxMode::None => {
                yaml.push_str(&format!("\nsandbox:\n  mode: none\n  name: right-{name}\n"));
            }
        }

        // Telegram token + chat IDs.
        if let Some(ref token) = ov.telegram_token {
            yaml.push_str(&format!("\ntelegram_token: \"{token}\"\n"));
            if !ov.allowed_chat_ids.is_empty() {
                yaml.push_str("\nallowed_chat_ids:\n");
                for id in &ov.allowed_chat_ids {
                    yaml.push_str(&format!("  - {id}\n"));
                }
            }
        }

        // Model — always written; defaults to "sonnet" when not overridden.
        let model = ov.model.as_deref().unwrap_or("sonnet");
        yaml.push_str(&format!("\nmodel: \"{model}\"\n"));

        // Environment variables (from overrides only).
        if !ov.env.is_empty() {
            yaml.push_str("\nenv:\n");
            for (k, v) in &ov.env {
                yaml.push_str(&format!("  {k}: \"{v}\"\n"));
            }
        }

        // Memory provider (only written for non-default providers).
        if matches!(ov.memory_provider, MemoryProvider::Hindsight) {
            let mut memory_section = String::from("\nmemory:\n  provider: hindsight\n");
            if let Some(ref key) = ov.memory_api_key {
                memory_section.push_str(&format!("  api_key: \"{key}\"\n"));
            }
            if let Some(ref bank) = ov.memory_bank_id {
                memory_section.push_str(&format!("  bank_id: \"{bank}\"\n"));
            }
            if ov.memory_recall_budget != DEFAULT_RECALL_BUDGET {
                memory_section.push_str(&format!("  recall_budget: {}\n", ov.memory_recall_budget));
            }
            if ov.memory_recall_max_tokens != DEFAULT_RECALL_MAX_TOKENS {
                memory_section.push_str(&format!(
                    "  recall_max_tokens: {}\n",
                    ov.memory_recall_max_tokens
                ));
            }
            yaml.push_str(&memory_section);
        }

        // STT block — always written so explicit user choice survives round-trip.
        yaml.push_str(&format!(
            "\nstt:\n  enabled: {}\n  model: {}\n",
            ov.stt.enabled,
            ov.stt.model.yaml_str(),
        ));

        std::fs::write(&agent_yaml_path, &yaml)
            .map_err(|e| miette::miette!("Failed to update agent.yaml: {}", e))?;
    }

    // Generate policy.yaml when sandbox mode is openshell.
    if matches!(ov.sandbox_mode, SandboxMode::Openshell) {
        let policy_yaml = crate::codegen::policy::generate_policy(
            crate::runtime::MCP_HTTP_PORT,
            &ov.network_policy,
            None,
        );
        std::fs::write(agents_dir.join("policy.yaml"), &policy_yaml)
            .map_err(|e| miette::miette!("Failed to write policy.yaml: {e}"))?;
    }

    // Seed allowlist.yaml from the user-provided first trusted user.
    // Idempotent — skipped when allowlist.yaml already exists (wizard re-run, --force, etc.).
    if let Some(ov) = overrides
        && !ov.allowed_chat_ids.is_empty()
    {
        let report = crate::agent::allowlist::migrate_from_legacy(
            &agents_dir,
            &ov.allowed_chat_ids,
            chrono::Utc::now(),
        )
        .map_err(|e| miette::miette!("seed allowlist.yaml: {e:#}"))?;
        if !report.already_present && (report.migrated_users + report.migrated_groups) > 0 {
            tracing::info!(
                users = report.migrated_users,
                groups = report.migrated_groups,
                "seeded allowlist.yaml with {} users, {} groups from wizard input",
                report.migrated_users,
                report.migrated_groups,
            );
        }
    }

    // Pre-trust the agent directory in the agent-local .claude.json (D-06).
    let trust_agent = crate::agent::AgentDef {
        name: name.to_owned(),
        path: agents_dir.clone(),
        identity_path: agents_dir.join("IDENTITY.md"),
        config: None,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    };
    crate::codegen::generate_agent_claude_json(&trust_agent)?;

    // Create credential symlink so agent can use OAuth under HOME override (D-07, D-08).
    crate::codegen::create_credential_symlink(&trust_agent, &host_home)?;

    Ok(agents_dir)
}

/// Initialize the Right Agent home directory with a default "right" agent.
///
/// Creates `home/agents/right/` with template files via [`init_agent`].
///
/// Returns an error if the agents directory already exists.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
pub fn init_right_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
    sandbox_mode: &SandboxMode,
    memory_provider: MemoryProvider,
    memory_api_key: Option<String>,
    memory_bank_id: Option<String>,
    memory_recall_budget: RecallBudget,
    memory_recall_max_tokens: u32,
) -> miette::Result<()> {
    let agents_parent = crate::config::agents_dir(home);
    if agents_parent.join("right").exists() {
        return Err(miette::miette!(
            "Right Agent home already initialized at {}. Use `right config` to change settings.",
            agents_parent.join("right").display()
        ));
    }

    let overrides = InitOverrides {
        sandbox_mode: *sandbox_mode,
        network_policy: *network_policy,
        telegram_token: telegram_token.map(|t| t.to_string()),
        allowed_chat_ids: telegram_allowed_chat_ids.to_vec(),
        model: None,
        env: HashMap::new(),
        memory_provider,
        memory_api_key,
        memory_bank_id,
        memory_recall_budget,
        memory_recall_max_tokens,
        stt: SttConfig::default(),
    };
    let _agents_dir = init_agent(&agents_parent, "right", Some(&overrides))?;

    println!("Created Right Agent home at {}", home.display());
    println!("  agents/right/AGENTS.md");
    println!("  agents/right/BOOTSTRAP.md");
    println!("  agents/right/TOOLS.md");
    println!("  agents/right/agent.yaml");
    println!("  agents/right/.claude/skills/rightskills/SKILL.md  (skills.sh manager)");
    println!("  agents/right/.claude/skills/rightcron/SKILL.md");

    if telegram_token.is_some() {
        println!("  Telegram bot token saved");
        println!("  agents/right/.claude/settings.json (Telegram plugin enabled)");
    }

    if matches!(sandbox_mode, SandboxMode::Openshell) {
        println!("  agents/right/policy.yaml (OpenShell sandbox policy)");
    }

    Ok(())
}

/// Validate a Telegram bot token format.
///
/// Expected format: `<numeric_id>:<alphanumeric_string>`
/// Example: `123456789:AAHfiqksKZ8WmB...`
///
/// This is a format check only -- does not verify the token against Telegram's API.
pub fn validate_telegram_token(token: &str) -> miette::Result<()> {
    let parts: Vec<&str> = token.splitn(2, ':').collect();
    if parts.len() != 2
        || parts[0].is_empty()
        || !parts[0].chars().all(|c| c.is_ascii_digit())
        || parts[1].is_empty()
    {
        return Err(miette::miette!(
            help = "Token format: 123456789:AAHfiqksKZ8WmB...",
            "Invalid Telegram bot token format"
        ));
    }
    Ok(())
}

/// Run an inquire prompt, handling cancel/interrupt:
/// - `Ok(v)` → `Ok(Some(v))`
/// - Esc (`OperationCanceled`) → `Ok(None)` — caller's "go back" signal.
/// - Ctrl+C (`OperationInterrupted`) → "Cancel setup?" confirm prompt.
///   Yes (or Ctrl+C on confirm) → `Err`; No (or Esc on confirm) → re-run prompt.
/// - Other err → `Err`.
///
/// The closure rebuilds the prompt each retry so it can be re-run after a
/// declined cancel. inquire prompts consume their builder, hence the closure.
pub fn inquire_back<T, F>(mut prompt: F) -> miette::Result<Option<T>>
where
    F: FnMut() -> Result<T, inquire::InquireError>,
{
    loop {
        match prompt() {
            Ok(v) => return Ok(Some(v)),
            Err(inquire::InquireError::OperationCanceled) => return Ok(None),
            Err(inquire::InquireError::OperationInterrupted) => match inquire::Confirm::new(
                "cancel?",
            )
            .with_default(false)
            .with_help_message("y = exit setup, n = return to current question")
            .prompt()
            {
                Ok(true) | Err(inquire::InquireError::OperationInterrupted) => {
                    return Err(miette::miette!("Setup cancelled by user."));
                }
                Ok(false) | Err(_) => continue,
            },
            Err(e) => return Err(miette::miette!("prompt failed: {e:#}")),
        }
    }
}

/// Prompt for sandbox mode. Returns `None` on Esc.
pub fn prompt_sandbox_mode() -> miette::Result<Option<SandboxMode>> {
    let Some(choice) = inquire_back(|| {
        inquire::Select::new(
            "sandbox mode:",
            vec![
                "openshell — isolated container (recommended)",
                "none — direct host access (computer-use, chrome)",
            ],
        )
        .with_starting_cursor(0)
        .prompt()
    })?
    else {
        return Ok(None);
    };
    Ok(Some(if choice.starts_with("openshell") {
        SandboxMode::Openshell
    } else {
        SandboxMode::None
    }))
}

/// Prompt for network policy. Returns `None` on Esc.
pub fn prompt_network_policy() -> miette::Result<Option<NetworkPolicy>> {
    let Some(choice) = inquire_back(|| {
        inquire::Select::new(
            "network policy:",
            vec![
                "permissive — all https domains (recommended)",
                "restrictive — anthropic/claude domains only",
            ],
        )
        .with_starting_cursor(0)
        .prompt()
    })?
    else {
        return Ok(None);
    };
    Ok(Some(if choice.starts_with("permissive") {
        NetworkPolicy::Permissive
    } else {
        NetworkPolicy::Restrictive
    }))
}

/// Prompt for memory provider. Returns `None` on Esc.
pub fn prompt_memory_provider() -> miette::Result<Option<MemoryProvider>> {
    let Some(choice) = inquire_back(|| {
        inquire::Select::new(
            "memory provider:",
            vec![
                "hindsight — hindsight cloud api (recommended)",
                "file — agent manages MEMORY.md",
            ],
        )
        .with_starting_cursor(0)
        .prompt()
    })?
    else {
        return Ok(None);
    };
    Ok(Some(if choice.starts_with("hindsight") {
        MemoryProvider::Hindsight
    } else {
        MemoryProvider::File
    }))
}

/// Prompt for Hindsight API key. Returns `Ok(None)` on Esc (back).
/// Empty input means "use HINDSIGHT_API_KEY env var at runtime".
pub fn prompt_hindsight_api_key() -> miette::Result<Option<Option<String>>> {
    let Some(input) = inquire_back(|| {
        inquire::Text::new("hindsight api key (enter to use HINDSIGHT_API_KEY env var):").prompt()
    })?
    else {
        return Ok(None);
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(Some(None))
    } else {
        Ok(Some(Some(trimmed.to_string())))
    }
}

/// Prompt for Hindsight bank ID. Returns `Ok(None)` on Esc (back).
/// Empty input means "use agent name as default".
pub fn prompt_hindsight_bank_id(agent_name: &str) -> miette::Result<Option<Option<String>>> {
    let prompt_text = format!("hindsight bank id (default: {agent_name}):");
    let Some(input) = inquire_back(|| inquire::Text::new(&prompt_text).prompt())? else {
        return Ok(None);
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(Some(None))
    } else {
        Ok(Some(Some(trimmed.to_string())))
    }
}

/// Prompt for Hindsight recall budget. Returns `Ok(None)` on Esc (back).
pub fn prompt_recall_budget() -> miette::Result<Option<RecallBudget>> {
    let Some(choice) = inquire_back(|| {
        inquire::Select::new(
            "recall budget:",
            vec![
                "mid — balanced (default)",
                "low — smaller context, cheaper",
                "high — more context, higher cost",
            ],
        )
        .with_starting_cursor(0)
        .prompt()
    })?
    else {
        return Ok(None);
    };
    Ok(Some(if choice.starts_with("low") {
        RecallBudget::Low
    } else if choice.starts_with("high") {
        RecallBudget::High
    } else {
        RecallBudget::Mid
    }))
}

/// Prompt for recall max tokens. Returns `Ok(None)` on Esc (back).
/// Empty input means "use default".
pub fn prompt_recall_max_tokens() -> miette::Result<Option<u32>> {
    let prompt_text = format!("recall max tokens (default: {DEFAULT_RECALL_MAX_TOKENS}):");
    let Some(input) = inquire_back(|| inquire::Text::new(&prompt_text).prompt())? else {
        return Ok(None);
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Some(DEFAULT_RECALL_MAX_TOKENS));
    }
    let parsed: u32 = trimmed
        .parse()
        .map_err(|e| miette::miette!("invalid integer '{trimmed}': {e}"))?;
    Ok(Some(parsed))
}

/// Result of `validate_hindsight_key`.
#[derive(Debug, Clone)]
pub enum ValidationResult {
    /// API key is accepted — 200 from `GET /v1/default/banks`.
    Valid { banks: usize },
    /// API key is rejected — 401/403 from Hindsight.
    Invalid { status: u16 },
    /// Hindsight unreachable — timeout, 5xx, or network error.
    /// Caller decides whether to proceed despite this.
    Unreachable { detail: String },
}

/// Validate a Hindsight API key by calling `GET /v1/default/banks`.
///
/// Read-only, no side effects. Uses dummy `bank_id` and `budget` since the
/// list-banks endpoint does not depend on either. Returns a classified
/// [`ValidationResult`] so wizards can show contextual messages.
pub async fn validate_hindsight_key(api_key: &str) -> ValidationResult {
    let client = crate::memory::hindsight::HindsightClient::new(
        api_key,
        "_probe",
        "mid",
        DEFAULT_RECALL_MAX_TOKENS,
        None,
    );
    match client.list_banks().await {
        Ok(banks) => ValidationResult::Valid { banks: banks.len() },
        Err(crate::memory::MemoryError::Hindsight { status, .. })
            if status == 401 || status == 403 =>
        {
            ValidationResult::Invalid { status }
        }
        Err(e) => ValidationResult::Unreachable {
            detail: format!("{e:#}"),
        },
    }
}

/// Run the memory configuration wizard with Esc-to-go-back support.
///
/// Returns `None` on Esc from the provider selection (caller should go back).
// interactive wizard return type; a dedicated struct is out of scope here
#[allow(clippy::type_complexity)]
pub fn prompt_memory_config(
    agent_name: &str,
) -> miette::Result<
    Option<(
        MemoryProvider,
        Option<String>,
        Option<String>,
        RecallBudget,
        u32,
    )>,
> {
    loop {
        let Some(provider) = prompt_memory_provider()? else {
            return Ok(None);
        };
        if !matches!(provider, MemoryProvider::Hindsight) {
            return Ok(Some((
                provider,
                None,
                None,
                DEFAULT_RECALL_BUDGET,
                DEFAULT_RECALL_MAX_TOKENS,
            )));
        }
        // Hindsight: prompt for API key, Esc goes back to provider selection.
        let Some(api_key) = prompt_hindsight_api_key()? else {
            continue;
        };
        let Some(bank_id) = prompt_hindsight_bank_id(agent_name)? else {
            continue;
        };
        let Some(budget) = prompt_recall_budget()? else {
            continue;
        };
        let Some(max_tokens) = prompt_recall_max_tokens()? else {
            continue;
        };
        return Ok(Some((provider, api_key, bank_id, budget, max_tokens)));
    }
}

/// Every prompt label string used by `right-agent::init`. Source-of-truth list
/// for the brand voice regression tests (`tests/voice_pass.rs`). When you add
/// or change a prompt, update this array — failing to do so is caught by tests.
pub const PROMPT_LABELS: &[&str] = &[
    // inquire_back: ctrl+c confirm
    "cancel?",
    // prompt_sandbox_mode
    "sandbox mode:",
    // prompt_network_policy
    "network policy:",
    // prompt_memory_provider
    "memory provider:",
    // prompt_hindsight_api_key
    "hindsight api key (enter to use HINDSIGHT_API_KEY env var):",
    // prompt_hindsight_bank_id — dynamic; static prefix used for voice check
    "hindsight bank id (default: ",
    // prompt_recall_budget
    "recall budget:",
    // prompt_recall_max_tokens — dynamic; static prefix used for voice check
    "recall max tokens (default: ",
];

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use std::collections::HashMap;

    use super::*;
    use crate::agent::types::{MemoryProvider, NetworkPolicy, SandboxMode};

    #[test]
    fn init_creates_default_agent_files() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let agents_dir = dir.path().join("agents").join("right");
        assert!(
            !agents_dir.join("IDENTITY.md").exists(),
            "IDENTITY.md must not be created by init"
        );
        assert!(
            !agents_dir.join("SOUL.md").exists(),
            "SOUL.md must not be created by init"
        );
        assert!(
            !agents_dir.join("USER.md").exists(),
            "USER.md must not be created by init"
        );
        assert!(
            agents_dir.join("staging").is_dir(),
            "staging/ dir should be created"
        );
        assert!(agents_dir.join("AGENTS.md").exists());
        assert!(
            agents_dir.join("TOOLS.md").exists(),
            "TOOLS.md must be created by init"
        );
        let tools_content = std::fs::read_to_string(agents_dir.join("TOOLS.md")).unwrap();
        assert!(
            tools_content.contains("Tool selection"),
            "TOOLS.md must be seeded from template"
        );
        assert!(
            agents_dir.join("policy.yaml").exists(),
            "policy.yaml should be created for openshell mode"
        );
        assert!(
            agents_dir.join("BOOTSTRAP.md").exists(),
            "BOOTSTRAP.md should always be created"
        );
        assert!(
            agents_dir.join("agent.yaml").exists(),
            "agent.yaml should always be created"
        );
        assert!(
            agents_dir
                .join(".claude/skills/rightskills/SKILL.md")
                .exists(),
            "rightskills skill should be installed"
        );
        assert!(
            agents_dir
                .join(".claude/skills/rightcron/SKILL.md")
                .exists(),
            "rightcron skill should be installed"
        );
    }

    #[test]
    fn init_errors_if_already_initialized() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let result = init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        );
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("already initialized"),
            "expected 'already initialized' in: {err}"
        );
        // miette's Debug wraps long lines, so check for both words individually.
        assert!(
            err.contains("right") && err.contains("config"),
            "expected 'right config' (not --force) in: {err}"
        );
    }

    #[test]
    fn init_with_telegram_writes_token_inline_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token: \"123456:ABCdef\""),
            "agent.yaml must contain inline telegram_token, got:\n{yaml}"
        );
    }

    #[test]
    fn init_creates_bootstrap_md() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let bootstrap =
            std::fs::read_to_string(dir.path().join("agents/right/BOOTSTRAP.md")).unwrap();
        assert!(
            bootstrap.contains("First-run onboarding"),
            "BOOTSTRAP.md should contain onboarding content"
        );
    }

    #[test]
    fn validate_telegram_token_accepts_valid_format() {
        assert!(validate_telegram_token("123456:ABCdef").is_ok());
        assert!(validate_telegram_token("1:A").is_ok());
        assert!(validate_telegram_token("999999999:AAHfiqksKZ8WmBzzHc_12345").is_ok());
    }

    #[test]
    fn validate_telegram_token_rejects_no_colon() {
        assert!(validate_telegram_token("invalid").is_err());
    }

    #[test]
    fn validate_telegram_token_rejects_empty_numeric_part() {
        assert!(validate_telegram_token(":ABCdef").is_err());
    }

    #[test]
    fn validate_telegram_token_rejects_empty_alpha_part() {
        assert!(validate_telegram_token("123:").is_err());
    }

    #[test]
    fn validate_telegram_token_rejects_non_numeric_first_part() {
        assert!(validate_telegram_token("abc:def").is_err());
    }

    #[test]
    fn validate_telegram_token_rejects_empty_string() {
        assert!(validate_telegram_token("").is_err());
    }

    #[test]
    fn init_with_telegram_creates_settings_json() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let settings_path = dir.path().join("agents/right/.claude/settings.json");
        assert!(
            settings_path.exists(),
            "settings.json should be created when telegram token is provided"
        );

        let content = std::fs::read_to_string(&settings_path).unwrap();
        // CC Telegram plugin must NOT be enabled — it races with the native Rust bot
        // for getUpdates on the same token, causing intermittent message drops.
        assert!(
            !content.contains("enabledPlugins"),
            "settings.json must NOT contain enabledPlugins — CC plugin races with native teloxide bot"
        );
        assert!(
            !content.contains("telegram@claude-plugins-official"),
            "telegram@claude-plugins-official must NOT be in settings.json"
        );
        assert!(
            content.contains("spinnerTipsEnabled"),
            "settings.json should contain spinnerTipsEnabled"
        );
        assert!(
            content.contains("prefersReducedMotion"),
            "settings.json should contain prefersReducedMotion"
        );
    }

    #[test]
    fn init_creates_settings_without_sandbox_section() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let settings_path = dir.path().join("agents/right/.claude/settings.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(
            json.get("sandbox").is_none(),
            "settings.json should not contain sandbox section — OpenShell is the security layer"
        );
        assert_eq!(json["skipDangerousModePermissionPrompt"], true);
        assert_eq!(json["autoMemoryEnabled"], true);
    }

    #[test]
    fn init_without_telegram_creates_settings_without_plugin() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let settings_path = dir.path().join("agents/right/.claude/settings.json");
        assert!(
            settings_path.exists(),
            "settings.json should always be created"
        );

        let content = std::fs::read_to_string(&settings_path).unwrap();
        assert!(
            content.contains("skipDangerousModePermissionPrompt"),
            "settings.json should contain skipDangerousModePermissionPrompt"
        );
        assert!(
            content.contains("spinnerTipsEnabled"),
            "settings.json should contain spinnerTipsEnabled"
        );
        assert!(
            content.contains("prefersReducedMotion"),
            "settings.json should contain prefersReducedMotion"
        );
        assert!(
            !content.contains("enabledPlugins"),
            "settings.json should NOT contain enabledPlugins without telegram"
        );
    }

    #[test]
    fn init_with_telegram_allowed_chat_ids_writes_yaml() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[12345678_i64, 100200300_i64],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("allowed_chat_ids:"),
            "agent.yaml must contain allowed_chat_ids section, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 12345678"),
            "agent.yaml must list 12345678, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 100200300"),
            "agent.yaml must list 100200300, got:\n{yaml}"
        );
        // access.json is no longer written
        assert!(
            !dir.path()
                .join("agents/right/.claude/channels/telegram/access.json")
                .exists(),
            "access.json must NOT be written"
        );
    }

    #[test]
    fn init_with_telegram_sets_token_inline_with_chat_ids() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[12345678_i64],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token: \"123456:ABCdef\""),
            "agent.yaml must contain inline telegram_token, got:\n{yaml}"
        );
        assert!(
            yaml.contains("allowed_chat_ids:"),
            "agent.yaml must contain allowed_chat_ids section, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 12345678"),
            "agent.yaml must list chat id 12345678, got:\n{yaml}"
        );
    }

    #[test]
    fn init_with_telegram_no_chat_ids_does_not_write_allowed_chat_ids() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token"),
            "telegram_token must be set"
        );
        assert!(
            !yaml.contains("allowed_chat_ids"),
            "allowed_chat_ids must not appear when no chat IDs provided"
        );
    }

    #[test]
    fn init_writes_network_policy_restrictive_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Restrictive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: restrictive"),
            "agent.yaml must contain network_policy: restrictive, got:\n{yaml}"
        );
    }

    #[test]
    fn init_writes_network_policy_permissive_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_right_home(
            dir.path(),
            None,
            &[],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
            DEFAULT_RECALL_BUDGET,
            DEFAULT_RECALL_MAX_TOKENS,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: permissive"),
            "agent.yaml must contain network_policy: permissive, got:\n{yaml}"
        );
    }

    #[test]
    fn init_generates_policy_yaml_for_openshell_mode() {
        let dir = tempdir().unwrap();
        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::Openshell,
            network_policy: NetworkPolicy::Permissive,
            telegram_token: Some("123456:ABCdef".to_string()),
            allowed_chat_ids: vec![],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig::default(),
        };
        init_agent(&dir.path().join("agents"), "test-agent", Some(&overrides)).unwrap();
        let policy_path = dir.path().join("agents/test-agent/policy.yaml");
        assert!(
            policy_path.exists(),
            "policy.yaml must be generated for openshell mode"
        );
        let content = std::fs::read_to_string(&policy_path).unwrap();
        assert!(
            content.contains("version: 1"),
            "policy must be valid OpenShell format"
        );
    }

    #[test]
    fn init_skips_policy_yaml_for_none_mode() {
        let dir = tempdir().unwrap();
        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::None,
            network_policy: NetworkPolicy::Permissive,
            telegram_token: None,
            allowed_chat_ids: vec![],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig::default(),
        };
        init_agent(&dir.path().join("agents"), "test-agent", Some(&overrides)).unwrap();
        let policy_path = dir.path().join("agents/test-agent/policy.yaml");
        assert!(
            !policy_path.exists(),
            "policy.yaml must NOT exist for none mode"
        );
    }

    #[test]
    fn init_writes_sandbox_mode_to_agent_yaml() {
        let dir = tempdir().unwrap();
        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::None,
            network_policy: NetworkPolicy::Permissive,
            telegram_token: None,
            allowed_chat_ids: vec![],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig::default(),
        };
        init_agent(&dir.path().join("agents"), "test-agent", Some(&overrides)).unwrap();
        let yaml =
            std::fs::read_to_string(dir.path().join("agents/test-agent/agent.yaml")).unwrap();
        assert!(
            yaml.contains("mode: none"),
            "agent.yaml must contain sandbox mode: none"
        );
    }

    #[test]
    fn init_agent_seeds_allowlist_yaml_from_overrides() {
        let dir = tempdir().unwrap();
        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::None,
            network_policy: NetworkPolicy::Restrictive,
            telegram_token: Some("123:ABC".to_string()),
            allowed_chat_ids: vec![42, -1001234],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig::default(),
        };
        init_agent(&dir.path().join("agents"), "testbot", Some(&overrides)).unwrap();

        let allowlist_path = dir.path().join("agents/testbot/allowlist.yaml");
        assert!(allowlist_path.exists(), "allowlist.yaml must be written");
        let content = std::fs::read_to_string(&allowlist_path).unwrap();
        assert!(
            content.contains("id: 42"),
            "user 42 must be seeded, got:\n{content}"
        );
        assert!(
            content.contains("id: -1001234"),
            "group -1001234 must be seeded, got:\n{content}"
        );
    }

    #[test]
    fn init_agent_writes_stt_block_to_yaml() {
        use crate::agent::types::{AgentConfig, SttConfig, WhisperModel};

        let tmp = tempfile::TempDir::new().unwrap();
        let agents_parent = tmp.path();

        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::default(),
            network_policy: NetworkPolicy::default(),
            telegram_token: Some("t".into()),
            allowed_chat_ids: vec![1],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig {
                enabled: true,
                model: WhisperModel::Tiny,
            },
        };

        let agent_dir = init_agent(agents_parent, "test-stt", Some(&overrides)).unwrap();
        let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();

        let cfg: AgentConfig = serde_saphyr::from_str(&yaml).unwrap();
        assert!(
            cfg.stt.enabled,
            "stt block must be written; default would be false"
        );
        assert_eq!(cfg.stt.model, WhisperModel::Tiny);
    }

    #[test]
    fn init_agent_writes_stt_block_with_enabled_false_round_trips() {
        use crate::agent::types::{SttConfig, WhisperModel};

        let tmp = tempfile::TempDir::new().unwrap();
        let agents_parent = tmp.path();

        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::default(),
            network_policy: NetworkPolicy::default(),
            telegram_token: None,
            allowed_chat_ids: vec![],
            model: None,
            env: HashMap::new(),
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig {
                enabled: false,
                model: WhisperModel::Medium,
            },
        };

        let agent_dir = init_agent(agents_parent, "test-stt-false", Some(&overrides)).unwrap();
        let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();

        // The stt: block must be present even when enabled=false.
        assert!(
            yaml.contains("stt:"),
            "stt: block must be written even when enabled=false; got:\n{yaml}"
        );

        let cfg: crate::agent::types::AgentConfig = serde_saphyr::from_str(&yaml).unwrap();
        assert!(
            !cfg.stt.enabled,
            "enabled: false must survive round-trip through agent.yaml"
        );
        assert_eq!(
            cfg.stt.model,
            WhisperModel::Medium,
            "non-default model must survive round-trip (guards against revert to if-enabled gating)"
        );
    }

    #[test]
    fn init_agent_with_overrides_applies_saved_config() {
        let dir = tempdir().unwrap();
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let overrides = InitOverrides {
            sandbox_mode: SandboxMode::None,
            network_policy: NetworkPolicy::Permissive,
            telegram_token: Some("999888:XYZtoken".to_string()),
            allowed_chat_ids: vec![111, 222],
            model: Some("opus".to_string()),
            env,
            memory_provider: MemoryProvider::File,
            memory_api_key: None,
            memory_bank_id: None,
            memory_recall_budget: DEFAULT_RECALL_BUDGET,
            memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
            stt: SttConfig::default(),
        };
        init_agent(
            &dir.path().join("agents"),
            "override-test",
            Some(&overrides),
        )
        .unwrap();

        let yaml =
            std::fs::read_to_string(dir.path().join("agents/override-test/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: permissive"),
            "agent.yaml must contain network_policy: permissive, got:\n{yaml}"
        );
        assert!(
            yaml.contains("mode: none"),
            "agent.yaml must contain sandbox mode: none, got:\n{yaml}"
        );
        assert!(
            yaml.contains("telegram_token: \"999888:XYZtoken\""),
            "agent.yaml must contain telegram token, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 111"),
            "agent.yaml must list chat id 111, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 222"),
            "agent.yaml must list chat id 222, got:\n{yaml}"
        );
        assert!(
            yaml.contains("model: \"opus\""),
            "agent.yaml must contain model: opus, got:\n{yaml}"
        );
        assert!(
            yaml.contains("FOO: \"bar\""),
            "agent.yaml must contain env FOO: bar, got:\n{yaml}"
        );
    }

    #[test]
    fn init_agent_writes_explicit_sandbox_name_with_right_prefix() {
        let dir = tempdir().unwrap();
        let agents_parent = dir.path();
        let agent_dir = init_agent(agents_parent, "foo", None).unwrap();

        let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();
        assert!(
            yaml.contains("name: right-foo"),
            "agent.yaml must contain explicit sandbox.name 'right-foo'; got:\n{yaml}"
        );
    }
}
