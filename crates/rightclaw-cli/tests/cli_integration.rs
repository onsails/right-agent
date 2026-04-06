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
    assert!(dir.path().join("agents/right/SOUL.md").exists());
    assert!(dir.path().join("agents/right/AGENTS.md").exists());
    assert!(!dir.path().join("agents/right/policy.yaml").exists(), "policy.yaml should NOT be created");
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

// --- Phase 3 Plan 04: Doctor and Init --telegram-token tests ---

#[test]
fn test_help_shows_doctor() {
    rightclaw()
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
    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    // Doctor should report the valid agent.
    rightclaw()
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

    rightclaw()
        .args(["--home", home, "doctor"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("agents/"));
}

#[test]
fn test_init_with_telegram_token() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args([
            "--home", home,
            "init",
            "--telegram-token", "123456:ABCdef",
            "--telegram-allowed-chat-ids", "12345678,100200300",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Telegram"));

    // Verify agent was created (policy.yaml no longer created).
    assert!(dir.path().join("agents/right/IDENTITY.md").exists());
    assert!(!dir.path().join("agents/right/policy.yaml").exists(), "policy.yaml should NOT be created");

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

    rightclaw()
        .args(["--home", home, "init", "--telegram-token", "invalid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid Telegram bot token"));
}

#[test]
fn test_init_help_shows_telegram_token_flag() {
    rightclaw()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--telegram-token"));
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

#[test]
fn test_init_yes_no_telegram_prompt() {
    // Regression for UAT gap: `rightclaw init -y` must not block on stdin
    // waiting for a Telegram token when --telegram-token is omitted.
    // cert.pem is absent in CI so the tunnel section is skipped;
    // the only previously-blocking call was prompt_telegram_token().
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init", "-y", "--tunnel-hostname", "example.com"])
        .assert()
        .success();
}

// --- Phase 43 Plan 01: Chrome detection + single config write ---

#[test]
fn test_init_always_writes_config() {
    // D-11: config.yaml must be written even when no cloudflared cert and no Chrome detected.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Use --telegram-token to avoid interactive prompt; no cloudflared cert in tmpdir.
    rightclaw()
        .args(["--home", home, "init", "--telegram-token", "123456:ABCdef"])
        .assert()
        .success();

    assert!(
        dir.path().join("config.yaml").exists(),
        "config.yaml must exist after init even with no tunnel and no chrome"
    );
}

#[test]
fn test_init_chrome_path_arg_warns_when_mcp_missing() {
    // CHROME-02: --chrome-path override accepted; CHROME-03: missing MCP binary warns + skips.
    // /dev/null exists as a regular file on Linux — satisfies chrome binary existence check.
    // Use --telegram-token to avoid interactive stdin prompt.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args([
            "--home", home,
            "init",
            "--telegram-token", "123456:ABCdef",
            "--chrome-path", "/dev/null",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("chrome-devtools-mcp not found"));
}

// --- Phase 43 Plan 02: Per-run Chrome path revalidation in cmd_up ---

#[test]
fn test_up_warns_when_chrome_path_missing() {
    // INJECT-03: rightclaw up with chrome_path pointing to non-existent file must warn and skip
    // injection, not abort. The up command will fail after the warn (no process-compose in test
    // env), but the warn must appear before that failure.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Initialize first so agent structure exists.
    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    // Write a config.yaml with a chrome_path that doesn't exist on disk.
    let config_content = "chrome:\n  chrome_path: /nonexistent/chrome\n  mcp_binary_path: /nonexistent/mcp\n";
    fs::write(dir.path().join("config.yaml"), config_content).unwrap();

    // Run `up` — it will fail (no process-compose or already running), but the chrome warn
    // must appear first. tracing_subscriber writes to stdout by default (see main.rs init).
    let output = rightclaw()
        .args(["--home", home, "up"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("no longer exists"),
        "expected chrome path warn in stdout, got: {stdout}"
    );
}
