pub mod error;
mod migrations;

pub use error::MemoryError;

/// Opens (or creates) the per-agent SQLite memory database at `agent_path/memory.db`.
///
/// - Creates the file if absent (idempotent).
/// - Enables WAL journal mode and sets busy_timeout=5000ms.
/// - Applies all pending schema migrations via rusqlite_migration.
pub fn open_db(agent_path: &std::path::Path) -> Result<(), MemoryError> {
    todo!("implement open_db")
}

#[cfg(test)]
mod tests {
    use super::open_db;
    use tempfile::tempdir;

    #[test]
    fn open_db_creates_file() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        assert!(
            dir.path().join("memory.db").exists(),
            "memory.db should exist after open_db"
        );
    }

    #[test]
    fn open_db_is_idempotent() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        open_db(dir.path()).unwrap(); // second call must also succeed
        assert!(dir.path().join("memory.db").exists());
    }

    #[test]
    fn schema_has_memories_table() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
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
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
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
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
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
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "journal_mode should be WAL after open_db");
    }

    #[test]
    fn user_version_is_1() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1, "user_version should be 1 after V1 migration");
    }

    #[test]
    fn memory_events_blocks_update() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
        conn.execute(
            "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "UPDATE memory_events SET actor='x' WHERE id=1",
            [],
        );
        assert!(result.is_err(), "UPDATE on memory_events should be blocked");
    }

    #[test]
    fn memory_events_blocks_delete() {
        let dir = tempdir().unwrap();
        open_db(dir.path()).unwrap();
        let conn =
            rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
        conn.execute(
            "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
            [],
        )
        .unwrap();
        let result = conn.execute("DELETE FROM memory_events WHERE id=1", []);
        assert!(result.is_err(), "DELETE on memory_events should be blocked");
    }
}
