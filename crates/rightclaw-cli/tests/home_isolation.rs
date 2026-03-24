use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn rightclaw() -> Command {
    Command::cargo_bin("rightclaw").unwrap()
}

// --- Plan 02 sandbox path tests (HOME-05) ---

/// After init, settings.json should contain allowRead with absolute agent path (HOME-05).
#[test]
fn init_settings_contain_allow_read() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    let settings_path = dir.path().join("agents/right/.claude/settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    let allow_read = json["sandbox"]["filesystem"]["allowRead"]
        .as_array()
        .expect("allowRead should be present");
    assert!(!allow_read.is_empty(), "allowRead should not be empty");
    // Path should be absolute (starts with /)
    let first = allow_read[0].as_str().unwrap();
    assert!(first.starts_with('/'), "allowRead path should be absolute: {first}");
}

/// After init, denyRead should use absolute paths, not tilde-relative (HOME-05).
#[test]
fn init_settings_deny_read_absolute() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    let settings_path = dir.path().join("agents/right/.claude/settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    let deny_read = json["sandbox"]["filesystem"]["denyRead"]
        .as_array()
        .expect("denyRead should be present");
    for path in deny_read {
        let s = path.as_str().unwrap();
        assert!(!s.starts_with("~/"), "denyRead should not use tilde: {s}");
        assert!(s.starts_with('/'), "denyRead should be absolute: {s}");
    }
}

/// After init, denyRead should include .ssh, .aws, .gnupg paths.
#[test]
fn init_settings_deny_read_includes_sensitive_paths() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    let settings_path = dir.path().join("agents/right/.claude/settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    let deny_read = json["sandbox"]["filesystem"]["denyRead"]
        .as_array()
        .unwrap();
    let deny_strs: Vec<&str> = deny_read.iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(deny_strs.iter().any(|s| s.ends_with("/.ssh")), "missing .ssh in denyRead: {deny_strs:?}");
    assert!(deny_strs.iter().any(|s| s.ends_with("/.aws")), "missing .aws in denyRead: {deny_strs:?}");
    assert!(deny_strs.iter().any(|s| s.ends_with("/.gnupg")), "missing .gnupg in denyRead: {deny_strs:?}");
}

// --- Plan 01 artifact tests (D-11: credential symlink, .claude.json, missing-creds) ---

/// After init, agent .claude.json should contain hasTrustDialogAccepted (HOME-02, PERM-02).
#[test]
fn init_agent_claude_json_has_trust() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    let claude_json_path = dir.path().join("agents/right/.claude.json");
    assert!(claude_json_path.exists(), ".claude.json should exist at {}", claude_json_path.display());

    let content = fs::read_to_string(&claude_json_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Find hasTrustDialogAccepted in any project entry
    let projects = json["projects"].as_object()
        .expect("projects key should exist in .claude.json");
    let has_trust = projects.values().any(|proj| {
        proj.get("hasTrustDialogAccepted")
            .and_then(|v| v.as_bool())
            == Some(true)
    });
    assert!(has_trust, ".claude.json should have hasTrustDialogAccepted: true, got: {content}");
}

/// After init, agent .claude/.credentials.json should be a symlink to host creds
/// when host creds exist. If they don't exist, the symlink won't be created (warning only).
#[test]
fn init_agent_credentials_is_symlink() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    rightclaw()
        .args(["--home", home, "init"])
        .assert()
        .success();

    let creds_path = dir.path().join("agents/right/.claude/.credentials.json");
    if creds_path.exists() || creds_path.symlink_metadata().is_ok() {
        let metadata = creds_path.symlink_metadata().unwrap();
        assert!(metadata.file_type().is_symlink(),
            "credentials.json should be a symlink, not a regular file");
        let target = fs::read_link(&creds_path).unwrap();
        assert!(target.to_str().unwrap().contains(".claude/.credentials.json"),
            "symlink target should point to host .claude/.credentials.json, got: {}",
            target.display());
    }
    // If symlink doesn't exist, host has no creds — covered by init_warns_when_host_creds_missing
}

/// rightclaw init succeeds with warning when host OAuth credentials are absent.
/// Simulate by setting HOME to a dir with no .claude/.credentials.json.
#[test]
fn init_warns_when_host_creds_missing() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_str().unwrap();

    // Use a separate temp dir as the "host home" — it has no .claude/.credentials.json
    let fake_host_home = tempdir().unwrap();

    let result = rightclaw()
        .args(["--home", home, "init"])
        .env("HOME", fake_host_home.path())
        .assert()
        .success();

    // When host creds are missing, rightclaw should still succeed (warn, not error)
    // and stderr should contain the warning about missing credentials
    result.stderr(
        predicates::str::contains("no OAuth credentials").or(
            predicates::str::contains("ANTHROPIC_API_KEY")
        )
    );
}

// --- Live integration tests (require claude binary + credentials) ---

#[test]
#[ignore = "requires live claude credentials and claude binary"]
fn oauth_with_credential_symlink_works() {
    // This test would:
    // 1. Create a temp agent dir
    // 2. Symlink real host credentials into it
    // 3. Run `claude -p "hi" --output-format json` with HOME=agent_dir
    // 4. Assert response contains no error
    // Placeholder -- implement when running manual validation.
}

#[test]
#[ignore = "requires live claude credentials and claude binary"]
fn deny_read_blocks_host_ssh() {
    // This test would:
    // 1. Set up agent with sandbox config
    // 2. Run `claude -p "cat ~/.ssh/config" --output-format json` with HOME=agent_dir
    // 3. Assert sandbox refusal in output
    // Placeholder -- implement when running manual validation.
}
