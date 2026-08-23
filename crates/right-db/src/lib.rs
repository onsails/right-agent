//! Per-agent SQLite plumbing for `right`.
//!
//! Owns `data.db` open/migrate logic and the central migration registry.
//! Domain crates (`right-mcp`, `right-memory`, `right-codegen`, the slim
//! `right-agent`, `right-bot`) call `open_connection` here; new tables
//! are added by editing the central `migrations::MIGRATIONS` array.

#![warn(unreachable_pub)]

pub mod bootstrap_answers;
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
///
/// # WAL desync recovery
///
/// On a Turso multiprocess-WAL desync (open fails `is_wal_corruption`), the
/// connection escalates: first reset the `-tshm`/`-shm` sidecars and retry;
/// on a repeat desync drop the un-checkpointed `-wal` too (its frames are
/// unrecoverable when the authority cannot rebuild); if the desync survives
/// even the WAL drop, propagate the error instead of looping. See
/// tursodatabase/turso#769.
pub async fn open_connection(agent_path: &Path, migrate: bool) -> Result<Connection, DbError> {
    let db_path = agent_path.join("data.db");
    let mut retries = 0;
    let mut desync = DesyncRecovery::default();
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
            Err(error) if error.is_wal_corruption() => match desync.on_desync() {
                DesyncAction::ResetSidecars => {
                    tracing::warn!(
                        path = %db_path.display(),
                        removed = "-tshm,-shm",
                        error = format!("{error:#}"),
                        "WAL desync detected; resetting -tshm/-shm sidecars and retrying \
                         (tursodatabase/turso#769)",
                    );
                    recover_wal_sidecars(agent_path, false).await?;
                }
                DesyncAction::ResetWalAndSidecars => {
                    tracing::warn!(
                        path = %db_path.display(),
                        removed = "-tshm,-shm,-wal",
                        error = format!("{error:#}"),
                        "WAL desync persists after sidecar reset; dropping the \
                         un-checkpointed WAL and retrying (tursodatabase/turso#769)",
                    );
                    recover_wal_sidecars(agent_path, true).await?;
                }
                DesyncAction::Fail => {
                    tracing::error!(
                        path = %db_path.display(),
                        error = format!("{error:#}"),
                        "WAL desync persists after WAL drop; the database needs manual \
                         recovery — refusing to loop (tursodatabase/turso#769)",
                    );
                    return Err(error);
                }
            },
            Err(error) => return Err(error),
        }
    }
}

/// Escalation state for repeated WAL desyncs within one `open_connection`.
#[derive(Default)]
struct DesyncRecovery {
    attempts: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesyncAction {
    /// First desync: reset `-tshm`/`-shm`, keep the WAL (salvage its frames).
    ResetSidecars,
    /// Repeat desync: the WAL's frames are unrecoverable — drop `-wal` too.
    ResetWalAndSidecars,
    /// Desync survives the WAL drop: main-database corruption or an upstream
    /// bug — propagate instead of looping.
    Fail,
}

impl DesyncRecovery {
    fn on_desync(&mut self) -> DesyncAction {
        self.attempts += 1;
        match self.attempts {
            1 => DesyncAction::ResetSidecars,
            2 => DesyncAction::ResetWalAndSidecars,
            _ => DesyncAction::Fail,
        }
    }

    #[cfg(test)]
    fn on_success(&mut self) {
        self.attempts = 0;
    }
}

/// Delete the Turso WAL-coordination sidecars (`-tshm` authority snapshot,
/// `-shm` wal-index) so the next open cold-rebuilds. `drop_wal` also removes
/// the un-checkpointed `-wal`: on a repeat desync the authority cannot
/// rebuild from it, so its frames are unrecoverable and keeping the file only
/// guarantees the next open hits the same short read. See
/// tursodatabase/turso#769.
fn remove_wal_sidecars_with_escalation(agent_path: &Path, drop_wal: bool) -> Result<(), DbError> {
    let mut suffixes = vec!["-tshm", "-shm"];
    if drop_wal {
        suffixes.push("-wal");
    }
    for suffix in suffixes {
        let sidecar = agent_path.join(format!("data.db{suffix}"));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(DbError::SidecarRemove {
                    path: sidecar,
                    source: e,
                });
            }
        }
    }
    Ok(())
}

/// Recover from a WAL-sidecar desync (turso#769). Serializes across the bot and
/// aggregator processes via the per-agent bootstrap lock, re-checks whether a
/// sibling already healed the database, and otherwise resets the sidecars.
async fn recover_wal_sidecars(agent_path: &Path, drop_wal: bool) -> Result<(), DbError> {
    let _lock = bootstrap_lock::acquire(agent_path).await?;
    let db_path = agent_path.join("data.db");
    // Re-check under the lock with the SAME semantics as the production open
    // (create: true) so the probe reproduces the desync exactly. Reaching
    // recovery means the first open already failed with is_wal_corruption,
    // which proves data.db + its sidecars existed; the file cannot vanish
    // during the locked probe, so create: true cannot mask a missing database.
    match Connection::open_local(db_path, true).await {
        Ok(_) => return Ok(()),
        Err(error) if error.is_wal_corruption() => {}
        Err(error) => return Err(error),
    }
    remove_wal_sidecars_with_escalation(agent_path, drop_wal)
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

    #[tokio::test]
    async fn remove_wal_sidecars_drops_authority_keeps_wal() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["data.db", "data.db-wal", "data.db-shm", "data.db-tshm"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        super::remove_wal_sidecars_with_escalation(dir.path(), false).unwrap();

        assert!(dir.path().join("data.db").exists(), "main db must remain");
        assert!(dir.path().join("data.db-wal").exists(), "-wal must remain");
        assert!(
            !dir.path().join("data.db-shm").exists(),
            "-shm must be removed"
        );
        assert!(
            !dir.path().join("data.db-tshm").exists(),
            "-tshm must be removed"
        );

        // Idempotent: re-running with sidecars already gone is fine.
        super::remove_wal_sidecars_with_escalation(dir.path(), false).unwrap();
    }

    #[tokio::test]
    async fn remove_wal_sidecars_with_escalation_removes_wal_on_repeat_desync() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["data.db", "data.db-wal", "data.db-shm", "data.db-tshm"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }

        super::remove_wal_sidecars_with_escalation(dir.path(), false).unwrap();
        assert!(
            dir.path().join("data.db-wal").exists(),
            "first desync keeps -wal"
        );

        for name in ["data.db-wal", "data.db-shm", "data.db-tshm"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        super::remove_wal_sidecars_with_escalation(dir.path(), true).unwrap();
        assert!(
            !dir.path().join("data.db-wal").exists(),
            "repeat desync drops -wal"
        );
        assert!(dir.path().join("data.db").exists(), "main db must remain");
    }

    #[test]
    fn persistent_desync_escalates_to_wal_reset_then_fails() {
        let mut state = super::DesyncRecovery::default();
        assert_eq!(state.on_desync(), super::DesyncAction::ResetSidecars);
        assert_eq!(state.on_desync(), super::DesyncAction::ResetWalAndSidecars);
        assert_eq!(state.on_desync(), super::DesyncAction::Fail);
        assert_eq!(state.on_desync(), super::DesyncAction::Fail);
    }

    #[test]
    fn successful_open_resets_desync_escalation() {
        let mut state = super::DesyncRecovery::default();
        assert_eq!(state.on_desync(), super::DesyncAction::ResetSidecars);
        state.on_success();
        assert_eq!(state.on_desync(), super::DesyncAction::ResetSidecars);
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
