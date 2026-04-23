use std::fmt;
use std::path::Path;

use owo_colors::OwoColorize;

const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";
const MCP_ISSUES_PREFIX: &str = "missing: ";

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
        let status_str = match self.status {
            CheckStatus::Pass => format!("{}", "ok".green()),
            CheckStatus::Fail => format!("{}", "FAIL".red().bold()),
            CheckStatus::Warn => format!("{}", "warn".yellow()),
        };
        write!(f, "  {:<24} {:<6} {}", self.name, status_str, self.detail)?;
        if let Some(ref fix) = self.fix {
            write!(f, "\n{:>32}{fix}", "fix: ".dimmed())?;
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
        check_binary("rightclaw", Some("https://github.com/onsails/rightclaw")),
        check_binary(
            "process-compose",
            Some("https://f1bonacc1.github.io/process-compose/installation/"),
        ),
        check_binary(
            "claude",
            Some("https://docs.anthropic.com/en/docs/claude-code"),
        ),
        check_binary("openshell", Some("https://github.com/NVIDIA/OpenShell")),
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

    // cloudflared binary check — Warn severity (D-03, OAUTH-04)
    checks.push(check_cloudflared_binary());

    // Tunnel config + credentials checks (unified).
    checks.extend(check_tunnel_state(home));

    // Tunnel health check — only when tunnel is configured.
    if let Ok(cfg) = crate::config::read_global_config(home)
        && cfg.tunnel.is_some()
    {
        checks.push(check_tunnel_health(home));
    }

    // STT checks — ffmpeg presence and model cache (Task 17).
    checks.extend(check_stt(home));

    // MCP token status check — Warn when any agent has missing/expired tokens (REFRESH-03)
    checks.push(check_mcp_tokens(home));

    // OpenShell mTLS certs check
    checks.push(check_openshell_mtls_certs());

    // OpenShell gateway health check (gRPC)
    checks.push(check_openshell_gateway_health());

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

/// Check whether the OpenShell sandbox for a given agent exists and is READY.
///
/// Returns `None` when OpenShell is not ready (certs missing, not installed) —
/// the caller skips the check silently in that case.
fn check_sandbox_for_agent(
    agent_name: &str,
    config: Option<&crate::agent::types::AgentConfig>,
) -> Option<DoctorCheck> {
    // Only check if OpenShell is available.
    let mtls_dir = match crate::openshell::preflight_check() {
        crate::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return None, // OpenShell not ready — skip sandbox check
    };

    let sandbox = config
        .map(|c| crate::openshell::resolve_sandbox_name(agent_name, c))
        .unwrap_or_else(|| crate::openshell::sandbox_name(agent_name));

    // Requires a tokio runtime — skip gracefully in sync test contexts.
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return None,
    };

    let result = tokio::task::block_in_place(|| {
        handle.block_on(async {
            let mut client = crate::openshell::connect_grpc(&mtls_dir).await?;
            crate::openshell::is_sandbox_ready(&mut client, &sandbox).await
        })
    });

    match result {
        Ok(true) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Pass,
            detail: format!("sandbox '{sandbox}' exists and READY"),
            fix: None,
        }),
        Ok(false) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Fail,
            detail: format!("sandbox '{sandbox}' not found"),
            fix: Some(format!(
                "Run `rightclaw agent init {agent_name}` to create it"
            )),
        }),
        Err(e) => Some(DoctorCheck {
            name: format!("sandbox/{agent_name}"),
            status: CheckStatus::Warn,
            detail: format!("sandbox check failed: {e:#}"),
            fix: None,
        }),
    }
}

/// Validate agent directory structure.
///
/// Checks that agents/ exists and contains at least one valid agent
/// (directory with IDENTITY.md).
fn check_agent_structure(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let agents_dir = crate::config::agents_dir(home);

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

        let agents_md_exists = path.join("AGENTS.md").exists();
        let identity_exists = path.join("IDENTITY.md").exists();
        let soul_exists = path.join("SOUL.md").exists();
        let user_exists = path.join("USER.md").exists();
        let bootstrap_exists = path.join("BOOTSTRAP.md").exists();

        // Parse agent.yaml to get config (for sandbox mode check etc.)
        let agent_config = crate::agent::discovery::parse_agent_config(&path)
            .ok()
            .flatten();

        if !agents_md_exists {
            checks.push(DoctorCheck {
                name: format!("agents/{name}/AGENTS.md"),
                status: CheckStatus::Fail,
                detail: "AGENTS.md missing".to_string(),
                fix: Some("Run `rightclaw init` or create AGENTS.md manually".to_string()),
            });
        }

        if bootstrap_exists {
            valid_agents += 1;
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Pass,
                detail: "valid agent (onboarding pending)".to_string(),
                fix: None,
            });
            checks.push(DoctorCheck {
                name: format!("agents/{name}/BOOTSTRAP.md"),
                status: CheckStatus::Warn,
                detail: "first-run onboarding pending".to_string(),
                fix: Some("Send a message to the agent to start onboarding".to_string()),
            });
        } else {
            // No bootstrap — check identity files.
            if identity_exists {
                valid_agents += 1;
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/"),
                    status: CheckStatus::Pass,
                    detail: "valid agent".to_string(),
                    fix: None,
                });
            }
            if !identity_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/IDENTITY.md"),
                    status: CheckStatus::Warn,
                    detail: "IDENTITY.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
            if !soul_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/SOUL.md"),
                    status: CheckStatus::Warn,
                    detail: "SOUL.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
            if !user_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/USER.md"),
                    status: CheckStatus::Warn,
                    detail: "USER.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
        }

        // Check sandbox existence for openshell agents.
        let is_openshell = agent_config
            .as_ref()
            .map(|c| {
                matches!(
                    c.sandbox_mode(),
                    crate::agent::types::SandboxMode::Openshell
                )
            })
            .unwrap_or(true); // default sandbox mode is openshell
        if is_openshell && let Some(check) = check_sandbox_for_agent(&name, agent_config.as_ref()) {
            checks.push(check);
        }

        // Memory layer health (queue size, oldest-row age, long-standing alerts).
        if path.join("data.db").exists() {
            for mut chk in check_memory(&path) {
                chk.name = format!("{name}/{}", chk.name);
                checks.push(chk);
            }
            for mut chk in check_cron_targets(&path) {
                chk.name = format!("{name}/{}", chk.name);
                checks.push(chk);
            }
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
            "--ro-bind",
            "/",
            "/",
            "--unshare-net",
            "--dev",
            "/dev",
            "true",
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
            let detail =
                if stderr.contains("RTM_NEWADDR") || stderr.contains("Operation not permitted") {
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
        Ok(ref v) if v.get("allowManagedDomainsOnly").and_then(|v| v.as_bool()) == Some(true) => {
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
/// For each agent with a telegram_token configured, calls the
/// Telegram getWebhookInfo API. Emits:
/// - Pass when no webhook is active (result.url is empty)
/// - Warn when an active webhook is found (would compete with long-polling)
/// - Warn when the HTTP check fails (skipped gracefully)
///
/// Agents without a telegram token produce no check (silent skip, PC-05).
fn check_webhook_info_for_agents(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = crate::config::agents_dir(home);
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
fn resolve_token_from_config(
    _agent_path: &Path,
    config: &crate::agent::types::AgentConfig,
) -> Option<String> {
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
            Ok(body["result"]["url"].as_str().unwrap_or("").to_string())
        })
    })
}

/// Check if `cloudflared` binary is available in PATH. (D-03, OAUTH-04)
///
/// Warn severity — cloudflared is optional for non-OAuth deployments.
/// Absence only becomes critical when OAuth callbacks via named tunnel are needed.
fn check_cloudflared_binary() -> DoctorCheck {
    let raw = check_binary(
        "cloudflared",
        Some(
            "install cloudflared: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/",
        ),
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

/// Check whether a cloudflare tunnel is configured in `<home>/config.yaml`. (D-03)
///
/// Warn severity — tunnel is optional for bots that don't use MCP OAuth.
/// When tunnel is absent, `/mcp auth` will fail at runtime but other commands work.
/// Unified tunnel config + credentials check.
fn check_tunnel_state(home: &Path) -> Vec<DoctorCheck> {
    let config = match crate::config::read_global_config(home) {
        Ok(cfg) => cfg,
        Err(e) => {
            return vec![DoctorCheck {
                name: "tunnel-config".to_string(),
                status: CheckStatus::Warn,
                detail: format!("failed to read config.yaml: {e:#}"),
                fix: None,
            }];
        }
    };

    let tunnel_cfg = match &config.tunnel {
        Some(t) => t,
        None => {
            return vec![DoctorCheck {
                name: "tunnel-config".to_string(),
                status: CheckStatus::Warn,
                detail: "no tunnel configured".to_string(),
                fix: Some("run `rightclaw config set` to configure tunnel".to_string()),
            }];
        }
    };

    let mut checks = vec![DoctorCheck {
        name: "tunnel-config".to_string(),
        status: CheckStatus::Pass,
        detail: format!("tunnel configured: {}", tunnel_cfg.hostname),
        fix: None,
    }];

    if tunnel_cfg.credentials_file.exists() {
        checks.push(DoctorCheck {
            name: "tunnel-credentials".to_string(),
            status: CheckStatus::Pass,
            detail: format!(
                "credentials file present at {}",
                tunnel_cfg.credentials_file.display()
            ),
            fix: None,
        });
    } else {
        checks.push(DoctorCheck {
            name: "tunnel-credentials".to_string(),
            status: CheckStatus::Warn,
            detail: format!(
                "credentials file missing: {}",
                tunnel_cfg.credentials_file.display()
            ),
            fix: Some("run `rightclaw config set` to reconfigure tunnel".to_string()),
        });
    }

    checks
}

/// Check tunnel reachability using the tunnel health module.
fn check_tunnel_health(home: &Path) -> DoctorCheck {
    use crate::tunnel::health::{TunnelState, check_tunnel};

    let state = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(check_tunnel(home))
    });

    match state {
        TunnelState::Healthy => DoctorCheck {
            name: "tunnel-health".to_string(),
            status: CheckStatus::Pass,
            detail: "tunnel reachable".to_string(),
            fix: None,
        },
        TunnelState::NotConfigured => DoctorCheck {
            name: "tunnel-health".to_string(),
            status: CheckStatus::Warn,
            detail: "skipped — no tunnel configured".to_string(),
            fix: None,
        },
        TunnelState::NotRunning => DoctorCheck {
            name: "tunnel-health".to_string(),
            status: CheckStatus::Warn,
            detail: "skipped — cloudflared not running".to_string(),
            fix: Some("run `rightclaw up` to start cloudflared".to_string()),
        },
        TunnelState::Unhealthy { reason } => DoctorCheck {
            name: "tunnel-health".to_string(),
            status: CheckStatus::Warn,
            detail: format!("hostname not reachable: {reason}"),
            fix: Some("check DNS and Cloudflare dashboard".to_string()),
        },
    }
}

/// Check MCP OAuth token status across all agents. (REFRESH-03)
///
/// Aggregates missing/expired tokens into a single Warn check.
/// Tokens with expires_at=0 (non-expiring) count as ok (REFRESH-04).
/// Only synchronous file I/O — no HTTP calls.
fn check_mcp_tokens_impl(home: &Path) -> DoctorCheck {
    let agents_dir = crate::config::agents_dir(home);

    if !agents_dir.exists() {
        return DoctorCheck {
            name: "mcp-tokens".to_string(),
            status: CheckStatus::Pass,
            detail: "all present".to_string(),
            fix: None,
        };
    }

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => {
            return DoctorCheck {
                name: "mcp-tokens".to_string(),
                status: CheckStatus::Pass,
                detail: "all present".to_string(),
                fix: None,
            };
        }
    };

    // Count registered servers across agents for diagnostic output.
    let mut total_servers = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let conn = match crate::memory::open_connection(&path, false) {
            Ok(c) => c,
            Err(_) => continue, // skip agents with unreadable DB
        };

        let servers = match crate::mcp::credentials::db_list_servers(&conn) {
            Ok(s) => s,
            Err(_) => continue,
        };
        total_servers += servers.len();
    }

    // Auth state is no longer tracked here (tokens live in the Aggregator's
    // oauth-state.json), so we always pass -- just report the count.
    DoctorCheck {
        name: "mcp-tokens".to_string(),
        status: CheckStatus::Pass,
        detail: format!("{total_servers} server(s) registered"),
        fix: None,
    }
}

/// Return MCP auth issues for display in `rightclaw up` before TUI takes over (D-13).
///
/// Returns `Some(problems)` when any agent has missing/expired MCP tokens, `None` when all ok.
/// Uses the same logic as the doctor mcp-tokens check.
pub fn mcp_auth_issues(home: &Path) -> Option<Vec<String>> {
    let check = check_mcp_tokens(home);
    if check.status == CheckStatus::Warn {
        // Extract the problem list from "missing/expired: agent1/notion, agent2/linear"
        let problems: Vec<String> = check
            .detail
            .strip_prefix(MCP_ISSUES_PREFIX)
            .unwrap_or(&check.detail)
            .split(", ")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        (!problems.is_empty()).then_some(problems)
    } else {
        None
    }
}

/// Check MCP token status across all agents — reads directly from mcp.json headers.
fn check_mcp_tokens(home: &Path) -> DoctorCheck {
    check_mcp_tokens_impl(home)
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

/// Check that OpenShell mTLS certificates exist.
///
/// Verifies ca.crt, tls.crt, tls.key in ~/.config/openshell/gateways/openshell/mtls/.
/// Severity: Fail — without mTLS certs, gRPC connection to OpenShell gateway is impossible.
fn check_openshell_mtls_certs() -> DoctorCheck {
    let mtls_dir = crate::openshell::default_mtls_dir();
    let required = ["ca.crt", "tls.crt", "tls.key"];
    let missing: Vec<&str> = required
        .iter()
        .filter(|f| !mtls_dir.join(f).exists())
        .copied()
        .collect();

    if missing.is_empty() {
        DoctorCheck {
            name: "openshell-mtls".to_string(),
            status: CheckStatus::Pass,
            detail: format!("certs present in {}", mtls_dir.display()),
            fix: None,
        }
    } else {
        DoctorCheck {
            name: "openshell-mtls".to_string(),
            status: CheckStatus::Fail,
            detail: format!("missing: {} in {}", missing.join(", "), mtls_dir.display()),
            fix: Some(
                "Install OpenShell and run `openshell auth login` to generate mTLS certificates"
                    .to_string(),
            ),
        }
    }
}

/// Check OpenShell gateway health via gRPC Health RPC.
///
/// Connects to 127.0.0.1:8080 with mTLS and calls Health RPC.
/// Uses block_in_place to run async gRPC call from sync context.
fn check_openshell_gateway_health() -> DoctorCheck {
    let mtls_dir = crate::openshell::default_mtls_dir();

    // Skip if certs are missing (the mtls check already flags this)
    if !mtls_dir.join("ca.crt").exists() {
        return DoctorCheck {
            name: "openshell-gateway".to_string(),
            status: CheckStatus::Warn,
            detail: "skipped — mTLS certs missing".to_string(),
            fix: None,
        };
    }

    let result = tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("failed to create runtime: {e}"))?;
        rt.block_on(async {
            let mut client = crate::openshell::connect_grpc(&mtls_dir)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let resp = client
                .health(crate::openshell_proto::openshell::v1::HealthRequest {})
                .await
                .map_err(|e| format!("Health RPC failed: {e:#}"))?;
            let status = resp.into_inner().status;
            Ok::<i32, String>(status)
        })
    });

    match result {
        Ok(1) => DoctorCheck {
            name: "openshell-gateway".to_string(),
            status: CheckStatus::Pass,
            detail: "gateway healthy".to_string(),
            fix: None,
        },
        Ok(status) => DoctorCheck {
            name: "openshell-gateway".to_string(),
            status: CheckStatus::Warn,
            detail: format!("gateway status: {status} (expected 1=HEALTHY)"),
            fix: None,
        },
        Err(e) => DoctorCheck {
            name: "openshell-gateway".to_string(),
            status: CheckStatus::Fail,
            detail: format!("gateway unreachable: {e}"),
            fix: Some("Ensure OpenShell gateway is running: `openshell gateway start`".to_string()),
        },
    }
}

/// Validate cron `target_chat_id` values for a single agent.
///
/// Surfaces:
/// - cron_specs rows with `target_chat_id IS NULL` → WARN (operator must `cron_update`)
/// - cron_specs rows whose `target_chat_id` is no longer in `allowlist.yaml` → WARN
///
/// Returns one `DoctorCheck` per problem found, plus a single Pass when the agent
/// has crons and all of them are healthy. Returns an empty Vec if the agent has no crons.
pub fn check_cron_targets(agent_dir: &Path) -> Vec<DoctorCheck> {
    let mut out = Vec::new();

    let conn = match crate::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("open data.db: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let allowlist_state = match crate::agent::allowlist::read_file(agent_dir) {
        Ok(Some(file)) => crate::agent::allowlist::AllowlistState::from_file(file),
        Ok(None) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Warn,
                detail: "allowlist.yaml is missing — cron targets cannot be validated".into(),
                fix: Some("run `rightclaw agent allow <user_id>` from a trusted account".into()),
            });
            return out;
        }
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("read allowlist.yaml: {e}"),
                fix: None,
            });
            return out;
        }
    };

    let mut stmt = match conn.prepare("SELECT job_name, target_chat_id FROM cron_specs") {
        Ok(s) => s,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("prepare query: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let rows = match stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
    }) {
        Ok(r) => r,
        Err(e) => {
            out.push(DoctorCheck {
                name: "cron targets".into(),
                status: CheckStatus::Fail,
                detail: format!("query: {e:#}"),
                fix: None,
            });
            return out;
        }
    };

    let mut total = 0usize;
    let mut warned = 0usize;
    for row in rows {
        let (job_name, target) = match row {
            Ok(v) => v,
            Err(e) => {
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Fail,
                    detail: format!("row read: {e:#}"),
                    fix: None,
                });
                continue;
            }
        };
        total += 1;
        match target {
            None => {
                warned += 1;
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Warn,
                    detail: format!("cron '{job_name}' has no target_chat_id"),
                    fix: Some(format!(
                        "call cron_update job_name={job_name} target_chat_id=<chat_id>; \
                         or recreate the cron in the desired chat"
                    )),
                });
            }
            Some(id) if !allowlist_state.is_chat_allowed(id) => {
                warned += 1;
                out.push(DoctorCheck {
                    name: "cron targets".into(),
                    status: CheckStatus::Warn,
                    detail: format!(
                        "cron '{job_name}' targets chat {id} which is no longer in allowlist"
                    ),
                    fix: Some(format!(
                        "call cron_update job_name={job_name} target_chat_id=<chat_id>; \
                         or `rightclaw agent allow_all {id}` to re-open"
                    )),
                });
            }
            Some(_) => {}
        }
    }

    if total > 0 && warned == 0 {
        out.push(DoctorCheck {
            name: "cron targets".into(),
            status: CheckStatus::Pass,
            detail: format!("{total} cron(s) with valid targets"),
            fix: None,
        });
    }
    out
}

/// Run memory-subsystem checks against a single agent directory.
///
/// Hardened from the plan's `if let Ok(...)` scaffolding: when an underlying
/// SQLite query fails (retain count, oldest age, alert existence), we emit a
/// `Fail` check with the error detail rather than silently dropping the check.
/// This preserves FAIL-FAST semantics while still letting the doctor emit all
/// other checks (one failing check shouldn't hide the rest).
pub fn check_memory(agent_dir: &Path) -> Vec<DoctorCheck> {
    let mut out = Vec::new();
    let db_path = agent_dir.join("data.db");

    // 1. data.db opens.
    let conn = match crate::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            out.push(DoctorCheck {
                name: "memory db".into(),
                status: CheckStatus::Fail,
                detail: format!("open {}: {e:#}", db_path.display()),
                fix: Some("verify agent dir and permissions".into()),
            });
            return out;
        }
    };

    // 2. journal_mode.
    let mode: Result<String, _> = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0));
    match mode {
        Ok(m) if m.eq_ignore_ascii_case("wal") => {
            out.push(DoctorCheck {
                name: "memory db WAL".into(),
                status: CheckStatus::Pass,
                detail: "journal_mode=wal".into(),
                fix: None,
            });
        }
        Ok(other) => out.push(DoctorCheck {
            name: "memory db WAL".into(),
            status: CheckStatus::Fail,
            detail: format!("journal_mode={other}"),
            fix: Some("re-run bot startup to apply PRAGMA".into()),
        }),
        Err(e) => out.push(DoctorCheck {
            name: "memory db WAL".into(),
            status: CheckStatus::Fail,
            detail: format!("PRAGMA failed: {e:#}"),
            fix: None,
        }),
    }

    // 3. user_version matches migration.
    let expected: u32 = 17;
    match conn.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0)) {
        Ok(version) if version == expected => out.push(DoctorCheck {
            name: "memory schema".into(),
            status: CheckStatus::Pass,
            detail: format!("user_version={version}"),
            fix: None,
        }),
        Ok(version) => out.push(DoctorCheck {
            name: "memory schema".into(),
            status: CheckStatus::Fail,
            detail: format!("user_version={version}, expected={expected}"),
            fix: Some("start the bot to run pending migrations".into()),
        }),
        Err(e) => out.push(DoctorCheck {
            name: "memory schema".into(),
            status: CheckStatus::Fail,
            detail: format!("PRAGMA user_version failed: {e:#}"),
            fix: None,
        }),
    }

    // 4. pending_retains row count.
    match crate::memory::retain_queue::count(&conn) {
        Ok(n) => {
            let (st, detail) = match n {
                n if n < 500 => (CheckStatus::Pass, format!("{n} entries")),
                n if n <= 900 => (
                    CheckStatus::Warn,
                    format!("retain backlog growing: {n} entries"),
                ),
                n => (
                    CheckStatus::Fail,
                    format!("retain backlog near cap: {n}/1000 entries"),
                ),
            };
            out.push(DoctorCheck {
                name: "retain backlog count".into(),
                status: st,
                detail,
                fix: None,
            });
        }
        Err(e) => out.push(DoctorCheck {
            name: "retain backlog count".into(),
            status: CheckStatus::Fail,
            detail: format!("query failed: {e:#}"),
            fix: None,
        }),
    }

    // 5. oldest age.
    match crate::memory::retain_queue::oldest_age(&conn) {
        Ok(Some(age)) => {
            let hours = age.as_secs() / 3600;
            let (st, detail) = if hours < 1 {
                (CheckStatus::Pass, format!("oldest {hours}h"))
            } else if hours <= 12 {
                (
                    CheckStatus::Warn,
                    format!("drain behind by {hours}h — upstream may be degraded"),
                )
            } else {
                (
                    CheckStatus::Fail,
                    format!("drain severely stuck ({hours}h) — investigate logs"),
                )
            };
            out.push(DoctorCheck {
                name: "retain backlog age".into(),
                status: st,
                detail,
                fix: None,
            });
        }
        Ok(None) => {
            // Queue empty — no age to report. Skip silently (no check emitted).
        }
        Err(e) => out.push(DoctorCheck {
            name: "retain backlog age".into(),
            status: CheckStatus::Fail,
            detail: format!("query failed: {e:#}"),
            fix: None,
        }),
    }

    // 6. memory_alerts rows older than 24h.
    use crate::memory::alert_types::{AUTH_FAILED, CLIENT_FLOOD};
    for alert_type in [AUTH_FAILED, CLIENT_FLOOD] {
        match conn.query_row::<bool, _, _>(
            "SELECT EXISTS(SELECT 1 FROM memory_alerts WHERE alert_type = ?1 \
                 AND datetime(first_sent_at) < datetime('now', '-24 hours'))",
            [alert_type],
            |r| r.get(0),
        ) {
            Ok(true) => {
                out.push(DoctorCheck {
                    name: format!("memory alert: {alert_type}"),
                    status: CheckStatus::Fail,
                    detail: format!("{alert_type} standing for >24h"),
                    fix: Some(
                        if alert_type == AUTH_FAILED {
                            "rotate memory.api_key / HINDSIGHT_API_KEY and restart"
                        } else {
                            "check ~/.rightclaw/logs/ for repeated 4xx"
                        }
                        .into(),
                    ),
                });
            }
            Ok(false) => {
                // No standing alert of this type — no check emitted.
            }
            Err(e) => out.push(DoctorCheck {
                name: format!("memory alert: {alert_type}"),
                status: CheckStatus::Fail,
                detail: format!("query failed: {e:#}"),
                fix: None,
            }),
        }
    }

    out
}

/// Check STT prerequisites across all agents.
///
/// Emits:
/// - Warn "ffmpeg" if any agent has `stt.enabled` and ffmpeg is absent from PATH.
/// - Warn "stt-model/<name>" for each agent with `stt.enabled` whose model file is not cached.
/// - Silent when no agents have `stt.enabled = true`.
fn check_stt(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = crate::config::agents_dir(home);
    if !agents_dir.exists() {
        return vec![];
    }

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    // Collect agents with stt.enabled.
    let mut stt_agents: Vec<(String, crate::agent::types::SttConfig)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let config = match crate::agent::discovery::parse_agent_config(&path) {
            Ok(Some(c)) => c,
            Ok(None) | Err(_) => continue,
        };
        if config.stt.enabled {
            stt_agents.push((name, config.stt));
        }
    }

    if stt_agents.is_empty() {
        return vec![];
    }

    let mut out = Vec::new();

    // ffmpeg check — one shared check for all stt agents.
    if !crate::stt::ffmpeg_available() {
        out.push(DoctorCheck {
            name: "ffmpeg".to_string(),
            status: CheckStatus::Warn,
            detail: "ffmpeg not found in PATH — voice transcription disabled".to_string(),
            fix: Some("brew install ffmpeg  # macOS\napt install ffmpeg  # Linux".to_string()),
        });
    }

    // Per-agent model cache check.
    for (name, stt) in &stt_agents {
        let model_path = crate::stt::model_cache_path(home, stt.model);
        if !model_path.exists() {
            out.push(DoctorCheck {
                name: format!("stt-model/{name}"),
                status: CheckStatus::Warn,
                detail: format!("{name}: whisper model {} not cached", stt.model.filename()),
                fix: Some("run: rightclaw up".to_string()),
            });
        }
    }

    out
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;

#[cfg(test)]
mod cron_target_tests {
    use super::*;
    use crate::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistFile};

    fn write_allowlist(agent_dir: &std::path::Path, users: &[i64], groups: &[i64]) {
        let now = chrono::Utc::now();
        let mut file = AllowlistFile::default();
        for &id in users {
            file.users.push(AllowedUser {
                id,
                label: None,
                added_by: None,
                added_at: now,
            });
        }
        for &id in groups {
            file.groups.push(AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
            });
        }
        crate::agent::allowlist::write_file(agent_dir, &file).unwrap();
    }

    fn seed_cron(conn: &rusqlite::Connection, name: &str, target_chat_id: Option<i64>) {
        let now = chrono::Utc::now().to_rfc3339();
        match target_chat_id {
            Some(id) => conn.execute(
                "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, target_chat_id, created_at, updated_at) \
                 VALUES (?1, '*/5 * * * *', 'p', 1.0, ?2, ?3, ?3)",
                rusqlite::params![name, id, now],
            ).unwrap(),
            None => conn.execute(
                "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
                 VALUES (?1, '*/5 * * * *', 'p', 1.0, ?2, ?2)",
                rusqlite::params![name, now],
            ).unwrap(),
        };
    }

    #[test]
    fn null_target_warns() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[100], &[]);
        let conn = crate::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", None);
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Warn)
            .collect();
        assert_eq!(warns.len(), 1, "expected 1 warn, got {checks:?}");
        assert!(warns[0].detail.contains("j1"));
    }

    #[test]
    fn target_outside_allowlist_warns() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[100], &[]);
        let conn = crate::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", Some(-999));
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Warn)
            .collect();
        assert_eq!(warns.len(), 1, "expected 1 warn, got {checks:?}");
        assert!(warns[0].detail.contains("-999"));
    }

    #[test]
    fn valid_target_passes() {
        let dir = tempfile::tempdir().unwrap();
        write_allowlist(dir.path(), &[], &[-200]);
        let conn = crate::memory::open_connection(dir.path(), true).unwrap();
        seed_cron(&conn, "j1", Some(-200));
        drop(conn);

        let checks = check_cron_targets(dir.path());
        let warns: Vec<_> = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Warn)
            .collect();
        assert!(warns.is_empty(), "expected no warns, got {checks:?}");
    }
}

#[cfg(test)]
mod stt_doctor_tests {
    use super::*;
    use crate::agent::types::{SttConfig, WhisperModel};
    use std::path::PathBuf;

    /// Write a minimal agent.yaml with the given stt config to `agent_dir`.
    fn write_agent_yaml(agent_dir: &std::path::Path, stt: &SttConfig) {
        let yaml = format!(
            "stt:\n  enabled: {}\n  model: {}\n",
            stt.enabled,
            match stt.model {
                WhisperModel::Tiny => "tiny",
                WhisperModel::Base => "base",
                WhisperModel::Small => "small",
                WhisperModel::Medium => "medium",
                WhisperModel::LargeV3 => "large-v3",
            }
        );
        std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();
    }

    /// Create a minimal agent dir under `home/agents/<name>/` and write agent.yaml.
    fn make_agent(
        home: &std::path::Path,
        name: &str,
        enabled: bool,
        model: WhisperModel,
    ) -> PathBuf {
        let agents_dir = home.join("agents").join(name);
        std::fs::create_dir_all(&agents_dir).unwrap();
        let stt = SttConfig { enabled, model };
        write_agent_yaml(&agents_dir, &stt);
        agents_dir
    }

    #[test]
    fn warn_on_missing_model_when_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_agent(tmp.path(), "a", true, WhisperModel::Small);
        let reports = check_stt(tmp.path());
        assert!(
            reports.iter().any(|r| r.detail.contains("ggml-small.bin")),
            "expected model warning, got {reports:?}"
        );
    }

    #[test]
    fn warn_severity_is_warn_not_fail() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_agent(tmp.path(), "a", true, WhisperModel::Tiny);
        let reports = check_stt(tmp.path());
        for r in &reports {
            assert_ne!(
                r.status,
                CheckStatus::Fail,
                "STT doctor should only emit Warn, not Fail: {r:?}"
            );
        }
    }

    #[test]
    fn silent_when_stt_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_agent(tmp.path(), "a", false, WhisperModel::Small);
        let reports = check_stt(tmp.path());
        assert!(
            reports.iter().all(|r| !r.detail.contains("ggml")),
            "expected no model warning for disabled stt, got {reports:?}"
        );
        // Also no ffmpeg warning — no enabled stt agent means need_ffmpeg=false.
        assert!(
            reports.iter().all(|r| r.name != "ffmpeg"),
            "expected no ffmpeg warning when stt disabled, got {reports:?}"
        );
    }

    #[test]
    fn pass_when_model_cached() {
        let tmp = tempfile::TempDir::new().unwrap();
        make_agent(tmp.path(), "a", true, WhisperModel::Tiny);
        // Create the model cache file.
        let cache_path = crate::stt::model_cache_path(tmp.path(), WhisperModel::Tiny);
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        std::fs::write(&cache_path, b"fake model").unwrap();
        let reports = check_stt(tmp.path());
        assert!(
            reports.iter().all(|r| !r.detail.contains("ggml-tiny.bin")),
            "expected no model warning when model is cached, got {reports:?}"
        );
    }

    #[test]
    fn silent_when_no_agents_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Don't create agents/ at all.
        let reports = check_stt(tmp.path());
        assert!(reports.is_empty(), "expected empty, got {reports:?}");
    }
}
