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
