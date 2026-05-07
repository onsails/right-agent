use right_core::agent_types::AgentDef;

/// Generate Telegram channel config for an agent under HOME override.
///
/// Writes to `$AGENT_DIR/.claude/channels/telegram/`:
///   - `.env` with `TELEGRAM_BOT_TOKEN=<token>` (always overwritten)
///
/// No-ops silently if `telegram_token` is not set.
pub fn generate_telegram_channel_config(agent: &AgentDef) -> miette::Result<()> {
    let token = match resolve_telegram_token(agent)? {
        Some(t) => t,
        None => return Ok(()),
    };

    let channel_dir = agent.path.join(".claude").join("channels").join("telegram");
    std::fs::create_dir_all(&channel_dir)
        .map_err(|e| miette::miette!("failed to create telegram channel dir: {e:#}"))?;

    // Always overwrite .env (idempotent, D-08)
    std::fs::write(
        channel_dir.join(".env"),
        format!("TELEGRAM_BOT_TOKEN={token}\n"),
    )
    .map_err(|e| miette::miette!("failed to write telegram .env: {e:#}"))?;

    tracing::debug!(agent = %agent.name, "wrote telegram channel config");
    Ok(())
}

/// Resolve the Telegram bot token from AgentConfig.
///
/// Returns `telegram_token` if set, `None` otherwise.
pub(crate) fn resolve_telegram_token(agent: &AgentDef) -> miette::Result<Option<String>> {
    let config = match agent.config.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };

    Ok(config.telegram_token.clone())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use right_core::agent_types::{AgentConfig, AgentDef, RestartPolicy};
    use tempfile::tempdir;

    fn make_agent(dir: &Path, config: Option<AgentConfig>) -> AgentDef {
        AgentDef {
            name: "test-agent".to_string(),
            path: dir.to_path_buf(),
            identity_path: dir.join("IDENTITY.md"),
            config,
            soul_path: None,
            user_path: None,
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
            network_policy: Default::default(),
            model: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: std::collections::HashMap::new(),
            secret: None,
            attachments: Default::default(),
            show_thinking: true,
            memory: None,
            stt: Default::default(),
        }
    }

    #[test]
    fn no_config_returns_ok_writes_nothing() {
        let dir = tempdir().unwrap();
        let agent = make_agent(dir.path(), None);
        generate_telegram_channel_config(&agent).unwrap();
        assert!(
            !dir.path().join(".claude/channels/telegram/.env").exists(),
            ".env should not be written when no config"
        );
    }

    #[test]
    fn no_token_returns_ok_writes_nothing() {
        let dir = tempdir().unwrap();
        let agent = make_agent(dir.path(), Some(base_config()));
        generate_telegram_channel_config(&agent).unwrap();
        assert!(
            !dir.path().join(".claude/channels/telegram/.env").exists(),
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

        let content =
            std::fs::read_to_string(dir.path().join(".claude/channels/telegram/.env")).unwrap();
        assert_eq!(content, "TELEGRAM_BOT_TOKEN=123456:ABCtoken\n");
    }

    #[test]
    fn token_does_not_write_access_json() {
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
            "access.json should not be written (access is managed by the bot, not codegen)"
        );
    }

    #[test]
    fn called_twice_overwrites_env() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            telegram_token: Some("TOKEN_V1".to_string()),
            ..base_config()
        };
        let agent = make_agent(dir.path(), Some(config));
        generate_telegram_channel_config(&agent).unwrap();

        let config2 = AgentConfig {
            telegram_token: Some("TOKEN_V2".to_string()),
            ..base_config()
        };
        let agent2 = make_agent(dir.path(), Some(config2));
        generate_telegram_channel_config(&agent2).unwrap();

        let env_content =
            std::fs::read_to_string(dir.path().join(".claude/channels/telegram/.env")).unwrap();
        assert!(
            env_content.contains("TOKEN_V2"),
            ".env should be overwritten"
        );
    }
}
