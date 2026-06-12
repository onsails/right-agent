//! Per-agent SQLite plumbing for `right`.
//!
//! Owns `data.db` open/migrate logic and the central migration registry.
//! Domain crates (`right-mcp`, `right-memory`, `right-codegen`, the slim
//! `right-agent`, `right-bot`) call `open_connection` here; new tables
//! are added by editing the central `migrations::MIGRATIONS` array.

#![warn(unreachable_pub)]

mod bootstrap_lock;
pub mod connection;
pub mod conversation;
pub mod error;
pub mod forum_topics;
pub mod migrations;
mod multiprocess_io;
pub mod params;
pub mod row;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod thread_focus;
pub mod transaction;

pub use connection::Connection;
pub use error::DbError;
pub use migrations::MIGRATIONS;
pub use params::params_from_iter;
pub use row::Row;
pub use transaction::Transaction;

use std::path::Path;
use std::time::Duration;

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
/// - **Cold-boot contention.** A concurrent caller that opens the same
///   database while a batch is in flight blocks on the immediate
///   transaction for the full batch duration, not just the next pending
///   version. Under WAL + 5s `busy_timeout`, a slow first-boot batch can
///   force the second opener to time out.
pub async fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let mut retries = 0;
    loop {
        match open_connection_once(agent_path, migrate).await {
            Ok(conn) => return Ok(conn),
            Err(error) if error.is_transient() && retries < DB_OPEN_MAX_RETRIES => {
                retries += 1;
                log_transient_db_retry(
                    &db_path,
                    retries,
                    DB_OPEN_MAX_RETRIES,
                    &error,
                    "transient database open failed; retrying",
                );
                tokio::time::sleep(DB_OPEN_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
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

const DB_OPEN_RETRY_DELAY: Duration = Duration::from_millis(50);
// Turso's open and WAL setup already do their own lock waiting. One full retry
// covers a lock released just after that internal wait without masking stuck
// duplicate processes.
const DB_OPEN_MAX_RETRIES: usize = 1;

fn log_transient_db_retry(
    db_path: &Path,
    retries: usize,
    max_retries: usize,
    error: &DbError,
    message: &'static str,
) {
    let diagnostics = DbRetryDiagnostics::capture(error);
    tracing::warn!(
        path = %db_path.display(),
        pid = diagnostics.pid,
        process = %diagnostics.process,
        transient_kind = diagnostics.transient_kind,
        retries,
        max_retries,
        error = format!("{error:#}"),
        "{message}",
    );
}

struct DbRetryDiagnostics {
    pid: u32,
    process: String,
    transient_kind: &'static str,
}

impl DbRetryDiagnostics {
    fn capture(error: &DbError) -> Self {
        Self {
            pid: std::process::id(),
            process: current_process_name(),
            transient_kind: error
                .transient_kind()
                .map(|kind| kind.as_str())
                .unwrap_or("unknown"),
        }
    }
}

fn current_process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Open the per-agent SQLite database, dropping the connection.
/// Used when the caller only needs the file created and migrated.
pub async fn open_db(agent_path: &Path, migrate: bool) -> Result<(), DbError> {
    open_connection(agent_path, migrate).await.map(drop)
}

/// Open the per-agent SQLite database in read-only mode.
///
/// Unlike [`open_connection`], this never creates the database file and
/// never runs migrations. It is intended for read-only consumers such as
/// the Telegram Mini App dashboard, where the "do not create or mutate"
/// guarantee must be structural, not advisory.
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

    #[test]
    fn transient_retry_diagnostics_include_process_identity_and_kind() {
        let error = DbError::Open {
            path: "data.db".into(),
            source: turso::Error::Busy("locked".into()),
        };

        let diagnostics = DbRetryDiagnostics::capture(&error);

        assert_eq!(diagnostics.pid, std::process::id());
        assert_eq!(diagnostics.transient_kind, "turso_busy");
        assert!(
            !diagnostics.process.trim().is_empty(),
            "process name must be present"
        );
    }
}
