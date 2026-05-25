use right_db::{MIGRATIONS, open_connection, open_connection_readonly, open_db};
use tempfile::tempdir;

#[tokio::test]
async fn open_db_creates_file() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    assert!(
        dir.path().join("data.db").exists(),
        "data.db should exist after open_db",
    );
}

#[tokio::test]
async fn open_connection_applies_migrations() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    assert_eq!(
        query_user_version(&conn).await,
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
        .await
        .unwrap();
    assert_eq!(count, 1, "sessions table should exist");
}

#[tokio::test]
async fn open_connection_without_migration_leaves_schema_unmigrated() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();
    assert_eq!(
        query_user_version(&conn).await,
        0,
        "migrate=false should not apply migrations"
    );
    assert_eq!(
        query_table_count(&conn, "sessions").await,
        0,
        "sessions table should not exist"
    );
}

#[tokio::test]
async fn open_connection_without_migration_preserves_existing_schema() {
    let dir = tempdir().unwrap();
    open_connection(dir.path(), true).await.unwrap();

    let conn = open_connection(dir.path(), false).await.unwrap();
    assert_eq!(
        query_user_version(&conn).await,
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
        "migrate=false should not downgrade schema"
    );
    assert_eq!(
        query_table_count(&conn, "sessions").await,
        1,
        "sessions table should still exist"
    );
}

#[tokio::test]
async fn turso_open_connection_creates_file_and_preserves_local_path() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();
    let db_path = dir.path().join("data.db");

    assert!(db_path.exists(), "local turso open should create data.db");
    assert_eq!(conn.path(), db_path.as_path());

    conn.execute_batch("CREATE TABLE local_probe (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    assert_eq!(query_table_count(&conn, "local_probe").await, 1);
}

#[tokio::test]
async fn open_connection_async_api_works_inside_tokio_runtime() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();

    conn.execute_batch("CREATE TABLE runtime_probe (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    assert_eq!(query_table_count(&conn, "runtime_probe").await, 1);
}

#[tokio::test]
async fn open_connection_readonly_missing_db_returns_error() {
    let dir = tempdir().unwrap();
    let err = open_connection_readonly(dir.path())
        .await
        .expect_err("missing db should not open");

    assert!(
        !dir.path().join("data.db").exists(),
        "readonly open must not create data.db",
    );
    assert!(
        err.is_open_error(),
        "expected readonly open failure, got {err:?}"
    );
}

#[tokio::test]
async fn execute_accepts_empty_params_array() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();

    conn.execute("CREATE TABLE empty_params_probe (id INTEGER)", [])
        .await
        .unwrap();
    assert_eq!(query_table_count(&conn, "empty_params_probe").await, 1);
}

#[tokio::test]
async fn open_connection_sets_sqlite_pragmas() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();

    let journal_mode: String = conn
        .query_one("PRAGMA journal_mode", (), |row| row.get(0))
        .await
        .unwrap();
    let busy_timeout_ms: i64 = conn
        .query_one("PRAGMA busy_timeout", (), |row| row.get(0))
        .await
        .unwrap();

    assert_eq!(journal_mode.to_lowercase(), "wal");
    assert_eq!(busy_timeout_ms, 5000);
}

#[tokio::test]
async fn turso_open_connection_readonly_requires_existing_db() {
    let dir = tempdir().unwrap();

    let err = open_connection_readonly(dir.path())
        .await
        .expect_err("missing db should not open");

    assert!(
        !dir.path().join("data.db").exists(),
        "readonly open must not create data.db",
    );
    assert!(
        err.is_open_error(),
        "expected readonly open failure, got {err:?}"
    );
}

#[tokio::test]
async fn open_connection_readonly_rejects_writes() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();

    let conn = open_connection_readonly(dir.path()).await.unwrap();
    assert_eq!(
        query_user_version(&conn).await,
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );

    let err = conn
        .execute("CREATE TABLE write_probe (id INTEGER)", ())
        .await
        .expect_err("readonly connection should reject writes");

    assert!(err.to_string().contains("readonly database"));
}

#[tokio::test]
async fn migrations_idempotent() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    // Re-opening with migrate=true must not error.
    open_db(dir.path(), true).await.unwrap();
}

#[tokio::test]
async fn turso_migrations_set_latest_user_version() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    assert_eq!(
        query_user_version(&conn).await,
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );
}

#[tokio::test]
async fn turso_migrations_are_idempotent_on_existing_data_db() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    open_db(dir.path(), true).await.unwrap();

    let conn = open_connection(dir.path(), false).await.unwrap();
    assert_eq!(
        query_user_version(&conn).await,
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );
}

#[tokio::test]
async fn turso_migrations_static_runs_with_right_db_connection() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();

    MIGRATIONS.to_latest(&conn).await.unwrap();

    assert_eq!(
        query_user_version(&conn).await,
        right_db::migrations::LATEST_SCHEMA_VERSION as i64,
    );
}

#[tokio::test]
async fn turso_supports_conversation_fts_index() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    conn.execute(
        "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
         VALUES (1, 0, 'user', ?)",
        ["needle phrase"],
    )
    .await
    .unwrap();

    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM conversation_messages WHERE content MATCH ?",
            ["needle"],
            |row| row.get(0),
        )
        .await
        .unwrap();

    assert_eq!(count, 1);
}

#[tokio::test]
async fn turso_supports_returning_clause() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    let id: i64 = conn
        .query_one(
            "INSERT INTO conversation_messages (chat_id, thread_id, role, content)
             VALUES (1, 0, 'assistant', 'returning probe')
             RETURNING id",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();

    assert!(id > 0);
}

#[tokio::test]
async fn turso_transaction_rolls_back_on_error() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    conn.execute_batch("CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY, value TEXT UNIQUE)")
        .await
        .unwrap();

    let tx = conn.transaction().await.unwrap();
    tx.execute_batch("INSERT INTO rollback_probe (value) VALUES ('same')")
        .await
        .unwrap();
    let count: i64 = tx
        .query_one("SELECT COUNT(*) FROM rollback_probe", (), |row| row.get(0))
        .await
        .unwrap();
    assert_eq!(count, 1);
    let result = tx
        .execute_batch("INSERT INTO rollback_probe (value) VALUES ('same')")
        .await;

    assert!(result.is_err());
    tx.rollback().await.unwrap();
    let count: i64 = conn
        .query_one("SELECT COUNT(*) FROM rollback_probe", (), |row| row.get(0))
        .await
        .unwrap();
    assert_eq!(count, 0);
}

async fn query_user_version(conn: &right_db::Connection) -> i64 {
    conn.query_one("PRAGMA user_version", (), |row| row.get(0))
        .await
        .unwrap()
}

async fn query_table_count(conn: &right_db::Connection, table_name: &str) -> i64 {
    conn.query_one(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        [table_name],
        |row| row.get(0),
    )
    .await
    .unwrap()
}

async fn query_index_count(conn: &right_db::Connection, index_name: &str) -> i64 {
    conn.query_one(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?",
        [index_name],
        |row| row.get(0),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn schema_has_memories_table() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    let conn = open_connection_readonly(dir.path()).await.unwrap();
    assert_eq!(
        query_table_count(&conn, "memories").await,
        1,
        "memories table should exist"
    );
}

#[tokio::test]
async fn schema_has_memory_events_table() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    let conn = open_connection_readonly(dir.path()).await.unwrap();
    assert_eq!(
        query_table_count(&conn, "memory_events").await,
        1,
        "memory_events table should exist"
    );
}

#[tokio::test]
async fn schema_has_conversation_messages_tables() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    let conn = open_connection_readonly(dir.path()).await.unwrap();

    assert_eq!(
        query_table_count(&conn, "conversation_messages").await,
        1,
        "conversation_messages table should exist"
    );
    assert_eq!(
        query_index_count(&conn, "idx_conversation_messages_turso_fts").await,
        1,
        "idx_conversation_messages_turso_fts index should exist"
    );
}

#[tokio::test]
async fn schema_has_memories_turso_fts_index() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    let conn = open_connection_readonly(dir.path()).await.unwrap();
    assert_eq!(
        query_index_count(&conn, "idx_memories_turso_fts").await,
        1,
        "idx_memories_turso_fts index should exist"
    );
}

#[tokio::test]
async fn schema_has_conversation_messages_table() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).await.unwrap();
    let conn = open_connection_readonly(dir.path()).await.unwrap();

    assert_eq!(
        query_table_count(&conn, "conversation_messages").await,
        1,
        "conversation_messages table should exist"
    );
    assert_eq!(
        query_index_count(&conn, "idx_conversation_messages_turso_fts").await,
        1,
        "idx_conversation_messages_turso_fts index should exist"
    );
}

#[tokio::test]
async fn schema_has_async_runs_table_and_no_cron_runs_table() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true)
        .await
        .expect("open db");

    let async_count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='async_runs'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(async_count, 1, "async_runs table should exist");

    let cron_count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cron_runs'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(cron_count, 0, "cron_runs table should be removed");
}

#[tokio::test]
async fn async_runs_insert_and_update() {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true)
        .await
        .expect("open db");

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
    .await
    .unwrap();

    conn.execute(
        "UPDATE async_runs
         SET finished_at='2026-04-01T00:01:00Z', exit_code=0, status='success'
         WHERE id='run-1'",
        (),
    )
    .await
    .unwrap();

    let row: (Option<String>, Option<i64>, String) = conn
        .query_one(
            "SELECT finished_at, exit_code, status FROM async_runs WHERE id='run-1'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("2026-04-01T00:01:00Z"));
    assert_eq!(row.1, Some(0));
    assert_eq!(row.2, "success");
}

#[tokio::test]
async fn memory_events_blocks_update() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    conn.execute(
        "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
        (),
    )
    .await
    .unwrap();
    let result = conn
        .execute("UPDATE memory_events SET actor='x' WHERE id=1", ())
        .await;
    assert!(result.is_err(), "UPDATE on memory_events should be blocked");
}

#[tokio::test]
async fn memory_events_blocks_delete() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    conn.execute(
        "INSERT INTO memory_events (event_type, actor) VALUES ('store', 'test-agent')",
        (),
    )
    .await
    .unwrap();
    let result = conn
        .execute("DELETE FROM memory_events WHERE id=1", ())
        .await;
    assert!(result.is_err(), "DELETE on memory_events should be blocked");
}

#[tokio::test]
async fn v23_user_cron_with_bg_prefix_stays_cron() {
    // Regression: validate_job_name allows `bg-` prefixes, so a user could
    // create a real recurring cron named e.g. `bg-status-check`. The v22
    // migration must not reclassify such surviving cron rows as background.
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();
    MIGRATIONS.to_version(&conn, 21).await.unwrap();

    conn.execute(
        "INSERT INTO cron_specs (
            job_name, schedule, prompt, max_budget_usd, created_at, updated_at,
            target_chat_id
         ) VALUES (
            'bg-status-check', '0 9 * * *', 'check status', 1.0,
            '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z',
            -100
         )",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES (
            'bg-status-check-run', 'bg-status-check', '2026-05-18T09:00:00Z',
            'success', '/log/bg-status-check-run.ndjson', 'pending'
         )",
        (),
    )
    .await
    .unwrap();

    MIGRATIONS.to_latest(&conn).await.unwrap();

    let (kind, handoff_state): (String, Option<String>) = conn
        .query_one(
            "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-status-check-run'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(kind, "cron");
    assert!(handoff_state.is_none());
}

#[tokio::test]
async fn v23_orphan_bg_cron_run_classifies_as_background() {
    // Orphaned cron_runs row (cron_specs already deleted by the legacy
    // one-shot-bg-then-cleanup path) keeps the legacy bg-prefix heuristic so
    // the run still surfaces correctly post-migration.
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), false).await.unwrap();
    MIGRATIONS.to_version(&conn, 21).await.unwrap();

    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, delivery_status)
         VALUES (
            'bg-orphan-run', 'bg-orphan', '2026-05-18T02:00:00Z',
            'success', '/log/bg-orphan-run.ndjson', 'silent'
         )",
        (),
    )
    .await
    .unwrap();

    MIGRATIONS.to_latest(&conn).await.unwrap();

    let (kind, handoff_state): (String, Option<String>) = conn
        .query_one(
            "SELECT kind, handoff_state FROM async_runs WHERE id = 'bg-orphan-run'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(kind, "background");
    assert_eq!(handoff_state.as_deref(), Some("spawned"));
}

#[tokio::test]
async fn open_connection_returns_live_connection() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    // Verify memories table is accessible
    let count: i64 = conn
        .query_one(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='memories'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(count, 1, "memories table should exist via open_connection");
}
