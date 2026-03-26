/// Errors that can occur in the memory module.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("content rejected: possible prompt injection detected")]
    InjectionDetected,

    #[error("memory not found: id {0}")]
    NotFound(i64),
}
