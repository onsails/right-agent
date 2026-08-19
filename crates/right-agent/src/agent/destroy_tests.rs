use super::*;

fn entry(n: &str) -> right_agent_config::ProviderEntry {
    right_agent_config::ProviderEntry {
        name: n.to_string(),
        type_: right_agent_config::ProviderType::BuiltIn("right-fal".into()),
        label: None,
        generic: None,
    }
}

#[test]
fn refcount_keeps_record_when_another_agent_references_it() {
    let agents = vec![
        ("agent-a".to_string(), vec![entry("fal-a1b2c3")]),
        ("right".to_string(), vec![entry("fal-a1b2c3")]),
    ];
    let plan = plan_destroy_provider_cascade("agent-a", &agents, true);
    assert!(plan.detach.contains(&"fal-a1b2c3".to_string()));
    assert!(
        !plan.delete.contains(&"fal-a1b2c3".to_string()),
        "still referenced by right"
    );
}

#[test]
fn refcount_deletes_record_when_last_reference() {
    let agents = vec![("agent-a".to_string(), vec![entry("fal-a1b2c3")])];
    let plan = plan_destroy_provider_cascade("agent-a", &agents, true);
    assert!(plan.delete.contains(&"fal-a1b2c3".to_string()));
}

#[test]
fn refcount_fails_closed_when_siblings_incomplete() {
    // When sibling enumeration was incomplete (all_complete=false), the
    // cascade must NOT delete gateway records — only detach.
    // This prevents deleting a record still referenced by an unread agent.
    let agents = vec![("agent-a".to_string(), vec![entry("fal-a1b2c3")])];
    let plan = plan_destroy_provider_cascade("agent-a", &agents, false);
    assert!(
        plan.detach.contains(&"fal-a1b2c3".to_string()),
        "detach must still be populated"
    );
    assert!(
        plan.delete.is_empty(),
        "delete must be empty when siblings incomplete"
    );
}

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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  name: right-test-agent\n").unwrap();

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
        result.sandbox_deleted,
        "every agent is sandboxed: deletion is always attempted"
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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  name: right-isolated\n").unwrap();

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
    std::fs::write(
        agents_dir.join("agent.yaml"),
        "sandbox:\n  name: right-backup-test\n",
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
    // The sandbox is unreachable in tests, so `sandbox.tar.gz` is absent and
    // the backup degrades to the config-file copies.
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
// way to reach it. Sandbox backups come down as `sandbox.tar.gz` over SSH and
// never tar the host agent directory.

#[tokio::test]
async fn destroy_with_backup_vacuum_copies_data_db() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-db");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox: {}\n").unwrap();
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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox: {}\n").unwrap();
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
