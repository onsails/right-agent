use super::*;

/// A sandbox name no real sandbox can share.
///
/// `destroy_agent` deletes the sandbox named in `agent.yaml`, and the
/// microsandbox catalog is host-global — it has no `--home` isolation to hide
/// behind. A fixed name like `right-test-agent` would let a unit test delete a
/// developer's real microVM, so every test that reaches the delete path uses a
/// name that cannot exist.
fn unique_sandbox_name(scope: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    format!("right-destroy-test-{scope}-{}-{nanos}", std::process::id())
}

// The refcount-plan tests are gone with the planner itself: destroy no longer
// computes a detach/delete plan from every sibling's `agent.yaml`. The store
// owns the refcount — `ProviderStore::remove` re-homes an owned record to a
// surviving borrower, and `unshare` drops a borrowed reference — and those
// semantics are covered in `right_providers::store_tests`.
//
// The `set_provider_shared_from` / `rehome_owner_in_agent_yaml` tests are gone
// with the functions themselves: provider ownership lives in providers.db
// (`right_providers::ProviderStore`), so destroy no longer rewrites
// `shared_from:` lines in surviving agents' agent.yaml.

#[tokio::test]
async fn destroy_agent_removes_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("test-agent");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("removes-dir");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();

    let options = DestroyOptions {
        agent_name: "test-agent".into(),
        backup: false,
    };

    let result = destroy_agent(home, &options).await.unwrap();

    assert!(
        !result.agent_stopped,
        "PC not running, should not have stopped"
    );
    assert!(
        !result.sandbox_deleted,
        "no sandbox exists under this name — destroy must not report a deletion it did not perform"
    );
    assert!(result.backup_path.is_none());
    assert!(result.dir_removed);
    assert!(
        !result.pc_reloaded,
        "PC not running, should not have reloaded"
    );
    assert!(!agents_dir.exists(), "agent dir should be deleted");
}

#[tokio::test]
async fn destroy_nonexistent_agent_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home.join("agents")).unwrap();

    let options = DestroyOptions {
        agent_name: "nonexistent".into(),
        backup: false,
    };

    let result = destroy_agent(home, &options).await;
    assert!(result.is_err());
}

/// Guards the `--home` isolation invariant: when `<home>/run/state.json`
/// does not exist, `destroy_agent` must not touch process-compose at all.
/// `PcClient::from_home` is the only public constructor, so there is no
/// way for destroy to contact the user's live PC from an isolated home.
#[tokio::test]
async fn destroy_skips_pc_when_no_runtime_state() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("isolated");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("isolated");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();

    // No <home>/run/state.json exists.
    assert!(!home.join("run").join("state.json").exists());

    let options = DestroyOptions {
        agent_name: "isolated".into(),
        backup: false,
    };

    let result = destroy_agent(home, &options).await.unwrap();

    assert!(
        !result.agent_stopped,
        "no runtime state → PC skipped → agent not stopped"
    );
    assert!(
        !result.pc_reloaded,
        "no runtime state → PC skipped → not reloaded"
    );
    assert!(result.dir_removed, "agent dir should still be removed");
    assert!(!agents_dir.exists(), "agent dir should be deleted");
}

#[tokio::test]
async fn destroy_with_backup_creates_backup_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-test");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("backup-test");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();
    std::fs::write(agents_dir.join("IDENTITY.md"), "# Test agent").unwrap();

    let options = DestroyOptions {
        agent_name: "backup-test".into(),
        backup: true,
    };

    let result = destroy_agent(home, &options).await.unwrap();

    assert!(
        result.backup_path.is_some(),
        "backup should have been created"
    );
    let backup_path = result.backup_path.unwrap();
    assert!(backup_path.exists(), "backup dir should exist");
    // No sandbox exists under this run's unique name, so `sandbox.tar.gz` is
    // absent and the backup is the config-file copies. A sandbox that exists
    // but cannot be archived is a hard error instead — see
    // `destroy_with_backup_aborts_before_destructive_steps_when_backup_fails`.
    assert!(
        !backup_path.join("sandbox.tar.gz").exists(),
        "no sandbox to archive"
    );
    assert!(
        backup_path.join("agent.yaml").exists(),
        "agent.yaml must always be copied into the backup"
    );
    assert!(
        result.dir_removed,
        "agent dir should be removed after backup"
    );
}

// `destroy_with_backup_excludes_database_sidecars_from_no_sandbox_tar` is gone
// with the host-tar backup branch it guarded: a sandboxless agent was the only
// way to reach it. Sandbox backups come down as `sandbox.tar.gz`, archived
// inside the guest, and never tar the host agent directory.

#[tokio::test]
async fn destroy_with_backup_vacuum_copies_data_db() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-db");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("backup-db");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();
    let conn = right_db::open_connection(&agents_dir, true).await.unwrap();
    conn.execute(
        "INSERT INTO auth_tokens (token) VALUES (?1)",
        right_db::params!["token-for-backup"],
    )
    .await
    .unwrap();
    drop(conn);

    let options = DestroyOptions {
        agent_name: "backup-db".into(),
        backup: true,
    };

    let result = destroy_agent(home, &options).await.unwrap();
    let backup_path = result.backup_path.expect("backup path must be recorded");
    let backup_conn = right_db::open_database_path_readonly(backup_path.join("data.db"))
        .await
        .expect("backup database must be readable");
    let count: i64 = backup_conn
        .query_row("SELECT COUNT(*) FROM auth_tokens", (), |row| row.get(0))
        .await
        .unwrap();

    assert_eq!(count, 1);
}

#[tokio::test]
async fn destroy_with_backup_copies_allowlist_yaml() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-allowlist");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("backup-allowlist");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();
    let allowlist = "\
version: 1
users:
  - id: 111
    label: alice
    added_by: null
    added_at: 2026-05-16T12:00:00Z
groups:
  - id: -222
    label: ops
    opened_by: null
    opened_at: 2026-05-16T12:00:00Z
";
    std::fs::write(agents_dir.join("allowlist.yaml"), allowlist).unwrap();

    let options = DestroyOptions {
        agent_name: "backup-allowlist".into(),
        backup: true,
    };

    let result = destroy_agent(home, &options).await.unwrap();
    let backup_path = result.backup_path.expect("backup path must be recorded");

    assert_eq!(
        std::fs::read_to_string(backup_path.join("allowlist.yaml")).unwrap(),
        allowlist,
        "pre-destroy backup must preserve allowlist.yaml outside sandbox.tar.gz"
    );
}

/// A destroy that was asked for a backup and could not produce one must stop
/// before it deletes anything. `run_backup` is fatal end to end for exactly
/// this reason: the earlier behaviour degraded to a partial backup and then
/// destroyed the agent anyway.
///
/// The failure is forced on the host side (a regular file where the backup
/// directory must be created) because a unit test has no live sandbox to fail
/// against; the abort path it exercises is the same `?` every sandbox-archive
/// failure takes.
#[tokio::test]
async fn destroy_with_backup_aborts_before_destructive_steps_when_backup_fails() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-fails");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let sandbox = unique_sandbox_name("backup-fails");
    std::fs::write(
        agents_dir.join("agent.yaml"),
        format!("sandbox:\n  name: {sandbox}\n"),
    )
    .unwrap();
    std::fs::write(agents_dir.join("IDENTITY.md"), "# irreplaceable").unwrap();

    // `backups_dir(home, agent)` is `<home>/backups/<agent>`; a regular file
    // there makes `create_dir_all` fail.
    std::fs::create_dir_all(home.join("backups")).unwrap();
    std::fs::write(home.join("backups").join("backup-fails"), "not a dir").unwrap();

    let options = DestroyOptions {
        agent_name: "backup-fails".into(),
        backup: true,
    };

    let error = destroy_agent(home, &options)
        .await
        .expect_err("a backup that cannot be written must fail the destroy");
    assert!(
        format!("{error:#}").contains("backup dir"),
        "error must name the failed backup step, got: {error:#}"
    );

    assert!(
        agents_dir.exists(),
        "agent directory must survive a failed backup"
    );
    assert_eq!(
        std::fs::read_to_string(agents_dir.join("IDENTITY.md")).unwrap(),
        "# irreplaceable",
        "agent data must be untouched after a failed backup"
    );
}

/// Guards that `sandbox.providers` in `agent.yaml` parses correctly for
/// both built-in and generic entries. This is the property a backup/restore
/// cycle depends on: the field must not be silently dropped when the YAML
/// is written to a backup tarball and re-read on restore.
#[test]
fn sandbox_providers_round_trip_parse() {
    let yaml = r#"
sandbox:
  providers:
    - name: foo-anthropic
      type: anthropic
      label: anthropic
    - name: foo-acme
      type: generic
      label: acme
      generic:
        env_var: ACME_TOKEN
        header_name: X-Acme-Token
        upstream_host: api.acme.com
        upstream_path_prefix: /v1
"#;
    // Parse once — both entries must be present.
    let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let sandbox = cfg.sandbox.as_ref().expect("sandbox must be present");
    assert_eq!(
        sandbox.providers.len(),
        2,
        "expected 2 providers after parse"
    );
    assert_eq!(sandbox.providers[0].name, "foo-anthropic");
    assert_eq!(sandbox.providers[1].name, "foo-acme");

    // Parse again from the same source — simulates reading the backed-up agent.yaml.
    // AgentConfig does not derive Serialize so we re-parse the original YAML string;
    // this is identical to what backup/restore does (copy the file, re-read it).
    let reparsed: right_agent_config::AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    let reparsed_sandbox = reparsed.sandbox.expect("sandbox must survive re-parse");
    assert_eq!(
        reparsed_sandbox.providers.len(),
        2,
        "providers must survive backup/restore re-parse"
    );
    assert_eq!(reparsed_sandbox.providers[0].name, "foo-anthropic");
    assert_eq!(reparsed_sandbox.providers[1].name, "foo-acme");

    // Verify generic entry fields survived.
    let generic = reparsed_sandbox.providers[1]
        .generic
        .as_ref()
        .expect("second provider must have generic config");
    assert_eq!(generic.env_var, "ACME_TOKEN");
    assert_eq!(generic.upstream_hosts, vec!["api.acme.com".to_string()]);
}
