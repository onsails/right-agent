use thiserror::Error;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("agent not found: {name}")]
    AgentNotFound { name: String },

    #[error("no Telegram token found; set RC_TELEGRAM_TOKEN or configure agent.yaml")]
    NoToken,

    #[error("database error: {0}")]
    DbError(#[from] rightclaw::memory::MemoryError),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("signal handler registration failed: {0}")]
    SignalError(String),
}
