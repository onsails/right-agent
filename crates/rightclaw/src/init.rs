use std::path::Path;

const DEFAULT_IDENTITY: &str = include_str!("../../../templates/right/IDENTITY.md");
const DEFAULT_SOUL: &str = include_str!("../../../templates/right/SOUL.md");
const DEFAULT_AGENTS: &str = include_str!("../../../templates/right/AGENTS.md");
const DEFAULT_POLICY: &str = include_str!("../../../templates/right/policy.yaml");
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/BOOTSTRAP.md");
const DEFAULT_POLICY_TELEGRAM: &str = include_str!("../../../templates/right/policy-telegram.yaml");
const DEFAULT_AGENT_YAML: &str = include_str!("../../../templates/right/agent.yaml");
const SKILL_CLAWHUB: &str = include_str!("../../../skills/clawhub/SKILL.md");
const SKILL_CRONSYNC: &str = include_str!("../../../skills/cronsync/SKILL.md");

/// Initialize the RightClaw home directory with a default "right" agent.
///
/// Creates `home/agents/right/` with template files: IDENTITY.md, SOUL.md,
/// AGENTS.md, BOOTSTRAP.md, and policy.yaml (base or Telegram variant).
///
/// When `telegram_token` is provided:
/// - Uses `policy-telegram.yaml` instead of base `policy.yaml`
/// - Writes the token to `telegram_env_dir/.env` for Claude Code's Telegram plugin
/// - Creates `.claude/settings.json` with `enabledPlugins` for automatic Telegram plugin activation
///
/// `telegram_env_dir` controls where the `.env` file is written. Pass `None` to
/// use the default `~/.claude/channels/telegram/` path.
///
/// Returns an error if the agents directory already exists.
pub fn init_rightclaw_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_user_id: Option<&str>,
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

    let policy_template = if telegram_token.is_some() {
        DEFAULT_POLICY_TELEGRAM
    } else {
        DEFAULT_POLICY
    };

    // Expand ~ to actual home directory in policy (OpenShell requires absolute paths).
    let home_dir_str = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    let policy_content = policy_template.replace("~/", &format!("{home_dir_str}/"));

    let files: &[(&str, &str)] = &[
        ("IDENTITY.md", DEFAULT_IDENTITY),
        ("SOUL.md", DEFAULT_SOUL),
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];

    for (filename, content) in files {
        let path = agents_dir.join(filename);
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("Failed to write {}: {}", path.display(), e))?;
    }

    // Write policy separately (it has expanded paths).
    let policy_path = agents_dir.join("policy.yaml");
    std::fs::write(&policy_path, &policy_content)
        .map_err(|e| miette::miette!("Failed to write {}: {}", policy_path.display(), e))?;

    // Install built-in skills into .claude/skills/ (standard Agent Skills path).
    // Claude Code discovers skills from .claude/skills/ relative to cwd.
    let built_in_skills: &[(&str, &str)] = &[
        ("clawhub/SKILL.md", SKILL_CLAWHUB),
        ("cronsync/SKILL.md", SKILL_CRONSYNC),
    ];
    for (skill_path, content) in built_in_skills {
        let path = agents_dir.join(".claude").join("skills").join(skill_path);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
            miette::miette!("Failed to create skill directory: {}", e)
        })?;
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("Failed to write {}: {}", path.display(), e))?;
    }

    // Write Telegram bot token to .env and create .claude/settings.json if provided.
    if let Some(token) = telegram_token {
        let env_dir = match telegram_env_dir {
            Some(dir) => dir.to_path_buf(),
            None => {
                let home_dir = dirs::home_dir()
                    .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
                home_dir.join(".claude").join("channels").join("telegram")
            }
        };
        std::fs::create_dir_all(&env_dir).map_err(|e| {
            miette::miette!("Failed to create {}: {}", env_dir.display(), e)
        })?;
        std::fs::write(
            env_dir.join(".env"),
            format!("TELEGRAM_BOT_TOKEN={token}\n"),
        )
        .map_err(|e| {
            miette::miette!("Failed to write telegram .env: {}", e)
        })?;

        // Create .claude/settings.json with Telegram plugin auto-enabled.
        let claude_dir = agents_dir.join(".claude");
        std::fs::create_dir_all(&claude_dir).map_err(|e| {
            miette::miette!("Failed to create {}: {}", claude_dir.display(), e)
        })?;
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"enabledPlugins":{"telegram@claude-plugins-official":true}}"#,
        )
        .map_err(|e| {
            miette::miette!("Failed to write settings.json: {}", e)
        })?;

        // Create .mcp.json marker so the shell wrapper detects Telegram and adds --channels flag.
        // The file content doesn't matter for the plugin (it's loaded via settings.json),
        // but the wrapper uses .mcp.json existence to decide whether to pass --channels.
        std::fs::write(
            agents_dir.join(".mcp.json"),
            r#"{"telegram": true}"#,
        )
        .map_err(|e| {
            miette::miette!("Failed to write .mcp.json: {}", e)
        })?;

        // Pre-pair Telegram user via access.json so no interactive pairing is needed.
        // The Telegram plugin re-reads this file on every inbound message.
        if let Some(user_id) = telegram_user_id {
            let access_json = format!(
                r#"{{"dmPolicy":"allowlist","allowFrom":["{user_id}"],"pending":{{}},"groups":{{}}}}"#
            );
            std::fs::write(env_dir.join("access.json"), access_json).map_err(|e| {
                miette::miette!("Failed to write access.json: {}", e)
            })?;
        }
    }

    // Pre-trust the agent directory in Claude Code's config so the
    // workspace trust dialog doesn't block non-interactive launches.
    // See: https://github.com/anthropics/claude-code/issues/28506
    pre_trust_directory(&agents_dir)?;

    println!("Created RightClaw home at {}", home.display());
    println!("  agents/right/IDENTITY.md");
    println!("  agents/right/SOUL.md");
    println!("  agents/right/AGENTS.md");
    println!("  agents/right/BOOTSTRAP.md");
    println!("  agents/right/policy.yaml");
    println!("  agents/right/agent.yaml");
    println!("  agents/right/.claude/skills/clawhub/SKILL.md");
    println!("  agents/right/.claude/skills/cronsync/SKILL.md");

    if telegram_token.is_some() {
        println!("  Telegram bot token saved");
        println!("  agents/right/.claude/settings.json (Telegram plugin enabled)");
    }

    Ok(())
}

/// Pre-trust a directory in Claude Code's `~/.claude.json` config.
///
/// Sets `hasTrustDialogAccepted: true` for the given directory so Claude Code
/// doesn't show the "Quick safety check" workspace trust dialog on launch.
/// This is necessary because `--dangerously-skip-permissions` does NOT bypass
/// the trust dialog (see: github.com/anthropics/claude-code/issues/28506).
fn pre_trust_directory(agent_dir: &Path) -> miette::Result<()> {
    let home_dir = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    let claude_json_path = home_dir.join(".claude.json");

    let mut config: serde_json::Value = if claude_json_path.exists() {
        let content = std::fs::read_to_string(&claude_json_path)
            .map_err(|e| miette::miette!("failed to read {}: {}", claude_json_path.display(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse {}: {}", claude_json_path.display(), e))?
    } else {
        serde_json::json!({})
    };

    let abs_path = std::fs::canonicalize(agent_dir)
        .unwrap_or_else(|_| agent_dir.to_path_buf());
    let path_key = abs_path.display().to_string();

    let projects = config
        .as_object_mut()
        .ok_or_else(|| miette::miette!("~/.claude.json is not a JSON object"))?
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}));

    let project = projects
        .as_object_mut()
        .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
        .entry(&path_key)
        .or_insert_with(|| serde_json::json!({}));

    let project_obj = project
        .as_object_mut()
        .ok_or_else(|| miette::miette!("project entry is not a JSON object"))?;
    project_obj.insert(
        "hasTrustDialogAccepted".to_owned(),
        serde_json::Value::Bool(true),
    );
    std::fs::write(
        &claude_json_path,
        serde_json::to_string_pretty(&config)
            .map_err(|e| miette::miette!("failed to serialize config: {e}"))?,
    )
    .map_err(|e| miette::miette!("failed to write {}: {}", claude_json_path.display(), e))?;

    // Also write skipDangerousModePermissionPrompt to ~/.claude/settings.json
    // (user-level settings — separate file from .claude.json).
    let user_settings_dir = home_dir.join(".claude");
    std::fs::create_dir_all(&user_settings_dir).map_err(|e| {
        miette::miette!("failed to create {}: {}", user_settings_dir.display(), e)
    })?;
    let user_settings_path = user_settings_dir.join("settings.json");

    let mut user_settings: serde_json::Value = if user_settings_path.exists() {
        let content = std::fs::read_to_string(&user_settings_path).map_err(|e| {
            miette::miette!("failed to read {}: {}", user_settings_path.display(), e)
        })?;
        serde_json::from_str(&content).map_err(|e| {
            miette::miette!("failed to parse {}: {}", user_settings_path.display(), e)
        })?
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = user_settings.as_object_mut() {
        obj.insert(
            "skipDangerousModePermissionPrompt".to_owned(),
            serde_json::Value::Bool(true),
        );
    }

    std::fs::write(
        &user_settings_path,
        serde_json::to_string_pretty(&user_settings)
            .map_err(|e| miette::miette!("failed to serialize settings: {e}"))?,
    )
    .map_err(|e| miette::miette!("failed to write {}: {}", user_settings_path.display(), e))?;

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
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

        let agents_dir = dir.path().join("agents").join("right");
        assert!(agents_dir.join("IDENTITY.md").exists());
        assert!(agents_dir.join("SOUL.md").exists());
        assert!(agents_dir.join("AGENTS.md").exists());
        assert!(agents_dir.join("policy.yaml").exists());
        assert!(
            agents_dir.join("BOOTSTRAP.md").exists(),
            "BOOTSTRAP.md should always be created"
        );
        assert!(
            agents_dir.join("agent.yaml").exists(),
            "agent.yaml should always be created"
        );
        assert!(
            agents_dir.join(".claude/skills/clawhub/SKILL.md").exists(),
            "clawhub skill should be installed"
        );
        assert!(
            agents_dir.join(".claude/skills/cronsync/SKILL.md").exists(),
            "cronsync skill should be installed"
        );
    }

    #[test]
    fn init_identity_contains_right() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

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
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

        let result = init_rightclaw_home(dir.path(), None, None, None);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("already initialized"),
            "expected 'already initialized' in: {err}"
        );
    }

    #[test]
    fn init_without_telegram_uses_base_policy() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

        let policy = std::fs::read_to_string(
            dir.path().join("agents/right/policy.yaml"),
        )
        .unwrap();
        // Base policy has telegram commented out
        assert!(
            policy.contains("# telegram_api:"),
            "base policy should have telegram_api commented out"
        );
    }

    #[test]
    fn init_with_telegram_uses_telegram_policy() {
        let dir = tempdir().unwrap();
        let env_dir = tempdir().unwrap();
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            None,
            Some(env_dir.path()),
        )
        .unwrap();

        let policy = std::fs::read_to_string(
            dir.path().join("agents/right/policy.yaml"),
        )
        .unwrap();
        // Telegram policy has telegram_api uncommented
        assert!(
            policy.contains("telegram_api:") && !policy.contains("# telegram_api:"),
            "telegram policy should have telegram_api uncommented"
        );
    }

    #[test]
    fn init_with_telegram_writes_token_env_file() {
        let dir = tempdir().unwrap();
        let env_dir = tempdir().unwrap();
        init_rightclaw_home(
            dir.path(),
            Some("123456:ABCdef"),
            None,
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
        init_rightclaw_home(dir.path(), None, None, Some(env_dir.path())).unwrap();

        assert!(
            !env_dir.path().join(".env").exists(),
            ".env should not exist when no telegram token"
        );
    }

    #[test]
    fn init_creates_bootstrap_md() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

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
        init_rightclaw_home(dir.path(), Some("123456:ABCdef"), None, Some(env_dir.path())).unwrap();

        let settings_path = dir
            .path()
            .join("agents/right/.claude/settings.json");
        assert!(
            settings_path.exists(),
            "settings.json should be created when telegram token is provided"
        );

        let content = std::fs::read_to_string(&settings_path).unwrap();
        assert!(
            content.contains("enabledPlugins"),
            "settings.json should contain enabledPlugins"
        );
        assert!(
            content.contains("telegram@claude-plugins-official"),
            "settings.json should enable telegram plugin"
        );
    }

    #[test]
    fn init_without_telegram_no_settings_json() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, None, None).unwrap();

        let settings_path = dir
            .path()
            .join("agents/right/.claude/settings.json");
        assert!(
            !settings_path.exists(),
            "settings.json should NOT be created when no telegram token"
        );
    }
}
