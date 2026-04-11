use rusqlite_migration::{Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");
const V4_SCHEMA: &str = include_str!("sql/v4_sessions.sql");
const V5_SCHEMA: &str = include_str!("sql/v5_cron_feedback.sql");
const V6_SCHEMA: &str = include_str!("sql/v6_cron_specs.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| {
        Migrations::new(vec![
            M::up(V1_SCHEMA),
            M::up(V2_SCHEMA),
            M::up(V3_SCHEMA),
            M::up(V4_SCHEMA),
            M::up(V5_SCHEMA),
            M::up(V6_SCHEMA),
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
}
