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
/// Checks 4 binaries in PATH (rightclaw, process-compose, openshell, claude)
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
            "openshell",
            Some("curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh"),
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
fn check_binary(name: &str, fix_hint: Option<&str>) -> DoctorCheck {
    match which::which(name) {
        Ok(path) => DoctorCheck {
            name: name.to_string(),
            status: CheckStatus::Pass,
            detail: path.display().to_string(),
            fix: None,
        },
        Err(_) => DoctorCheck {
            name: name.to_string(),
            status: CheckStatus::Fail,
            detail: "not found in PATH".to_string(),
            fix: fix_hint.map(|s| s.to_string()),
        },
    }
}

/// Validate agent directory structure.
///
/// Checks that agents/ exists and contains at least one valid agent
/// (directory with IDENTITY.md + policy.yaml).
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
        let policy_exists = path.join("policy.yaml").exists();
        let bootstrap_exists = path.join("BOOTSTRAP.md").exists();

        if identity_exists && policy_exists {
            valid_agents += 1;
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Pass,
                detail: "valid agent".to_string(),
                fix: None,
            });
        } else {
            let mut missing = Vec::new();
            if !identity_exists {
                missing.push("IDENTITY.md");
            }
            if !policy_exists {
                missing.push("policy.yaml");
            }
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Fail,
                detail: format!("missing {}", missing.join(", ")),
                fix: Some("Each agent needs IDENTITY.md and policy.yaml".to_string()),
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
        std::fs::write(agent_dir.join("policy.yaml"), "version: 1").unwrap();

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
        std::fs::write(agent_dir.join("policy.yaml"), "version: 1").unwrap();
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
    fn run_doctor_reports_missing_required_files() {
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("agents").join("broken");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Only IDENTITY.md, no policy.yaml
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Broken").unwrap();

        let checks = run_doctor(dir.path());

        let agent_check = checks
            .iter()
            .find(|c| c.name.contains("agents/broken/"))
            .expect("should have a check for agents/broken/");
        assert_eq!(agent_check.status, CheckStatus::Fail);
        assert!(agent_check.detail.contains("policy.yaml"));
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
    fn run_doctor_always_checks_all_four_binaries() {
        let dir = tempdir().unwrap();
        let checks = run_doctor(dir.path());

        let binary_names: Vec<&str> = checks
            .iter()
            .filter(|c| !c.name.starts_with("agents"))
            .map(|c| c.name.as_str())
            .collect();

        assert!(binary_names.contains(&"rightclaw"), "missing rightclaw check");
        assert!(binary_names.contains(&"process-compose"), "missing process-compose check");
        assert!(binary_names.contains(&"openshell"), "missing openshell check");
        assert!(binary_names.contains(&"claude"), "missing claude check");
    }
}
