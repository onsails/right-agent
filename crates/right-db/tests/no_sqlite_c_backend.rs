//! Regression guard for onsails/right-agent#79: the legacy pre-Turso
//! `rusqlite` FTS5 scrubber was removed, so neither `rusqlite` nor its
//! `libsqlite3-sys` C backend may re-enter the workspace dependency graph.
//! Re-adding either would put SQLite's bundled C build back on every compile,
//! which is exactly what removing the scrubber was meant to eliminate.

use std::path::PathBuf;

fn workspace_cargo_lock() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.exists() {
            return candidate;
        }
        assert!(
            dir.pop(),
            "workspace Cargo.lock not found above CARGO_MANIFEST_DIR",
        );
    }
}

#[test]
fn workspace_has_no_rusqlite_or_libsqlite3_sys() {
    let lock = workspace_cargo_lock();
    let contents =
        std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("read {}: {e}", lock.display()));

    for forbidden in ["rusqlite", "libsqlite3-sys"] {
        let needle = format!("name = \"{forbidden}\"");
        assert!(
            !contents.contains(&needle),
            "`{forbidden}` reappeared in {} — the #79 scrubber removal must keep \
             SQLite's C backend out of the build; the runtime database boundary is Turso",
            lock.display(),
        );
    }
}
