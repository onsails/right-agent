//! Per-agent SQLite plumbing for `right`.
//!
//! The Aggregator owns each live filesystem connection. Offline callers use
//! these open/migrate primitives only after proving runtime quiescence; new
//! tables are added by editing the central `migrations::MIGRATIONS` array.

#![warn(unreachable_pub)]
#![allow(clippy::unnecessary_mut_passed, clippy::unnecessary_literal_unwrap)]

pub mod bootstrap_answers;
mod bootstrap_lock;
pub mod connection;
pub mod conversation;
pub mod error;
pub mod forum_topics;
pub mod migrations;
pub mod params;
mod repair;
pub mod row;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod thread_focus;
pub mod transaction;

pub use connection::Connection;
pub use error::DbError;
pub use migrations::MIGRATIONS;
pub use params::params_from_iter;
pub use repair::{FileDigest, RepairReport, RepairRequest, TableInvariant, repair_legacy_wal};
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
/// Idempotent. Standard local WAL journal mode + 5s busy_timeout. The
/// connection is returned for the Aggregator's single live owner; use
/// [`open_db`] when an offline caller only needs to create and migrate it.
///
/// # Migration semantics
///
/// When `migrate = true`, all pending migrations run inside a single
/// immediate transaction. This has two load-bearing consequences:
///
/// - **All-or-nothing rollback.** If migration N fails, every prior
///   migration in the same batch is rolled back along with it. The
///   on-disk `user_version` is unchanged on failure. This is intentional
///   and codified by
///   `migration_runner_semantics_rolls_back_all_pending_migrations_on_later_failure`.
/// - **Cold-boot contention.** Another offline migrator blocks on the immediate
///   transaction for the full batch duration. Live runtime code instead routes
///   through the Aggregator's retained owner connection.
///
/// Open failures propagate unchanged. Legacy multiprocess-WAL recovery is an
/// explicit offline operation through [`repair_legacy_wal`], never an open-time
/// retry or sidecar mutation.
pub async fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    open_connection_once(agent_path, migrate).await
}
async fn open_connection_once(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");

    if migrate {
        let _bootstrap_lock = bootstrap_lock::acquire(agent_path).await?;
        let conn = Connection::open_local(db_path, true).await?;
        conn.apply_connection_pragmas().await?;
        migrations::MIGRATIONS.to_latest(&conn).await?;
        return Ok(conn);
    }

    let conn = Connection::open_local(db_path, true).await?;
    conn.apply_connection_pragmas().await?;
    Ok(conn)
}

/// Open the per-agent SQLite database, dropping the connection.
/// Used when the caller only needs the file created and migrated.
pub async fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError> {
    open_connection(agent_path, migrate).await.map(drop)
}

/// Open the per-agent SQLite database in read-only mode.
///
/// Unlike [`open_connection`], this never creates the database file and
/// never runs migrations. Runtime consumers use typed owner IPC; this adapter
/// is for offline callers that have already proved quiescence.
pub async fn open_connection_readonly(agent_dir: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = agent_dir.as_ref().join("data.db");
    open_database_path_readonly(db_path).await
}

/// Open an explicit SQLite database path in read-only mode.
///
/// This never creates the database file and never runs migrations. Use
/// [`open_connection_readonly`] for the standard per-agent `data.db` path.
pub async fn open_database_path_readonly(db_path: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = db_path.as_ref().to_path_buf();
    let conn = Connection::open_local(db_path, false).await?;
    conn.apply_readonly_pragmas().await?;
    Ok(conn)
}

/// Open an explicit SQLite database path in read-write mode, creating the
/// file if it does not exist.
///
/// No migrations are run: the caller owns the schema of the database it
/// names. This exists for the standalone, non-per-agent databases Right
/// keeps under `~/.right` (currently `providers.db`), which must still go
/// through this crate so `right-db` stays the only owner of driver details.
/// Use [`open_connection`] for the per-agent `data.db`.
pub async fn open_database_path(db_path: impl AsRef<Path>) -> Result<Connection, DbError> {
    let db_path = db_path.as_ref().to_path_buf();
    let conn = Connection::open_local(db_path, true).await?;
    conn.apply_connection_pragmas().await?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migration_open_waits_for_existing_bootstrap_lock() {
        use fs4::FileExt;

        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".right-db-migrate.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        FileExt::lock(&lock_file).unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(750),
            open_connection(dir.path(), true),
        )
        .await;
        assert!(
            result.is_err(),
            "migrate=true open must wait for the bootstrap lock"
        );

        FileExt::unlock(&lock_file).unwrap();
        open_connection(dir.path(), true).await.unwrap();
    }
}
