use right_db::{MIGRATIONS, open_connection, open_connection_readonly, open_db};
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
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
        "latest migration should be current schema version"
    );
    // After migrations, the current sessions table should exist.
    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            (),
            |row| row.get(0),
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
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
        "migrate=false should not downgrade schema"
    );
    assert_eq!(
        query_table_count(&conn, "sessions"),
        1,
        "sessions table should still exist"
    );
}

#[test]
fn libsql_open_connection_creates_file_and_preserves_local_path() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).unwrap();
    let db_path = dir.path().join("data.db");

    assert!(db_path.exists(), "local libsql open should create data.db");
    assert_eq!(conn.path(), db_path.as_path());

    conn.execute_batch("CREATE TABLE local_probe (id INTEGER PRIMARY KEY)")
        .unwrap();
    assert_eq!(query_table_count(&conn, "local_probe"), 1);
}

#[test]
fn open_connection_sets_sqlite_pragmas() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).unwrap();

    let journal_mode: String = conn
        .query_one("PRAGMA journal_mode", (), |row| row.get(0))
        .unwrap();
    let busy_timeout_ms: i64 = conn
        .query_one("PRAGMA busy_timeout", (), |row| row.get(0))
        .unwrap();

    assert_eq!(journal_mode.to_lowercase(), "wal");
    assert_eq!(busy_timeout_ms, 5000);
}

#[test]
fn libsql_open_connection_readonly_requires_existing_db() {
    let dir = tempdir().unwrap();

    let err = open_connection_readonly(dir.path()).expect_err("missing db should not open");

    assert!(
        !dir.path().join("data.db").exists(),
        "readonly open must not create data.db",
    );
    assert!(
        err.is_open_error(),
        "expected readonly open failure, got {err:?}"
    );
}

#[test]
fn open_connection_readonly_rejects_writes() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();

    let conn = open_connection_readonly(dir.path()).unwrap();
    assert_eq!(
        query_user_version(&conn),
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );

    let err = conn
        .execute("CREATE TABLE write_probe (id INTEGER)", ())
        .expect_err("readonly connection should reject writes");

    assert!(err.to_string().contains("readonly database"));
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

fn query_user_version(conn: &right_db::Connection) -> i64 {
    conn.query_one("PRAGMA user_version", (), |row| row.get(0))
        .unwrap()
}

fn query_table_count(conn: &right_db::Connection, table_name: &str) -> i64 {
    conn.query_one(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        [table_name],
        |row| row.get(0),
    )
    .unwrap()
}

fn query_rusqlite_table_count(conn: &rusqlite::Connection, table_name: &str) -> i64 {
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
fn schema_has_conversation_messages_tables() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();

    assert_eq!(
        query_rusqlite_table_count(&conn, "conversation_messages"),
        1,
        "conversation_messages table should exist"
    );
    assert_eq!(
        query_rusqlite_table_count(&conn, "conversation_messages_fts"),
        1,
        "conversation_messages_fts table should exist"
    );
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
fn schema_has_conversation_messages_table() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("data.db")).unwrap();

    for table in ["conversation_messages", "conversation_messages_fts"] {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{table} table should exist");
    }
}

#[test]
fn schema_has_async_runs_table_and_no_cron_runs_table() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true).expect("open db");

    let async_count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(async_count, 1, "async_runs table should exist");

    let cron_count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cron_count, 0, "cron_runs table should be removed");
}

#[test]
fn async_runs_insert_and_update() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true).expect("open db");

    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id, status,
            delivery_required, delivery_status, created_at, updated_at
         ) VALUES (
            'run-1', 'cron', 'deploy-check', 'run-1', -100, 'running',
            0, 'none', '2026-04-01T00:00:00Z', '2026-04-01T00:00:00Z'
         )",
        (),
    )
    .unwrap();

    conn.execute(
        "UPDATE async_runs
         SET finished_at='2026-04-01T00:01:00Z', exit_code=0, status='success'
         WHERE id='run-1'",
        (),
    )
    .unwrap();

    let row: (Option<String>, Option<i64>, String) = conn
        .query_one(
            "SELECT finished_at, exit_code, status FROM async_runs WHERE id='run-1'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("2026-04-01T00:01:00Z"));
    assert_eq!(row.1, Some(0));
    assert_eq!(row.2, "success");
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
fn v23_user_cron_with_bg_prefix_stays_cron() {
    // Regression: validate_job_name allows `bg-` prefixes, so a user could
    // create a real recurring cron named e.g. `bg-status-check`. The v22
    // migration must not reclassify such surviving cron rows as background.
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 21).unwrap();

    conn.execute(
        "INSERT INTO cron_specs (
            job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
            target_chat_id
         ) VALUES (
            'bg-status-check', '0 9 * * *', 'check status', 1.0,
            '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
            -100
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES (
            'bg-status-check-run', 'bg-status-check', '2026-05-18T09:00:00Z',
            'success', '/log/bg-status-check-run.ndjson', 'pending'
         )",
        [],
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let (kind, handoff_state): (String, Option<String>) = conn
        .query_row(
            "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-status-check-run'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "cron");
    assert!(handoff_state.is_none());
}

#[test]
fn v23_orphan_bg_cron_run_classifies_as_background() {
    // Orphaned cron_runs row (cron_specs already deleted by the legacy
    // one-shot-bg-then-cleanup path) keeps the legacy bg-prefix heuristic so
    // the run still surfaces correctly post-migration.
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 21).unwrap();

    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES (
            'bg-orphan-run', 'bg-orphan', '2026-05-18T02:00:00Z',
            'success', '/log/bg-orphan-run.ndjson', 'silent'
         )",
        [],
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let (kind, handoff_state): (String, Option<String>) = conn
        .query_row(
            "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-orphan-run'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "background");
    assert_eq!(handoff_state.as_deref(), Some("spawned"));
}

#[test]
fn open_connection_returns_live_connection() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    // Verify memories table is accessible
    let count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "memories table should exist via open_connection");
}
