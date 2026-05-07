use right_db::{MIGRATIONS, open_connection, open_db};
use tempfile::tempdir;

#[test]
fn open_db_creates_file() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    assert!(
        dir.path().join("data.db").exists(),
        "data.db should exist after open_db",
    );
}

#[test]
fn open_connection_applies_migrations() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    assert_eq!(
        query_user_version(&conn),
        19,
        "latest migration should be v19"
    );
    // After migrations, the current sessions table should exist.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "sessions table should exist");
}

#[test]
fn open_connection_without_migration_leaves_schema_unmigrated() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).unwrap();
    assert_eq!(
        query_user_version(&conn),
        0,
        "migrate=false should not apply migrations"
    );
    assert_eq!(
        query_table_count(&conn, "sessions"),
        0,
        "sessions table should not exist"
    );
}

#[test]
fn open_connection_without_migration_preserves_existing_schema() {
    let dir = tempdir().unwrap();
    open_connection(dir.path(), true).unwrap();

    let conn = open_connection(dir.path(), false).unwrap();
    assert_eq!(
        query_user_version(&conn),
        19,
        "migrate=false should not downgrade schema"
    );
    assert_eq!(
        query_table_count(&conn, "sessions"),
        1,
        "sessions table should still exist"
    );
}

#[test]
fn open_connection_sets_sqlite_pragmas() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).unwrap();

    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let busy_timeout_ms: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();

    assert_eq!(journal_mode.to_lowercase(), "wal");
    assert_eq!(busy_timeout_ms, 5000);
}

#[test]
fn migrations_idempotent() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    // Re-opening with migrate=true must not error.
    open_db(dir.path(), true).unwrap();
}

#[test]
fn migrations_static_runs_in_memory() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
}

fn query_user_version(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn query_table_count(conn: &rusqlite::Connection, table_name: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        [table_name],
        |row| row.get(0),
    )
    .unwrap()
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
