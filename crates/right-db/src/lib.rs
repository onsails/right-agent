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
    prepare_legacy_fts5_schema_for_turso(&db_path).await?;
    let conn = Connection::open_local(db_path, true).await?;
    conn.apply_connection_pragmas().await?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&conn).await?;
    }
    Ok(conn)
}

/// Migration version that replaced the legacy SQLite FTS5 virtual tables with
/// Turso's `CREATE INDEX ... USING fts`. Any DB at this version or newer is
/// guaranteed to no longer contain the legacy schema, so the scrubber probe
/// is permanently a no-op and we skip it.
const LEGACY_FTS5_SCRUBBED_AT_USER_VERSION: i64 = 34;
const DB_OPEN_RETRY_DELAY: Duration = Duration::from_millis(50);
// Turso's open and WAL setup already do their own lock waiting. One full retry
// covers a lock released just after that internal wait without masking stuck
// duplicate processes.
const DB_OPEN_MAX_RETRIES: usize = 1;
const LEGACY_FTS5_PROBE_RETRY_DELAY: Duration = Duration::from_millis(50);
// The rusqlite pre-Turso probe already uses SQLite's 5s busy_timeout. One
// extra attempt covers a lock released just after that timeout without hiding
// stuck writers.
const LEGACY_FTS5_PROBE_MAX_RETRIES: usize = 1;

async fn prepare_legacy_fts5_schema_for_turso(db_path: &Path) -> Result<(), DbError> {
    let mut retries = 0;
    loop {
        match prepare_legacy_fts5_schema_for_turso_once(db_path).await {
            Ok(()) => return Ok(()),
            Err(error) if error.is_transient() && retries < LEGACY_FTS5_PROBE_MAX_RETRIES => {
                retries += 1;
                log_transient_db_retry(
                    db_path,
                    retries,
                    LEGACY_FTS5_PROBE_MAX_RETRIES,
                    &error,
                    "transient legacy SQLite scrubber probe failed; retrying",
                );
                tokio::time::sleep(LEGACY_FTS5_PROBE_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
}

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

async fn prepare_legacy_fts5_schema_for_turso_once(db_path: &Path) -> Result<(), DbError> {
    if legacy_fts5_schema_exists(db_path)? {
        legacy_fts5_scrubber_overlap_probe(db_path).await;
        scrub_legacy_fts5_schema(db_path)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn arm_legacy_fts5_scrubber_overlap_probe(
    db_path: &Path,
) -> LegacyFts5ScrubberOverlapProbeGuard {
    legacy_fts5_scrubber_overlap_probe_test_support::arm(db_path)
}

#[cfg(test)]
pub(crate) struct LegacyFts5ScrubberOverlapProbeGuard {
    db_path: std::path::PathBuf,
    probe: std::sync::Arc<legacy_fts5_scrubber_overlap_probe_test_support::ProbeState>,
}

#[cfg(test)]
impl LegacyFts5ScrubberOverlapProbeGuard {
    pub(crate) fn overlap_observed(&self) -> bool {
        self.probe.overlap_observed()
    }
}

#[cfg(test)]
impl Drop for LegacyFts5ScrubberOverlapProbeGuard {
    fn drop(&mut self) {
        legacy_fts5_scrubber_overlap_probe_test_support::disarm(&self.db_path, &self.probe);
    }
}

#[cfg(test)]
async fn legacy_fts5_scrubber_overlap_probe(db_path: &Path) {
    legacy_fts5_scrubber_overlap_probe_test_support::enter(db_path).await;
}

#[cfg(not(test))]
async fn legacy_fts5_scrubber_overlap_probe(_db_path: &Path) {}

#[cfg(test)]
mod legacy_fts5_scrubber_overlap_probe_test_support {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use super::LegacyFts5ScrubberOverlapProbeGuard;

    const PROBE_HOLD: Duration = Duration::from_millis(100);

    pub(crate) struct ProbeState {
        active: AtomicUsize,
        overlap: AtomicBool,
    }

    impl ProbeState {
        fn new() -> Self {
            Self {
                active: AtomicUsize::new(0),
                overlap: AtomicBool::new(false),
            }
        }

        pub(crate) fn overlap_observed(&self) -> bool {
            self.overlap.load(Ordering::SeqCst)
        }
    }

    struct ActiveProbe {
        probe: Arc<ProbeState>,
    }

    impl Drop for ActiveProbe {
        fn drop(&mut self) {
            self.probe.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    static PROBES: OnceLock<Mutex<HashMap<PathBuf, Arc<ProbeState>>>> = OnceLock::new();

    pub(crate) fn arm(db_path: &Path) -> LegacyFts5ScrubberOverlapProbeGuard {
        let probe = Arc::new(ProbeState::new());
        probes()
            .lock()
            .unwrap()
            .insert(db_path.to_path_buf(), Arc::clone(&probe));
        LegacyFts5ScrubberOverlapProbeGuard {
            db_path: db_path.to_path_buf(),
            probe,
        }
    }

    pub(crate) async fn enter(db_path: &Path) {
        let probe = probes().lock().unwrap().get(db_path).cloned();
        let Some(probe) = probe else {
            return;
        };

        if probe.active.fetch_add(1, Ordering::SeqCst) > 0 {
            probe.overlap.store(true, Ordering::SeqCst);
        }
        let _active = ActiveProbe { probe };
        tokio::time::sleep(PROBE_HOLD).await;
    }

    pub(crate) fn disarm(db_path: &Path, probe: &Arc<ProbeState>) {
        let mut probes = probes().lock().unwrap();
        if probes
            .get(db_path)
            .is_some_and(|current| Arc::ptr_eq(current, probe))
        {
            probes.remove(db_path);
        }
    }

    fn probes() -> &'static Mutex<HashMap<PathBuf, Arc<ProbeState>>> {
        PROBES.get_or_init(|| Mutex::new(HashMap::new()))
    }
}

fn legacy_fts5_schema_exists(db_path: &Path) -> Result<bool, DbError> {
    if !db_path.exists() {
        return Ok(false);
    }

    // Probe via bundled rusqlite, NOT Turso: Turso cannot resolve every
    // table in legacy schemas containing SQLite FTS5 virtual tables, so we
    // must not open the file through Turso before the scrubber runs. The
    // current Turso reads happen to work only because `PRAGMA user_version`
    // and `sqlite_master` scans don't materialize FTS5 column lists --
    // implicit, not structural. Use the same rusqlite path the scrubber uses.
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| DbError::LegacySqlite {
                path: db_path.to_path_buf(),
                source,
            })?;
    conn.busy_timeout(connection::BUSY_TIMEOUT)
        .map_err(|source| DbError::LegacySqlite {
            path: db_path.to_path_buf(),
            source,
        })?;

    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|source| DbError::LegacySqlite {
            path: db_path.to_path_buf(),
            source,
        })?;
    if user_version >= LEGACY_FTS5_SCRUBBED_AT_USER_VERSION {
        return Ok(false);
    }
    let legacy_object_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM sqlite_master
             WHERE type = 'table'
               AND name IN ('memories_fts', 'conversation_messages_fts')
               AND lower(sql) LIKE 'create virtual table%using fts5%'",
            [],
            |row| row.get(0),
        )
        .map_err(|source| DbError::LegacySqlite {
            path: db_path.to_path_buf(),
            source,
        })?;
    Ok(legacy_object_count > 0)
}

fn scrub_legacy_fts5_schema(db_path: &Path) -> Result<(), DbError> {
    // Turso cannot resolve every table in legacy schemas that contain SQLite
    // FTS5 virtual tables. Remove only those known legacy objects with SQLite
    // before handing the file to Turso.
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|source| DbError::LegacySqlite {
                path: db_path.to_path_buf(),
                source,
            })?;
    conn.busy_timeout(connection::BUSY_TIMEOUT)
        .map_err(|source| DbError::LegacySqlite {
            path: db_path.to_path_buf(),
            source,
        })?;

    conn.execute_batch(
        "BEGIN IMMEDIATE;

         DROP TRIGGER IF EXISTS memories_ai;
         DROP TRIGGER IF EXISTS memories_ad;
         DROP TRIGGER IF EXISTS memories_au;

         DROP TRIGGER IF EXISTS conversation_messages_ai;
         DROP TRIGGER IF EXISTS conversation_messages_ad;
         DROP TRIGGER IF EXISTS conversation_messages_au;

         DROP TABLE IF EXISTS memories_fts;
         DROP TABLE IF EXISTS conversation_messages_fts;

         COMMIT;",
    )
    .map_err(|source| DbError::LegacySqlite {
        path: db_path.to_path_buf(),
        source,
    })?;

    Ok(())
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
    use crate::test_support::{hold_exclusive_sqlite_lock, legacy_probe_retry_lock_hold};

    #[tokio::test]
    async fn open_connection_retries_transient_legacy_probe_lock() {
        let dir = tempfile::tempdir().unwrap();
        open_connection(dir.path(), true).await.unwrap();

        let db_path = dir.path().join("data.db");
        let lock = hold_exclusive_sqlite_lock(db_path, legacy_probe_retry_lock_hold());
        let result = open_connection(dir.path(), false).await;
        lock.join().expect("release sqlite lock");

        result.expect("open_connection should recover from transient legacy probe lock");
    }

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
        lock_file.lock().unwrap();

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
        let error = DbError::LegacySqlite {
            path: "data.db".into(),
            source: rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                None,
            ),
        };

        let diagnostics = DbRetryDiagnostics::capture(&error);

        assert_eq!(diagnostics.pid, std::process::id());
        assert_eq!(diagnostics.transient_kind, "sqlite_busy");
        assert!(
            !diagnostics.process.trim().is_empty(),
            "process name must be present"
        );
    }
}
