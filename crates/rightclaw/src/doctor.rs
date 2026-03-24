use std::fmt;
use std::path::Path;

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
/// Checks 3 binaries in PATH (rightclaw, process-compose, claude)
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

    // Agent structure checks
    checks.extend(check_agent_structure(home));

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
}
