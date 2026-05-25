use crate::credentials::{delete_auth_token, get_auth_token, save_auth_token};

// --- auth_token tests ---

#[tokio::test]
async fn save_and_get_auth_token() {
    let (_dir, conn) = right_db::test_support::migrated_connection().await;
    save_auth_token(&conn, "test-token-123").await.unwrap();
    assert_eq!(
        get_auth_token(&conn).await.unwrap(),
        Some("test-token-123".to_string())
    );
}

#[tokio::test]
async fn get_auth_token_empty() {
    let (_dir, conn) = right_db::test_support::migrated_connection().await;
    assert_eq!(get_auth_token(&conn).await.unwrap(), None);
}

#[tokio::test]
async fn save_auth_token_replaces_existing() {
    let (_dir, conn) = right_db::test_support::migrated_connection().await;
    save_auth_token(&conn, "old-token").await.unwrap();
    save_auth_token(&conn, "new-token").await.unwrap();
    assert_eq!(
        get_auth_token(&conn).await.unwrap(),
        Some("new-token".to_string())
    );
    let count: i64 = conn
        .query_one("SELECT COUNT(*) FROM auth_tokens", (), |r| r.get(0))
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn delete_auth_token_works() {
    let (_dir, conn) = right_db::test_support::migrated_connection().await;
    save_auth_token(&conn, "token").await.unwrap();
    delete_auth_token(&conn).await.unwrap();
    assert_eq!(get_auth_token(&conn).await.unwrap(), None);
}
