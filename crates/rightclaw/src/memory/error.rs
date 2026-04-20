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

    #[error("hindsight API error (HTTP {status}): {body}")]
    Hindsight { status: u16, body: String },

    #[error("hindsight request timed out")]
    HindsightTimeout,

    #[error("hindsight connection error: {0}")]
    HindsightConnect(String),

    #[error("hindsight response parse error: {0}")]
    HindsightParse(String),

    #[error("hindsight request error: {0}")]
    HindsightOther(String),

    #[deprecated(note = "use HindsightTimeout/Connect/Parse/Other variants")]
    #[error("hindsight request failed: {0}")]
    HindsightRequest(String),
}

impl MemoryError {
    /// Convert a reqwest::Error from send/recv into the appropriate classified variant.
    pub fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            MemoryError::HindsightTimeout
        } else if e.is_connect() || e.is_request() {
            MemoryError::HindsightConnect(format!("{e:#}"))
        } else if e.is_decode() || e.is_body() {
            MemoryError::HindsightParse(format!("{e:#}"))
        } else {
            MemoryError::HindsightOther(format!("{e:#}"))
        }
    }

    /// Convert a JSON deserialization error.
    pub fn from_parse(e: impl std::fmt::Display) -> Self {
        MemoryError::HindsightParse(format!("{e:#}"))
    }
}
