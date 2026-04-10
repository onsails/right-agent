use rusqlite_migration::{Migrations, M};

const V1_SCHEMA: &str = include_str!("sql/v1_schema.sql");
const V2_SCHEMA: &str = include_str!("sql/v2_telegram_sessions.sql");
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");
const V4_SCHEMA: &str = include_str!("sql/v4_sessions.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| {
        Migrations::new(vec![
            M::up(V1_SCHEMA),
            M::up(V2_SCHEMA),
            M::up(V3_SCHEMA),
            M::up(V4_SCHEMA),
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
}
