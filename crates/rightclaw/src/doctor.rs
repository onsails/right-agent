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
/// adds Linux-only sandbox dependency checks (bwrap, socat, bwrap smoke test,
/// ripgrep PATH check), and validates agent directory structure.
/// Unlike `verify_dependencies()`, doctor runs ALL checks and collects results
/// -- never short-circuits.
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

        // DOC-01: ripgrep PATH check (sandbox dependency)
        checks.push(check_rg_in_path());
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

    // DOC-02: per-agent settings.json ripgrep.command validation (cross-platform)
    checks.extend(check_ripgrep_in_settings(home));

    // Tunnel credentials file existence check — only when tunnel is configured
    if let Ok(global_cfg) = crate::config::read_global_config(home)
        && let Some(ref tunnel_cfg) = global_cfg.tunnel
    {
        checks.push(check_tunnel_credentials_file(tunnel_cfg));
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

/// Check if ripgrep (`rg`) is available in PATH. (DOC-01)
///
/// Uses Warn (not Fail) when absent — ripgrep is a sandbox dependency but its
/// absence is recoverable by reinstalling and running `rightclaw up` again.
fn check_rg_in_path() -> DoctorCheck {
    let raw = check_binary(
        "rg",
        Some("Install ripgrep: nix profile install nixpkgs#ripgrep / apt install ripgrep / brew install ripgrep"),
    );
    DoctorCheck {
        status: if raw.status == CheckStatus::Pass {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        ..raw
    }
}

/// Validate per-agent settings.json sandbox.ripgrep.command. (DOC-02)
///
/// For each agent directory under `home/agents/`, checks that:
/// - `.claude/settings.json` exists
/// - The JSON is valid
/// - `sandbox.ripgrep.command` key is present
/// - The path it points to exists as a file on disk
///
/// Cross-platform — not gated to Linux.
/// Returns an empty Vec when the agents directory does not exist.
fn check_ripgrep_in_settings(home: &Path) -> Vec<DoctorCheck> {
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

        let check_name = format!("sandbox-rg/{name}");
        let settings_path = path.join(".claude").join("settings.json");

        if !settings_path.exists() {
            checks.push(DoctorCheck {
                name: check_name,
                status: CheckStatus::Warn,
                detail: "settings.json not found".to_string(),
                fix: Some("Run `rightclaw up` to generate agent settings".to_string()),
            });
            continue;
        }

        let content = match std::fs::read_to_string(&settings_path) {
            Ok(c) => c,
            Err(e) => {
                checks.push(DoctorCheck {
                    name: check_name,
                    status: CheckStatus::Warn,
                    detail: format!("cannot read settings.json: {e}"),
                    fix: None,
                });
                continue;
            }
        };

        let parsed: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                checks.push(DoctorCheck {
                    name: check_name,
                    status: CheckStatus::Warn,
                    detail: "settings.json is not valid JSON".to_string(),
                    fix: Some("Run `rightclaw up` to regenerate settings".to_string()),
                });
                continue;
            }
        };

        match parsed["sandbox"]["ripgrep"]["command"].as_str() {
            None => {
                checks.push(DoctorCheck {
                    name: check_name,
                    status: CheckStatus::Warn,
                    detail: "sandbox.ripgrep.command absent -- CC sandbox will fail at launch"
                        .to_string(),
                    fix: Some(
                        "Ensure ripgrep is installed, then run `rightclaw up` to regenerate settings"
                            .to_string(),
                    ),
                });
            }
            Some(cmd) => {
                if std::path::Path::new(cmd).is_file() {
                    checks.push(DoctorCheck {
                        name: check_name,
                        status: CheckStatus::Pass,
                        detail: cmd.to_string(),
                        fix: None,
                    });
                } else {
                    checks.push(DoctorCheck {
                        name: check_name,
                        status: CheckStatus::Warn,
                        detail: format!(
                            "sandbox.ripgrep.command points to non-existent path: {cmd}"
                        ),
                        fix: Some(
                            "Reinstall ripgrep and run `rightclaw up` to regenerate settings"
                                .to_string(),
                        ),
                    });
                }
            }
        }
    }

    checks
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
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
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
    })
}

/// Check that the credentials file stored in TunnelConfig actually exists on disk.
///
/// Warn severity — missing file means cloudflared will fail at `rightclaw up` time.
fn check_tunnel_credentials_file(tunnel_cfg: &crate::config::TunnelConfig) -> DoctorCheck {
    if tunnel_cfg.credentials_file.exists() {
        DoctorCheck {
            name: "tunnel-credentials".to_string(),
            status: CheckStatus::Pass,
            detail: format!(
                "credentials file present at {}",
                tunnel_cfg.credentials_file.display()
            ),
            fix: None,
        }
    } else {
        DoctorCheck {
            name: "tunnel-credentials".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "credentials file not found at {}",
                tunnel_cfg.credentials_file.display()
            ),
            fix: Some(
                "re-run `rightclaw init --tunnel-credentials-file PATH --tunnel-hostname HOSTNAME` to restore"
                    .to_string(),
            ),
        }
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
#[path = "doctor_tests.rs"]
mod tests;
