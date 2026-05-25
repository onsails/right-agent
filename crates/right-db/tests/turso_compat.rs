use right_db::{open_database_path_readonly, open_db};
use tempfile::tempdir;

async fn open_turso(path: &std::path::Path) -> turso::Result<turso::Connection> {
    let path = path
        .to_str()
        .expect("temp database path should be valid UTF-8");
    let db = turso::Builder::new_local(path)
        .experimental_index_method(true)
        .build()
        .await?;
    db.connect()
}

#[tokio::test(flavor = "current_thread")]
async fn turso_opens_current_right_db_file_before_backend_swap() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).expect("current right-db should create migrated data.db");
    let db_path = dir.path().join("data.db");

    let conn = open_turso(&db_path)
        .await
        .expect("turso should open data.db created by current right-db");

    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    let row = rows.next().await.unwrap().expect("user_version row");
    let version: i64 = row.get(0).unwrap();
    assert_eq!(version, right_db::migrations::LATEST_SCHEMA_VERSION as i64);

    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("sessions count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn turso_direct_supports_required_local_sql_features() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("turso-direct.db");
    let mut conn = open_turso(&db_path).await.unwrap();

    conn.execute_batch(
        "CREATE TABLE docs (
             id INTEGER PRIMARY KEY,
             content TEXT NOT NULL
         );
         CREATE TABLE docs_audit (
             doc_id INTEGER NOT NULL,
             content TEXT NOT NULL
         );
         CREATE INDEX docs_turso_fts ON docs USING fts(content);
         CREATE TRIGGER docs_ai AFTER INSERT ON docs BEGIN
             INSERT INTO docs_audit(doc_id, content) VALUES (new.id, new.content);
         END;",
    )
    .await
    .unwrap();

    let id: i64 = {
        let mut rows = conn
            .query(
                "INSERT INTO docs (content) VALUES ('needle phrase') RETURNING id",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("RETURNING row");
        row.get(0).unwrap()
    };
    assert!(id > 0);

    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM docs WHERE content MATCH ?",
            ["needle"],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("fts count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1, "Turso FTS index should find inserted content");

    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM docs_audit WHERE content = ?",
            ["needle phrase"],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("trigger count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 1, "trigger should copy inserted content");

    let tx = conn
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await
        .unwrap();
    tx.execute("INSERT INTO docs (content) VALUES (?1)", ["rolled back"])
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM docs WHERE content = 'rolled back'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("rollback count row");
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn current_readonly_contract_still_rejects_writes_before_backend_swap() {
    let dir = tempdir().unwrap();
    open_db(dir.path(), true).unwrap();
    let db_path = dir.path().join("data.db");

    let conn = open_database_path_readonly(&db_path).unwrap();
    let err = conn
        .execute("CREATE TABLE readonly_probe (id INTEGER)", ())
        .expect_err("readonly connection should reject writes");

    assert!(err.to_string().contains("readonly database"));
}
