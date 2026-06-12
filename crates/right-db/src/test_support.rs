use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use tokio::task::JoinHandle;

pub async fn migrated_connection() -> (TempDir, crate::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).await.unwrap();
    (dir, conn)
}

/// Hold a writable Turso write transaction on `db_path` for `hold_for`, taking
/// the multiprocess WAL write lock so a racing writer must wait on the shared
/// `busy_timeout`. Resolves only after the lock is held so the caller can race
/// the next write against it; release happens when `hold_for` elapses.
pub async fn hold_write_lock(db_path: PathBuf, hold_for: Duration) -> JoinHandle<()> {
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let conn = crate::Connection::open_local(db_path, true)
            .await
            .expect("open write-lock holder connection");
        conn.apply_connection_pragmas()
            .await
            .expect("apply holder pragmas");
        let tx = conn
            .transaction()
            .await
            .expect("begin immediate write lock");
        tx.execute(
            "CREATE TABLE IF NOT EXISTS write_lock_probe (id INTEGER PRIMARY KEY)",
            (),
        )
        .await
        .expect("write under held lock");
        locked_tx.send(()).expect("signal write lock acquired");
        tokio::time::sleep(hold_for).await;
        tx.rollback().await.expect("release write lock");
    });
    locked_rx.await.expect("write lock acquired");
    handle
}
