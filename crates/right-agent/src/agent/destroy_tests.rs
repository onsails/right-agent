use super::*;

fn owned(n: &str) -> right_agent_config::ProviderEntry {
    right_agent_config::ProviderEntry {
        name: n.to_string(),
        type_: right_agent_config::ProviderType::BuiltIn("right-fal".into()),
        label: None,
        generic: None,
        shared_from: None,
    }
}

fn borrowed(n: &str, from: &str) -> right_agent_config::ProviderEntry {
    right_agent_config::ProviderEntry {
        name: n.to_string(),
        type_: right_agent_config::ProviderType::BuiltIn("right-fal".into()),
        label: None,
        generic: None,
        shared_from: Some(from.to_string()),
    }
}

#[test]
fn refcount_keeps_record_when_borrower_remains() {
    let agents = vec![
        ("riskoff".to_string(), vec![owned("fal-a1b2c3")]),
        ("right".to_string(), vec![borrowed("fal-a1b2c3", "riskoff")]),
    ];
    let plan = plan_destroy_provider_cascade("riskoff", &agents, true);
    assert!(plan.detach.contains(&"fal-a1b2c3".to_string()));
    assert!(
        !plan.delete.contains(&"fal-a1b2c3".to_string()),
        "still referenced by right"
    );
    assert_eq!(
        plan.rehome_owner_to.get("fal-a1b2c3").map(String::as_str),
        Some("right")
    );
}

#[test]
fn refcount_deletes_record_when_last_reference() {
    let agents = vec![("riskoff".to_string(), vec![owned("fal-a1b2c3")])];
    let plan = plan_destroy_provider_cascade("riskoff", &agents, true);
    assert!(plan.delete.contains(&"fal-a1b2c3".to_string()));
    assert!(plan.rehome_owner_to.is_empty());
}

#[test]
fn refcount_borrower_delete_keeps_record_no_rehome() {
    // Deleting a BORROWER (right) while the owner (riskoff) survives: record kept,
    // NO re-home (right didn't own it).
    let agents = vec![
        ("riskoff".to_string(), vec![owned("fal-a1b2c3")]),
        ("right".to_string(), vec![borrowed("fal-a1b2c3", "riskoff")]),
    ];
    let plan = plan_destroy_provider_cascade("right", &agents, true);
    assert!(plan.detach.contains(&"fal-a1b2c3".to_string()));
    assert!(!plan.delete.contains(&"fal-a1b2c3".to_string()));
    assert!(plan.rehome_owner_to.is_empty());
}

#[test]
fn refcount_fails_closed_when_siblings_incomplete() {
    // When sibling enumeration was incomplete (all_complete=false), the
    // cascade must NOT delete or re-home gateway records — only detach.
    // This prevents deleting a record still referenced by an unread agent.
    let agents = vec![("riskoff".to_string(), vec![owned("fal-a1b2c3")])];
    let plan = plan_destroy_provider_cascade("riskoff", &agents, false);
    assert!(
        plan.detach.contains(&"fal-a1b2c3".to_string()),
        "detach must still be populated"
    );
    assert!(
        plan.delete.is_empty(),
        "delete must be empty when siblings incomplete"
    );
    assert!(
        plan.rehome_owner_to.is_empty(),
        "rehome must be empty when siblings incomplete"
    );
}

#[test]
fn set_provider_shared_from_clears_and_repoints() {
    let yaml = "sandbox:\n  mode: openshell\n  providers:\n    - name: 'fal-a1b2c3'\n      type: 'right-fal'\n      shared_from: 'riskoff'\n";
    // clear → becomes owned
    let cleared = set_provider_shared_from(yaml, "fal-a1b2c3", None);
    assert!(!cleared.contains("shared_from"), "got: {cleared}");
    // cleared entry round-trips back to an OWNED ProviderEntry
    let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&cleared).unwrap();
    let e = cfg
        .sandbox
        .unwrap()
        .providers
        .into_iter()
        .find(|p| p.name == "fal-a1b2c3")
        .unwrap();
    assert!(e.shared_from.is_none(), "cleared entry must be owned");
    // repoint
    let repointed = set_provider_shared_from(yaml, "fal-a1b2c3", Some("right"));
    assert!(
        repointed.contains("shared_from: 'right'"),
        "got: {repointed}"
    );
    // round-trips through the real parser back to a ProviderEntry
    let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&repointed).unwrap();
    let e = cfg
        .sandbox
        .unwrap()
        .providers
        .into_iter()
        .find(|p| p.name == "fal-a1b2c3")
        .unwrap();
    assert_eq!(e.shared_from.as_deref(), Some("right"));
}

#[test]
fn set_provider_shared_from_inserts_when_absent_and_leaves_other_blocks() {
    let yaml = "sandbox:\n  providers:\n    - name: 'a-1'\n      type: 'right-fal'\n    - name: 'b-2'\n      type: 'right-fal'\n";
    let out = set_provider_shared_from(yaml, "b-2", Some("riskoff"));
    let cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&out).unwrap();
    let b = cfg
        .sandbox
        .unwrap()
        .providers
        .into_iter()
        .find(|p| p.name == "b-2")
        .unwrap();
    assert_eq!(b.shared_from.as_deref(), Some("riskoff"));
    // unrelated block untouched
    assert!(out.contains("- name: 'a-1'"));
}

/// Filesystem-level re-home: the new owner's borrowed entry becomes owned,
/// and every OTHER surviving borrower that pointed at the deleted owner is
/// repointed to the new owner. Pure (tempdir, no gateway).
#[test]
fn rehome_owner_in_agent_yaml_clears_new_owner_and_repoints_others() {
    let dir = tempfile::TempDir::new().unwrap();
    let agents_dir = dir.path().join("agents");

    // `right` borrows fal-a1b2c3 from `riskoff` (the deleted owner).
    let right_dir = agents_dir.join("right");
    std::fs::create_dir_all(&right_dir).unwrap();
    std::fs::write(
        right_dir.join("agent.yaml"),
        "sandbox:\n  mode: openshell\n  providers:\n    - name: 'fal-a1b2c3'\n      type: 'right-fal'\n      shared_from: 'riskoff'\n",
    )
    .unwrap();

    // `third` also borrows fal-a1b2c3 from `riskoff`.
    let third_dir = agents_dir.join("third");
    std::fs::create_dir_all(&third_dir).unwrap();
    std::fs::write(
        third_dir.join("agent.yaml"),
        "sandbox:\n  mode: openshell\n  providers:\n    - name: 'fal-a1b2c3'\n      type: 'right-fal'\n      shared_from: 'riskoff'\n",
    )
    .unwrap();

    let agents = vec![
        ("right".to_string(), vec![borrowed("fal-a1b2c3", "riskoff")]),
        ("third".to_string(), vec![borrowed("fal-a1b2c3", "riskoff")]),
    ];

    // Re-home from deleted owner `riskoff` to new owner `right`.
    rehome_owner_in_agent_yaml(&agents_dir, "fal-a1b2c3", "riskoff", "right", &agents);

    // New owner `right` is now OWNED (shared_from cleared).
    let right_yaml = std::fs::read_to_string(right_dir.join("agent.yaml")).unwrap();
    let right_cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&right_yaml).unwrap();
    let right_entry = right_cfg
        .sandbox
        .unwrap()
        .providers
        .into_iter()
        .find(|p| p.name == "fal-a1b2c3")
        .unwrap();
    assert!(
        right_entry.shared_from.is_none(),
        "new owner must be owned; got: {right_yaml}"
    );

    // Other borrower `third` is repointed to the new owner `right`.
    let third_yaml = std::fs::read_to_string(third_dir.join("agent.yaml")).unwrap();
    let third_cfg: right_agent_config::AgentConfig = serde_saphyr::from_str(&third_yaml).unwrap();
    let third_entry = third_cfg
        .sandbox
        .unwrap()
        .providers
        .into_iter()
        .find(|p| p.name == "fal-a1b2c3")
        .unwrap();
    assert_eq!(
        third_entry.shared_from.as_deref(),
        Some("right"),
        "other borrower must be repointed; got: {third_yaml}"
    );
}

fn tar_entries(path: &Path) -> Vec<String> {
    let output = std::process::Command::new("tar")
        .args(["-tzf", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tar -tzf failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn destroy_nonsandboxed_agent_removes_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("test-agent");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

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
        "non-sandboxed agent, no sandbox to delete"
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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
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
    assert!(
        backup_path.join("sandbox.tar.gz").exists(),
        "sandbox.tar.gz should exist"
    );
    assert!(
        result.dir_removed,
        "agent dir should be removed after backup"
    );
}

#[tokio::test]
async fn destroy_with_backup_excludes_database_sidecars_from_no_sandbox_tar() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-sidecars");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    std::fs::write(agents_dir.join("notes.txt"), "keep me").unwrap();
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        std::fs::write(agents_dir.join(sidecar), sidecar).unwrap();
    }

    let options = DestroyOptions {
        agent_name: "backup-sidecars".into(),
        backup: true,
    };

    let result = destroy_agent(home, &options).await.unwrap();
    let backup_path = result.backup_path.expect("backup path must be recorded");
    let entries = tar_entries(&backup_path.join("sandbox.tar.gz"));

    assert!(
        entries.contains(&"backup-sidecars/notes.txt".to_string()),
        "regular no-sandbox files should still be archived"
    );
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        assert!(
            !entries.contains(&format!("backup-sidecars/{sidecar}")),
            "pre-destroy no-sandbox backup tar must not contain database sidecar {sidecar}"
        );
    }
}

#[tokio::test]
async fn destroy_with_backup_vacuum_copies_data_db() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();

    let agents_dir = home.join("agents").join("backup-db");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
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
    std::fs::write(agents_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
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
  mode: none
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
