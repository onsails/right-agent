use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn right() -> Command {
    Command::cargo_bin("right").unwrap()
}

/// Minimal valid global config — tunnel is mandatory after the webhooks cutover,
/// so any test that creates a home + config.yaml manually must include a tunnel
/// block. Used by tests that don't go through `right init`.
fn minimal_config_yaml(home: &std::path::Path) -> String {
    let creds = home.join("test-creds.json");
    fs::write(&creds, "{}").unwrap();
    format!(
        "tunnel:\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
        creds.display()
    )
}

#[test]
fn test_help_output() {
    right()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Multi-agent runtime"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("list"));
}

#[test]
fn test_init_creates_structure() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    // Identity files are NOT created by init — bootstrap creates them.
    assert!(!dir.path().join("agents/right/IDENTITY.md").exists());
    assert!(!dir.path().join("agents/right/SOUL.md").exists());
    assert!(dir.path().join("agents/right/AGENTS.md").exists());
    assert!(dir.path().join("agents/right/BOOTSTRAP.md").exists());
}

#[test]
fn test_init_generates_per_agent_codegen() {
    // Regression: 59243d0 moved per-agent codegen to bot startup but init creates
    // the sandbox directly. Without run_single_agent_codegen, agent defs and schemas
    // are missing when prepare_staging_dir runs, causing sandbox upload to fail.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--sandbox-mode",
            "none",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success();

    let claude_dir = dir.path().join("agents/right/.claude");

    // AGENTS.md and TOOLS.md live at agent root
    assert!(
        dir.path().join("agents/right/AGENTS.md").exists(),
        "missing AGENTS.md at agent root"
    );
    assert!(
        dir.path().join("agents/right/TOOLS.md").exists(),
        "missing TOOLS.md at agent root"
    );

    // Schema and prompt files
    assert!(
        claude_dir.join("system-prompt.md").exists(),
        "missing .claude/system-prompt.md"
    );
    assert!(
        claude_dir.join("reply-schema.json").exists(),
        "missing .claude/reply-schema.json"
    );
    assert!(
        claude_dir.join("cron-schema.json").exists(),
        "missing .claude/cron-schema.json"
    );
    assert!(
        claude_dir.join("bootstrap-schema.json").exists(),
        "missing .claude/bootstrap-schema.json"
    );

    // MCP config and memory database
    assert!(
        dir.path().join("agents/right/mcp.json").exists(),
        "missing mcp.json"
    );
    assert!(
        dir.path().join("agents/right/data.db").exists(),
        "missing data.db"
    );
}

#[test]
fn test_init_twice_fails() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn test_list_after_init() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    right()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("right"))
        .stdout(predicate::str::contains("1 agent"));
}

#[test]
fn test_list_empty() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();
    fs::create_dir(dir.path().join("agents")).unwrap();

    right()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No agents found"));
}

#[test]
fn test_list_no_agents_dir() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("right init"));
}

// --- Phase 3 Plan 04: Doctor and Init --telegram-token tests ---

#[test]
fn test_help_shows_doctor() {
    right()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("doctor"));
}

#[test]
fn test_doctor_in_valid_home() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Initialize first so agent structure exists.
    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    // Doctor should report the valid agent.
    right()
        .args(["--home", home, "doctor"])
        .assert()
        // May still fail overall (process-compose not in PATH)
        // but should contain the agent check.
        .stdout(predicate::str::contains("agents/right/"));
}

#[test]
fn test_doctor_missing_agents_dir() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args(["--home", home, "doctor"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("agents/"));
}

#[test]
fn test_init_with_telegram_token() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
            "--telegram-token",
            "123456:ABCdef",
            "--telegram-allowed-chat-ids",
            "12345678,100200300",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Telegram"));

    // Verify agent was created.
    assert!(dir.path().join("agents/right/BOOTSTRAP.md").exists());

    // Verify allowed_chat_ids written to agent.yaml
    let yaml = fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
    assert!(
        yaml.contains("allowed_chat_ids:"),
        "agent.yaml must contain allowed_chat_ids section, got:\n{yaml}"
    );
}

#[test]
fn test_init_with_invalid_telegram_token() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args(["--home", home, "init", "--telegram-token", "invalid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid Telegram bot token"));
}

#[test]
fn test_init_help_shows_telegram_token_flag() {
    right()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--telegram-token"));
}

// --- Phase 2 Plan 03: New subcommand tests ---

#[test]
fn test_help_shows_new_subcommands() {
    right()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("up"))
        .stdout(predicate::str::contains("down"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("restart"))
        .stdout(predicate::str::contains("attach"));
}

#[test]
fn test_up_help_shows_new_flags() {
    right()
        .args(["up", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--agents"))
        .stdout(predicate::str::contains("--detach"))
        .stdout(predicate::str::contains("--debug"));
}

#[test]
fn test_down_help() {
    right()
        .args(["down", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stop all agents"));
}

#[test]
fn test_status_help() {
    right()
        .args(["status", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Show running agent status"));
}

#[test]
fn test_restart_help() {
    right()
        .args(["restart", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent"));
}

#[test]
fn test_attach_help() {
    right()
        .args(["attach", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Attach to running"));
}

/// Requires no right instance running (port 18927 must be free).
#[test]
#[ignore = "requires no running right instance on port 18927"]
fn test_status_no_running_instance() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create run dir but no socket -- simulates no running instance.
    fs::create_dir_all(dir.path().join("run")).unwrap();

    right()
        .args(["--home", home, "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No running instance"));
}

/// Requires no right instance running (port 18927 must be free).
#[test]
#[ignore = "requires no running right instance on port 18927"]
fn test_down_no_state_file() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create run dir but no state.json -- simulates no running instance.
    fs::create_dir_all(dir.path().join("run")).unwrap();

    right()
        .args(["--home", home, "down"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No running instance"));
}

#[test]
fn test_init_yes_no_telegram_prompt() {
    // Regression for UAT gap: `right init -y` must not block on stdin
    // waiting for a Telegram token when --telegram-token is omitted.
    // cert.pem is absent in CI so the tunnel section is skipped;
    // the only previously-blocking call was prompt_telegram_token().
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "example.com",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();
}

#[test]
fn test_init_always_writes_config() {
    // D-11: config.yaml must be written even when no cloudflared cert detected.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Use -y to avoid interactive prompts (inquire requires TTY).
    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
            "--sandbox-mode",
            "none",
            "--telegram-token",
            "123456:ABCdef",
        ])
        .assert()
        .success();

    assert!(
        dir.path().join("config.yaml").exists(),
        "config.yaml must exist after init even with no tunnel"
    );
}

// --- Task 5: Reload integration tests ---

#[test]
#[ignore = "requires no running right instance on port 18927"]
fn reload_fails_when_not_running() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal agent structure so discovery doesn't fail first.
    let agent_dir = dir.path().join("agents").join("test-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nsandbox:\n  mode: none\n",
    )
    .unwrap();

    right()
        .args(["--home", home, "reload"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nothing running"));
}

#[test]
fn agent_init_recap_suggests_right_up() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();
    std::fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-bot",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("right up"));
}

// --- Task 2: --force and --fresh flag tests ---

#[test]
fn test_agent_init_force_recreates_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    // Create agent.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    // Write a marker file in the agent dir.
    let marker = dir.path().join("agents/test-agent/MARKER.txt");
    fs::write(&marker, "canary").unwrap();
    assert!(marker.exists());

    // Re-init with --force-recreate.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force-recreate",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    // Agent dir exists, MARKER.txt is gone, agent.yaml exists.
    assert!(dir.path().join("agents/test-agent").exists());
    assert!(
        !marker.exists(),
        "MARKER.txt should be wiped by --force-recreate"
    );
    assert!(dir.path().join("agents/test-agent/agent.yaml").exists());
}

#[test]
fn test_agent_init_fresh_without_force_recreate_errors() {
    right()
        .args([
            "--home",
            "/tmp/doesnt-matter",
            "agent",
            "init",
            "test-agent",
            "--fresh",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--force-recreate"));
}

#[test]
fn test_agent_init_force_recreate_preserves_config() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    // Create agent with specific config.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "preserve-test",
            "-y",
            "--sandbox-mode",
            "none",
            "--network-policy",
            "permissive",
        ])
        .assert()
        .success();

    // Re-init with --force-recreate (no --fresh) — should preserve config.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "preserve-test",
            "--force-recreate",
            "-y",
        ])
        .assert()
        .success();

    let yaml = fs::read_to_string(dir.path().join("agents/preserve-test/agent.yaml")).unwrap();
    assert!(
        yaml.contains("mode: none"),
        "agent.yaml should preserve sandbox mode: none after --force-recreate, got:\n{yaml}"
    );
}

#[test]
fn test_agent_init_force_recreate_on_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    // --force-recreate on non-existent agent should just create it.
    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "new-agent",
            "--force-recreate",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .success();

    assert!(dir.path().join("agents/new-agent/agent.yaml").exists());
}

// --- Agent SSH regression tests ---

/// Regression: cmd_agent_ssh must discover agents correctly.
/// Previously it passed `home` instead of `home/agents` to discover_agents,
/// so no agents were ever found.
#[test]
fn test_agent_ssh_finds_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal agent structure with openshell sandbox.
    let agent_dir = dir.path().join("agents").join("test-agent");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nsandbox:\n  mode: openshell\n",
    )
    .unwrap();

    // SSH should fail because process-compose isn't running — but NOT because
    // the agent wasn't found. The old bug would give "Agent 'test-agent' not found".
    right()
        .args(["--home", home, "agent", "ssh", "test-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not running").or(predicate::str::contains("SSH config")))
        .stderr(predicate::str::contains("not found").not());
}

/// Agent SSH must reject agents without openshell sandbox.
#[test]
fn test_agent_ssh_rejects_no_sandbox() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    let agent_dir = dir.path().join("agents").join("local-agent");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nsandbox:\n  mode: none\n",
    )
    .unwrap();

    right()
        .args(["--home", home, "agent", "ssh", "local-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("without sandbox"));
}

/// `right agent list` should work the same as `right list`.
#[test]
fn test_agent_list() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create agent directory manually (avoids sandbox creation side effects).
    let agent_dir = dir.path().join("agents").join("myagent");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nsandbox:\n  mode: none\n",
    )
    .unwrap();

    right()
        .args(["--home", home, "agent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myagent"))
        .stdout(predicate::str::contains("1 agent"));
}

/// Validate generated OpenShell policy against a live sandbox.
/// Creates an ephemeral sandbox via `ensure_sandbox`, applies the policy, then destroys it.
#[tokio::test]
async fn test_policy_validates_against_openshell() {
    let _slot = right_agent::openshell::acquire_sandbox_slot();
    let mtls_dir = match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let sandbox_name = "right-test-policy-validate";

    right_agent::test_cleanup::pkill_test_orphans(sandbox_name);
    right_agent::test_cleanup::register_test_sandbox(sandbox_name);

    // Clean up leftover from a previous failed run.
    let mut client = right_agent::openshell::connect_grpc(&mtls_dir).await.unwrap();
    if right_agent::openshell::sandbox_exists(&mut client, sandbox_name)
        .await
        .unwrap()
    {
        right_agent::openshell::delete_sandbox(sandbox_name).await;
        right_agent::openshell::wait_for_deleted(&mut client, sandbox_name, 60, 2)
            .await
            .expect("cleanup of leftover sandbox failed");
    }

    // Generate the policy under test.
    let policy_yaml = right_agent::codegen::policy::generate_policy(
        right_agent::runtime::MCP_HTTP_PORT,
        &right_agent::agent::types::NetworkPolicy::Permissive,
        None,
    );
    let tmpdir = tempdir().unwrap();
    let policy_path = tmpdir.path().join("test-policy.yaml");
    fs::write(&policy_path, &policy_yaml).unwrap();

    // Create sandbox with the generated policy — this validates the YAML is accepted.
    let mut child = right_agent::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    let ready = right_agent::openshell::wait_for_ready(&mut client, sandbox_name, 120, 2).await;
    let _ = child.kill().await;

    // Cleanup regardless of outcome.
    right_agent::openshell::delete_sandbox(sandbox_name).await;
    right_agent::test_cleanup::unregister_test_sandbox(sandbox_name);

    ready.expect("sandbox did not become READY — generated policy may be invalid");
}

// --- Task 9: No-sandbox backup and restore integration tests ---

#[test]
fn test_agent_backup_and_restore_no_sandbox() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    // Set up a no-sandbox agent manually.
    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  mode: none\nnetwork_policy: permissive\n",
    )
    .unwrap();
    fs::write(
        agent_dir.join("IDENTITY.md"),
        "# Test Agent\nI am a test agent.\n",
    )
    .unwrap();
    fs::write(agent_dir.join("AGENTS.md"), "# Agents\n").unwrap();
    fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();
    fs::write(agent_dir.join("test-file.txt"), "hello world\n").unwrap();

    // Create a data.db with a test table.
    let db_path = agent_dir.join("data.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT)", [])
        .unwrap();
    conn.execute("INSERT INTO test (val) VALUES ('backup-test')", [])
        .unwrap();
    drop(conn);

    // Run backup.
    right()
        .args(["--home", home_str, "agent", "backup", "test-agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox.tar.gz"))
        .stdout(predicate::str::contains("agent.yaml"))
        .stdout(predicate::str::contains("data.db"));

    // Find backup directory.
    let backups_dir = home.path().join("backups").join("test-agent");
    assert!(backups_dir.exists(), "backups dir should exist");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();

    // Verify backup contents.
    assert!(
        backup_dir.join("sandbox.tar.gz").exists(),
        "should have sandbox.tar.gz"
    );
    assert!(
        backup_dir.join("agent.yaml").exists(),
        "should have agent.yaml"
    );
    assert!(backup_dir.join("data.db").exists(), "should have data.db");

    // Delete original agent.
    fs::remove_dir_all(&agent_dir).unwrap();
    assert!(!agent_dir.exists());

    // Restore to new agent name via agent init --from-backup.
    // Needs agents dir and config.yaml to exist (home structure).
    fs::write(home.path().join("config.yaml"), minimal_config_yaml(home.path())).unwrap();

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "init",
            "restored-agent",
            "--from-backup",
            backup_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restored"));

    // Verify restored files.
    let restored_dir = home.path().join("agents").join("restored-agent");
    assert!(restored_dir.exists(), "restored agent dir should exist");
    assert!(
        restored_dir.join("agent.yaml").exists(),
        "should have agent.yaml"
    );

    // Verify the test file was restored from tar (--strip-components=1 used during extraction).
    assert!(
        restored_dir.join("test-file.txt").exists(),
        "test-file.txt should be restored"
    );
    assert_eq!(
        fs::read_to_string(restored_dir.join("test-file.txt")).unwrap(),
        "hello world\n"
    );

    // Verify restored database.
    let restored_db = rusqlite::Connection::open(restored_dir.join("data.db")).unwrap();
    let val: String = restored_db
        .query_row("SELECT val FROM test WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(val, "backup-test");
}

#[test]
fn test_agent_backup_sandbox_only() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# Test\n").unwrap();
    fs::write(agent_dir.join("AGENTS.md"), "# Agents\n").unwrap();

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "backup",
            "test-agent",
            "--sandbox-only",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox.tar.gz"));

    let backups_dir = home.path().join("backups").join("test-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();

    assert!(backup_dir.join("sandbox.tar.gz").exists());
    assert!(
        !backup_dir.join("agent.yaml").exists(),
        "sandbox-only should not have agent.yaml"
    );
    assert!(
        !backup_dir.join("data.db").exists(),
        "sandbox-only should not have data.db"
    );
}

#[test]
fn test_agent_restore_fails_if_agent_exists() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    // Create existing agent.
    let agent_dir = home.path().join("agents").join("existing");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

    // Create a fake backup dir.
    let backup_dir = home.path().join("fake-backup");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(backup_dir.join("sandbox.tar.gz"), "fake").unwrap();
    fs::write(backup_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "init",
            "existing",
            "--from-backup",
            backup_dir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

// --- Task 5: Agent destroy integration tests ---

#[test]
fn test_destroy_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create home structure but no agent
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();

    right()
        .args(["--home", home, "agent", "destroy", "nonexistent", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn test_destroy_agent_force() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create an agent via init first
    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--sandbox-mode",
            "none",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success();

    // Verify agent exists
    assert!(dir.path().join("agents/right").exists());

    // Destroy with --force (no TTY prompts)
    right()
        .args(["--home", home, "agent", "destroy", "right", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Destroyed agent"));

    // Verify agent directory is gone
    assert!(!dir.path().join("agents/right").exists());
}

#[test]
fn test_destroy_agent_force_with_backup() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--sandbox-mode",
            "none",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success();

    right()
        .args([
            "--home", home, "agent", "destroy", "right", "--force", "--backup",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup saved to"))
        .stdout(predicate::str::contains("Destroyed agent"));

    assert!(
        !dir.path().join("agents/right").exists(),
        "agent dir should be removed"
    );
    assert!(
        dir.path().join("backups/right").exists(),
        "backup dir should exist"
    );
}

#[test]
fn test_help_lists_destroy() {
    right()
        .args(["agent", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("destroy"));
}

#[test]
fn test_agent_init_bare_force_rejected() {
    // Agent init no longer accepts --force; only --force-recreate.
    // (`agent destroy --force` and `right init --force` are unchanged —
    // separate commands with separate semantics.)
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("config.yaml"), minimal_config_yaml(dir.path())).unwrap();

    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force",
            "-y",
            "--sandbox-mode",
            "none",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}
