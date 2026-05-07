use crate::credentials::{delete_auth_token, get_auth_token, save_auth_token};

// --- auth_token tests ---

#[test]
fn save_and_get_auth_token() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
    save_auth_token(&conn, "test-token-123").unwrap();
    assert_eq!(
        get_auth_token(&conn).unwrap(),
        Some("test-token-123".to_string())
    );
}

#[test]
fn get_auth_token_empty() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
    assert_eq!(get_auth_token(&conn).unwrap(), None);
}

#[test]
fn save_auth_token_replaces_existing() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
    save_auth_token(&conn, "old-token").unwrap();
    save_auth_token(&conn, "new-token").unwrap();
    assert_eq!(
        get_auth_token(&conn).unwrap(),
        Some("new-token".to_string())
    );
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM auth_tokens", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn delete_auth_token_works() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
    save_auth_token(&conn, "token").unwrap();
    delete_auth_token(&conn).unwrap();
    assert_eq!(get_auth_token(&conn).unwrap(), None);
}
