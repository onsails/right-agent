//! Per-agent SQLite plumbing for `right`.
//!
//! Owns `data.db` open/migrate logic and the central migration registry.
//! Domain crates (`right-mcp`, `right-memory`, `right-codegen`, the slim
//! `right-agent`, `right-bot`) call `open_connection` here; new tables
//! are added by editing the central `migrations::MIGRATIONS` array.

#![warn(unreachable_pub)]

pub mod connection;
pub mod conversation;
pub mod error;
pub mod migrations;
pub mod params;
pub mod row;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transaction;

pub use connection::Connection;
pub use error::DbError;
pub use migrations::MIGRATIONS;
pub use params::params_from_iter;
pub use row::Row;
pub use transaction::Transaction;

use std::path::Path;

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>, DbError>;
}

impl<T> OptionalExtension<T> for Result<T, DbError> {
    fn optional(self) -> Result<Option<T>, DbError> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(DbError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// Open the per-agent SQLite database, applying migrations if requested.
///
/// Idempotent. WAL journal mode + 5s busy_timeout. The connection is
/// returned for callers that need it; use [`open_db`] when you only
/// want to ensure the file exists.
pub fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let conn = Connection::open_local(db_path, true)?;
    conn.apply_connection_pragmas()?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&conn)?;
    }
    Ok(conn)
}

/// Open the per-agent SQLite database, dropping the connection.
/// Used when the caller only needs the file created and migrated.
pub fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError> {
    open_connection(agent_path, migrate).map(drop)
}

/// Open the per-agent SQLite database in read-only mode.
///
/// Unlike [`open_connection`], this never creates the database file and
/// never runs migrations. It is intended for read-only consumers such as
/// the Telegram Mini App dashboard, where the "do not create or mutate"
/// guarantee must be structural, not advisory.
pub fn open_connection_readonly(agent_dir: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = agent_dir.as_ref().join("data.db");
    open_database_path_readonly(db_path)
}

/// Open an explicit SQLite database path in read-only mode.
///
/// This never creates the database file and never runs migrations. Use
/// [`open_connection_readonly`] for the standard per-agent `data.db` path.
pub fn open_database_path_readonly(db_path: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = db_path.as_ref().to_path_buf();
    let conn = Connection::open_local(db_path, false)?;
    conn.apply_readonly_pragmas()?;
    Ok(conn)
}
