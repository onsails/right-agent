pub mod bot;
pub mod dispatch;
pub mod filter;
pub mod handler;
pub mod session;
pub mod worker;

pub use dispatch::run_telegram;
pub use session::effective_thread_id;

/// Bot adaptor type alias used by WorkerContext and dispatch logic.
/// Ordering: CacheMe<Throttle<Bot>> per BOT-03 (Throttle inner, CacheMe outer).
pub type BotType = teloxide::adaptors::CacheMe<teloxide::adaptors::throttle::Throttle<teloxide::Bot>>;

use std::path::Path;
use rightclaw::agent::types::AgentConfig;

/// Resolve Telegram token using priority chain (D-13):
/// 1. RC_TELEGRAM_TOKEN env var
/// 2. RC_TELEGRAM_TOKEN_FILE env var (read file contents)
/// 3. agent.yaml telegram_token_file field
/// 4. agent.yaml telegram_token field
/// Returns Err if no non-empty value found.
/// Extract token value from file contents.
/// Handles both raw token and `KEY=VALUE` env-file format (written by `rightclaw init`).
/// Splits on the first `=` if present; returns trimmed value. Falls back to full trimmed content.
fn token_from_file_content(content: &str) -> String {
    let trimmed = content.trim();
    if let Some((_, value)) = trimmed.split_once('=') {
        value.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn resolve_token(agent_dir: &Path, config: &AgentConfig) -> miette::Result<String> {
    // 1. RC_TELEGRAM_TOKEN env var
    if let Ok(token) = std::env::var("RC_TELEGRAM_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }
    // 2. RC_TELEGRAM_TOKEN_FILE env var
    if let Ok(path) = std::env::var("RC_TELEGRAM_TOKEN_FILE") {
        return std::fs::read_to_string(&path)
            .map(|s| token_from_file_content(&s))
            .map_err(|e| miette::miette!("RC_TELEGRAM_TOKEN_FILE read error: {e}"));
    }
    // 3. agent.yaml telegram_token_file
    if let Some(rel) = &config.telegram_token_file {
        return std::fs::read_to_string(agent_dir.join(rel))
            .map(|s| token_from_file_content(&s))
            .map_err(|e| miette::miette!("telegram_token_file read error: {e}"));
    }
    // 4. agent.yaml telegram_token
    if let Some(token) = &config.telegram_token {
        if !token.is_empty() {
            return Ok(token.clone());
        }
    }
    Err(miette::miette!(
        help = "Set RC_TELEGRAM_TOKEN env var or add telegram_token/telegram_token_file to agent.yaml",
        "No Telegram token found for this agent"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::agent::types::AgentConfig;
    use std::collections::HashMap;

    fn minimal_config() -> AgentConfig {
        AgentConfig {
            restart: Default::default(),
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            telegram_token_file: None,
            telegram_token: None,
            telegram_user_id: None,
            allowed_chat_ids: vec![],
            env: HashMap::new(),
        }
    }

    /// Regression test: token file written by `rightclaw init` uses KEY=VALUE format
    /// (`TELEGRAM_BOT_TOKEN=<token>`). resolve_token must return the value, not the full line.
    #[test]
    fn resolve_token_parses_key_value_env_file_format() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join(".telegram.env");
        std::fs::write(&env_file, "TELEGRAM_BOT_TOKEN=123:abc_TEST_token\n").unwrap();

        let mut config = minimal_config();
        config.telegram_token_file = Some(".telegram.env".to_string());

        let token = resolve_token(dir.path(), &config).unwrap();
        assert_eq!(token, "123:abc_TEST_token", "must extract value from KEY=VALUE line");
    }

    #[test]
    fn resolve_token_reads_raw_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join(".raw.env");
        std::fs::write(&env_file, "123:raw_token\n").unwrap();

        let mut config = minimal_config();
        config.telegram_token_file = Some(".raw.env".to_string());

        let token = resolve_token(dir.path(), &config).unwrap();
        assert_eq!(token, "123:raw_token");
    }

    #[test]
    fn resolve_token_inline_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = minimal_config();
        config.telegram_token = Some("999:inline_token".to_string());

        let token = resolve_token(dir.path(), &config).unwrap();
        assert_eq!(token, "999:inline_token");
    }

    #[test]
    fn resolve_token_returns_err_when_nothing_configured() {
        let dir = tempfile::tempdir().unwrap();
        let config = minimal_config();
        // No RC_TELEGRAM_TOKEN or RC_TELEGRAM_TOKEN_FILE env vars, no config fields.
        // Only fails if neither RC_TELEGRAM_TOKEN nor RC_TELEGRAM_TOKEN_FILE env vars are set.
        // We can't guarantee env is clean in test runner, so only assert err when both config
        // fields are None — rely on file/env chain being absent.
        // This is a best-effort test; skip if env vars happen to be set.
        if std::env::var("RC_TELEGRAM_TOKEN").is_err()
            && std::env::var("RC_TELEGRAM_TOKEN_FILE").is_err()
        {
            assert!(resolve_token(dir.path(), &config).is_err());
        }
    }
}
