use std::fmt;
use std::path::Path;

const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";

/// Status of a single doctor check.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
}

/// A single diagnostic check result.
#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub fix: Option<String>,
}

impl fmt::Display for DoctorCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let icon = match self.status {
            CheckStatus::Pass => "ok",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Warn => "warn",
        };
        write!(f, "  {:<20} {:<6} {}", self.name, icon, self.detail)?;
        if let Some(ref fix) = self.fix {
            write!(f, "\n    fix: {fix}")?;
        }
        Ok(())
    }
}

/// Run all doctor checks against the given RightClaw home directory.
///
/// Checks 3 core binaries in PATH (rightclaw, process-compose, claude),
/// adds Linux-only sandbox dependency checks (bwrap, socat, bwrap smoke test),
/// and validates agent directory structure. Unlike `verify_dependencies()`,
/// doctor runs ALL checks and collects results -- never short-circuits.
pub fn run_doctor(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = vec![
        check_binary(
            "rightclaw",
            Some("https://github.com/onsails/rightclaw"),
        ),
        check_binary(
            "process-compose",
            Some("https://f1bonacc1.github.io/process-compose/installation/"),
        ),
        check_binary(
            "claude",
            Some("https://docs.anthropic.com/en/docs/claude-code"),
        ),
    ];

    // Linux-only sandbox dependency checks
    if std::env::consts::OS == "linux" {
        let bwrap_check = check_binary(
            "bwrap",
            Some("Install bubblewrap: sudo apt install bubblewrap (or dnf/pacman)"),
        );
        let bwrap_found = bwrap_check.status == CheckStatus::Pass;
        checks.push(bwrap_check);

        checks.push(check_binary(
            "socat",
            Some("Install socat: sudo apt install socat (or dnf/pacman)"),
        ));

        // Only run smoke test if bwrap binary was found
        if bwrap_found {
            checks.push(check_bwrap_sandbox());
        }
    }

    // Agent structure checks
    checks.extend(check_agent_structure(home));

    // Telegram webhook checks — warn when active webhook would conflict with long-polling (PC-05).
    checks.extend(check_webhook_info_for_agents(home));

    // sqlite3 binary check — Warn (non-fatal): bundled SQLite in rightclaw binary makes
    // sqlite3 optional. Present on all standard macOS/Linux installs. (Phase 16, DOCTOR-01).
    {
        let raw = check_binary("sqlite3", None);
        checks.push(DoctorCheck {
            status: if raw.status == CheckStatus::Pass {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            ..raw
        });
    }

    // Managed settings conflict check — cross-platform, D-05.
    // Only emits a check if the file exists (D-08: silent skip when absent).
    if let Some(check) = check_managed_settings(MANAGED_SETTINGS_PATH) {
        checks.push(check);
    }

    checks
}

/// Check if a binary is available in PATH.
///
/// Tries the primary name first, then any alternatives (e.g. `claude-bun` for `claude`).
fn check_binary(name: &str, fix_hint: Option<&str>) -> DoctorCheck {
    let alternatives = match name {
        "claude" => &["claude", "claude-bun"][..],
        _ => std::slice::from_ref(&name),
    };

    for alt in alternatives {
        if let Ok(path) = which::which(alt) {
            let detail = if *alt != name {
                format!("{} (as {})", path.display(), alt)
            } else {
                path.display().to_string()
            };
            return DoctorCheck {
                name: name.to_string(),
                status: CheckStatus::Pass,
                detail,
                fix: None,
            };
        }
    }

    DoctorCheck {
        name: name.to_string(),
        status: CheckStatus::Fail,
        detail: "not found in PATH".to_string(),
        fix: fix_hint.map(|s| s.to_string()),
    }
}

/// Validate agent directory structure.
///
/// Checks that agents/ exists and contains at least one valid agent
/// (directory with IDENTITY.md).
fn check_agent_structure(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let agents_dir = home.join("agents");

    if !agents_dir.exists() {
        checks.push(DoctorCheck {
            name: "agents/".to_string(),
            status: CheckStatus::Fail,
            detail: "agents directory not found".to_string(),
            fix: Some("Run `rightclaw init` to create the default agent".to_string()),
        });
        return checks;
    }

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(e) => {
            checks.push(DoctorCheck {
                name: "agents/".to_string(),
                status: CheckStatus::Fail,
                detail: format!("cannot read agents directory: {e}"),
                fix: None,
            });
            return checks;
        }
    };

    let mut valid_agents = 0u32;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let identity_exists = path.join("IDENTITY.md").exists();
        let bootstrap_exists = path.join("BOOTSTRAP.md").exists();

        if identity_exists {
            valid_agents += 1;
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Pass,
                detail: "valid agent".to_string(),
                fix: None,
            });
        } else {
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Fail,
                detail: "missing IDENTITY.md".to_string(),
                fix: Some("Each agent needs IDENTITY.md".to_string()),
            });
        }

        if bootstrap_exists {
            checks.push(DoctorCheck {
                name: format!("agents/{name}/BOOTSTRAP.md"),
                status: CheckStatus::Warn,
                detail: "first-run onboarding pending".to_string(),
                fix: Some("Launch the agent to complete onboarding".to_string()),
            });
        }
    }

    if valid_agents == 0 {
        checks.push(DoctorCheck {
            name: "agents/".to_string(),
            status: CheckStatus::Fail,
            detail: "no valid agents found".to_string(),
            fix: Some("Run `rightclaw init` to create the default agent".to_string()),
        });
    }

    checks
}

/// Check if bubblewrap sandbox works by running a smoke test.
///
/// Runs `bwrap --ro-bind / / --unshare-net --dev /dev true` which exercises the
/// same code path Claude Code's sandbox-runtime uses. Must include `--unshare-net`
/// to detect AppArmor restrictions on network namespace creation (RTM_NEWADDR).
fn check_bwrap_sandbox() -> DoctorCheck {
    let result = std::process::Command::new("bwrap")
        .args([
            "--ro-bind", "/", "/", "--unshare-net", "--dev", "/dev", "true",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Pass,
            detail: "bubblewrap sandbox functional".to_string(),
            fix: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = if stderr.contains("RTM_NEWADDR")
                || stderr.contains("Operation not permitted")
            {
                "AppArmor restricts bubblewrap user namespaces".to_string()
            } else if stderr.contains("No permissions") {
                "unprivileged user namespaces disabled".to_string()
            } else {
                format!("bubblewrap sandbox test failed: {}", stderr.trim())
            };
            DoctorCheck {
                name: "bwrap-sandbox".to_string(),
                status: CheckStatus::Fail,
                detail,
                fix: Some(bwrap_fix_guidance()),
            }
        }
        Err(e) => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Fail,
            detail: format!("failed to run bwrap smoke test: {e}"),
            fix: Some(bwrap_fix_guidance()),
        },
    }
}

/// Check if /etc/claude-code/managed-settings.json exists and warn about potential conflicts.
///
/// Returns None when the file is absent (D-08: silent skip).
/// Returns Warn with rich detail when allowManagedDomainsOnly:true (D-06).
/// Returns Warn with generic detail for any other content (D-07).
fn check_managed_settings(path: &str) -> Option<DoctorCheck> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None, // D-08: silent skip when file absent or unreadable
    };

    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&content);
    let (detail, fix) = match parsed {
        Ok(ref v)
            if v.get("allowManagedDomainsOnly")
                .and_then(|v| v.as_bool())
                == Some(true) =>
        {
            // D-06: strict mode active
            (
                "allowManagedDomainsOnly:true \u{2014} per-agent allowedDomains may be overridden by system policy"
                    .to_string(),
                Some(
                    "Review /etc/claude-code/managed-settings.json \u{2014} enabled via: sudo rightclaw config strict-sandbox"
                        .to_string(),
                ),
            )
        }
        _ => {
            // D-07: file exists but content unexpected, unparseable, or flag absent/false
            (
                "managed-settings.json found \u{2014} content may affect agent sandbox behavior"
                    .to_string(),
                Some("Review /etc/claude-code/managed-settings.json".to_string()),
            )
        }
    };

    Some(DoctorCheck {
        name: "managed-settings".to_string(),
        status: CheckStatus::Warn,
        detail,
        fix,
    })
}

/// Check Telegram webhook status for all agents that have a configured token.
///
/// For each agent with a telegram_token or telegram_token_file, calls the
/// Telegram getWebhookInfo API. Emits:
/// - Pass when no webhook is active (result.url is empty)
/// - Warn when an active webhook is found (would compete with long-polling)
/// - Warn when the HTTP check fails (skipped gracefully)
///
/// Agents without a telegram token produce no check (silent skip, PC-05).
fn check_webhook_info_for_agents(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = home.join("agents");
    if !agents_dir.exists() {
        return vec![];
    }

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut checks = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Parse agent.yaml — skip agents we can't read
        let config = match crate::agent::discovery::parse_agent_config(&path) {
            Ok(Some(c)) => c,
            Ok(None) | Err(_) => continue,
        };

        // Resolve telegram token inline.
        // TODO: use crate::codegen::telegram::resolve_telegram_token after Plan 01 merges
        // (resolve_telegram_token will be pub(crate) after Plan 01).
        let token = resolve_token_from_config(&path, &config);
        let token = match token {
            Some(t) => t,
            None => continue, // No telegram token configured — skip silently
        };

        checks.push(make_webhook_check(&name, fetch_webhook_url(&token)));
    }

    checks
}

/// Inline token resolver for doctor.rs.
///
/// Duplicates the logic from codegen::telegram::resolve_telegram_token.
/// TODO: replace with `crate::codegen::telegram::resolve_telegram_token` after Plan 01 makes it pub(crate).
fn resolve_token_from_config(
    agent_path: &Path,
    config: &crate::agent::types::AgentConfig,
) -> Option<String> {
    if let Some(ref file_path) = config.telegram_token_file {
        let abs = agent_path.join(file_path);
        let content = std::fs::read_to_string(&abs).ok()?;
        let trimmed = content.trim();
        let token = trimmed
            .strip_prefix("TELEGRAM_BOT_TOKEN=")
            .unwrap_or(trimmed);
        return Some(token.to_string());
    }

    config.telegram_token.clone()
}

/// Build a DoctorCheck from a webhook URL fetch result.
///
/// Extracted for testability — callers can pass any Ok/Err result
/// to verify the check construction logic without network calls.
fn make_webhook_check(agent_name: &str, webhook_url_result: Result<String, String>) -> DoctorCheck {
    match webhook_url_result {
        Ok(url) if url.is_empty() => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Pass,
            detail: "no active webhook".to_string(),
            fix: None,
        },
        Ok(url) => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Warn,
            detail: format!("active webhook found: {url}"),
            fix: Some(format!(
                "Run rightclaw bot --agent {agent_name} to clear the webhook, or call deleteWebhook manually"
            )),
        },
        Err(e) => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Warn,
            detail: format!("webhook check skipped: {e}"),
            fix: None,
        },
    }
}

/// Fetch the active webhook URL for a Telegram bot token.
///
/// Returns Ok("") when no webhook is active, Ok(url) when one is set,
/// Err(description) when the HTTP call fails.
fn fetch_webhook_url(token: &str) -> Result<String, String> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to create runtime: {e}"))?;
    rt.block_on(async {
        let url = format!("https://api.telegram.org/bot{token}/getWebhookInfo");
        let resp = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {e}"))?;
        Ok(body["result"]["url"]
            .as_str()
            .unwrap_or("")
            .to_string())
    })
}

/// Generate fix guidance for bubblewrap sandbox failures.
///
/// Primary fix: per-application AppArmor profile (targeted, secure).
/// Secondary fix: system-wide sysctl disable (temporary workaround).
fn bwrap_fix_guidance() -> String {
    "\
Create an AppArmor profile for bwrap:

  sudo tee /etc/apparmor.d/bwrap << 'PROFILE'
  abi <abi/4.0>,
  include <tunables/global>

  profile bwrap /usr/bin/bwrap flags=(unconfined) {
    userns,
    include if exists <local/bwrap>
  }
  PROFILE

  sudo apparmor_parser -r /etc/apparmor.d/bwrap

Or temporarily disable the restriction:

  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0

For persistent fix, add to /etc/sysctl.d/60-bwrap-userns.conf:

  kernel.apparmor_restrict_unprivileged_userns=0

See: https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces"
        .to_string()
}

#[cfg(test)]
mod tests {
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
}
