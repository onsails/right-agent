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
