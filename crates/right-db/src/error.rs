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

    #[error("legacy SQLite migration on {path}: {source}")]
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
}

fn is_turso_constraint(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Constraint(_))
}

#[cfg(test)]
mod tests {
    use crate::Connection;
    use crate::params;

    #[test]
    fn is_constraint_violation_detects_unique_violation() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE constraint_probe (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE
             )",
        )
        .expect("create table");

        conn.execute(
            "INSERT INTO constraint_probe (name) VALUES (?1)",
            params!["duplicate-name"],
        )
        .expect("first insert succeeds");

        let err = conn
            .execute(
                "INSERT INTO constraint_probe (name) VALUES (?1)",
                params!["duplicate-name"],
            )
            .expect_err("duplicate insert must fail");

        assert!(
            err.is_constraint_violation(),
            "expected constraint violation, got: {err:#}",
        );
    }
}
