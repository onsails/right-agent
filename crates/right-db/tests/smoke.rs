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
