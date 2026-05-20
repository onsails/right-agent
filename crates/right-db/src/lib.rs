//! Per-agent SQLite plumbing for `right`.
//!
//! Owns `data.db` open/migrate logic and the central migration registry.
//! Domain crates (`right-mcp`, `right-memory`, `right-codegen`, the slim
//! `right-agent`, `right-bot`) call `open_connection` here; new tables
//! are added by editing the central `migrations::MIGRATIONS` array.

#![warn(unreachable_pub)]

pub mod conversation;
pub mod error;
pub mod migrations;

pub use error::DbError;
pub use migrations::MIGRATIONS;

use std::path::Path;
use std::time::Duration;

/// Open the per-agent SQLite database, applying migrations if requested.
///
/// Idempotent. WAL journal mode + 5s busy_timeout. The connection is
/// returned for callers that need it; use [`open_db`] when you only
/// want to ensure the file exists.
pub fn open_connection(agent_path: &Path, migrate: bool) -> Result<rusqlite::Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&mut conn)?;
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
pub fn open_connection_readonly(
    agent_dir: impl AsRef<Path>,
) -> rusqlite::Result<rusqlite::Connection> {
    let db_path = agent_dir.as_ref().join("data.db");
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
        | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let conn = rusqlite::Connection::open_with_flags(&db_path, flags)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}
