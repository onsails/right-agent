pub mod circuit;
pub mod classify;
pub mod error;
pub mod guard;
pub mod hindsight;
pub(crate) mod migrations;
pub mod prefetch;
pub mod resilient;
pub mod retain_queue;
pub mod status;
pub mod store;

pub use classify::ErrorKind;
pub use error::MemoryError;
pub use resilient::{ResilientError, ResilientHindsight};
pub use status::MemoryStatus;

/// Dedup keys for rows in the `memory_alerts` table.
///
/// These strings appear in SQL queries across `memory_alerts.rs`, `doctor.rs`,
/// and their tests. Keeping them here prevents silent drift that would break
/// dedup (a typo makes the same alert fire twice).
pub mod alert_types {
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const CLIENT_FLOOD: &str = "client_flood";
}

/// Opens (or creates) the per-agent SQLite memory database at `agent_path/data.db`.
///
/// - Creates the file if absent (idempotent).
/// - Enables WAL journal mode and sets busy_timeout=5000ms.
/// - When `migrate` is true, applies all pending schema migrations.
///
/// Both the MCP aggregator and bot processes pass `migrate: true` for their
/// per-agent databases. Migrations are idempotent so concurrent callers are safe.
pub fn open_db(agent_path: &std::path::Path, migrate: bool) -> Result<(), MemoryError> {
    open_connection(agent_path, migrate).map(drop)
}

/// Opens (or creates) the per-agent SQLite memory database and returns the live connection.
///
/// Unlike `open_db`, this function returns the `Connection` for use by store operations.
/// Callers are responsible for keeping it alive for the duration of their operations.
///
/// - Same WAL + busy_timeout logic as `open_db`.
/// - Idempotent: safe to call multiple times on the same path.
/// - When `migrate` is true, applies all pending schema migrations.
///
/// Both the MCP aggregator and bot processes pass `migrate: true` for their
/// per-agent databases. Migrations are idempotent so concurrent callers are safe.
pub fn open_connection(
    agent_path: &std::path::Path,
    migrate: bool,
) -> Result<rusqlite::Connection, MemoryError> {
    let db_path = agent_path.join("data.db");
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    if migrate {
        migrations::MIGRATIONS.to_latest(&mut conn)?;
    }
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::{open_connection, open_db};
    use tempfile::tempdir;

    #[test]
    fn open_db_creates_file() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        assert!(
            dir.path().join("data.db").exists(),
            "data.db should exist after open_db"
        );
    }

    #[test]
    fn open_db_is_idempotent() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        open_db(dir.path(), true).unwrap(); // second call must also succeed
        assert!(dir.path().join("data.db").exists());
    }

    #[test]
    fn schema_has_memories_table() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "memories table should exist");
    }

    #[test]
    fn schema_has_memory_events_table() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memory_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "memory_events table should exist");
    }

    #[test]
    fn schema_has_memories_fts() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "memories_fts virtual table should exist");
    }

    #[test]
    fn wal_mode_enabled() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "journal_mode should be WAL after open_db");
    }

    #[test]
    fn user_version_is_19() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 19, "user_version should be 19 after V19 migration");
    }

    #[test]
    fn schema_has_cron_runs_table() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "cron_runs table should exist after V3 migration");
    }

    #[test]
    fn cron_runs_insert_and_update() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();

        // Insert a running job
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path) VALUES ('run-1', 'deploy-check', '2026-04-01T00:00:00Z', 'running', '/tmp/deploy-check-run-1.txt')",
            [],
        )
        .unwrap();

        // Verify finished_at and exit_code are NULL while running
        let (finished_at, exit_code, status): (Option<String>, Option<i32>, String) = conn
            .query_row(
                "SELECT finished_at, exit_code, status FROM cron_runs WHERE id='run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_none(),
            "finished_at should be NULL while running"
        );
        assert!(
            exit_code.is_none(),
            "exit_code should be NULL while running"
        );
        assert_eq!(status, "running");

        // Update to success
        conn.execute(
            "UPDATE cron_runs SET finished_at='2026-04-01T00:01:00Z', exit_code=0, status='success' WHERE id='run-1'",
            [],
        )
        .unwrap();

        let (finished_at, exit_code, status): (Option<String>, Option<i32>, String) = conn
            .query_row(
                "SELECT finished_at, exit_code, status FROM cron_runs WHERE id='run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(finished_at.as_deref(), Some("2026-04-01T00:01:00Z"));
        assert_eq!(exit_code, Some(0));
        assert_eq!(status, "success");
    }

    #[test]
    fn schema_has_sessions_table() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "sessions table should exist after V4 migration");
    }

    #[test]
    fn sessions_partial_unique_active() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (42, 0, 'uuid-1', 1)",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (42, 0, 'uuid-2', 1)",
            [],
        );
        assert!(
            result.is_err(),
            "partial unique index should prevent two active sessions per (chat_id, thread_id)"
        );
    }

    #[test]
    fn memory_events_blocks_update() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        conn.execute(
            "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
            [],
        )
        .unwrap();
        let result = conn.execute("UPDATE memory_events SET actor='x' WHERE id=1", []);
        assert!(result.is_err(), "UPDATE on memory_events should be blocked");
    }

    #[test]
    fn memory_events_blocks_delete() {
        let dir = tempdir().unwrap();
        open_db(dir.path(), true).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();
        conn.execute(
            "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
            [],
        )
        .unwrap();
        let result = conn.execute("DELETE FROM memory_events WHERE id=1", []);
        assert!(result.is_err(), "DELETE on memory_events should be blocked");
    }

    // --- open_connection tests ---

    #[test]
    fn open_connection_returns_live_connection() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // Verify memories table is accessible
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "memories table should exist via open_connection");
    }

    #[test]
    fn open_connection_is_idempotent() {
        let dir = tempdir().unwrap();
        let conn1 = open_connection(dir.path(), true);
        assert!(conn1.is_ok(), "first open_connection call should succeed");
        drop(conn1);
        let conn2 = open_connection(dir.path(), true);
        assert!(
            conn2.is_ok(),
            "second open_connection call should also succeed"
        );
    }

    #[test]
    fn open_connection_creates_db_file() {
        let dir = tempdir().unwrap();
        assert!(
            !dir.path().join("data.db").exists(),
            "db file should not exist before open_connection"
        );
        let _conn = open_connection(dir.path(), true).unwrap();
        assert!(
            dir.path().join("data.db").exists(),
            "db file should exist after open_connection"
        );
    }

    #[test]
    fn open_connection_no_migrate_skips_schema() {
        let dir = tempdir().unwrap();
        // Open without migration — DB file is created but has no tables.
        let conn = open_connection(dir.path(), false).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "memories table should NOT exist with migrate=false"
        );
    }

    #[test]
    fn open_connection_no_migrate_after_migrate_works() {
        let dir = tempdir().unwrap();
        // First: migrate.
        let conn1 = open_connection(dir.path(), true).unwrap();
        drop(conn1);
        // Second: open without migration — tables are already there.
        let conn2 = open_connection(dir.path(), false).unwrap();
        let count: i64 = conn2
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "memories table should exist from prior migration");
    }
}
