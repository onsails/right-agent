use thiserror::Error;

#[derive(Debug, Error)]
pub enum UsageError {
    #[error("database error: {0}")]
    Db(#[from] right_db::DbError),
    #[error("invalid result JSON: {0}")]
    InvalidJson(String),
}
