//! Regression guard for onsails/right-agent#79: the legacy pre-Turso
//! `rusqlite` FTS5 scrubber was removed, so neither `rusqlite` nor its
//! `libsqlite3-sys` C backend may be re-introduced **by a Right-owned crate**.
//!
//! `libsqlite3-sys` is still permitted in the workspace lockfile when the only
//! dependents are microsandbox's own runtime-internal crates. The sandbox
//! backend (`microsandbox` → `microsandbox-db` → `sqlx-sqlite`) owns its
//! private store and is not part of Right's runtime database boundary. The #79
//! invariant is that no `right-*` crate links SQLite's C backend; scoping the
//! assertion to dependents, rather than to the lockfile's crate list, keeps the
//! invariant while naming the third-party exception explicitly.

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

/// Names of `right-*` workspace crates whose `[package].dependencies` block
/// lists `package`.
fn right_dependents_of(lock_contents: &str, package: &str) -> Vec<String> {
    let mut offenders = Vec::new();
    for block in lock_contents.split("[[package]]") {
        let Some(block) = block.strip_prefix('\n') else {
            continue;
        };
        let Some(name) = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("name = "))
            .map(|line| line.trim().trim_matches('"').to_string())
        else {
            continue;
        };
        if !name.starts_with("right") {
            continue;
        }
        let deps = block
            .split_once("dependencies = [")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let mentions = deps
            .lines()
            .take_while(|line| !line.trim_start().starts_with(']'))
            .any(|line| line.trim().trim_end_matches(',').trim_matches('"') == package);
        if mentions {
            offenders.push(name);
        }
    }
    offenders
}

#[test]
fn workspace_has_no_rusqlite_or_libsqlite3_sys_from_right_crates() {
    let lock = workspace_cargo_lock();
    let contents =
        std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("read {}: {e}", lock.display()));

    // `rusqlite` has no permitted dependent at all.
    assert!(
        !contents.contains("name = \"rusqlite\""),
        "`rusqlite` reappeared in {} — the #79 scrubber removal must keep \
         SQLite's C backend out of Right's build; the runtime database boundary is Turso",
        lock.display(),
    );

    // `libsqlite3-sys` may appear only because the microsandbox sandbox backend
    // compiles it for its own private store. No `right-*` crate may depend on it.
    if contents.contains("name = \"libsqlite3-sys\"") {
        let offenders = right_dependents_of(&contents, "libsqlite3-sys");
        assert!(
            offenders.is_empty(),
            "`libsqlite3-sys` is pulled in by Right-owned crates {offenders:?}. Only the \
             microsandbox sandbox backend may compile the C backend; a `right-*` crate must \
             never link SQLite's C build (see onsails/right-agent#79). Lockfile: {}",
            lock.display(),
        );
    }
}
