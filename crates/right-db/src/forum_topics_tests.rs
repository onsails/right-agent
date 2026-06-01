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
async fn upsert_then_list_roundtrips() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Bugs", Some(7322096), None)
        .await
        .unwrap();
    let rows = list(&db, 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_thread_id, 5);
    assert_eq!(rows[0].name.as_deref(), Some("Bugs"));
    assert_eq!(rows[0].icon_color, Some(7322096));
    assert_eq!(rows[0].state, "open");
}

#[tokio::test]
async fn list_is_strictly_scoped_to_one_chat() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "ChatA topic", None, None)
        .await
        .unwrap();
    upsert_created(&db, 200, 9, "ChatB topic", None, None)
        .await
        .unwrap();
    let a = list(&db, 100).await.unwrap();
    let b = list(&db, 200).await.unwrap();
    assert_eq!(a.len(), 1, "chat 100 must see only its own topic");
    assert_eq!(a[0].name.as_deref(), Some("ChatA topic"));
    assert_eq!(b.len(), 1, "chat 200 must see only its own topic");
    assert_eq!(b[0].name.as_deref(), Some("ChatB topic"));
}

#[tokio::test]
async fn set_state_closes_and_reopens() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Bugs", None, None)
        .await
        .unwrap();
    set_state(&db, 100, 5, "closed").await.unwrap();
    assert_eq!(list(&db, 100).await.unwrap()[0].state, "closed");
    set_state(&db, 100, 5, "open").await.unwrap();
    assert_eq!(list(&db, 100).await.unwrap()[0].state, "open");
}

#[tokio::test]
async fn update_edited_changes_name_only() {
    let db = migrated().await;
    upsert_created(&db, 100, 5, "Old", Some(7322096), None)
        .await
        .unwrap();
    update_edited(&db, 100, 5, Some("New"), None).await.unwrap();
    let rows = list(&db, 100).await.unwrap();
    assert_eq!(rows[0].name.as_deref(), Some("New"));
    assert_eq!(rows[0].icon_color, Some(7322096), "icon untouched");
}

#[tokio::test]
async fn update_edited_is_noop_for_untracked_topic() {
    let db = migrated().await;
    update_edited(&db, 100, 999, Some("ghost"), None)
        .await
        .unwrap();
    assert!(list(&db, 100).await.unwrap().is_empty());
}
