use crate::agent::AgentDef;

/// Generate Telegram channel config for an agent under HOME override.
///
/// Writes to `$AGENT_DIR/.claude/channels/telegram/`:
///   - `.env` with `TELEGRAM_BOT_TOKEN=<token>` (always overwritten)
///   - `access.json` with allowlist (only if `telegram_user_id` set)
///
/// No-ops silently if neither `telegram_token` nor `telegram_token_file` is set.
pub fn generate_telegram_channel_config(agent: &AgentDef) -> miette::Result<()> {
    let token = match resolve_telegram_token(agent)? {
        Some(t) => t,
        None => return Ok(()),
    };

    let channel_dir = agent.path.join(".claude").join("channels").join("telegram");
    std::fs::create_dir_all(&channel_dir)
        .map_err(|e| miette::miette!("failed to create telegram channel dir: {e:#}"))?;

    // Always overwrite .env (idempotent, D-08)
    std::fs::write(channel_dir.join(".env"), format!("TELEGRAM_BOT_TOKEN={token}\n"))
        .map_err(|e| miette::miette!("failed to write telegram .env: {e:#}"))?;

    // access.json only if user_id set
    if let Some(user_id) = agent.config.as_ref().and_then(|c| c.telegram_user_id.as_deref()) {
        let access_json = format!(
            r#"{{"dmPolicy":"allowlist","allowFrom":["{user_id}"],"pending":{{}},"groups":{{}}}}"#
        );
        // Always overwrite access.json (D-08)
        std::fs::write(channel_dir.join("access.json"), access_json)
            .map_err(|e| miette::miette!("failed to write access.json: {e:#}"))?;
    }

    tracing::debug!(agent = %agent.name, "wrote telegram channel config");
    Ok(())
}

/// Resolve the Telegram bot token from AgentConfig.
///
/// Precedence: `telegram_token_file` > `telegram_token` > `None`.
/// `telegram_token_file` path is resolved relative to `agent.path`.
fn resolve_telegram_token(agent: &AgentDef) -> miette::Result<Option<String>> {
    let config = match agent.config.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };

    if let Some(ref file_path) = config.telegram_token_file {
        let abs = agent.path.join(file_path);
        let content = std::fs::read_to_string(&abs).map_err(|e| {
            miette::miette!(
                "failed to read telegram token file {}: {e:#}",
                abs.display()
            )
        })?;
        // Handle dotenv format: strip `TELEGRAM_BOT_TOKEN=` prefix if present.
        // init writes a .env file (dotenv format for CC), but resolve_telegram_token
        // must return just the raw token value.
        let trimmed = content.trim();
        let token = trimmed
            .strip_prefix("TELEGRAM_BOT_TOKEN=")
            .unwrap_or(trimmed);
        return Ok(Some(token.to_string()));
    }

    Ok(config.telegram_token.clone())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
    use tempfile::tempdir;

    fn make_agent(dir: &Path, config: Option<AgentConfig>) -> AgentDef {
        AgentDef {
            name: "test-agent".to_string(),
            path: dir.to_path_buf(),
            identity_path: dir.join("IDENTITY.md"),
            config,
            soul_path: None,
            user_path: None,
            agents_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        }
    }

    fn base_config() -> AgentConfig {
        AgentConfig {
            restart: RestartPolicy::OnFailure,
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            telegram_token_file: None,
            telegram_token: None,
            telegram_user_id: None,
            allowed_chat_ids: vec![],
            env: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn no_config_returns_ok_writes_nothing() {
        let dir = tempdir().unwrap();
        let agent = make_agent(dir.path(), None);
        generate_telegram_channel_config(&agent).unwrap();
        assert!(
            !dir.path()
                .join(".claude/channels/telegram/.env")
                .exists(),
            ".env should not be written when no config"
        );
    }

    #[test]
    fn no_token_returns_ok_writes_nothing() {
        let dir = tempdir().unwrap();
        let agent = make_agent(dir.path(), Some(base_config()));
        generate_telegram_channel_config(&agent).unwrap();
        assert!(
            !dir.path()
                .join(".claude/channels/telegram/.env")
                .exists(),
            ".env should not be written when no token"
        );
    }

    #[test]
    fn inline_token_writes_env_file() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            telegram_token: Some("123456:ABCtoken".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/.env"),
        )
        .unwrap();
        assert_eq!(content, "TELEGRAM_BOT_TOKEN=123456:ABCtoken\n");
    }

    #[test]
    fn token_and_user_id_writes_access_json() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            telegram_token: Some("123:abc".to_string()),
            telegram_user_id: Some("987654321".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let access =
            std::fs::read_to_string(dir.path().join(".claude/channels/telegram/access.json"))
                .unwrap();
        assert!(access.contains("allowlist"), "dmPolicy should be allowlist");
        assert!(access.contains("987654321"), "user_id should be in allowFrom");
    }

    #[test]
    fn token_without_user_id_does_not_write_access_json() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            telegram_token: Some("123:abc".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        assert!(
            !dir.path()
                .join(".claude/channels/telegram/access.json")
                .exists(),
            "access.json should not be written without user_id"
        );
    }

    #[test]
    fn token_file_reads_token_relative_to_agent_path() {
        let dir = tempdir().unwrap();
        // Write the token file into the agent directory
        std::fs::write(dir.path().join(".telegram.env"), "999:tokenFromFile\n").unwrap();

        let config = AgentConfig {
            telegram_token_file: Some(".telegram.env".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/.env"),
        )
        .unwrap();
        assert_eq!(
            content, "TELEGRAM_BOT_TOKEN=999:tokenFromFile\n",
            "token from file should be trimmed and written"
        );
    }

    #[test]
    fn token_file_takes_precedence_over_inline_token() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".telegram.env"), "FILE_TOKEN\n").unwrap();

        let config = AgentConfig {
            telegram_token_file: Some(".telegram.env".to_string()),
            telegram_token: Some("INLINE_TOKEN".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/.env"),
        )
        .unwrap();
        assert!(
            content.contains("FILE_TOKEN"),
            "file token should take precedence"
        );
        assert!(
            !content.contains("INLINE_TOKEN"),
            "inline token should be ignored"
        );
    }

    #[test]
    fn called_twice_overwrites_env_and_access_json() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            telegram_token: Some("TOKEN_V1".to_string()),
            telegram_user_id: Some("111".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let config2 = AgentConfig {
            telegram_token: Some("TOKEN_V2".to_string()),
            telegram_user_id: Some("222".to_string()),
            ..base_config()
        };
        let agent2 = make_agent(dir.path(), Some(config2));
        generate_telegram_channel_config(&agent2).unwrap();

        let env_content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/.env"),
        )
        .unwrap();
        assert!(env_content.contains("TOKEN_V2"), ".env should be overwritten");

        let access_content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/access.json"),
        )
        .unwrap();
        assert!(access_content.contains("222"), "access.json should be overwritten");
    }

    #[test]
    fn token_file_in_dotenv_format_strips_prefix() {
        let dir = tempdir().unwrap();
        // Write dotenv format (what `init.rs` produces)
        std::fs::write(
            dir.path().join(".telegram.env"),
            "TELEGRAM_BOT_TOKEN=999:tokenFromDotenv\n",
        )
        .unwrap();

        let config = AgentConfig {
            telegram_token_file: Some(".telegram.env".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let content = std::fs::read_to_string(
            dir.path().join(".claude/channels/telegram/.env"),
        )
        .unwrap();
        assert_eq!(
            content, "TELEGRAM_BOT_TOKEN=999:tokenFromDotenv\n",
            "prefix must not be doubled — dotenv prefix should be stripped before re-writing"
        );
    }
}
