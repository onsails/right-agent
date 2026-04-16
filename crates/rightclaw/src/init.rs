use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent::types::{MemoryProvider, NetworkPolicy, SandboxMode};

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
}

const DEFAULT_AGENTS: &str = include_str!("../../../templates/right/agent/AGENTS.md");
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/agent/BOOTSTRAP.md");
const DEFAULT_AGENT_YAML: &str = include_str!("../../../templates/right/agent/agent.yaml");

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
        ("TOOLS.md", ""),
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
    let host_home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Generate .claude/settings.json via codegen (D-17, D-18).
    {
        let settings = crate::codegen::generate_settings()?;
        let claude_dir = agents_dir.join(".claude");
        std::fs::create_dir_all(&claude_dir).map_err(|e| {
            miette::miette!("Failed to create {}: {}", claude_dir.display(), e)
        })?;
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
                yaml.push_str("\nsandbox:\n  mode: openshell\n  policy_file: policy.yaml\n");
            }
            SandboxMode::None => {
                yaml.push_str("\nsandbox:\n  mode: none\n");
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
            yaml.push_str(&memory_section);
        }

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

/// Initialize the RightClaw home directory with a default "right" agent.
///
/// Creates `home/agents/right/` with template files via [`init_agent`].
///
/// Returns an error if the agents directory already exists.
pub fn init_rightclaw_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
    sandbox_mode: &SandboxMode,
    memory_provider: MemoryProvider,
    memory_api_key: Option<String>,
    memory_bank_id: Option<String>,
) -> miette::Result<()> {
    let agents_parent = crate::config::agents_dir(home);
    if agents_parent.join("right").exists() {
        return Err(miette::miette!(
            "RightClaw home already initialized at {}. Use `rightclaw config` to change settings.",
            agents_parent.join("right").display()
        ));
    }

    let overrides = InitOverrides {
        sandbox_mode: sandbox_mode.clone(),
        network_policy: network_policy.clone(),
        telegram_token: telegram_token.map(|t| t.to_string()),
        allowed_chat_ids: telegram_allowed_chat_ids.to_vec(),
        model: None,
        env: HashMap::new(),
        memory_provider,
        memory_api_key,
        memory_bank_id,
    };
    let _agents_dir = init_agent(&agents_parent, "right", Some(&overrides))?;

    println!("Created RightClaw home at {}", home.display());
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

/// Helper: convert inquire error. Returns `Ok(None)` on Esc (back), propagates real errors.
fn inquire_back<T>(result: Result<T, inquire::InquireError>) -> miette::Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(miette::miette!("prompt failed: {e:#}")),
    }
}

/// Prompt for sandbox mode. Returns `None` on Esc.
pub fn prompt_sandbox_mode() -> miette::Result<Option<SandboxMode>> {
    let options = vec![
        "OpenShell — run in isolated container (recommended)",
        "None — run directly on host (for computer-use, Chrome, etc.)",
    ];
    let result = inquire::Select::new("Sandbox mode:", options)
        .with_starting_cursor(0)
        .prompt();
    let Some(choice) = inquire_back(result)? else { return Ok(None) };
    Ok(Some(if choice.starts_with("OpenShell") {
        SandboxMode::Openshell
    } else {
        SandboxMode::None
    }))
}

/// Prompt for network policy. Returns `None` on Esc.
pub fn prompt_network_policy() -> miette::Result<Option<NetworkPolicy>> {
    let options = vec![
        "Permissive — all HTTPS domains allowed (recommended)",
        "Restrictive — Anthropic/Claude domains only",
    ];
    let result = inquire::Select::new("Network policy for sandbox:", options)
        .with_starting_cursor(0)
        .prompt();
    let Some(choice) = inquire_back(result)? else { return Ok(None) };
    Ok(Some(if choice.starts_with("Permissive") {
        NetworkPolicy::Permissive
    } else {
        NetworkPolicy::Restrictive
    }))
}

/// Prompt for memory provider. Returns `None` on Esc.
pub fn prompt_memory_provider() -> miette::Result<Option<MemoryProvider>> {
    let options = vec![
        "Hindsight — Hindsight Cloud API (recommended)",
        "File — agent manages MEMORY.md",
    ];
    let result = inquire::Select::new("Memory provider:", options)
        .with_starting_cursor(0)
        .prompt();
    let Some(choice) = inquire_back(result)? else { return Ok(None) };
    Ok(Some(if choice.starts_with("Hindsight") {
        MemoryProvider::Hindsight
    } else {
        MemoryProvider::File
    }))
}

/// Prompt for Hindsight API key. Returns `Ok(None)` on Esc (back).
/// Empty input means "use HINDSIGHT_API_KEY env var at runtime".
pub fn prompt_hindsight_api_key() -> miette::Result<Option<Option<String>>> {
    let result = inquire::Text::new("Hindsight API key (Enter to use HINDSIGHT_API_KEY env var):")
        .prompt();
    let Some(input) = inquire_back(result)? else { return Ok(None) };
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
    let result = inquire::Text::new(&format!("Hindsight bank ID (default: {agent_name}):"))
        .prompt();
    let Some(input) = inquire_back(result)? else { return Ok(None) };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(Some(None))
    } else {
        Ok(Some(Some(trimmed.to_string())))
    }
}

/// Run the memory configuration wizard with Esc-to-go-back support.
///
/// Returns `None` on Esc from the provider selection (caller should go back).
pub fn prompt_memory_config(
    agent_name: &str,
) -> miette::Result<Option<(MemoryProvider, Option<String>, Option<String>)>> {
    loop {
        let Some(provider) = prompt_memory_provider()? else {
            return Ok(None);
        };
        if !matches!(provider, MemoryProvider::Hindsight) {
            return Ok(Some((provider, None, None)));
        }
        // Hindsight: prompt for API key, Esc goes back to provider selection.
        let Some(api_key) = prompt_hindsight_api_key()? else {
            continue;
        };
        // Prompt for bank ID, Esc goes back to API key? No — back to provider.
        let Some(bank_id) = prompt_hindsight_bank_id(agent_name)? else {
            continue;
        };
        return Ok(Some((provider, api_key, bank_id)));
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use std::collections::HashMap;

    use super::*;
    use crate::agent::types::{MemoryProvider, NetworkPolicy, SandboxMode};

    #[test]
    fn init_creates_default_agent_files() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let agents_dir = dir.path().join("agents").join("right");
        assert!(!agents_dir.join("IDENTITY.md").exists(), "IDENTITY.md must not be created by init");
        assert!(!agents_dir.join("SOUL.md").exists(), "SOUL.md must not be created by init");
        assert!(!agents_dir.join("USER.md").exists(), "USER.md must not be created by init");
        assert!(agents_dir.join("staging").is_dir(), "staging/ dir should be created");
        assert!(agents_dir.join("AGENTS.md").exists());
        assert!(agents_dir.join("TOOLS.md").exists(), "TOOLS.md must be created by init");
        let tools_content = std::fs::read_to_string(agents_dir.join("TOOLS.md")).unwrap();
        assert_eq!(tools_content, "", "TOOLS.md must be created empty");
        assert!(agents_dir.join("policy.yaml").exists(), "policy.yaml should be created for openshell mode");
        assert!(
            agents_dir.join("BOOTSTRAP.md").exists(),
            "BOOTSTRAP.md should always be created"
        );
        assert!(
            agents_dir.join("agent.yaml").exists(),
            "agent.yaml should always be created"
        );
        assert!(
            agents_dir.join(".claude/skills/rightskills/SKILL.md").exists(),
            "rightskills skill should be installed"
        );
        assert!(
            agents_dir.join(".claude/skills/rightcron/SKILL.md").exists(),
            "rightcron skill should be installed"
        );
    }

    #[test]
    fn init_errors_if_already_initialized() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let result = init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("already initialized"),
            "expected 'already initialized' in: {err}"
        );
        // miette's Debug wraps long lines, so check for both words individually.
        assert!(
            err.contains("rightclaw") && err.contains("config"),
            "expected 'rightclaw config' (not --force) in: {err}"
        );
    }

    #[test]
    fn init_with_telegram_writes_token_inline_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token: \"123456:ABCdef\""),
            "agent.yaml must contain inline telegram_token, got:\n{yaml}"
        );
    }

    #[test]
    fn init_creates_bootstrap_md() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let bootstrap = std::fs::read_to_string(
            dir.path().join("agents/right/BOOTSTRAP.md"),
        )
        .unwrap();
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
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let settings_path = dir
            .path()
            .join("agents/right/.claude/settings.json");
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
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

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
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let settings_path = dir
            .path()
            .join("agents/right/.claude/settings.json");
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
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[12345678_i64, 100200300_i64],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
        )
        .unwrap();

        let yaml =
            std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
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
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[12345678_i64],
            &NetworkPolicy::Permissive,
            &SandboxMode::Openshell,
            MemoryProvider::File,
            None,
            None,
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
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

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
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Restrictive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: restrictive"),
            "agent.yaml must contain network_policy: restrictive, got:\n{yaml}"
        );
    }

    #[test]
    fn init_writes_network_policy_permissive_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell, MemoryProvider::File, None, None).unwrap();

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
        };
        init_agent(
            &dir.path().join("agents"),
            "test-agent",
            Some(&overrides),
        )
        .unwrap();
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
        };
        init_agent(
            &dir.path().join("agents"),
            "test-agent",
            Some(&overrides),
        )
        .unwrap();
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
        };
        init_agent(
            &dir.path().join("agents"),
            "test-agent",
            Some(&overrides),
        )
        .unwrap();
        let yaml =
            std::fs::read_to_string(dir.path().join("agents/test-agent/agent.yaml")).unwrap();
        assert!(
            yaml.contains("mode: none"),
            "agent.yaml must contain sandbox mode: none"
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
}
