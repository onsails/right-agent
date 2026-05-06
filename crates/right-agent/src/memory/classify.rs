//! Classification of `MemoryError` into operational `ErrorKind`.

use super::MemoryError;

/// Operational classification used by the resilient wrapper to decide retry,
/// breaker-tick, enqueue, and surface behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Transient,   // 5xx, timeout, connect error
    RateLimited, // 429
    Auth,        // 401, 403
    Client,      // 400, 404, 422 (caller bug or upstream API drift)
    Malformed,   // response body parse error
}

impl MemoryError {
    /// Classify this error. Non-Hindsight variants (Db/Sqlite/Migration/Injection/NotFound)
    /// are unreachable at the wrapper boundary and classified as `Transient` defensively.
    pub fn classify(&self) -> ErrorKind {
        match self {
            MemoryError::Hindsight { status, .. } => match *status {
                401 | 403 => ErrorKind::Auth,
                429 => ErrorKind::RateLimited,
                400 | 404 | 422 => ErrorKind::Client,
                _ => ErrorKind::Transient,
            },
            MemoryError::HindsightTimeout => ErrorKind::Transient,
            MemoryError::HindsightConnect(_) => ErrorKind::Transient,
            MemoryError::HindsightParse(_) => ErrorKind::Malformed,
            MemoryError::HindsightOther(_) => ErrorKind::Transient,
            MemoryError::Db(_)
            | MemoryError::Sqlite(_)
            | MemoryError::Migration(_)
            | MemoryError::InjectionDetected
            | MemoryError::NotFound(_) => ErrorKind::Transient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(status: u16) -> MemoryError {
        MemoryError::Hindsight {
            status,
            body: String::new(),
        }
    }

    #[test]
    fn classify_5xx_transient() {
        assert_eq!(h(500).classify(), ErrorKind::Transient);
        assert_eq!(h(502).classify(), ErrorKind::Transient);
        assert_eq!(h(503).classify(), ErrorKind::Transient);
        assert_eq!(h(504).classify(), ErrorKind::Transient);
    }

    #[test]
    fn classify_429_rate_limited() {
        assert_eq!(h(429).classify(), ErrorKind::RateLimited);
    }

    #[test]
    fn classify_auth() {
        assert_eq!(h(401).classify(), ErrorKind::Auth);
        assert_eq!(h(403).classify(), ErrorKind::Auth);
    }

    #[test]
    fn classify_client() {
        assert_eq!(h(400).classify(), ErrorKind::Client);
        assert_eq!(h(404).classify(), ErrorKind::Client);
        assert_eq!(h(422).classify(), ErrorKind::Client);
    }

    #[test]
    fn classify_timeout_transient() {
        assert_eq!(
            MemoryError::HindsightTimeout.classify(),
            ErrorKind::Transient
        );
    }

    #[test]
    fn classify_connect_transient() {
        assert_eq!(
            MemoryError::HindsightConnect("dns".into()).classify(),
            ErrorKind::Transient
        );
    }

    #[test]
    fn classify_parse_malformed() {
        assert_eq!(
            MemoryError::HindsightParse("bad json".into()).classify(),
            ErrorKind::Malformed
        );
    }

    #[test]
    fn classify_db_transient() {
        assert_eq!(
            MemoryError::Db(right_db::DbError::Sqlite(
                rusqlite::Error::ExecuteReturnedResults
            ))
            .classify(),
            ErrorKind::Transient
        );
    }
}
