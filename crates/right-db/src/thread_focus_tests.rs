use super::*;
use tempfile::TempDir;

struct TestDb {
    _dir: TempDir,
    conn: Connection,
}

impl std::ops::Deref for TestDb {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

async fn migrated() -> TestDb {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).await.unwrap();
    TestDb { _dir: dir, conn }
}

#[tokio::test]
async fn get_missing_returns_none() {
    let db = migrated().await;
    assert!(get(&db, 100, 0).await.unwrap().is_none());
}

#[tokio::test]
async fn set_operator_then_get_roundtrips() {
    let db = migrated().await;
    set_operator(&db, 100, 7, Some("be concise")).await.unwrap();
    let row = get(&db, 100, 7).await.unwrap().unwrap();
    assert_eq!(row.operator_focus.as_deref(), Some("be concise"));
    assert_eq!(row.agent_focus, None);
}

#[tokio::test]
async fn operator_and_agent_columns_do_not_clobber() {
    let db = migrated().await;
    set_operator(&db, 100, 0, Some("op text")).await.unwrap();
    set_agent(&db, 100, 0, Some("agent text")).await.unwrap();
    let row = get(&db, 100, 0).await.unwrap().unwrap();
    assert_eq!(row.operator_focus.as_deref(), Some("op text"));
    assert_eq!(row.agent_focus.as_deref(), Some("agent text"));
}

#[tokio::test]
async fn set_none_clears_one_column_only() {
    let db = migrated().await;
    set_operator(&db, 100, 0, Some("op")).await.unwrap();
    set_agent(&db, 100, 0, Some("ag")).await.unwrap();
    set_agent(&db, 100, 0, None).await.unwrap();
    let row = get(&db, 100, 0).await.unwrap().unwrap();
    assert_eq!(row.operator_focus.as_deref(), Some("op"));
    assert_eq!(row.agent_focus, None);
}

#[tokio::test]
async fn set_operator_none_clears_operator_only() {
    let db = migrated().await;
    set_operator(&db, 100, 0, Some("op")).await.unwrap();
    set_agent(&db, 100, 0, Some("ag")).await.unwrap();
    set_operator(&db, 100, 0, None).await.unwrap();
    let row = get(&db, 100, 0).await.unwrap().unwrap();
    assert_eq!(row.operator_focus, None);
    assert_eq!(row.agent_focus.as_deref(), Some("ag"));
}

#[tokio::test]
async fn scope_is_keyed_by_chat_and_thread() {
    let db = migrated().await;
    set_operator(&db, 100, 0, Some("general")).await.unwrap();
    set_operator(&db, 100, 9, Some("topic-9")).await.unwrap();
    set_operator(&db, 200, 0, Some("chat-200-general"))
        .await
        .unwrap();
    assert_eq!(
        get(&db, 100, 0)
            .await
            .unwrap()
            .unwrap()
            .operator_focus
            .as_deref(),
        Some("general")
    );
    assert_eq!(
        get(&db, 100, 9)
            .await
            .unwrap()
            .unwrap()
            .operator_focus
            .as_deref(),
        Some("topic-9")
    );
    assert_eq!(
        get(&db, 200, 0)
            .await
            .unwrap()
            .unwrap()
            .operator_focus
            .as_deref(),
        Some("chat-200-general")
    );
}
