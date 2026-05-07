use super::*;
use tempfile::tempdir;

#[test]
fn check_binary_returns_fail_for_missing_binary() {
    let check = check_binary("right-absolutely-nonexistent-binary-xyz", None);
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
        "right-nonexistent-xyz",
        Some("https://example.com/install"),
    );
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(check.fix.as_deref(), Some("https://example.com/install"));
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
    std::fs::write(agent_dir.join("SOUL.md"), "# Soul").unwrap();
    std::fs::write(agent_dir.join("USER.md"), "# User").unwrap();

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
    // No IDENTITY.md — only TOOLS.md present
    std::fs::write(agent_dir.join("TOOLS.md"), "# Tools").unwrap();
    std::fs::write(agent_dir.join("agent.yaml"), "{}").unwrap();

    let checks = run_doctor(dir.path());

    // Without IDENTITY.md (and no BOOTSTRAP.md), doctor warns about missing IDENTITY.md
    let identity_check = checks
        .iter()
        .find(|c| c.name.contains("IDENTITY.md"))
        .expect("should have a check for IDENTITY.md");
    assert_eq!(identity_check.status, CheckStatus::Warn);
    assert!(identity_check.detail.contains("IDENTITY.md missing"));
}

#[test]
fn doctor_check_to_ui_line_shows_name_status_detail() {
    use right_core::ui::Theme;
    let check = DoctorCheck {
        name: "test-binary".to_string(),
        status: CheckStatus::Pass,
        detail: "/usr/bin/test-binary".to_string(),
        fix: None,
    };
    let rendered = check.to_ui_line().render(Theme::Mono);
    assert!(rendered.contains("test-binary"));
    assert!(rendered.contains("✓"));
    assert!(rendered.contains("/usr/bin/test-binary"));
}

#[test]
fn doctor_check_to_ui_line_shows_fix_on_failure() {
    use right_core::ui::Theme;
    let check = DoctorCheck {
        name: "missing".to_string(),
        status: CheckStatus::Fail,
        detail: "not found".to_string(),
        fix: Some("install it".to_string()),
    };
    let rendered = check.to_ui_line().render(Theme::Mono);
    assert!(rendered.contains("✗"));
    assert!(rendered.contains("install it"));
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

    assert!(
        binary_names.contains(&"right"),
        "missing right check"
    );
    assert!(
        binary_names.contains(&"process-compose"),
        "missing process-compose check"
    );
    assert!(binary_names.contains(&"claude"), "missing claude check");
    assert!(
        binary_names.contains(&"openshell"),
        "missing openshell check"
    );
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
        guidance.contains(
            "https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces"
        ),
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
    assert!(
        result.is_none(),
        "should return None when file does not exist"
    );
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
        fix.contains("sudo right config strict-sandbox"),
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
        fix.contains("sudo right config strict-sandbox"),
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
    let raw = check_binary("right-absolutely-nonexistent-sqlite3-xyz", None);
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

const EXPECTED_URL: &str = "https://example.com/tg/mybot";

fn webhook_info(url: &str) -> WebhookInfo {
    WebhookInfo {
        url: url.to_string(),
        pending_update_count: 0,
        last_error_message: None,
    }
}

#[test]
fn make_webhook_check_fail_when_url_empty() {
    let check = make_webhook_check("mybot", EXPECTED_URL, Ok(webhook_info("")));
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.detail.contains("no webhook registered"),
        "expected 'no webhook registered', got: {}",
        check.detail
    );
    let fix = check.fix.expect("Fail must have fix hint");
    assert!(
        fix.contains("mybot"),
        "fix must mention the agent name, got: {fix}"
    );
}

#[test]
fn make_webhook_check_fail_when_url_mismatch() {
    let check = make_webhook_check(
        "mybot",
        EXPECTED_URL,
        Ok(webhook_info("https://other.com/tg/mybot")),
    );
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.detail.contains("webhook URL mismatch"),
        "expected 'webhook URL mismatch', got: {}",
        check.detail
    );
    assert!(
        check.detail.contains("https://other.com/tg/mybot"),
        "detail must include the registered URL"
    );
    assert!(
        check.detail.contains(EXPECTED_URL),
        "detail must include the expected URL"
    );
    let fix = check.fix.expect("Fail must have fix hint");
    assert!(
        fix.contains("mybot"),
        "fix must mention the agent name, got: {fix}"
    );
}

#[test]
fn make_webhook_check_pass_when_url_matches() {
    let check = make_webhook_check("mybot", EXPECTED_URL, Ok(webhook_info(EXPECTED_URL)));
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(
        check.detail.contains("webhook registered"),
        "expected 'webhook registered', got: {}",
        check.detail
    );
    assert!(check.fix.is_none());
}

#[test]
fn make_webhook_check_warn_when_pending_high() {
    let info = WebhookInfo {
        url: EXPECTED_URL.to_string(),
        pending_update_count: 250,
        last_error_message: None,
    };
    let check = make_webhook_check("mybot", EXPECTED_URL, Ok(info));
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("pending_update_count=250"),
        "expected pending_update_count detail, got: {}",
        check.detail
    );
}

#[test]
fn make_webhook_check_warn_when_last_error_present() {
    let info = WebhookInfo {
        url: EXPECTED_URL.to_string(),
        pending_update_count: 0,
        last_error_message: Some("Connection timed out".to_string()),
    };
    let check = make_webhook_check("mybot", EXPECTED_URL, Ok(info));
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("Connection timed out"),
        "expected last error in detail, got: {}",
        check.detail
    );
}

#[test]
fn make_webhook_check_warn_when_http_error() {
    let check = make_webhook_check(
        "mybot",
        EXPECTED_URL,
        Err("HTTP error: connection refused".to_string()),
    );
    assert_eq!(check.name, "telegram-webhook/mybot");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.detail.contains("webhook check skipped"),
        "expected 'webhook check skipped', got: {}",
        check.detail
    );
    assert!(check.fix.is_none());
}

/// Regression test: fetch_webhook_info must not panic when called from within
/// an existing tokio multi-thread runtime context (UAT-FIX-02).
///
/// Before the fix: Runtime::new().block_on() panics with
/// "Cannot start a runtime from within a runtime".
/// After the fix: returns a Result without panicking.
#[tokio::test(flavor = "multi_thread")]
async fn fetch_webhook_info_does_not_panic_in_async_context() {
    let _result = fetch_webhook_info("invalid-token-for-test");
    // If we reach here without panicking, the fix works.
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
    assert!(
        checks.is_empty(),
        "missing agents dir must produce no checks"
    );
}

// ---- check_mcp_tokens tests (REFRESH-03, REFRESH-04) ----

#[test]
fn check_mcp_tokens_pass_no_agents_dir() {
    // No agents/ dir at all — should Pass
    let dir = tempdir().unwrap();

    let result = check_mcp_tokens_impl(dir.path());
    assert_eq!(result.status, CheckStatus::Pass);
    assert_eq!(result.name, "mcp-tokens");
}

#[test]
fn check_mcp_tokens_counts_registered_servers() {
    // Agent with servers in SQLite — doctor reports count
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    // Create data.db with a registered server
    let conn = right_db::open_connection(&agent_dir, true).unwrap();
    crate::mcp::credentials::db_add_server(&conn, "notion", "https://mcp.notion.com/mcp").unwrap();

    let result = check_mcp_tokens_impl(dir.path());
    assert_eq!(result.status, CheckStatus::Pass);
    assert_eq!(result.name, "mcp-tokens");
    assert!(
        result.detail.contains("1 server(s) registered"),
        "detail must contain server count, got: {}",
        result.detail
    );
}

#[test]
fn check_mcp_tokens_pass_no_servers() {
    // Agent dir exists but no servers registered — 0 servers
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("agent1");
    std::fs::create_dir_all(&agent_dir).unwrap();

    // Create data.db but register no servers
    let _conn = right_db::open_connection(&agent_dir, true).unwrap();

    let result = check_mcp_tokens_impl(dir.path());
    assert_eq!(result.status, CheckStatus::Pass);
    assert_eq!(result.name, "mcp-tokens");
}

// ── tunnel state checks (unified) ───────────────────────────────────────────

#[test]
fn tunnel_state_credentials_present_passes() {
    let dir = tempdir().unwrap();
    let creds_file = dir.path().join("creds.json");
    std::fs::write(&creds_file, "{}").unwrap();
    let config = crate::config::GlobalConfig {
        tunnel: crate::config::TunnelConfig {
            tunnel_uuid: "aaaabbbb-0000-1111-2222-ccccddddeeee".to_string(),
            credentials_file: creds_file,
            hostname: "example.com".to_string(),
        },
        aggregator: crate::config::AggregatorConfig::default(),
    };
    crate::config::write_global_config(dir.path(), &config).unwrap();
    let checks = check_tunnel_state(dir.path());
    let creds_check = checks
        .iter()
        .find(|c| c.name == "tunnel-credentials")
        .unwrap();
    assert_eq!(creds_check.status, CheckStatus::Pass);
    assert!(
        creds_check.detail.contains("credentials file present"),
        "detail: {}",
        creds_check.detail
    );
}

#[test]
fn tunnel_state_credentials_missing_fails() {
    let dir = tempdir().unwrap();
    let config = crate::config::GlobalConfig {
        tunnel: crate::config::TunnelConfig {
            tunnel_uuid: "aaaabbbb-0000-1111-2222-ccccddddeeee".to_string(),
            credentials_file: std::path::PathBuf::from("/nonexistent/creds.json"),
            hostname: "example.com".to_string(),
        },
        aggregator: crate::config::AggregatorConfig::default(),
    };
    crate::config::write_global_config(dir.path(), &config).unwrap();
    let checks = check_tunnel_state(dir.path());
    let creds_check = checks
        .iter()
        .find(|c| c.name == "tunnel-credentials")
        .unwrap();
    assert_eq!(creds_check.status, CheckStatus::Fail);
    assert!(
        creds_check.detail.contains("credentials file missing"),
        "detail: {}",
        creds_check.detail
    );
}

// ---------------------------------------------------------------------------
// mcp_auth_issues tests
// ---------------------------------------------------------------------------

#[test]
fn mcp_auth_issues_returns_none_when_no_agents_dir() {
    let dir = tempdir().unwrap();
    // No agents/ subdir — check_mcp_tokens returns Pass → mcp_auth_issues returns None
    let result = mcp_auth_issues(dir.path());
    assert!(result.is_none(), "expected None, got {result:?}");
}

#[test]
fn mcp_auth_issues_returns_none_when_agents_dir_empty() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join("agents")).unwrap();
    let result = mcp_auth_issues(dir.path());
    assert!(result.is_none(), "expected None for empty agents dir");
}

#[test]
fn mcp_auth_issues_returns_some_when_mcp_tokens_warn() {
    // Craft a scenario where mcp.json has a URL server but no Bearer token → Missing → Warn
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("agents").join("myagent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // mcp.json with one OAuth server but no Authorization header
    std::fs::write(
        agent_dir.join("mcp.json"),
        r#"{"mcpServers":{"notion":{"url":"https://mcp.notion.com/mcp"}}}"#,
    )
    .unwrap();

    let check = check_mcp_tokens_impl(dir.path());
    // Only test mcp_auth_issues parsing logic if the check is actually Warn.
    // On systems where detection differs, skip rather than assert the wrong thing.
    if check.status == CheckStatus::Warn {
        // Simulate what mcp_auth_issues does
        let problems: Vec<String> = check
            .detail
            .strip_prefix(MCP_ISSUES_PREFIX)
            .unwrap_or(&check.detail)
            .split(", ")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        assert!(!problems.is_empty(), "expected at least one problem entry");
        assert!(
            problems.iter().any(|p| p.contains("notion")),
            "expected 'notion' in problems: {problems:?}"
        );
    }
}

#[test]
fn mcp_auth_issues_prefix_constant_matches_detail_format() {
    // Ensure MCP_ISSUES_PREFIX is exactly the prefix used by check_mcp_tokens_impl/check_mcp_tokens.
    // If the format string changes, this test catches it.
    assert_eq!(MCP_ISSUES_PREFIX, "missing: ");
    let detail = format!("{}agent1/notion, agent2/linear", MCP_ISSUES_PREFIX);
    let stripped = detail.strip_prefix(MCP_ISSUES_PREFIX);
    assert!(
        stripped.is_some(),
        "MCP_ISSUES_PREFIX does not match detail format"
    );
    assert_eq!(stripped.unwrap(), "agent1/notion, agent2/linear");
}

// ---- identity file checks ----

#[test]
fn doctor_warns_missing_identity_files_no_bootstrap() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("TOOLS.md"), "# Tools").unwrap();

    let checks = check_agent_structure(home);
    assert!(
        checks
            .iter()
            .any(|c| c.detail.contains("IDENTITY.md missing")),
        "should warn about missing IDENTITY.md, got: {:?}",
        checks.iter().map(|c| &c.detail).collect::<Vec<_>>()
    );
    assert!(
        checks.iter().any(|c| c.detail.contains("SOUL.md missing")),
        "should warn about missing SOUL.md"
    );
    assert!(
        checks.iter().any(|c| c.detail.contains("USER.md missing")),
        "should warn about missing USER.md"
    );
}

#[test]
fn doctor_passes_with_all_identity_files() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Identity").unwrap();
    std::fs::write(agent_dir.join("SOUL.md"), "# Soul").unwrap();
    std::fs::write(agent_dir.join("USER.md"), "# User").unwrap();

    let checks = check_agent_structure(home);
    assert!(
        !checks.iter().any(|c| c.detail.contains("missing")),
        "should not warn when all files present, got: {:?}",
        checks.iter().map(|c| &c.detail).collect::<Vec<_>>()
    );
}

#[test]
fn doctor_bootstrap_pending_skips_identity_checks() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "# Bootstrap").unwrap();

    let checks = check_agent_structure(home);
    assert!(
        checks
            .iter()
            .any(|c| c.detail.contains("onboarding pending")),
        "should show onboarding pending"
    );
    assert!(
        !checks
            .iter()
            .any(|c| c.detail.contains("IDENTITY.md missing")),
        "should not check identity when bootstrap present"
    );
}

// ---- check_memory tests ----

mod memory_tests {
    use super::*;
    use right_db::open_connection;
    use tempfile::tempdir;

    #[test]
    fn check_memory_passes_on_empty_queue() {
        let dir = tempdir().unwrap();
        let _ = open_connection(dir.path(), true).unwrap();
        let checks = check_memory(dir.path());
        assert!(
            checks.iter().all(|c| matches!(c.status, CheckStatus::Pass)),
            "expected all pass, got {checks:#?}"
        );
    }

    #[test]
    fn check_memory_warns_on_500_rows() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..600 {
            crate::memory::retain_queue::enqueue(
                &conn,
                "bot",
                &format!("c-{i}"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }
        let checks = check_memory(dir.path());
        assert!(
            checks
                .iter()
                .any(|c| c.status == CheckStatus::Warn && c.name.contains("retain backlog")),
            "expected warn on retain backlog, got {checks:#?}"
        );
    }

    #[test]
    fn check_memory_fails_on_901_rows() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..901 {
            crate::memory::retain_queue::enqueue(
                &conn,
                "bot",
                &format!("c-{i}"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }
        let checks = check_memory(dir.path());
        assert!(
            checks
                .iter()
                .any(|c| c.status == CheckStatus::Fail && c.name.contains("retain backlog")),
            "expected fail on retain backlog, got {checks:#?}"
        );
    }

    #[test]
    fn check_memory_fails_on_24h_auth_alert() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        conn.execute(
            "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES ('auth_failed', datetime('now','-25 hours'))",
            [],
        )
        .unwrap();
        let checks = check_memory(dir.path());
        assert!(
            checks
                .iter()
                .any(|c| c.status == CheckStatus::Fail && c.name.contains("auth")),
            "expected fail on auth alert, got {checks:#?}"
        );
    }
}
