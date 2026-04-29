//! CLI surface tests for `right agent rebootstrap`.
//!
//! The full library-level happy path is covered by
//! `right-agent`'s `rebootstrap_sandbox` integration test, so here we only
//! exercise the CLI-level concerns: argument validation, missing-agent
//! errors, and the abort-on-cancel path.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn rebootstrap_unknown_agent_errors_with_name() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("right")
        .unwrap()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "ghost",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost"));
}

#[test]
fn rebootstrap_help_lists_yes_flag() {
    Command::cargo_bin("right")
        .unwrap()
        .args(["agent", "rebootstrap", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes"));
}

/// Regression for the 2026-04-29 incident: when state.json is present but
/// process-compose is unreachable (auth-broken, dead, port stolen), the
/// command MUST refuse rather than proceed with file ops. Previously this
/// was silently swallowed and the agent was left bootstrapped on disk
/// while the still-running bot served the old persona.
#[test]
fn rebootstrap_errors_when_state_present_but_pc_unreachable() {
    let home = tempfile::tempdir().unwrap();

    // Set up a minimal agent dir so `rebootstrap::plan` succeeds.
    let agent_dir = home.path().join("agents").join("ghosty");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# ghosty\n").unwrap();
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n").unwrap();

    // state.json points at a port nothing listens on. Reserved port 1 is
    // unused by anything reasonable; any TCP connect attempt will fail
    // immediately, mimicking "PC is dead". The token is irrelevant since
    // no server will accept the connection.
    let run_dir = home.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("state.json"),
        r#"{"agents":[{"name":"ghosty"}],"socket_path":"/tmp/x.sock","started_at":"2026-04-29T00:00:00Z","pc_port":1,"pc_api_token":"any"}"#,
    )
    .unwrap();

    Command::cargo_bin("right")
        .unwrap()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "ghosty",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Refusing to rebootstrap"));

    // File ops MUST NOT have run. IDENTITY.md must still be present and
    // BOOTSTRAP.md must NOT have been (re)written.
    assert!(
        agent_dir.join("IDENTITY.md").exists(),
        "IDENTITY.md should be untouched when PC is unreachable",
    );
    assert!(
        !agent_dir.join("BOOTSTRAP.md").exists(),
        "BOOTSTRAP.md should NOT have been written when PC is unreachable",
    );
}
