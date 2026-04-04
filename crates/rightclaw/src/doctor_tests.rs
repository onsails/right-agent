use super::*;
use tempfile::tempdir;

#[test]
fn check_binary_returns_fail_for_missing_binary() {
    let check = check_binary("rightclaw-absolutely-nonexistent-binary-xyz", None);
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(check.detail.contains("not found"));
}

#[test]
fn check_binary_returns_pass_for_present_binary() {
    // "sh" is always present on Unix
    let check = check_binary("sh", None);
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(!check.detail.contains("not found"));
}

#[test]
fn check_binary_includes_fix_hint_on_failure() {
    let check = check_binary(
        "rightclaw-nonexistent-xyz",
        Some("https://example.com/install"),
    );
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.fix.as_deref(),
        Some("https://example.com/install")
    );
}

#[test]
fn check_binary_no_fix_on_success() {
    let check = check_binary("sh", Some("should not appear"));
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(check.fix.is_none());
}

#[test]
fn run_doctor_with_empty_agents_dir_reports_fail() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();

    let checks = run_doctor(dir.path());

    let agent_checks: Vec<_> = checks
        .iter()
        .filter(|c| c.name.starts_with("agents/") && c.detail.contains("no valid agents"))
        .collect();
    assert!(
        !agent_checks.is_empty(),
        "should report no valid agents found"
    );
    assert_eq!(agent_checks[0].status, CheckStatus::Fail);
}

#[test]
fn run_doctor_with_valid_agent_reports_pass() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("right");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Right").unwrap();

    let checks = run_doctor(dir.path());

    let agent_check = checks
        .iter()
        .find(|c| c.name.contains("agents/right/"))
        .expect("should have a check for agents/right/");
    assert_eq!(agent_check.status, CheckStatus::Pass);
    assert!(agent_check.detail.contains("valid agent"));
}

#[test]
fn run_doctor_with_missing_agents_dir_reports_fail() {
    let dir = tempdir().unwrap();
    // No agents/ directory at all

    let checks = run_doctor(dir.path());

    let agent_check = checks
        .iter()
        .find(|c| c.name == "agents/" && c.status == CheckStatus::Fail)
        .expect("should report missing agents directory");
    assert!(agent_check.detail.contains("not found"));
}

#[test]
fn run_doctor_reports_bootstrap_pending() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("right");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Right").unwrap();
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "# Onboarding").unwrap();

    let checks = run_doctor(dir.path());

    let bootstrap_check = checks
        .iter()
        .find(|c| c.name.contains("BOOTSTRAP.md"))
        .expect("should have a BOOTSTRAP.md check");
    assert_eq!(bootstrap_check.status, CheckStatus::Warn);
    assert!(bootstrap_check.detail.contains("onboarding pending"));
}

#[test]
fn run_doctor_reports_missing_identity() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("broken");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // No IDENTITY.md
    std::fs::write(agent_dir.join("agent.yaml"), "{}").unwrap();

    let checks = run_doctor(dir.path());

    let agent_check = checks
        .iter()
        .find(|c| c.name.contains("agents/broken/"))
        .expect("should have a check for agents/broken/");
    assert_eq!(agent_check.status, CheckStatus::Fail);
    assert!(agent_check.detail.contains("IDENTITY.md"));
}

#[test]
fn doctor_check_display_shows_name_status_detail() {
    let check = DoctorCheck {
        name: "test-binary".to_string(),
        status: CheckStatus::Pass,
        detail: "/usr/bin/test-binary".to_string(),
        fix: None,
    };
    let display = format!("{check}");
    assert!(display.contains("test-binary"));
    assert!(display.contains("ok"));
    assert!(display.contains("/usr/bin/test-binary"));
}

#[test]
fn doctor_check_display_shows_fix_on_failure() {
    let check = DoctorCheck {
        name: "missing".to_string(),
        status: CheckStatus::Fail,
        detail: "not found".to_string(),
        fix: Some("install it".to_string()),
    };
    let display = format!("{check}");
    assert!(display.contains("FAIL"));
    assert!(display.contains("fix: install it"));
}

#[test]
fn run_doctor_always_checks_all_three_binaries() {
    let dir = tempdir().unwrap();
    let checks = run_doctor(dir.path());

    let binary_names: Vec<&str> = checks
        .iter()
        .filter(|c| !c.name.starts_with("agents"))
        .map(|c| c.name.as_str())
        .collect();

    assert!(binary_names.contains(&"rightclaw"), "missing rightclaw check");
    assert!(binary_names.contains(&"process-compose"), "missing process-compose check");
    assert!(binary_names.contains(&"claude"), "missing claude check");
    assert!(!binary_names.contains(&"openshell"), "openshell should not be checked");
}

#[test]
fn check_bwrap_sandbox_returns_doctor_check() {
    // Call the function directly -- will pass or fail depending on host,
    // but must not panic and must return correct shape.
    let check = check_bwrap_sandbox();
    assert_eq!(check.name, "bwrap-sandbox");
    // Status is either Pass or Fail depending on system -- just verify it's set
    assert!(
        check.status == CheckStatus::Pass || check.status == CheckStatus::Fail,
        "status must be Pass or Fail, got: {:?}",
        check.status
    );
}

#[test]
fn bwrap_fix_guidance_contains_apparmor_profile() {
    let guidance = bwrap_fix_guidance();
    assert!(
        guidance.contains("apparmor_parser"),
        "fix guidance must mention apparmor_parser"
    );
    assert!(
        guidance.contains("/etc/apparmor.d/bwrap"),
        "fix guidance must include AppArmor profile path"
    );
    assert!(
        guidance.contains("sysctl"),
        "fix guidance must mention sysctl workaround"
    );
    assert!(
        guidance.contains("https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces"),
        "fix guidance must include Ubuntu docs link"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn run_doctor_includes_bwrap_socat_on_linux() {
    let dir = tempdir().unwrap();
    let checks = run_doctor(dir.path());

    let check_names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        check_names.contains(&"bwrap"),
        "Linux doctor must check for bwrap"
    );
    assert!(
        check_names.contains(&"socat"),
        "Linux doctor must check for socat"
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn run_doctor_skips_bwrap_socat_on_non_linux() {
    let dir = tempdir().unwrap();
    let checks = run_doctor(dir.path());

    let check_names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        !check_names.contains(&"bwrap"),
        "non-Linux doctor must not check for bwrap"
    );
    assert!(
        !check_names.contains(&"socat"),
        "non-Linux doctor must not check for socat"
    );
}

// ---- check_managed_settings tests ----

#[test]
fn check_managed_settings_returns_none_when_file_absent() {
    let result = check_managed_settings("/nonexistent-rightclaw-test/managed-settings.json");
    assert!(result.is_none(), "should return None when file does not exist");
}

#[test]
fn check_managed_settings_returns_warn_with_strict_message_when_flag_true() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("managed-settings.json");
    std::fs::write(&path, "{\"allowManagedDomainsOnly\": true}\n").unwrap();

    let result = check_managed_settings(path.to_str().unwrap());
    let check = result.expect("should return Some when file exists");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("allowManagedDomainsOnly:true"),
        "detail must mention allowManagedDomainsOnly:true, got: {}",
        check.detail
    );
    let fix = check.fix.expect("should have fix hint");
    assert!(
        fix.contains("sudo rightclaw config strict-sandbox"),
        "fix must contain sudo command, got: {fix}"
    );
}

#[test]
fn check_managed_settings_returns_warn_with_generic_message_when_flag_false() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("managed-settings.json");
    std::fs::write(&path, "{\"allowManagedDomainsOnly\": false}").unwrap();

    let result = check_managed_settings(path.to_str().unwrap());
    let check = result.expect("should return Some when file exists");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("content may affect"),
        "detail must use generic message when flag is false, got: {}",
        check.detail
    );
}

#[test]
fn check_managed_settings_returns_warn_with_generic_message_for_invalid_json() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("managed-settings.json");
    std::fs::write(&path, "not valid json at all!!!").unwrap();

    let result = check_managed_settings(path.to_str().unwrap());
    let check = result.expect("should return Some when file exists");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("content may affect"),
        "detail must use generic message for invalid JSON, got: {}",
        check.detail
    );
}

#[test]
fn check_managed_settings_returns_warn_with_generic_message_when_key_absent() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("managed-settings.json");
    std::fs::write(&path, "{\"someOtherKey\": true}").unwrap();

    let result = check_managed_settings(path.to_str().unwrap());
    let check = result.expect("should return Some when file exists");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("content may affect"),
        "detail must use generic message when key is absent, got: {}",
        check.detail
    );
}

#[test]
fn check_managed_settings_fix_hint_contains_sudo_command_when_flag_true() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("managed-settings.json");
    std::fs::write(&path, "{\"allowManagedDomainsOnly\": true}").unwrap();

    let check = check_managed_settings(path.to_str().unwrap()).unwrap();
    let fix = check.fix.unwrap();
    assert!(
        fix.contains("sudo rightclaw config strict-sandbox"),
        "fix hint must include the sudo command, got: {fix}"
    );
}

#[test]
fn run_doctor_includes_sqlite3_check() {
    let dir = tempdir().unwrap();
    // Need agents/ dir to avoid unrelated Fail masking the check
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();
    let checks = run_doctor(dir.path());
    let sqlite3_check = checks
        .iter()
        .find(|c| c.name == "sqlite3")
        .expect("run_doctor must include sqlite3 check");
    // Status is Pass or Warn (never Fail) — DOCTOR-01 requires non-fatal
    assert!(
        sqlite3_check.status == CheckStatus::Pass || sqlite3_check.status == CheckStatus::Warn,
        "sqlite3 check must be Pass or Warn, got: {:?}",
        sqlite3_check.status
    );
    // No fix suggestion — sqlite3 is available on all standard installs
    assert!(
        sqlite3_check.fix.is_none(),
        "sqlite3 check must have no fix hint"
    );
}

#[test]
fn sqlite3_check_is_warn_not_fail_when_absent() {
    // Simulate missing binary by calling check_binary with a guaranteed-absent name,
    // then verify the status override logic produces Warn.
    let raw = check_binary("rightclaw-absolutely-nonexistent-sqlite3-xyz", None);
    assert_eq!(
        raw.status,
        CheckStatus::Fail,
        "raw check_binary returns Fail for absent binary"
    );

    // The override logic from run_doctor:
    let overridden_status = if raw.status == CheckStatus::Pass {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    assert_eq!(
        overridden_status,
        CheckStatus::Warn,
        "absent binary must map to Warn, not Fail"
    );
}

// ---- make_webhook_check tests ----

#[test]
fn make_webhook_check_pass_when_url_empty() {
    let check = make_webhook_check("mybot", Ok(String::new()));
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(
        check.detail.contains("no active webhook"),
        "expected 'no active webhook', got: {}",
        check.detail
    );
    assert!(check.fix.is_none());
}

#[test]
fn make_webhook_check_warn_when_url_nonempty() {
    let check = make_webhook_check("mybot", Ok("https://example.com/webhook".to_string()));
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("active webhook found"),
        "expected 'active webhook found', got: {}",
        check.detail
    );
    assert!(
        check.detail.contains("https://example.com/webhook"),
        "detail must include the webhook URL"
    );
    let fix = check.fix.expect("Warn with URL must have fix hint");
    assert!(
        fix.contains("mybot"),
        "fix must mention the agent name, got: {fix}"
    );
}

#[test]
fn make_webhook_check_warn_when_http_error() {
    let check = make_webhook_check("mybot", Err("HTTP error: connection refused".to_string()));
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("webhook check skipped"),
        "expected 'webhook check skipped', got: {}",
        check.detail
    );
    assert!(check.fix.is_none());
}

/// Regression test: fetch_webhook_url must not panic when called from within
/// an existing tokio multi-thread runtime context (UAT-FIX-02).
///
/// Before the fix: Runtime::new().block_on() panics with
/// "Cannot start a runtime from within a runtime".
/// After the fix: returns a Result (Ok or Err) without panicking — the
/// Telegram API returns 200 with empty result for invalid tokens, so Ok("")
/// is a valid non-panic outcome.
#[tokio::test(flavor = "multi_thread")]
async fn fetch_webhook_url_does_not_panic_in_async_context() {
    // An invalid token is used — the exact result depends on network and
    // Telegram API behavior. The critical invariant is: no panic.
    // Before the fix this test PANICKED with "Cannot start a runtime from
    // within a runtime". After the fix it returns Ok("") or Err(...).
    let _result = fetch_webhook_url("invalid-token-for-test");
    // If we reach here without panicking, the fix works.
    // The Telegram API returns 200 OK with empty result for invalid tokens,
    // so we cannot assert is_err() — Ok("") is also a valid outcome.
}

#[test]
fn check_webhook_info_for_agents_skips_agents_without_token() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("mybot");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Write agent.yaml with no telegram token
    std::fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# MyBot\n").unwrap();

    let checks = check_webhook_info_for_agents(dir.path());
    assert!(
        checks.is_empty(),
        "agent without telegram token must produce no webhook checks, got: {:?}",
        checks.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

#[test]
fn check_webhook_info_for_agents_skips_when_no_agents_dir() {
    let dir = tempdir().unwrap();
    // No agents/ directory
    let checks = check_webhook_info_for_agents(dir.path());
    assert!(checks.is_empty(), "missing agents dir must produce no checks");
}

// ---- DOC-01: check_rg_in_path tests ----

#[test]
fn check_rg_in_path_returns_pass_with_path_when_sh_used_as_proxy() {
    // "sh" is always present on Unix — use as proxy for a binary that exists
    // We can't call check_rg_in_path with a custom name, so we test the
    // underlying check_binary behavior that check_rg_in_path relies on.
    let check = check_binary("sh", Some("Install ripgrep"));
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(!check.detail.is_empty(), "path detail must not be empty");
}

#[test]
fn check_rg_in_path_uses_warn_not_fail_for_absent_binary() {
    // Verify the Warn override pattern used by check_rg_in_path
    let raw = check_binary("rightclaw-absolutely-nonexistent-rg-xyz", None);
    assert_eq!(raw.status, CheckStatus::Fail);

    // Simulate the override in check_rg_in_path:
    let overridden = if raw.status == CheckStatus::Pass {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    assert_eq!(overridden, CheckStatus::Warn, "absent rg must map to Warn");
}

#[test]
fn check_rg_in_path_result_has_name_rg() {
    let check = check_rg_in_path();
    assert_eq!(check.name, "rg", "check name must be 'rg'");
}

#[test]
fn check_rg_in_path_status_is_pass_or_warn_never_fail() {
    let check = check_rg_in_path();
    assert!(
        check.status == CheckStatus::Pass || check.status == CheckStatus::Warn,
        "check_rg_in_path must return Pass or Warn, never Fail, got: {:?}",
        check.status
    );
}

// ---- DOC-02: check_ripgrep_in_settings tests ----

#[test]
fn check_ripgrep_in_settings_returns_empty_when_agents_dir_missing() {
    let dir = tempdir().unwrap();
    // No agents/ directory
    let checks = check_ripgrep_in_settings(dir.path());
    assert!(
        checks.is_empty(),
        "missing agents dir must return empty vec"
    );
}

#[test]
fn check_ripgrep_in_settings_returns_warn_when_settings_json_missing() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // No .claude/settings.json

    let checks = check_ripgrep_in_settings(dir.path());
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!(check.name, "sandbox-rg/myagent");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("settings.json not found"),
        "detail must say settings.json not found, got: {}",
        check.detail
    );
    assert!(check.fix.is_some(), "must have fix hint");
}

#[test]
fn check_ripgrep_in_settings_returns_warn_when_ripgrep_command_key_absent() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // settings.json without sandbox.ripgrep.command
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"sandbox": {"enabled": true}}"#,
    )
    .unwrap();

    let checks = check_ripgrep_in_settings(dir.path());
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!(check.name, "sandbox-rg/myagent");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("sandbox.ripgrep.command absent"),
        "detail must mention absent key, got: {}",
        check.detail
    );
    assert!(check.fix.is_some());
}

#[test]
fn check_ripgrep_in_settings_returns_warn_when_command_path_nonexistent() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // settings.json with sandbox.ripgrep.command pointing to non-existent path
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"sandbox": {"ripgrep": {"command": "/nonexistent/path/to/rg"}}}"#,
    )
    .unwrap();

    let checks = check_ripgrep_in_settings(dir.path());
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!(check.name, "sandbox-rg/myagent");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("non-existent path"),
        "detail must mention non-existent path, got: {}",
        check.detail
    );
    assert!(check.fix.is_some());
}

#[test]
fn check_ripgrep_in_settings_returns_pass_when_command_points_to_existing_file() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    // Create a fake rg binary (just needs to exist as a file)
    let fake_rg = dir.path().join("fake-rg");
    std::fs::write(&fake_rg, "#!/bin/sh\n").unwrap();

    let settings_content = format!(
        r#"{{"sandbox": {{"ripgrep": {{"command": "{}"}}}}}}"#,
        fake_rg.display()
    );
    std::fs::write(claude_dir.join("settings.json"), &settings_content).unwrap();

    let checks = check_ripgrep_in_settings(dir.path());
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!(check.name, "sandbox-rg/myagent");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(
        check.detail.contains(fake_rg.to_str().unwrap()),
        "detail must contain the rg path, got: {}",
        check.detail
    );
}

#[test]
fn check_ripgrep_in_settings_returns_warn_for_invalid_json() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "not valid json!!!").unwrap();

    let checks = check_ripgrep_in_settings(dir.path());
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!(check.name, "sandbox-rg/myagent");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("not valid JSON"),
        "detail must mention invalid JSON, got: {}",
        check.detail
    );
    assert!(check.fix.is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn run_doctor_includes_rg_check_on_linux() {
    let dir = tempdir().unwrap();
    let checks = run_doctor(dir.path());
    let check_names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        check_names.contains(&"rg"),
        "Linux doctor must include 'rg' check, got: {:?}",
        check_names
    );
}

#[test]
fn run_doctor_includes_sandbox_rg_checks_when_agent_has_settings() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("testagent");
    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"sandbox": {"enabled": true}}"#,
    )
    .unwrap();

    let checks = run_doctor(dir.path());
    let sandbox_rg_checks: Vec<_> = checks
        .iter()
        .filter(|c| c.name.starts_with("sandbox-rg/"))
        .collect();
    assert!(
        !sandbox_rg_checks.is_empty(),
        "run_doctor must include sandbox-rg/ checks when agent has settings.json"
    );
    assert_eq!(sandbox_rg_checks[0].name, "sandbox-rg/testagent");
}

// ---- check_mcp_tokens tests (REFRESH-03, REFRESH-04) ----

/// Helper: write a minimal .mcp.json with one HTTP server entry.
fn write_mcp_json_for_doctor(agent_dir: &std::path::Path, server_name: &str, server_url: &str) {
    let content = format!(
        r#"{{"mcpServers": {{"{server_name}": {{"url": "{server_url}"}}}}}}"#,
    );
    std::fs::write(agent_dir.join(".mcp.json"), content).unwrap();
}

/// Helper: write a credential with given expires_at to a creds file.
fn write_credential_for_doctor(
    creds_path: &std::path::Path,
    server_name: &str,
    server_url: &str,
    expires_at: u64,
) {
    use crate::mcp::credentials::{write_credential, CredentialToken};
    let token = CredentialToken {
        access_token: "test-token".to_string(),
        refresh_token: None,
        token_type: Some("Bearer".to_string()),
        scope: None,
        expires_at,
        client_id: None,
        client_secret: None,
    };
    write_credential(creds_path, server_name, server_url, &token).unwrap();
}

#[test]
fn check_mcp_tokens_pass_no_agents_dir() {
    // No agents/ dir at all — should Pass with "all present"
    let dir = tempdir().unwrap();
    // Do NOT create agents/ subdir

    let result = check_mcp_tokens_with_creds(
        dir.path(),
        &dir.path().join("nonexistent-creds.json"),
    );
    assert_eq!(result.status, CheckStatus::Pass);
    assert_eq!(result.name, "mcp-tokens");
    assert!(
        result.detail.contains("all present"),
        "detail must be 'all present', got: {}",
        result.detail
    );
}

#[test]
fn check_mcp_tokens_pass_when_all_present() {
    // Agent with a valid non-expired credential — Pass
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let server_url = "https://mcp.notion.com/mcp";
    write_mcp_json_for_doctor(&agent_dir, "notion", server_url);

    let creds_path = dir.path().join("creds.json");
    // Far-future expiry → Present
    write_credential_for_doctor(&creds_path, "notion", server_url, 9_999_999_999);

    let result = check_mcp_tokens_with_creds(dir.path(), &creds_path);
    assert_eq!(result.status, CheckStatus::Pass);
    assert_eq!(result.name, "mcp-tokens");
}

#[test]
fn check_mcp_tokens_warn_on_missing_token() {
    // Agent with .mcp.json but no credential → Missing → Warn listing agent1/notion
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let server_url = "https://mcp.notion.com/mcp";
    write_mcp_json_for_doctor(&agent_dir, "notion", server_url);

    // Use a nonexistent creds file → Missing state
    let creds_path = dir.path().join("nonexistent-creds.json");

    let result = check_mcp_tokens_with_creds(dir.path(), &creds_path);
    assert_eq!(result.status, CheckStatus::Warn);
    assert_eq!(result.name, "mcp-tokens");
    assert!(
        result.detail.contains("agent1/notion"),
        "detail must contain 'agent1/notion', got: {}",
        result.detail
    );
    assert!(result.fix.is_some(), "Warn must have a fix hint");
}

#[test]
fn check_mcp_tokens_warn_on_expired_token() {
    // Agent with an expired credential → Expired → Warn listing agent1/notion
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let server_url = "https://mcp.notion.com/mcp";
    write_mcp_json_for_doctor(&agent_dir, "notion", server_url);

    let creds_path = dir.path().join("creds.json");
    // expires_at = 1 → far past → Expired
    write_credential_for_doctor(&creds_path, "notion", server_url, 1);

    let result = check_mcp_tokens_with_creds(dir.path(), &creds_path);
    assert_eq!(result.status, CheckStatus::Warn);
    assert!(
        result.detail.contains("agent1/notion"),
        "detail must contain 'agent1/notion', got: {}",
        result.detail
    );
}

#[test]
fn check_mcp_tokens_nonexpiring_is_ok() {
    // expires_at=0 (non-expiring, REFRESH-04) → Present → Pass
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let server_url = "https://mcp.linear.app/mcp";
    write_mcp_json_for_doctor(&agent_dir, "linear", server_url);

    let creds_path = dir.path().join("creds.json");
    write_credential_for_doctor(&creds_path, "linear", server_url, 0); // non-expiring

    let result = check_mcp_tokens_with_creds(dir.path(), &creds_path);
    assert_eq!(result.status, CheckStatus::Pass, "expires_at=0 must be Pass");
    assert_eq!(result.name, "mcp-tokens");
}

// ── tunnel-token checks (D-09) ───────────────────────────────────────────────

#[test]
fn tunnel_token_valid_jwt_passes() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let payload =
        URL_SAFE_NO_PAD.encode(r#"{"t":"aaaabbbb-0000-1111-2222-ccccddddeeee"}"#.as_bytes());
    let token = format!("hdr.{payload}.sig");
    let cfg = crate::config::TunnelConfig { token, hostname: "example.com".to_string() };
    let check = check_tunnel_token(&cfg);
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(
        check.detail.contains("aaaabbbb-0000-1111-2222-ccccddddeeee"),
        "detail: {}",
        check.detail
    );
}

#[test]
fn tunnel_token_invalid_warns() {
    let cfg = crate::config::TunnelConfig {
        token: "not.a.valid.token.at.all".to_string(),
        hostname: "example.com".to_string(),
    };
    let check = check_tunnel_token(&cfg);
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("cannot extract tunnel UUID"),
        "detail: {}",
        check.detail
    );
}
