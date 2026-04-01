pub mod bot;
pub mod dispatch;
pub mod filter;
pub mod session;

pub use dispatch::run_telegram;
pub use session::effective_thread_id;

use std::path::Path;
use rightclaw::agent::types::AgentConfig;

/// Resolve Telegram token using priority chain (D-13):
/// 1. RC_TELEGRAM_TOKEN env var
/// 2. RC_TELEGRAM_TOKEN_FILE env var (read file contents)
/// 3. agent.yaml telegram_token_file field
/// 4. agent.yaml telegram_token field
/// Returns Err if no non-empty value found.
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
            .map(|s| s.trim().to_string())
            .map_err(|e| miette::miette!("RC_TELEGRAM_TOKEN_FILE read error: {e}"));
    }
    // 3. agent.yaml telegram_token_file
    if let Some(rel) = &config.telegram_token_file {
        return std::fs::read_to_string(agent_dir.join(rel))
            .map(|s| s.trim().to_string())
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
