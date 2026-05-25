use tempfile::TempDir;

pub fn migrated_connection() -> (TempDir, crate::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::open_connection(dir.path(), true).unwrap();
    (dir, conn)
}
