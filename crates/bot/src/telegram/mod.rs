pub mod attachments;
pub mod bot;
pub mod dispatch;
pub mod filter;
pub mod handler;
pub mod oauth_callback;
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
/// 3. agent.yaml telegram_token field
///
/// Returns Err if no non-empty value found.
///
/// Extract token value from file contents.
/// Handles both raw token and `KEY=VALUE` env-file format.
/// Splits on the first `=` if present; returns trimmed value. Falls back to full trimmed content.
fn token_from_file_content(content: &str) -> String {
    let trimmed = content.trim();
    if let Some((_, value)) = trimmed.split_once('=') {
        value.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn resolve_token(_agent_dir: &Path, config: &AgentConfig) -> miette::Result<String> {
    // 1. RC_TELEGRAM_TOKEN env var
    if let Ok(token) = std::env::var("RC_TELEGRAM_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }
    // 2. RC_TELEGRAM_TOKEN_FILE env var
    if let Ok(path) = std::env::var("RC_TELEGRAM_TOKEN_FILE") {
        return std::fs::read_to_string(&path)
            .map(|s| token_from_file_content(&s))
            .map_err(|e| miette::miette!("RC_TELEGRAM_TOKEN_FILE read error: {e}"));
    }
    // 3. agent.yaml telegram_token
    if let Some(token) = &config.telegram_token
        && !token.is_empty()
    {
        return Ok(token.clone());
    }
    Err(miette::miette!(
        help = "Set RC_TELEGRAM_TOKEN env var or add telegram_token to agent.yaml",
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
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: HashMap::new(),
            secret: None,
        }
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
        // Best-effort test; skip if env vars happen to be set.
        if std::env::var("RC_TELEGRAM_TOKEN").is_err()
            && std::env::var("RC_TELEGRAM_TOKEN_FILE").is_err()
        {
            assert!(resolve_token(dir.path(), &config).is_err());
        }
    }
}
