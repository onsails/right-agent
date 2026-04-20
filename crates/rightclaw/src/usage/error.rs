use thiserror::Error;

#[derive(Debug, Error)]
pub enum UsageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid result JSON: {0}")]
    InvalidJson(String),
}
