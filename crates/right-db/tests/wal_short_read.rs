use std::fs::OpenOptions;

use tempfile::tempdir;

const FIFTH_FRAME_OFFSET: u64 = 16_512;
const FIVE_FRAME_WAL_LEN: u64 = 20_632;

#[tokio::test]
async fn open_connection_propagates_persistent_short_read_without_deleting_database_or_wal() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("data.db");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wal_probe.db"),
        &db_path,
    )
    .unwrap();

    let conn = right_db::open_connection(dir.path(), false).await.unwrap();
    for value in ["one", "two", "three", "four", "five"] {
        conn.execute("INSERT INTO wal_probe(value) VALUES (?1)", [value])
            .await
            .unwrap();
    }

    let wal_path = dir.path().join("data.db-wal");
    assert_eq!(
        std::fs::metadata(&wal_path).unwrap().len(),
        FIVE_FRAME_WAL_LEN,
        "fixture must put the fifth frame payload at offset 16512"
    );
    OpenOptions::new()
        .write(true)
        .open(&wal_path)
        .unwrap()
        .set_len(0)
        .unwrap();

    let reopened = right_db::open_connection(dir.path(), false)
        .await
        .expect("right-db open may succeed before a lazy WAL read");
    let error = reopened
        .query_row("SELECT count(*) FROM wal_probe", (), |row| {
            let count: i64 = row.get(0)?;
            Ok(count)
        })
        .await
        .expect_err("a persistent short-read must propagate from the first WAL read");
    assert!(
        format!("{error:#}").contains(&format!(
            "short read on WAL frame at offset {FIFTH_FRAME_OFFSET}"
        )),
        "persistent short-read must propagate unchanged: {error:#}"
    );
    assert!(
        db_path.exists(),
        "open failure must preserve the database file"
    );
    assert!(wal_path.exists(), "open failure must preserve the WAL file");
    assert_eq!(
        std::fs::metadata(&wal_path).unwrap().len(),
        0,
        "normal open must not replace the zero-byte WAL"
    );
}
