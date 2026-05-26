use std::path::PathBuf;

/// Errors from per-agent database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Database(#[from] turso::Error),

    #[error("database row not found")]
    NotFound,

    #[error("invalid database parameter: {0}")]
    InvalidParameter(String),

    #[error("database constraint violation: {0}")]
    Constraint(String),

    #[error("open database {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: turso::Error,
    },

    #[error("legacy SQLite scrubber on {path}: {source}")]
    LegacySqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("migration {version} on {path}: {source}")]
    Migration {
        path: PathBuf,
        version: u32,
        #[source]
        source: Box<DbError>,
    },

    #[error("migration version {version} on {path}: {message}")]
    MigrationVersion {
        path: PathBuf,
        version: u32,
        message: String,
    },
}

impl DbError {
    pub fn is_open_error(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn is_constraint_violation(&self) -> bool {
        match self {
            Self::Constraint(_) => true,
            Self::Database(error) => is_turso_constraint(error),
            Self::Migration { source, .. } => source.is_constraint_violation(),
            _ => false,
        }
    }

    /// True if the error is a retryable lock contention (SQLite `BUSY` /
    /// `BUSY_SNAPSHOT`). Callers retrying their own write loop should use
    /// this predicate instead of stringly classifying the `Display` form.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Database(error) => is_turso_transient(error),
            Self::Open { source, .. } => is_turso_transient(source),
            Self::LegacySqlite { source, .. } => is_rusqlite_transient(source),
            Self::Migration { source, .. } => source.is_transient(),
            _ => false,
        }
    }
}

fn is_turso_constraint(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Constraint(_))
}

fn is_turso_transient(error: &turso::Error) -> bool {
    match error {
        turso::Error::Busy(_) | turso::Error::BusySnapshot(_) => true,
        // Turso currently maps its lower-level file-locking error into the
        // generic Error variant, so classify only the precise lock owner case.
        turso::Error::Error(message) => {
            message.starts_with("Locking error:")
                && message.contains("File is locked by another process")
        }
        _ => false,
    }
}

fn is_rusqlite_transient(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
mod tests {
    use crate::Connection;
    use crate::DbError;
    use crate::params;

    #[test]
    fn is_transient_detects_busy_variants() {
        assert!(
            DbError::Database(turso::Error::Busy("locked".into())).is_transient(),
            "Busy must be transient",
        );
        assert!(
            DbError::Database(turso::Error::BusySnapshot("snapshot".into())).is_transient(),
            "BusySnapshot must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Busy("locked".into()),
            })
            .is_transient(),
            "Open(Busy) must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::BusySnapshot("snapshot".into()),
            })
            .is_transient(),
            "Open(BusySnapshot) must be transient",
        );
        assert!(
            (DbError::Open {
                path: "data.db".into(),
                source: turso::Error::Error(
                    "Locking error: Failed locking file '/tmp/data.db'. File is locked by another process"
                        .into()
                ),
            })
            .is_transient(),
            "Turso file locking errors must be transient",
        );
        assert!(
            (DbError::LegacySqlite {
                path: "data.db".into(),
                source: rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    None,
                ),
            })
            .is_transient(),
            "LegacySqlite(SQLITE_BUSY) must be transient",
        );
        assert!(
            (DbError::LegacySqlite {
                path: "data.db".into(),
                source: rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
                    None,
                ),
            })
            .is_transient(),
            "LegacySqlite(SQLITE_LOCKED) must be transient",
        );
        assert!(
            !DbError::Database(turso::Error::Constraint("unique".into())).is_transient(),
            "Constraint is not transient",
        );
        assert!(
            !DbError::NotFound.is_transient(),
            "NotFound is not transient",
        );
    }

    #[tokio::test]
    async fn is_constraint_violation_detects_unique_violation() {
        let conn = Connection::open_in_memory()
            .await
            .expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE constraint_probe (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE
             )",
        )
        .await
        .expect("create table");

        conn.execute(
            "INSERT INTO constraint_probe (name) VALUES (?1)",
            params!["duplicate-name"],
        )
        .await
        .expect("first insert succeeds");

        let err = conn
            .execute(
                "INSERT INTO constraint_probe (name) VALUES (?1)",
                params!["duplicate-name"],
            )
            .await
            .expect_err("duplicate insert must fail");

        assert!(
            err.is_constraint_violation(),
            "expected constraint violation, got: {err:#}",
        );
    }
}
