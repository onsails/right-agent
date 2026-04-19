use rusqlite::Transaction;
use rusqlite_migration::{HookError, Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");
const V4_SCHEMA: &str = include_str!("sql/v4_sessions.sql");
const V5_SCHEMA: &str = include_str!("sql/v5_cron_feedback.sql");
const V6_SCHEMA: &str = include_str!("sql/v6_cron_specs.sql");
const V7_SCHEMA: &str = include_str!("sql/v7_cron_trigger.sql");
const V8_SCHEMA: &str = include_str!("sql/v8_mcp_servers.sql");
const V9_SCHEMA: &str = include_str!("sql/v9_mcp_instructions.sql");
const V10_SCHEMA: &str = include_str!("sql/v10_mcp_auth.sql");
const V11_SCHEMA: &str = include_str!("sql/v11_auth_tokens.sql");
#[allow(dead_code)] // Doc-only: actual migration uses Rust hook for idempotency.
const V13_SCHEMA: &str = include_str!("sql/v13_one_shot_cron.sql");
const V14_SCHEMA: &str = include_str!("sql/v14_memory_failure_handling.sql");

/// v12: Add delivery_status and no_notify_reason columns to cron_runs,
/// backfill existing rows, and create auto-set trigger.
///
/// Implemented as a Rust hook (not pure SQL) because SQLite lacks
/// `ADD COLUMN IF NOT EXISTS` — the ALTER TABLE would fail with
/// "duplicate column name" if re-run on a database that already has
/// the columns.
fn v12_cron_diagnostics(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_runs') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("delivery_status")? {
        tx.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT")?;
    }
    if !has_column("no_notify_reason")? {
        tx.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT")?;
    }

    // Backfill existing rows (idempotent UPDATEs).
    tx.execute_batch(
        "UPDATE cron_runs SET delivery_status = 'delivered'
           WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
         UPDATE cron_runs SET delivery_status = 'pending'
           WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
         UPDATE cron_runs SET delivery_status = 'silent'
           WHERE notify_json IS NULL;",
    )?;

    // Trigger: auto-set delivery_status on INSERT (IF NOT EXISTS is idempotent).
    tx.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS cron_runs_delivery_status_insert
         AFTER INSERT ON cron_runs
         WHEN NEW.delivery_status IS NULL
         BEGIN
           UPDATE cron_runs SET delivery_status =
             CASE
               WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NOT NULL THEN 'delivered'
               WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NULL     THEN 'pending'
               ELSE 'silent'
             END
           WHERE id = NEW.id;
         END;",
    )?;

    Ok(())
}

/// v13: Add recurring and run_at columns to cron_specs for one-shot job support.
///
/// Idempotent — checks pragma_table_info before each ALTER.
fn v13_one_shot_cron(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_specs') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("recurring")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1")?;
    }
    if !has_column("run_at")? {
        tx.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")?;
    }

    Ok(())
}

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| {
        Migrations::new(vec![
            M::up(V1_SCHEMA),
            M::up(V2_SCHEMA),
            M::up(V3_SCHEMA),
            M::up(V4_SCHEMA),
            M::up(V5_SCHEMA),
            M::up(V6_SCHEMA),
            M::up(V7_SCHEMA),
            M::up(V8_SCHEMA),
            M::up(V9_SCHEMA),
            M::up(V10_SCHEMA),
            M::up(V11_SCHEMA),
            M::up_with_hook("", v12_cron_diagnostics),
            M::up_with_hook("", v13_one_shot_cron),
            M::up(V14_SCHEMA),
        ])
    });

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migrations_apply_cleanly_to_v4() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('id','chat_id','thread_id','root_session_id','label','is_active','created_at','last_used_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 8, "sessions table should have all 8 columns");
        let old_exists: bool = conn
            .prepare("SELECT 1 FROM telegram_sessions LIMIT 1")
            .is_ok();
        assert!(!old_exists, "telegram_sessions should be dropped");
    }

    #[test]
    fn sessions_partial_unique_index_enforces_single_active() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 1)",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 1)",
            [],
        );
        assert!(
            result.is_err(),
            "partial unique index should prevent two active sessions"
        );
    }

    #[test]
    fn sessions_allows_multiple_inactive() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'aaa', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) VALUES (1, 0, 'bbb', 0)",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE chat_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn migrations_apply_cleanly_to_v5() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"summary".to_string()), "summary column missing");
        assert!(cols.contains(&"notify_json".to_string()), "notify_json column missing");
        assert!(cols.contains(&"delivered_at".to_string()), "delivered_at column missing");
    }

    #[test]
    fn migrations_apply_cleanly_to_v7() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"triggered_at".to_string()),
            "triggered_at column missing"
        );
    }

    #[test]
    fn v8_mcp_servers_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://mcp.notion.com/mcp"),
        )
        .unwrap();

        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(url, "https://mcp.notion.com/mcp");

        // Test upsert
        conn.execute(
            "INSERT OR REPLACE INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("notion", "https://new-url.com/mcp"),
        )
        .unwrap();
        let url: String = conn
            .query_row(
                "SELECT url FROM mcp_servers WHERE name = ?1",
                ["notion"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(url, "https://new-url.com/mcp");
    }

    #[test]
    fn v9_mcp_servers_has_instructions_column() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url) VALUES (?1, ?2)",
            ("test-server", "https://example.com/mcp"),
        )
        .unwrap();

        let instructions: Option<String> = conn
            .query_row(
                "SELECT instructions FROM mcp_servers WHERE name = ?1",
                ["test-server"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            instructions.is_none(),
            "instructions should be NULL by default"
        );
    }

    #[test]
    fn v10_mcp_servers_has_auth_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO mcp_servers (name, url, auth_type, auth_token) VALUES (?1, ?2, ?3, ?4)",
            ("test", "https://example.com/mcp", "bearer", "sk-123"),
        )
        .unwrap();

        let auth_type: Option<String> = conn
            .query_row(
                "SELECT auth_type FROM mcp_servers WHERE name = ?1",
                ["test"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(auth_type.as_deref(), Some("bearer"));

        // Verify all new columns exist
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('mcp_servers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "auth_type",
            "auth_header",
            "auth_token",
            "refresh_token",
            "token_endpoint",
            "client_id",
            "client_secret",
            "expires_at",
        ] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v12_cron_diagnostics_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"delivery_status".to_string()),
            "delivery_status column missing"
        );
        assert!(
            cols.contains(&"no_notify_reason".to_string()),
            "no_notify_reason column missing"
        );
    }

    #[test]
    fn v12_backfill_delivery_status() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        // Insert a delivered run (has notify_json + delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json, delivered_at) \
             VALUES ('d1', 'j1', '2026-01-01T00:00:00Z', 'success', '/log', '{\"content\":\"hi\"}', '2026-01-01T00:05:00Z')",
            [],
        ).unwrap();
        // Insert a pending run (has notify_json, no delivered_at)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json) \
             VALUES ('p1', 'j1', '2026-01-01T01:00:00Z', 'success', '/log', '{\"content\":\"pending\"}')",
            [],
        ).unwrap();
        // Insert a silent run (no notify_json)
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary) \
             VALUES ('s1', 'j1', '2026-01-01T02:00:00Z', 'success', '/log', 'quiet')",
            [],
        ).unwrap();

        let status_of = |id: &str| -> Option<String> {
            conn.query_row(
                "SELECT delivery_status FROM cron_runs WHERE id = ?1",
                [id],
                |r| r.get(0),
            ).unwrap()
        };
        assert_eq!(status_of("d1").as_deref(), Some("delivered"));
        assert_eq!(status_of("p1").as_deref(), Some("pending"));
        assert_eq!(status_of("s1").as_deref(), Some("silent"));
    }

    #[test]
    fn v12_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v11 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 11).unwrap();

        // Manually add the columns that v12 would create.
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT").unwrap();
        conn.execute_batch("ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT").unwrap();

        // v12 must not fail even though columns already exist.
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"delivery_status".to_string()));
        assert!(cols.contains(&"no_notify_reason".to_string()));
    }

    #[test]
    fn migrations_apply_cleanly_to_v6() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"job_name".to_string()), "job_name column missing");
        assert!(cols.contains(&"schedule".to_string()), "schedule column missing");
        assert!(cols.contains(&"prompt".to_string()), "prompt column missing");
        assert!(cols.contains(&"lock_ttl".to_string()), "lock_ttl column missing");
        assert!(cols.contains(&"max_budget_usd".to_string()), "max_budget_usd column missing");
        assert!(cols.contains(&"created_at".to_string()), "created_at column missing");
        assert!(cols.contains(&"updated_at".to_string()), "updated_at column missing");
    }

    #[test]
    fn v13_one_shot_cron_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"recurring".to_string()),
            "recurring column missing"
        );
        assert!(
            cols.contains(&"run_at".to_string()),
            "run_at column missing"
        );
    }

    #[test]
    fn v13_idempotent_when_columns_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 12).unwrap();
        conn.execute_batch(
            "ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1",
        )
        .unwrap();
        conn.execute_batch("ALTER TABLE cron_specs ADD COLUMN run_at TEXT")
            .unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_specs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"recurring".to_string()));
        assert!(cols.contains(&"run_at".to_string()));
    }

    #[test]
    fn v13_existing_specs_get_recurring_true() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 12).unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('old-job', '*/5 * * * *', 'do stuff', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let recurring: i64 = conn
            .query_row(
                "SELECT recurring FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(recurring, 1, "existing specs must default to recurring=1");
        let run_at: Option<String> = conn
            .query_row(
                "SELECT run_at FROM cron_specs WHERE job_name = 'old-job'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(run_at.is_none(), "existing specs must have run_at=NULL");
    }

    #[test]
    fn v14_pending_retains_table_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "id", "content", "context", "document_id", "update_mode",
            "tags_json", "created_at", "attempts", "last_attempt_at",
            "last_error", "source",
        ] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v14_memory_alerts_table_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in ["alert_type", "first_sent_at"] {
            assert!(cols.contains(&col.to_string()), "{col} column missing");
        }
    }

    #[test]
    fn v14_pending_retains_created_index_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
                 AND name='idx_pending_retains_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "idx_pending_retains_created should exist");
    }

    #[test]
    fn v14_idempotent_when_tables_already_exist() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Apply up to v13 (version index is 1-based in to_version).
        MIGRATIONS.to_version(&mut conn, 13).unwrap();

        // Manually pre-create the tables that v14 would create, matching the
        // schema in sql/v14_memory_failure_handling.sql.
        conn.execute_batch(
            "CREATE TABLE pending_retains (
                 id              INTEGER PRIMARY KEY AUTOINCREMENT,
                 content         TEXT NOT NULL,
                 context         TEXT,
                 document_id     TEXT,
                 update_mode     TEXT,
                 tags_json       TEXT,
                 created_at      TEXT NOT NULL,
                 attempts        INTEGER NOT NULL DEFAULT 0,
                 last_attempt_at TEXT,
                 last_error      TEXT,
                 source          TEXT NOT NULL
             );
             CREATE TABLE memory_alerts (
                 alert_type    TEXT PRIMARY KEY,
                 first_sent_at TEXT NOT NULL
             );",
        )
        .unwrap();

        // v14 must not fail even though tables already exist.
        MIGRATIONS.to_latest(&mut conn).unwrap();

        let pending_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('pending_retains')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "id", "content", "context", "document_id", "update_mode",
            "tags_json", "created_at", "attempts", "last_attempt_at",
            "last_error", "source",
        ] {
            assert!(
                pending_cols.contains(&col.to_string()),
                "{col} column missing from pending_retains"
            );
        }

        let alert_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('memory_alerts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in ["alert_type", "first_sent_at"] {
            assert!(
                alert_cols.contains(&col.to_string()),
                "{col} column missing from memory_alerts"
            );
        }

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' \
                 AND name='idx_pending_retains_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_pending_retains_created should exist after idempotent v14"
        );
    }
}
