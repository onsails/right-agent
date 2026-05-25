use std::path::PathBuf;

/// Errors from per-agent database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Database(#[from] libsql::Error),

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
        source: libsql::Error,
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
            Self::Database(error) => is_libsql_constraint(error),
            Self::Migration { source, .. } => source.is_constraint_violation(),
            _ => false,
        }
    }
}

/// SQLite primary result code for constraint violations
/// (`SQLITE_CONSTRAINT = 19`). Extended codes encode subtype in the high
/// byte (e.g. `SQLITE_CONSTRAINT_UNIQUE = 2067`); the primary code is
/// always `code & 0xff`.
const SQLITE_CONSTRAINT: i32 = 19;

fn is_libsql_constraint(error: &libsql::Error) -> bool {
    match error {
        libsql::Error::SqliteFailure(code, _) => (*code as i32) & 0xff == SQLITE_CONSTRAINT,
        libsql::Error::RemoteSqliteFailure(code, extended, _) => {
            *code == SQLITE_CONSTRAINT || (*extended & 0xff) == SQLITE_CONSTRAINT
        }
        _ => false,
    }
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
