use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tempfile::TempDir;

pub async fn migrated_connection() -> (TempDir, crate::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).await.unwrap();
    (dir, conn)
}

/// Hold duration that forces `prepare_legacy_fts5_schema_for_turso` to exhaust
/// rusqlite's busy_timeout once and recover on the first retry. Long enough to
/// outlast `connection::BUSY_TIMEOUT`, short enough to leave headroom within the
/// retry budget (`BUSY_TIMEOUT * (1 + LEGACY_FTS5_PROBE_MAX_RETRIES)`).
pub fn legacy_probe_retry_lock_hold() -> Duration {
    crate::connection::BUSY_TIMEOUT + Duration::from_millis(1_500)
}

/// Take an exclusive `rusqlite` lock on `db_path` for `hold_for` to drive
/// transient-lock recovery paths in `open_connection`. Returns only after the
/// lock is acquired so the caller can race the next `open_connection` against it.
pub fn hold_exclusive_sqlite_lock(db_path: PathBuf, hold_for: Duration) -> JoinHandle<()> {
    let (locked_tx, locked_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let conn = rusqlite::Connection::open(db_path).expect("open sqlite lock connection");
        conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
            .expect("acquire exclusive sqlite lock");
        locked_tx.send(()).expect("send lock acquired");
        thread::sleep(hold_for);
        drop(conn);
    });
    locked_rx.recv().expect("exclusive lock acquired");
    handle
}
