use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::LazyLock;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn right() -> Command {
    Command::cargo_bin("right").unwrap()
}

fn right_with_init_auth() -> Command {
    let mut command = right();
    command.env("RIGHT_CLAUDE_SETUP_TOKEN", TEST_CLAUDE_SETUP_TOKEN);
    prepend_path(&mut command, successful_claude_probe_dir());
    command
}

const TEST_CLAUDE_SETUP_TOKEN: &str = "test-claude-setup-token";

static SUCCESSFUL_CLAUDE_PROBE_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("right-cli-probe-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("claude");
    fs::write(
        &executable,
        "#!/bin/sh\nif [ \"${1:-}\" = \"--version\" ]; then printf '%s\\n' '2.1.0 (Claude Code)'; exit 0; fi\nprintf '%s\\n' \"$@\" > \"${RIGHT_TEST_CLAUDE_INVOCATION_LOG:-/dev/null}\"\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"apiKeySource\":\"none\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\"}'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    dir
});

fn successful_claude_probe_dir() -> &'static Path {
    SUCCESSFUL_CLAUDE_PROBE_DIR.as_path()
}

fn prepend_path(command: &mut Command, dir: &Path) {
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(paths).unwrap());
}

fn write_fake_claude_probe(dir: &Path, stdout: &str, exit_code: i32) {
    let executable = dir.join("claude");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nif [ \"${{1:-}}\" = \"--version\" ]; then printf '%s\\n' '2.1.0 (Claude Code)'; exit 0; fi\nprintf '%s\\n' '{}'\nexit {exit_code}\n",
            stdout.replace('\'', "'\\\\''")
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
}

/// Minimal valid external-tunnel config for tests that do not exercise
/// Right-owned cloudflared ingress.
fn minimal_config_yaml(_home: &std::path::Path) -> String {
    "tunnel:\n  provider: \"external\"\n  hostname: \"test.example.com\"\n".to_string()
}

fn write_fake_cloudflared(dir: &Path, invocation_log: &Path, exit_code: i32) {
    let executable = dir.join("cloudflared");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nexit {exit_code}\n",
            invocation_log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(executable, permissions).unwrap();
}
#[test]
fn agent_init_non_interactive_without_claude_token_leaves_no_agent_directory() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    right()
        .env_remove("RIGHT_CLAUDE_SETUP_TOKEN")
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "missing-auth",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("claude setup-token"));

    assert!(!home.path().join("agents/missing-auth").exists());
}

#[tokio::test]
async fn agent_init_persists_claude_token_supplied_via_env() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    let assertion = right_with_init_auth()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "env-auth",
            "-y",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains(TEST_CLAUDE_SETUP_TOKEN).not())
        .stderr(predicate::str::contains(TEST_CLAUDE_SETUP_TOKEN).not());
    drop(assertion);

    let agent_dir = home.path().join("agents/env-auth");
    let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
    let stored = right_mcp::credentials::get_auth_token(&conn).await.unwrap();
    assert_eq!(stored.as_deref(), Some(TEST_CLAUDE_SETUP_TOKEN));
}

#[test]
fn agent_init_missing_cloudflared_tunnel_leaves_no_agent_directory() {
    let home = tempdir().unwrap();
    let fake_bin = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    let invocation_log = home.path().join("cloudflared-invocation.txt");
    let credentials = home.path().join("missing-tunnel.json");
    fs::write(
        &credentials,
        r#"{"TunnelID":"00000000-0000-0000-0000-000000000000"}"#,
    )
    .unwrap();
    fs::write(
        home.path().join("config.yaml"),
        format!(
            "tunnel:\n  provider: \"cloudflared\"\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
            credentials.display()
        ),
    )
    .unwrap();
    write_fake_cloudflared(fake_bin.path(), &invocation_log, 1);
    let path = format!(
        "{}:{}",
        fake_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    right_with_init_auth()
        .env("PATH", path)
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "dead-ingress",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot access configured tunnel"));

    assert!(!home.path().join("agents/dead-ingress").exists());
    assert_eq!(
        fs::read_to_string(invocation_log).unwrap(),
        "tunnel\n--loglevel\nerror\ninfo\n--output\njson\n00000000-0000-0000-0000-000000000000\n"
    );
}

#[test]
fn agent_init_malformed_cloudflare_credentials_leaves_no_agent_directory() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    let credentials = home.path().join("tunnel.json");
    fs::write(&credentials, "not-json").unwrap();
    fs::write(
        home.path().join("config.yaml"),
        format!(
            "tunnel:\n  provider: \"cloudflared\"\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
            credentials.display()
        ),
    )
    .unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "bad-creds",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not valid"));
    assert!(!home.path().join("agents/bad-creds").exists());
}

#[test]
fn agent_init_mismatched_cloudflare_tunnel_id_leaves_no_agent_directory() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    let credentials = home.path().join("tunnel.json");
    fs::write(
        &credentials,
        r#"{"TunnelID":"11111111-1111-1111-1111-111111111111"}"#,
    )
    .unwrap();
    fs::write(
        home.path().join("config.yaml"),
        format!(
            "tunnel:\n  provider: \"cloudflared\"\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
            credentials.display()
        ),
    )
    .unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "wrong-tunnel",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("credentials identify"));
    assert!(!home.path().join("agents/wrong-tunnel").exists());
}
#[test]
fn top_level_init_tunnel_failure_leaves_no_agent_state() {
    let home = tempdir().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "init",
            "-y",
            "--tunnel-provider",
            "external",
            "--tunnel-hostname",
            "https://invalid.example.com",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("bare domain"));

    assert!(!home.path().join("agents/right").exists());
}

/// `agent init` stores the credential but must not claim it was verified:
/// the probe that verifies it runs inside the agent's sandbox, which only the
/// bot's supervisor creates. A broken `claude` on the *host* PATH is therefore
/// irrelevant to init — proving the host binary is never consulted.
#[test]
fn agent_init_stores_claude_token_without_claiming_validation() {
    let home = tempdir().unwrap();
    let fake_bin = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();
    write_fake_claude_probe(
        fake_bin.path(),
        r#"{"type":"result","subtype":"error","is_error":true,"result":"invalid token"}"#,
        1,
    );

    let mut command = right_with_init_auth();
    prepend_path(&mut command, fake_bin.path());
    command
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "invalid-auth",
            "-y",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("credential stored"))
        .stdout(predicate::str::contains("authenticated").not())
        .stdout(predicate::str::contains(
            "created and checked on first `right up`",
        ))
        .stdout(predicate::str::contains(TEST_CLAUDE_SETUP_TOKEN).not())
        .stderr(predicate::str::contains(TEST_CLAUDE_SETUP_TOKEN).not());

    assert!(
        home.path().join("agents/invalid-auth").exists(),
        "init completes without a sandbox; the credential is checked at first bring-up"
    );
}

fn write_minimal_no_sandbox_backup(home: &Path) -> PathBuf {
    let backup = home.join("backup");
    let source = home.join("source");
    fs::create_dir_all(&backup).unwrap();
    fs::create_dir_all(&source).unwrap();
    fs::write(
        backup.join("agent.yaml"),
        "sandbox:\n  name: right-test\nnetwork_policy: permissive\n",
    )
    .unwrap();
    fs::write(source.join("marker"), "backup").unwrap();
    assert!(
        StdCommand::new("tar")
            .args([
                "czpf",
                backup.join("sandbox.tar.gz").to_str().unwrap(),
                "-C",
                source.to_str().unwrap(),
                "marker",
            ])
            .status()
            .unwrap()
            .success()
    );
    backup
}

#[test]
fn agent_restore_tunnel_preflight_failure_leaves_no_agent_state() {
    let home = tempdir().unwrap();
    let fake_bin = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    let backup = write_minimal_no_sandbox_backup(home.path());
    let invocation_log = home.path().join("cloudflared-restore-invocation.txt");
    let credentials = home.path().join("tunnel.json");
    fs::write(
        &credentials,
        r#"{"TunnelID":"00000000-0000-0000-0000-000000000000"}"#,
    )
    .unwrap();
    fs::write(
        home.path().join("config.yaml"),
        format!(
            "tunnel:\n  provider: \"cloudflared\"\n  tunnel_uuid: \"00000000-0000-0000-0000-000000000000\"\n  credentials_file: \"{}\"\n  hostname: \"test.example.com\"\n",
            credentials.display()
        ),
    )
    .unwrap();
    write_fake_cloudflared(fake_bin.path(), &invocation_log, 1);
    fs::copy(
        successful_claude_probe_dir().join("claude"),
        fake_bin.path().join("claude"),
    )
    .unwrap();

    let mut command = right_with_init_auth();
    prepend_path(&mut command, fake_bin.path());
    command
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "restore-dead-ingress",
            "-y",
            "--from-backup",
            backup.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot access configured tunnel"));

    assert!(!home.path().join("agents/restore-dead-ingress").exists());
}

#[ignore = "ci-msb: the restore auth probe runs inside a live microVM"]
#[test]
fn ci_msb_agent_restore_invalid_claude_probe_does_not_render_restored() {
    let home = tempdir().unwrap();
    let fake_bin = tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();
    let backup = write_minimal_no_sandbox_backup(home.path());
    write_fake_claude_probe(
        fake_bin.path(),
        r#"{"type":"result","subtype":"error","is_error":true,"result":"invalid token"}"#,
        1,
    );

    let mut command = right_with_init_auth();
    prepend_path(&mut command, fake_bin.path());
    command
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "invalid-restore-auth",
            "-y",
            "--from-backup",
            backup.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("| restored ").not())
        .stderr(predicate::str::contains(
            "Claude authentication validation failed",
        ))
        .stderr(predicate::str::contains(TEST_CLAUDE_SETUP_TOKEN).not());

    assert!(!home.path().join("agents/invalid-restore-auth").exists());
}

#[test]
fn agent_restore_without_target_claude_token_leaves_no_agent_state() {
    let home = tempdir().unwrap();
    let backup = write_minimal_no_sandbox_backup(home.path());
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    right()
        .env_remove("RIGHT_CLAUDE_SETUP_TOKEN")
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "init",
            "restored-without-auth",
            "-y",
            "--from-backup",
            backup.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("claude setup-token"));

    assert!(!home.path().join("agents/restored-without-auth").exists());
}

fn tar_entries(path: &Path) -> Vec<String> {
    let output = StdCommand::new("tar")
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

#[tokio::test]
async fn init_smoke_generates_codegen_list_and_doctor() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success();

    // Identity files are NOT created by init — bootstrap creates them.
    assert!(!dir.path().join("agents/right/IDENTITY.md").exists());
    assert!(!dir.path().join("agents/right/SOUL.md").exists());
    assert!(dir.path().join("agents/right/BOOTSTRAP.md").exists());

    let claude_dir = dir.path().join("agents/right/.claude");

    // TOOLS.md lives at agent root
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

    // MCP config, memory database, and Claude authentication.
    assert!(
        dir.path().join("agents/right/mcp.json").exists(),
        "missing mcp.json"
    );
    assert!(
        dir.path().join("agents/right/data.db").exists(),
        "missing data.db"
    );
    let conn = right_db::open_connection(&dir.path().join("agents/right"), false)
        .await
        .unwrap();
    let stored = right_mcp::credentials::get_auth_token(&conn).await.unwrap();
    assert_eq!(stored.as_deref(), Some(TEST_CLAUDE_SETUP_TOKEN));

    right()
        .args(["--home", home, "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("right"))
        .stdout(predicate::str::contains("1 agent"));

    right()
        .args(["--home", home, "doctor"])
        .assert()
        // May still fail overall (process-compose not in PATH)
        // but should contain the agent check.
        .stdout(predicate::str::contains("agents/right/"));
}

#[test]
fn init_twice_fails() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
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
fn init_with_telegram_token() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
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

    right_with_init_auth()
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
fn init_yes_no_telegram_prompt() {
    // Regression for UAT gap: `right init -y` must not block on stdin
    // waiting for a Telegram token when --telegram-token is omitted.
    // cert.pem is absent in CI so the tunnel section is skipped;
    // the only previously-blocking call was prompt_telegram_token().
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "example.com",
        ])
        .assert()
        .success();
}

#[test]
fn init_always_writes_config() {
    // D-11: config.yaml must be written even when no cloudflared cert detected.
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Use -y to avoid interactive prompts (inquire requires TTY).
    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
            "--tunnel-hostname",
            "test.example.com",
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
        "restart: never\nsandbox:\n  name: right-test\n",
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
    std::fs::write(
        dir.path().join("config.yaml"),
        minimal_config_yaml(dir.path()),
    )
    .unwrap();

    right_with_init_auth()
        .args(["--home", home, "agent", "init", "test-bot", "-y"])
        .assert()
        .success()
        .stdout(predicate::str::contains("right up"));
}

// --- Task 2: --force and --fresh flag tests ---

#[test]
fn agent_init_force_recreates_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(
        dir.path().join("config.yaml"),
        minimal_config_yaml(dir.path()),
    )
    .unwrap();

    // Create agent.
    right_with_init_auth()
        .args(["--home", home, "agent", "init", "test-agent", "-y"])
        .assert()
        .success();

    // Write a marker file in the agent dir.
    let marker = dir.path().join("agents/test-agent/MARKER.txt");
    fs::write(&marker, "canary").unwrap();
    assert!(marker.exists());

    // Re-init with --force-recreate.
    right_with_init_auth()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force-recreate",
            "-y",
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
fn agent_init_force_recreate_preserves_config() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(
        dir.path().join("config.yaml"),
        minimal_config_yaml(dir.path()),
    )
    .unwrap();

    // Create agent with specific config.
    right_with_init_auth()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "preserve-test",
            "-y",
            "--network-policy",
            "permissive",
        ])
        .assert()
        .success();

    // Re-init with --force-recreate (no --fresh) — should preserve config.
    right_with_init_auth()
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
        yaml.contains("network_policy: permissive"),
        "agent.yaml should preserve the saved network policy after --force-recreate, got:\n{yaml}"
    );
}

#[test]
fn agent_init_force_recreate_on_nonexistent_agent() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create minimal home structure.
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(
        dir.path().join("config.yaml"),
        minimal_config_yaml(dir.path()),
    )
    .unwrap();

    // --force-recreate on non-existent agent should just create it.
    right_with_init_auth()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "new-agent",
            "--force-recreate",
            "-y",
        ])
        .assert()
        .success();

    assert!(dir.path().join("agents/new-agent/agent.yaml").exists());
}
fn assert_force_recreate_rejects_reachable_runtime(status: &str) {
    let dir = tempdir().unwrap();
    let home = dir.path();
    fs::create_dir_all(home.join("agents/test-agent")).unwrap();
    fs::write(
        home.join("agents/test-agent/agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(home.join("config.yaml"), minimal_config_yaml(home)).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::create_dir_all(home.join("run")).unwrap();
    fs::write(
        home.join("run/state.json"),
        format!(
            r#"{{"agents":[{{"name":"test-agent"}}],"socket_path":"/tmp/test.sock","started_at":"2026-01-01T00:00:00Z","pc_port":{port},"pc_api_token":null}}"#
        ),
    )
    .unwrap();
    let status_for_server = status.to_owned();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let body = format!(
            r#"{{"data":[{{"name":"test-agent-bot","status":"{status_for_server}","pid":42,"system_time":"0s","exit_code":0}}]}}"#
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });

    right_with_init_auth()
        .args([
            "--home",
            home.to_str().unwrap(),
            "agent",
            "init",
            "test-agent",
            "--force-recreate",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("offline database access"));
    server.join().unwrap();
    assert!(home.join("agents/test-agent/agent.yaml").exists());
}

#[test]
fn agent_init_force_recreate_refuses_reachable_runtime() {
    for status in ["Running", "Restarting", "Pending"] {
        assert_force_recreate_rejects_reachable_runtime(status);
    }
}

#[test]
fn agent_init_force_recreate_refuses_unreachable_runtime() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    fs::create_dir_all(home.join("agents/test-agent")).unwrap();
    fs::write(
        home.join("agents/test-agent/agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(home.join("config.yaml"), minimal_config_yaml(home)).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::create_dir_all(home.join("run")).unwrap();
    fs::write(
        home.join("run/state.json"),
        format!(
            r#"{{"agents":[{{"name":"test-agent"}}],"socket_path":"/tmp/test.sock","started_at":"2026-01-01T00:00:00Z","pc_port":{port},"pc_api_token":null}}"#
        ),
    )
    .unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home.to_str().unwrap(),
            "agent",
            "init",
            "test-agent",
            "--force-recreate",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unreachable"));
    assert!(home.join("agents/test-agent/agent.yaml").exists());
}

// --- Agent SSH regression tests ---

/// Regression: cmd_agent_ssh must discover agents correctly.
/// Previously it passed `home` instead of `home/agents` to discover_agents,
/// `right agent ssh` is gone: SSH is not part of the sandbox transport any
/// more, so the subcommand must not be silently accepted.
#[test]
fn test_agent_ssh_subcommand_no_longer_exists() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    let agent_dir = dir.path().join("agents").join("test-agent");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nsandbox:\n  name: right-test\n",
    )
    .unwrap();

    right()
        .args(["--home", home, "agent", "ssh", "test-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
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
        "restart: never\nsandbox:\n  name: right-test\n",
    )
    .unwrap();

    right()
        .args(["--home", home, "agent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myagent"))
        .stdout(predicate::str::contains("1 agent"));
}

// --- Backup and restore integration tests ---

#[ignore = "ci-msb: restore unpacks the backup inside a live microVM"]
#[tokio::test]
async fn ci_msb_agent_backup_and_restore_no_sandbox() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    // Set up a no-sandbox agent manually.
    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\nnetwork_policy: permissive\n",
    )
    .unwrap();
    fs::write(
        agent_dir.join("IDENTITY.md"),
        "# Test Agent\nI am a test agent.\n",
    )
    .unwrap();
    fs::write(agent_dir.join("TOOLS.md"), "# Tools\n").unwrap();
    fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();
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
    fs::write(agent_dir.join("allowlist.yaml"), allowlist).unwrap();
    fs::write(agent_dir.join("test-file.txt"), "hello world\n").unwrap();

    // Create a data.db with a test table.
    right_db::open_db(&agent_dir, true).await.unwrap();
    let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
    conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT)", ())
        .await
        .unwrap();
    conn.execute("INSERT INTO test (val) VALUES ('backup-test')", ())
        .await
        .unwrap();
    right_mcp::credentials::save_auth_token(&conn, "source-backup-token")
        .await
        .unwrap();
    let _: i64 = conn
        .query_one("PRAGMA wal_checkpoint(TRUNCATE)", (), |row| row.get(0))
        .await
        .unwrap();
    drop(conn);
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        fs::write(agent_dir.join(sidecar), format!("{sidecar}\n")).unwrap();
    }

    // Run backup.
    right()
        .args(["--home", home_str, "agent", "backup", "test-agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox.tar.gz"))
        .stdout(predicate::str::contains("agent.yaml"))
        .stdout(predicate::str::contains("allowlist.yaml"))
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
    assert!(
        backup_dir.join("allowlist.yaml").exists(),
        "should have allowlist.yaml"
    );
    assert_eq!(
        fs::read_to_string(backup_dir.join("allowlist.yaml")).unwrap(),
        allowlist,
        "backup must preserve allowlist.yaml content"
    );
    assert!(backup_dir.join("data.db").exists(), "should have data.db");
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        assert!(
            !tar_entries.contains(&format!("test-agent/{sidecar}")),
            "no-sandbox backup tar must not contain database sidecar {sidecar}"
        );
    }

    // Delete original agent.
    fs::remove_dir_all(&agent_dir).unwrap();
    assert!(!agent_dir.exists());

    // Restore to new agent name via agent init --from-backup.
    // Needs agents dir and config.yaml to exist (home structure).
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    right_with_init_auth()
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
    assert_eq!(
        fs::read_to_string(restored_dir.join("allowlist.yaml")).unwrap(),
        allowlist,
        "restore must preserve allowlist.yaml content"
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

    assert!(
        !restored_dir.join("data.db-future").exists(),
        "restore must remove copied unknown sidecars before database open"
    );

    // Verify restored database.
    let restored_db = right_db::open_database_path_readonly(restored_dir.join("data.db"))
        .await
        .unwrap();
    let val: String = restored_db
        .query_row("SELECT val FROM test WHERE id = 1", (), |r| r.get(0))
        .await
        .unwrap();
    assert_eq!(val, "backup-test");
    let restored_token = right_mcp::credentials::get_auth_token(&restored_db)
        .await
        .unwrap();
    assert_eq!(restored_token.as_deref(), Some(TEST_CLAUDE_SETUP_TOKEN));
}

#[ignore = "ci-msb: restore unpacks the backup inside a live microVM"]
#[tokio::test]
async fn ci_msb_agent_restore_removes_legacy_db_sidecars() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    let backup_dir = home
        .path()
        .join("backups")
        .join("source-agent")
        .join("20260527-0100");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(
        backup_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    let backup_db = right_db::open_connection(&backup_dir, true).await.unwrap();
    backup_db
        .execute("CREATE TABLE legacy_restore_probe (val TEXT)", ())
        .await
        .unwrap();
    backup_db
        .execute(
            "INSERT INTO legacy_restore_probe (val) VALUES ('canonical db snapshot')",
            (),
        )
        .await
        .unwrap();
    let _: i64 = backup_db
        .query_one("PRAGMA wal_checkpoint(TRUNCATE)", (), |row| row.get(0))
        .await
        .unwrap();
    drop(backup_db);

    let tar_root = home.path().join("legacy-tar-root");
    let tar_agent = tar_root.join("source-agent");
    fs::create_dir_all(&tar_agent).unwrap();
    fs::write(
        tar_agent.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(tar_agent.join("notes.txt"), "from tar\n").unwrap();
    let tar_db = right_db::open_connection(&tar_agent, true).await.unwrap();
    tar_db
        .execute("CREATE TABLE legacy_restore_probe (val TEXT)", ())
        .await
        .unwrap();
    tar_db
        .execute(
            "INSERT INTO legacy_restore_probe (val) VALUES ('stale tar db')",
            (),
        )
        .await
        .unwrap();
    let _: i64 = tar_db
        .query_one("PRAGMA wal_checkpoint(TRUNCATE)", (), |row| row.get(0))
        .await
        .unwrap();
    drop(tar_db);
    for sidecar in [
        "data.db-wal",
        "data.db-shm",
        "data.db-tshm",
        "data.db-future",
    ] {
        fs::write(tar_agent.join(sidecar), format!("stale {sidecar}\n")).unwrap();
    }

    let tar_path = backup_dir.join("sandbox.tar.gz");
    let status = StdCommand::new("tar")
        .args([
            "czf",
            tar_path.to_str().unwrap(),
            "-C",
            tar_root.to_str().unwrap(),
            "source-agent",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "test tar creation must succeed");

    right_with_init_auth()
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

    let restored_dir = home.path().join("agents").join("restored-agent");
    assert_eq!(
        fs::read_to_string(restored_dir.join("notes.txt")).unwrap(),
        "from tar\n"
    );
    assert!(
        !restored_dir.join("data.db-future").exists(),
        "restore must remove stale copied sidecars before database open"
    );

    let restored_db = right_db::open_database_path_readonly(restored_dir.join("data.db"))
        .await
        .unwrap();
    let val: String = restored_db
        .query_row("SELECT val FROM legacy_restore_probe", (), |row| row.get(0))
        .await
        .unwrap();
    assert_eq!(val, "canonical db snapshot");
}

#[ignore = "ci-msb: restore unpacks the backup inside a live microVM"]
#[tokio::test]
async fn ci_msb_agent_restore_ignores_tar_db_without_canonical_snapshot() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    let backup_dir = home
        .path()
        .join("backups")
        .join("source-agent")
        .join("20260527-0200");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(
        backup_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();

    let tar_root = home.path().join("legacy-tar-root-no-canonical");
    let tar_agent = tar_root.join("source-agent");
    fs::create_dir_all(&tar_agent).unwrap();
    fs::write(
        tar_agent.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(tar_agent.join("notes.txt"), "from tar\n").unwrap();
    let tar_db = right_db::open_connection(&tar_agent, true).await.unwrap();
    tar_db
        .execute("CREATE TABLE tar_only_probe (val TEXT)", ())
        .await
        .unwrap();
    tar_db
        .execute("INSERT INTO tar_only_probe (val) VALUES ('tar db')", ())
        .await
        .unwrap();
    let _: i64 = tar_db
        .query_one("PRAGMA wal_checkpoint(TRUNCATE)", (), |row| row.get(0))
        .await
        .unwrap();
    drop(tar_db);

    let tar_path = backup_dir.join("sandbox.tar.gz");
    let status = StdCommand::new("tar")
        .args([
            "czf",
            tar_path.to_str().unwrap(),
            "-C",
            tar_root.to_str().unwrap(),
            "source-agent",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "test tar creation must succeed");

    right_with_init_auth()
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

    let restored_dir = home.path().join("agents").join("restored-agent");
    assert_eq!(
        fs::read_to_string(restored_dir.join("notes.txt")).unwrap(),
        "from tar\n"
    );
    let restored_db = right_db::open_database_path_readonly(restored_dir.join("data.db"))
        .await
        .unwrap();
    let tar_table_count: i64 = restored_db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tar_only_probe'",
            (),
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        tar_table_count, 0,
        "restore must not accept data.db from sandbox.tar.gz when backup root has no canonical data.db"
    );
}

#[test]
fn test_agent_restore_fails_before_partial_agent_for_missing_binding_mode() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    let backup_dir = home
        .path()
        .join("backups")
        .join("source-agent")
        .join("20260516-0117");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(backup_dir.join("sandbox.tar.gz"), "not a real tar").unwrap();
    fs::write(
        backup_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\nmemory:\n  provider: hindsight\n",
    )
    .unwrap();

    right_with_init_auth()
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
        .failure()
        .stderr(predicate::str::contains(
            "requires an explicit binding mode",
        ));

    assert!(
        !home.path().join("agents").join("restored-agent").exists(),
        "binding-mode rejection must not leave a partial target agent directory"
    );
}

#[ignore = "ci-msb: restore unpacks the backup inside a live microVM"]
#[test]
fn ci_msb_agent_backup_and_restore_preserves_source_hindsight_bank() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("source-agent");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\nmemory:\n  provider: hindsight\n",
    )
    .unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# Source Agent\n").unwrap();

    right()
        .args(["--home", home_str, "agent", "backup", "source-agent"])
        .assert()
        .success();

    let backups_dir = home.path().join("backups").join("source-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();

    fs::remove_dir_all(&agent_dir).unwrap();
    fs::write(
        home.path().join("config.yaml"),
        minimal_config_yaml(home.path()),
    )
    .unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home_str,
            "agent",
            "init",
            "restored-agent",
            "--from-backup",
            backup_dir.to_str().unwrap(),
            "--preserve-source-bindings",
        ])
        .assert()
        .success();

    let restored_yaml =
        fs::read_to_string(home.path().join("agents/restored-agent/agent.yaml")).unwrap();
    let restored_config = right_agent::agent::discovery::parse_agent_config(
        &home.path().join("agents/restored-agent"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        restored_config.memory.unwrap().bank_id.as_deref(),
        Some("source-agent"),
        "restored agent config must preserve the source Hindsight bank; yaml:\n{restored_yaml}"
    );
}

#[ignore = "ci-msb: backup archives the guest filesystem of a live microVM"]
#[test]
fn ci_msb_agent_backup_sandbox_only() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# Test\n").unwrap();
    fs::write(agent_dir.join("TOOLS.md"), "# Tools\n").unwrap();

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

#[ignore = "ci-msb: backup archives the guest filesystem of a live microVM"]
#[test]
fn ci_msb_agent_backup_excludes_rebuildable_dirs_by_default() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    fs::create_dir_all(agent_dir.join(".cache")).unwrap();
    fs::create_dir_all(agent_dir.join(".venv")).unwrap();
    fs::create_dir_all(agent_dir.join(".npm")).unwrap();
    fs::create_dir_all(agent_dir.join(".uv")).unwrap();
    fs::create_dir_all(agent_dir.join("custom-dir")).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(agent_dir.join(".claude/session.json"), "{}\n").unwrap();
    fs::write(agent_dir.join(".cache/cache.txt"), "cache\n").unwrap();
    fs::write(agent_dir.join(".venv/python.txt"), "venv\n").unwrap();
    fs::write(agent_dir.join(".npm/npm.txt"), "npm\n").unwrap();
    fs::write(agent_dir.join(".uv/uv.txt"), "uv\n").unwrap();
    fs::write(agent_dir.join("custom-dir/state.txt"), "state\n").unwrap();

    right()
        .args(["--home", home_str, "agent", "backup", "test-agent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup complete:"));

    let backups_dir = home.path().join("backups").join("test-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));

    assert!(tar_entries.contains(&"test-agent/.claude/session.json".to_string()));
    assert!(tar_entries.contains(&"test-agent/custom-dir/state.txt".to_string()));
    assert!(
        !tar_entries
            .iter()
            .any(|entry| entry.starts_with("test-agent/.cache/"))
    );
    assert!(
        !tar_entries
            .iter()
            .any(|entry| entry.starts_with("test-agent/.venv/"))
    );
    assert!(
        !tar_entries
            .iter()
            .any(|entry| entry.starts_with("test-agent/.npm/"))
    );
    assert!(
        !tar_entries
            .iter()
            .any(|entry| entry.starts_with("test-agent/.uv/"))
    );
}

#[ignore = "ci-msb: backup archives the guest filesystem of a live microVM"]
#[test]
fn ci_msb_agent_backup_include_rebuildable_keeps_rebuildable_dirs() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    let agent_dir = home.path().join("agents").join("test-agent");
    fs::create_dir_all(agent_dir.join(".cache")).unwrap();
    fs::create_dir_all(agent_dir.join(".venv")).unwrap();
    fs::create_dir_all(agent_dir.join(".npm")).unwrap();
    fs::create_dir_all(agent_dir.join(".uv")).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();
    fs::write(agent_dir.join(".cache/cache.txt"), "cache\n").unwrap();
    fs::write(agent_dir.join(".venv/python.txt"), "venv\n").unwrap();
    fs::write(agent_dir.join(".npm/npm.txt"), "npm\n").unwrap();
    fs::write(agent_dir.join(".uv/uv.txt"), "uv\n").unwrap();

    right()
        .args([
            "--home",
            home_str,
            "agent",
            "backup",
            "test-agent",
            "--include-rebuildable",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup complete:"));

    let backups_dir = home.path().join("backups").join("test-agent");
    let entries: Vec<_> = fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "should have exactly one backup");
    let backup_dir = entries[0].path();
    let tar_entries = tar_entries(&backup_dir.join("sandbox.tar.gz"));

    assert!(tar_entries.contains(&"test-agent/.cache/cache.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.venv/python.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.npm/npm.txt".to_string()));
    assert!(tar_entries.contains(&"test-agent/.uv/uv.txt".to_string()));
}

#[test]
fn test_agent_restore_fails_if_agent_exists() {
    let home = tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();

    // Create existing agent.
    let agent_dir = home.path().join("agents").join("existing");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();

    // Create a fake backup dir.
    let backup_dir = home.path().join("fake-backup");
    fs::create_dir_all(&backup_dir).unwrap();
    fs::write(backup_dir.join("sandbox.tar.gz"), "fake").unwrap();
    fs::write(
        backup_dir.join("agent.yaml"),
        "sandbox:\n  name: right-test\n",
    )
    .unwrap();

    right_with_init_auth()
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

#[ignore = "ci-msb: destroy deletes from the host-global microVM catalog"]
#[test]
fn ci_msb_destroy_agent_force() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Create an agent via init first
    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
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

#[ignore = "ci-msb: destroy backs up a live microVM before deleting it"]
#[test]
fn ci_msb_destroy_agent_force_with_backup() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    right_with_init_auth()
        .args([
            "--home",
            home,
            "init",
            "-y",
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
    fs::write(
        dir.path().join("config.yaml"),
        minimal_config_yaml(dir.path()),
    )
    .unwrap();

    right()
        .args([
            "--home",
            home,
            "agent",
            "init",
            "test-agent",
            "--force",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}
