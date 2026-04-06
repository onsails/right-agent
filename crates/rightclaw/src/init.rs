use std::path::Path;

const DEFAULT_IDENTITY: &str = include_str!("../../../templates/right/IDENTITY.md");
const DEFAULT_SOUL: &str = include_str!("../../../templates/right/SOUL.md");
const DEFAULT_USER: &str = include_str!("../../../templates/right/USER.md");
const DEFAULT_AGENTS: &str = include_str!("../../../templates/right/AGENTS.md");
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/BOOTSTRAP.md");
const DEFAULT_AGENT_YAML: &str = include_str!("../../../templates/right/agent.yaml");

/// Initialize the RightClaw home directory with a default "right" agent.
///
/// Creates `home/agents/right/` with template files: IDENTITY.md, SOUL.md,
/// AGENTS.md, BOOTSTRAP.md, and agent.yaml.
///
/// When `telegram_token` is provided:
/// - Writes the token to `telegram_env_dir/.env` for Claude Code's Telegram plugin
/// - Creates `.claude/settings.json` with `enabledPlugins` for automatic Telegram plugin activation
///
/// `telegram_env_dir` controls where the `.env` file is written. Pass `None` to
/// use the default `~/.claude/channels/telegram/` path.
///
/// When `telegram_allowed_chat_ids` is non-empty, writes an `allowed_chat_ids:` YAML
/// list into `agent.yaml` so `rightclaw up` enforces the allowlist on each launch.
///
/// Returns an error if the agents directory already exists.
pub fn init_rightclaw_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    telegram_env_dir: Option<&Path>,
) -> miette::Result<()> {
    let agents_dir = home.join("agents").join("right");

    if agents_dir.exists() {
        return Err(miette::miette!(
            "RightClaw home already initialized at {}. Use --force to reinitialize.",
            agents_dir.display()
        ));
    }

    std::fs::create_dir_all(&agents_dir).map_err(|e| {
        miette::miette!("Failed to create directory {}: {}", agents_dir.display(), e)
    })?;

    let files: &[(&str, &str)] = &[
        ("IDENTITY.md", DEFAULT_IDENTITY),
        ("SOUL.md", DEFAULT_SOUL),
        ("USER.md", DEFAULT_USER),
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];

    for (filename, content) in files {
        let path = agents_dir.join(filename);
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("Failed to write {}: {}", path.display(), e))?;
    }

    // Install built-in skills into .claude/skills/ (standard Agent Skills path).
    // Claude Code discovers skills from .claude/skills/ relative to cwd.
    crate::codegen::install_builtin_skills(&agents_dir)?;

    // Resolve host HOME once, before any HOME env manipulation (Phase 8).
    let host_home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Generate .claude/settings.json via codegen (D-17, D-18).
    // Build a synthetic AgentDef for the default "right" agent.
    {
        // Build synthetic AgentDef for generate_settings with telegram config if token provided.
        // Telegram plugin detection reads agent.config (D-01).
        let settings_config = telegram_token.map(|tok| crate::agent::AgentConfig {
            restart: crate::agent::RestartPolicy::OnFailure,
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            telegram_token: Some(tok.to_string()),
            telegram_token_file: None,
            allowed_chat_ids: vec![],
            env: std::collections::HashMap::new(),
        });
        let agent_def = crate::agent::AgentDef {
            name: "right".to_string(),
            path: agents_dir.clone(),
            identity_path: agents_dir.join("IDENTITY.md"),
            config: settings_config,
            soul_path: None,
            user_path: None,
            agents_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        };

        let settings = crate::codegen::generate_settings(&agent_def, false, &host_home, None, None)?;
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

    // Write Telegram bot token into the agent's channel dir and record it in agent.yaml.
    //
    // The token is stored at `AGENT_DIR/.claude/channels/telegram/.env` — which is where
    // CC reads it when the agent runs with HOME override. The token is NOT written inline
    // into agent.yaml (it's a secret); instead `telegram_token_file` points to the .env.
    // `telegram_env_dir` override is kept for tests only.
    if let Some(token) = telegram_token {
        let env_dir = match telegram_env_dir {
            Some(dir) => dir.to_path_buf(),
            None => agents_dir.join(".claude").join("channels").join("telegram"),
        };
        std::fs::create_dir_all(&env_dir).map_err(|e| {
            miette::miette!("Failed to create {}: {}", env_dir.display(), e)
        })?;
        std::fs::write(
            env_dir.join(".env"),
            format!("TELEGRAM_BOT_TOKEN={token}\n"),
        )
        .map_err(|e| miette::miette!("Failed to write telegram .env: {}", e))?;

        // Append telegram fields to agent.yaml so `rightclaw up` detects the token
        // and generates wrapper/settings/channel-config correctly on each launch.
        // telegram_token_file uses a relative path (relative to agent dir), not absolute.
        let agent_yaml_path = agents_dir.join("agent.yaml");
        let mut yaml = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("Failed to read agent.yaml: {}", e))?;
        yaml.push_str("\ntelegram_token_file: .claude/channels/telegram/.env\n");
        if !telegram_allowed_chat_ids.is_empty() {
            yaml.push_str("\nallowed_chat_ids:\n");
            for id in telegram_allowed_chat_ids {
                yaml.push_str(&format!("  - {id}\n"));
            }
        }
        std::fs::write(&agent_yaml_path, yaml)
            .map_err(|e| miette::miette!("Failed to update agent.yaml: {}", e))?;
    }

    // Pre-trust the agent directory in the agent-local .claude.json (D-06).
    // Under HOME override, CC reads $AGENT_DIR/.claude.json, not host ~/.claude.json.
    // See: https://github.com/anthropics/claude-code/issues/28506
    let trust_agent = crate::agent::AgentDef {
        name: "right".to_owned(),
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
    // host_home was already resolved at function start (before any HOME manipulation).
    crate::codegen::create_credential_symlink(&trust_agent, &host_home)?;

    println!("Created RightClaw home at {}", home.display());
    println!("  agents/right/IDENTITY.md");
    println!("  agents/right/SOUL.md");
    println!("  agents/right/USER.md");
    println!("  agents/right/AGENTS.md");
    println!("  agents/right/BOOTSTRAP.md");
    println!("  agents/right/agent.yaml");
    println!("  agents/right/.claude/skills/rightskills/SKILL.md  (skills.sh manager)");
    println!("  agents/right/.claude/skills/rightcron/SKILL.md");

    if telegram_token.is_some() {
        println!("  Telegram bot token saved");
        println!("  agents/right/.claude/settings.json (Telegram plugin enabled)");
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

/// Prompt the user for a Telegram bot token interactively.
///
/// Returns `Some(token)` if a valid token was entered, `None` if the user
/// pressed Enter to skip.
pub fn prompt_telegram_token() -> miette::Result<Option<String>> {
    use std::io::{self, Write};
    print!("Set up Telegram channel? (paste bot token or press Enter to skip): ");
    io::stdout().flush().map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("failed to read input: {e}"))?;
    let token = input.trim();
    if token.is_empty() {
        return Ok(None);
    }
    validate_telegram_token(token)?;
    Ok(Some(token.to_string()))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn init_creates_default_agent_files() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

        let agents_dir = dir.path().join("agents").join("right");
        assert!(agents_dir.join("IDENTITY.md").exists());
        assert!(agents_dir.join("SOUL.md").exists());
        assert!(agents_dir.join("USER.md").exists(), "USER.md should be created");
        assert!(agents_dir.join("AGENTS.md").exists());
        assert!(!agents_dir.join("policy.yaml").exists(), "policy.yaml should NOT be created");
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
    fn init_identity_contains_right() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

        let content =
            std::fs::read_to_string(dir.path().join("agents/right/IDENTITY.md")).unwrap();
        assert!(
            content.contains("Right"),
            "IDENTITY.md should contain 'Right'"
        );
    }

    #[test]
    fn init_errors_if_already_initialized() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

        let result = init_rightclaw_home(dir.path(), None, &[], None);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("already initialized"),
            "expected 'already initialized' in: {err}"
        );
    }

    #[test]
    fn init_with_telegram_writes_token_env_file() {
        let dir = tempdir().unwrap();
        let env_dir = tempdir().unwrap();
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[],
            Some(env_dir.path()),
        )
        .unwrap();

        let env_content = std::fs::read_to_string(env_dir.path().join(".env")).unwrap();
        assert_eq!(env_content, "TELEGRAM_BOT_TOKEN=123456:ABCdef\n");
    }

    #[test]
    fn init_without_telegram_does_not_write_env_file() {
        let dir = tempdir().unwrap();
        let env_dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], Some(env_dir.path())).unwrap();

        assert!(
            !env_dir.path().join(".env").exists(),
            ".env should not exist when no telegram token"
        );
    }

    #[test]
    fn init_creates_bootstrap_md() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

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
        let env_dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], Some(env_dir.path())).unwrap();

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
    fn init_creates_settings_with_sandbox_config() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

        let settings_path = dir.path().join("agents/right/.claude/settings.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(json["sandbox"]["enabled"], true);
        assert_eq!(json["sandbox"]["autoAllowBashIfSandboxed"], true);
        assert_eq!(json["sandbox"]["allowUnsandboxedCommands"], false);
        assert!(json["sandbox"]["filesystem"]["allowWrite"].as_array().is_some());
        assert!(json["sandbox"]["network"]["allowedDomains"].as_array().is_some());
    }

    #[test]
    fn init_without_telegram_creates_settings_without_plugin() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], None).unwrap();

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
    fn init_with_telegram_writes_env_to_agent_channels_dir() {
        // Default path (no telegram_env_dir override) must write to AGENT dir.
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], None).unwrap();

        let env_path = dir
            .path()
            .join("agents/right/.claude/channels/telegram/.env");
        assert!(env_path.exists(), ".env should be written to agent channels dir");
        let content = std::fs::read_to_string(&env_path).unwrap();
        assert_eq!(content, "TELEGRAM_BOT_TOKEN=123456:ABCdef\n");
    }

    #[test]
    fn init_with_telegram_allowed_chat_ids_writes_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[85743491_i64, 100200300_i64],
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
            yaml.contains("  - 85743491"),
            "agent.yaml must list 85743491, got:\n{yaml}"
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
    fn init_with_telegram_sets_token_file_in_agent_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            &[85743491_i64],
            None,
        )
        .unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token_file: .claude/channels/telegram/.env"),
            "agent.yaml must contain telegram_token_file reference, got:\n{yaml}"
        );
        assert!(
            yaml.contains("allowed_chat_ids:"),
            "agent.yaml must contain allowed_chat_ids section, got:\n{yaml}"
        );
        assert!(
            yaml.contains("  - 85743491"),
            "agent.yaml must list chat id 85743491, got:\n{yaml}"
        );
        // Token itself must NOT appear in agent.yaml
        assert!(
            !yaml.contains("123456:ABCdef"),
            "raw token must NOT appear in agent.yaml"
        );
    }

    #[test]
    fn init_with_telegram_no_chat_ids_does_not_write_allowed_chat_ids() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), &[], None).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("telegram_token_file"),
            "telegram_token_file must be set"
        );
        assert!(
            !yaml.contains("allowed_chat_ids"),
            "allowed_chat_ids must not appear when no chat IDs provided"
        );
    }
}
