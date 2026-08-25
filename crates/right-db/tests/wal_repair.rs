//! Deterministic fixture tests for the offline legacy multiprocess-WAL repair
//! (`right_db::repair_legacy_wal`).
//!
//! The tests build legacy state synthetically: a migrated `data.db` with
//! representative rows plus legacy coordination sidecar sentinels. They prove
//! the retained offline repair preserves the original database and WAL bytes
//! while producing a standard-local replacement with no `data.db-tshm`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use right_db::{RepairRequest, repair_legacy_wal};

/// Canary values prove the manifest/report never carries row content. They
/// deliberately hit the three secret-bearing shapes the plan calls out:
/// credentials (`auth_tokens.token`), prompts (`cron_specs.prompt`), and
/// message text (`conversation_messages.content`).
const CANARY_TOKEN: &str = "CANARY-TOKEN-9f8e7d6c5b4a";
const CANARY_PROMPT: &str = "CANARY-PROMPT-rotate-the-orbital-couch";
const CANARY_MESSAGE: &str = "CANARY-MESSAGE-body-never-leaves-the-db";

const FIXED_TIMESTAMP: &str = "20010203-040506";

/// Snapshot every `data.db*` artifact byte-for-byte, keyed by file name.
fn snapshot_db_artifacts(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "data.db" || name.starts_with("data.db-") {
            out.insert(name, std::fs::read(entry.path()).unwrap());
        }
    }
    out
}

/// Migrated agent DB with canary rows, plus legacy coordination sentinels.
/// Returns the agent dir, the backups dir (sibling, not yet created on disk),
/// the original artifact snapshot, and the original schema version.
async fn create_legacy_fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    BTreeMap<String, Vec<u8>>,
    i64,
) {
    let agent_home = tempfile::tempdir().unwrap();
    let agent_dir = agent_home.path().join("agents").join("riskoff");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
    conn.execute(
        "INSERT INTO auth_tokens (token) VALUES (?1)",
        [CANARY_TOKEN],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, created_at, updated_at) \
         VALUES ('nightly', '0 3 * * *', ?1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [CANARY_PROMPT],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO conversation_messages (chat_id, role, content) VALUES (42, 'user', ?1)",
        [CANARY_MESSAGE],
    )
    .await
    .unwrap();
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .await
        .unwrap();
    drop(conn);

    // Legacy multiprocess coordination sentinels. Only written when Turso did
    // not leave real ones behind: either way the repair must preserve the
    // bytes forensically and strip them from the staged copy only.
    for suffix in ["-tshm", "-shm"] {
        let sidecar = agent_dir.join(format!("data.db{suffix}"));
        if !sidecar.exists() {
            std::fs::write(&sidecar, b"legacy coordination sentinel").unwrap();
        }
    }

    let snapshot = snapshot_db_artifacts(&agent_dir);
    assert!(
        snapshot.contains_key("data.db"),
        "fixture must contain data.db"
    );
    assert!(
        snapshot.contains_key("data.db-wal"),
        "fixture must contain the WAL artifact whose bytes repair preserves"
    );

    let backups_home = tempfile::tempdir().unwrap();
    (agent_home, backups_home, snapshot, user_version)
}

fn repair_request(agent_dir: &Path, backups_dir: &Path, timestamp: &str) -> RepairRequest {
    RepairRequest {
        agent_dir: agent_dir.to_path_buf(),
        backups_dir: backups_dir.to_path_buf(),
        timestamp: timestamp.to_string(),
    }
}

fn assert_bytes_match(snapshot: &BTreeMap<String, Vec<u8>>, dir: &Path, context: &str) {
    for (name, bytes) in snapshot {
        let path = dir.join(name);
        assert!(
            path.is_file(),
            "{context}: preserved artifact {name} missing at {}",
            path.display()
        );
        assert_eq!(
            &std::fs::read(&path).unwrap(),
            bytes,
            "{context}: artifact {name} bytes changed"
        );
    }
}

#[tokio::test]
async fn repair_recovers_wal_and_preserves_forensic_bytes() {
    let (agent_home, backups_home, snapshot, user_version) = create_legacy_fixture().await;
    let agent_dir = agent_home.path().join("agents").join("riskoff");
    let backups_dir = backups_home.path().join("riskoff");

    let report = repair_legacy_wal(repair_request(&agent_dir, &backups_dir, FIXED_TIMESTAMP))
        .await
        .expect("repair must succeed on a healthy legacy fixture");

    // The recovered database is the new live file.
    let live_db = agent_dir.join("data.db");
    assert!(live_db.is_file(), "recovered data.db must exist");
    assert_eq!(
        report.quick_check, "ok",
        "replacement must pass quick_check"
    );
    assert_eq!(
        i64::from(report.schema_version),
        user_version,
        "replacement must keep the original schema version"
    );

    // No copied legacy sidecars beside the recovered snapshot.
    let live_now = snapshot_db_artifacts(&agent_dir);
    assert_eq!(
        live_now.keys().collect::<Vec<_>>(),
        ["data.db"],
        "live set must be exactly the standalone snapshot; found {live_now:?}"
    );
    assert!(
        !agent_dir.join("data.db-tshm").exists(),
        "the standard-local replacement must not create a legacy -tshm sidecar"
    );

    // Forensic copy: every original artifact byte-for-byte.
    assert_bytes_match(&snapshot, &report.forensic_dir, "forensic copy");
    // Live-pre-swap: the complete original set, moved not copied.
    assert_bytes_match(&snapshot, &report.live_pre_swap_dir, "live-pre-swap");

    // Report digests match the preserved originals.
    for digest in &report.preserved_files {
        let original = snapshot
            .get(&digest.name)
            .unwrap_or_else(|| panic!("report lists unknown artifact {}", digest.name));
        assert_eq!(
            digest.size,
            original.len() as u64,
            "size mismatch for {}",
            digest.name
        );
        assert!(
            !digest.sha256.is_empty(),
            "sha256 must be recorded for {}",
            digest.name
        );
    }

    // Fixed non-secret invariant report: every invariant table exists, counts
    // match the canary fixture (1 auth token, 1 cron spec, 1 message).
    let count_of = |table: &str| -> i64 {
        report
            .tables
            .iter()
            .find(|t| t.table == table)
            .unwrap_or_else(|| panic!("invariant report missing table {table}"))
            .rows
            .unwrap_or_else(|| panic!("table {table} must exist"))
    };
    assert_eq!(count_of("auth_tokens"), 1);
    assert_eq!(count_of("cron_specs"), 1);
    assert_eq!(count_of("conversation_messages"), 1);
    assert_eq!(count_of("async_runs"), 0);
    assert_eq!(count_of("usage_events"), 0);

    // The recovered snapshot serves reads and preserved the rows.
    let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
    let tokens: i64 = conn
        .query_row("SELECT COUNT(*) FROM auth_tokens", (), |row| row.get(0))
        .await
        .unwrap();
    assert_eq!(tokens, 1, "canary row must survive recovery");

    // The manifest exists, records hashes/counts/version, and never leaks
    // row values, credentials, prompts, or message text.
    let manifest = std::fs::read_to_string(&report.manifest_path).unwrap();
    for canary in [CANARY_TOKEN, CANARY_PROMPT, CANARY_MESSAGE] {
        assert!(
            !manifest.contains(canary),
            "manifest must not contain row value {canary}:\n{manifest}"
        );
    }
    assert!(manifest.contains("\"schema_version\""));
    assert!(manifest.contains("\"auth_tokens\""));
    assert!(manifest.contains("\"quick_check\": \"ok\""));
    assert!(manifest.contains("\"sha256\""));
    assert!(manifest.contains("\"swap_status\": \"swapped\""));
}

#[tokio::test]
async fn repair_fails_when_data_db_missing() {
    let agent_home = tempfile::tempdir().unwrap();
    let agent_dir = agent_home.path().join("agents").join("ghost");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let backups_dir = tempfile::tempdir().unwrap().path().join("ghost");

    let err = repair_legacy_wal(repair_request(&agent_dir, &backups_dir, FIXED_TIMESTAMP))
        .await
        .expect_err("repair without data.db must fail");
    assert!(
        err.to_string().contains("data.db"),
        "error must name data.db: {err}"
    );
    assert!(
        !backups_dir.exists(),
        "no recovery artifacts may be created when preflight fails"
    );
}

#[tokio::test]
async fn repair_rejects_path_unsafe_timestamp() {
    let (agent_home, backups_home, _snapshot, _version) = create_legacy_fixture().await;
    let agent_dir = agent_home.path().join("agents").join("riskoff");
    let backups_dir = backups_home.path().join("riskoff");

    for bad in ["../escape", "a/b", "", "with space"] {
        let err = repair_legacy_wal(repair_request(&agent_dir, &backups_dir, bad))
            .await
            .expect_err("path-unsafe timestamp must be rejected");
        assert!(
            err.to_string().contains("timestamp"),
            "error must name the timestamp: {err}"
        );
    }
}

#[tokio::test]
async fn repair_pre_swap_failure_leaves_live_set_untouched() {
    let (agent_home, _backups_home, snapshot, _version) = create_legacy_fixture().await;
    let agent_dir = agent_home.path().join("agents").join("riskoff");

    // backups_dir exists as a regular FILE: creating the recovery directory
    // fails before any live mutation.
    let backups_file = tempfile::tempdir().unwrap();
    let backups_dir = backups_file.path().join("riskoff");
    std::fs::write(&backups_dir, b"not a directory").unwrap();

    let err = repair_legacy_wal(repair_request(&agent_dir, &backups_dir, FIXED_TIMESTAMP))
        .await
        .expect_err("repair must fail when the recovery dir cannot be created");

    let live_now = snapshot_db_artifacts(&agent_dir);
    assert_eq!(
        live_now, snapshot,
        "pre-swap failure must leave the live set byte-identical: {err}"
    );
}

#[tokio::test]
async fn repair_mid_swap_failure_restores_complete_original_set() {
    let (agent_home, backups_home, snapshot, _version) = create_legacy_fixture().await;
    let agent_dir = agent_home.path().join("agents").join("riskoff");
    let backups_dir = backups_home.path().join("riskoff");

    // Force the SECOND live rename to fail: the swap moves data.db first,
    // then sidecars into live-pre-swap/. A pre-created non-empty directory at
    // the data.db-shm destination makes that rename fail after data.db has
    // already been moved out of the live set.
    let blocker = backups_dir
        .join(format!("wal-recovery-{FIXED_TIMESTAMP}"))
        .join("live-pre-swap")
        .join("data.db-shm");
    std::fs::create_dir_all(&blocker).unwrap();
    std::fs::write(blocker.join("occupant"), b"x").unwrap();
    assert!(
        snapshot.contains_key("data.db-shm"),
        "fixture must carry a data.db-shm sidecar for this failure injection"
    );

    let err = repair_legacy_wal(repair_request(&agent_dir, &backups_dir, FIXED_TIMESTAMP))
        .await
        .expect_err("mid-swap rename failure must propagate");
    let msg = err.to_string();
    assert!(
        msg.contains("rollback") || msg.contains("restored"),
        "error must carry rollback context: {msg}"
    );

    let live_now = snapshot_db_artifacts(&agent_dir);
    assert_eq!(
        live_now, snapshot,
        "rollback must restore the complete original set byte-identically"
    );

    // The failed repair must not leave its replacement temp file behind.
    let leftovers: Vec<PathBuf> = std::fs::read_dir(&agent_dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.contains("recovering").then(|| agent_dir.join(name))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "no recovering temp files may remain: {leftovers:?}"
    );
}
