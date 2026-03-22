use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn rightclaw() -> Command {
    Command::cargo_bin("rightclaw").unwrap()
}

#[test]
fn test_help_output() {
    rightclaw()
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

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    assert!(dir.path().join("agents/right/IDENTITY.md").exists());
    assert!(dir.path().join("agents/right/policy.yaml").exists());
    assert!(dir.path().join("agents/right/SOUL.md").exists());
    assert!(dir.path().join("agents/right/AGENTS.md").exists());
}

#[test]
fn test_init_twice_fails() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn test_list_after_init() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    rightclaw()
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

    rightclaw()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No agents found"));
}

#[test]
fn test_list_no_agents_dir() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rightclaw init"));
}

// --- Phase 2 Plan 03: New subcommand tests ---

#[test]
fn test_help_shows_new_subcommands() {
    rightclaw()
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
    rightclaw()
        .args(["up", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--agents"))
        .stdout(predicate::str::contains("--detach"))
        .stdout(predicate::str::contains("--no-sandbox"));
}

#[test]
fn test_down_help() {
    rightclaw()
        .args(["down", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stop all agents"));
}

#[test]
fn test_status_help() {
    rightclaw()
        .args(["status", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Show running agent status"));
}

#[test]
fn test_restart_help() {
    rightclaw()
        .args(["restart", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent"));
}

#[test]
fn test_attach_help() {
    rightclaw()
        .args(["attach", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Attach to running"));
}

#[test]
fn test_status_no_running_instance() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create run dir but no socket -- simulates no running instance.
    fs::create_dir_all(dir.path().join("run")).unwrap();

    rightclaw()
        .args(["--home", home, "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No running instance"));
}

#[test]
fn test_down_no_state_file() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create run dir but no state.json -- simulates no running instance.
    fs::create_dir_all(dir.path().join("run")).unwrap();

    rightclaw()
        .args(["--home", home, "down"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No running instance"));
}
