use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use rightclaw::agent::discover_agents;
use rightclaw::config::{read_global_config, write_global_config, TunnelConfig};
use rightclaw::init::validate_telegram_token;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What to do when a named tunnel already exists in the Cloudflare account.
enum TunnelExistingAction {
    Reuse,
    Rename,
    DeleteAndRecreate,
    Skip,
}

impl fmt::Display for TunnelExistingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reuse => write!(f, "Reuse existing tunnel"),
            Self::Rename => write!(f, "Create a new tunnel with a different name"),
            Self::DeleteAndRecreate => write!(f, "Delete and recreate the tunnel"),
            Self::Skip => write!(
                f,
                "Skip tunnel setup \x1b[33m(warning: MCP OAuth callbacks will not work)\x1b[0m"
            ),
        }
    }
}

/// A single entry from `cloudflared tunnel list -o json`.
#[derive(Debug, Deserialize)]
struct TunnelListEntry {
    id: String,
    name: String,
}

// ---------------------------------------------------------------------------
// Cloudflared CLI helpers (private)
// ---------------------------------------------------------------------------

/// Outcome of handling an existing tunnel interactively.
enum TunnelOutcome {
    /// Use this tunnel UUID.
    Uuid(String),
    /// User chose to skip tunnel setup entirely.
    Skipped,
}

/// Find an existing tunnel by name via cloudflared CLI.
fn find_tunnel_by_name(cf_bin: &Path, name: &str) -> miette::Result<Option<TunnelListEntry>> {
    let output = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "list", "-o", "json"])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel list: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel list failed: {stderr}"));
    }

    let tunnels: Vec<TunnelListEntry> = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("failed to parse cloudflared tunnel list JSON: {e:#}"))?;

    Ok(tunnels.into_iter().find(|t| t.name == name))
}

/// Create a new named tunnel via cloudflared CLI.
fn create_tunnel(cf_bin: &Path, name: &str) -> miette::Result<TunnelListEntry> {
    let output = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "create", "-o", "json", name])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel create: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel create failed: {stderr}"));
    }

    let entry: TunnelListEntry = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("failed to parse cloudflared tunnel create JSON: {e:#}"))?;

    Ok(entry)
}

/// Delete a named tunnel via cloudflared CLI (cleanup connections first).
fn delete_tunnel(cf_bin: &Path, name: &str) -> miette::Result<()> {
    // Cleanup active connections first (best-effort).
    let _ = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "cleanup", name])
        .output();

    let output = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "delete", name])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel delete: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel delete failed: {stderr}"));
    }

    Ok(())
}

/// Route a DNS CNAME record for the tunnel. Non-fatal: logs a warning on failure.
fn route_dns(cf_bin: &Path, uuid: &str, hostname: &str) {
    let result = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "route", "dns", "--overwrite-dns", uuid, hostname])
        .output();

    match result {
        Ok(output) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("cloudflared route dns failed (non-fatal): {stderr}");
        }
        Err(e) => {
            tracing::warn!("cloudflared route dns failed (non-fatal): {e:#}");
        }
        _ => {}
    }
}

/// Check whether the cloudflared login certificate exists at `~/.cloudflared/cert.pem`.
fn detect_cloudflared_cert() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".cloudflared").join("cert.pem").exists())
        .unwrap_or(false)
}

/// Return the path to the cloudflared credentials file for a given tunnel UUID.
fn cloudflared_credentials_path(uuid: &str) -> miette::Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    Ok(home.join(".cloudflared").join(format!("{uuid}.json")))
}

// ---------------------------------------------------------------------------
// Tunnel setup (public)
// ---------------------------------------------------------------------------

/// Run the interactive (or non-interactive) tunnel setup flow.
///
/// Returns `Some(TunnelConfig)` on success, `None` if tunnel setup was skipped
/// (no cert, or user chose Skip).
pub fn tunnel_setup(
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    interactive: bool,
) -> miette::Result<Option<TunnelConfig>> {
    if !detect_cloudflared_cert() {
        println!("No cloudflared certificate found (~/.cloudflared/cert.pem).");
        println!("Run `cloudflared tunnel login` first, then re-run this command.");
        return Ok(None);
    }

    let cf_bin = which::which("cloudflared")
        .map_err(|_| miette::miette!("cloudflared binary not found in PATH"))?;

    let existing = find_tunnel_by_name(&cf_bin, tunnel_name)?;

    let uuid = match existing {
        Some(entry) => {
            let has_local_creds = cloudflared_credentials_path(&entry.id)
                .map(|p| p.exists())
                .unwrap_or(false);

            if interactive {
                match handle_existing_tunnel(&cf_bin, &entry, tunnel_name, has_local_creds)? {
                    TunnelOutcome::Uuid(id) => id,
                    TunnelOutcome::Skipped => return Ok(None),
                }
            } else if has_local_creds {
                // Non-interactive: silently reuse when credentials are present.
                entry.id
            } else {
                // Non-interactive: cannot reuse without local credentials.
                // Delete and recreate so cloudflared can actually start.
                tracing::warn!(
                    "Tunnel '{}' exists but credentials file is missing locally — recreating",
                    entry.name
                );
                delete_tunnel(&cf_bin, &entry.name)?;
                let fresh = create_tunnel(&cf_bin, tunnel_name)?;
                println!("Recreated tunnel '{}' (UUID: {})", fresh.name, fresh.id);
                fresh.id
            }
        }
        None => {
            let entry = create_tunnel(&cf_bin, tunnel_name)?;
            println!("Created tunnel '{}' (UUID: {})", entry.name, entry.id);
            entry.id
        }
    };

    // Resolve hostname.
    let hostname = if let Some(h) = tunnel_hostname {
        h.to_string()
    } else if interactive {
        let input = inquire::Text::new("Tunnel hostname (e.g. right.example.com):")
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        input.trim().to_string()
    } else {
        return Err(miette::miette!(
            "tunnel hostname is required in non-interactive mode (use --tunnel-hostname)"
        ));
    };

    // Validate: must be a bare domain, not a URL.
    if hostname.starts_with("https://") || hostname.starts_with("http://") {
        return Err(miette::miette!(
            help = "Use just the domain, e.g. right.example.com",
            "Tunnel hostname must be a bare domain, not a URL"
        ));
    }

    if hostname.is_empty() {
        return Err(miette::miette!("tunnel hostname cannot be empty"));
    }

    // Route DNS (non-fatal).
    route_dns(&cf_bin, &uuid, &hostname);

    // Verify credentials file exists — this should always pass after the
    // checks above, but guard against unexpected state.
    let credentials_file = cloudflared_credentials_path(&uuid)?;
    if !credentials_file.exists() {
        return Err(miette::miette!(
            help = "Run `rightclaw config set` and select Tunnel to reconfigure",
            "Tunnel credentials file not found at {} — cloudflared cannot start without it",
            credentials_file.display()
        ));
    }

    Ok(Some(TunnelConfig {
        tunnel_uuid: uuid,
        credentials_file,
        hostname,
    }))
}

// ---------------------------------------------------------------------------
// Handle existing tunnel (interactive)
// ---------------------------------------------------------------------------

/// Prompt the user to decide what to do with an existing tunnel.
fn handle_existing_tunnel(
    cf_bin: &Path,
    existing: &TunnelListEntry,
    original_name: &str,
    has_local_creds: bool,
) -> miette::Result<TunnelOutcome> {
    let short_uuid = if existing.id.len() > 8 {
        &existing.id[..8]
    } else {
        &existing.id
    };
    println!(
        "Found tunnel '{}' in your Cloudflare account (UUID: {short_uuid}...)",
        existing.name
    );

    if !has_local_creds {
        println!(
            "⚠ Credentials file for this tunnel is missing locally.\n  \
             The tunnel may have been created on another machine.\n  \
             Choose \"Delete and recreate\" to generate new credentials on this machine."
        );
    }

    let mut options = Vec::new();
    if has_local_creds {
        options.push(TunnelExistingAction::Reuse);
    }
    options.push(TunnelExistingAction::DeleteAndRecreate);
    options.push(TunnelExistingAction::Rename);
    options.push(TunnelExistingAction::Skip);

    let selection = inquire::Select::new("What would you like to do?", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    match selection {
        TunnelExistingAction::Reuse => Ok(TunnelOutcome::Uuid(existing.id.clone())),

        TunnelExistingAction::Rename => {
            let new_name = inquire::Text::new("New tunnel name:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return Err(miette::miette!("tunnel name cannot be empty"));
            }
            let entry = create_tunnel(cf_bin, new_name)?;
            println!("Created tunnel '{}' (UUID: {})", entry.name, entry.id);
            Ok(TunnelOutcome::Uuid(entry.id))
        }

        TunnelExistingAction::DeleteAndRecreate => {
            let confirmed = inquire::Confirm::new(&format!(
                "This will permanently delete tunnel '{}'. Continue?",
                existing.name
            ))
            .with_default(false)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if !confirmed {
                return Err(miette::miette!("tunnel deletion cancelled"));
            }

            delete_tunnel(cf_bin, &existing.name)?;
            println!("Deleted tunnel '{}'", existing.name);

            let entry = create_tunnel(cf_bin, original_name)?;
            println!("Created tunnel '{}' (UUID: {})", entry.name, entry.id);
            Ok(TunnelOutcome::Uuid(entry.id))
        }

        TunnelExistingAction::Skip => {
            let confirmed = inquire::Confirm::new(
                "Skip tunnel setup? MCP OAuth callbacks will not work without a tunnel.",
            )
            .with_default(false)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if confirmed {
                Ok(TunnelOutcome::Skipped)
            } else {
                // User declined skip — re-prompt.
                handle_existing_tunnel(cf_bin, existing, original_name, has_local_creds)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram setup (public)
// ---------------------------------------------------------------------------

/// Run the interactive Telegram bot token setup flow.
///
/// Returns `Some(token)` if a token was entered (or kept), `None` if skipped.
pub fn telegram_setup(
    existing_token: Option<&str>,
    interactive: bool,
) -> miette::Result<Option<String>> {
    if !interactive {
        return Ok(None);
    }

    let prompt_msg = if let Some(token) = existing_token {
        let masked = if token.len() > 8 {
            format!("{}...{}", &token[..4], &token[token.len() - 4..])
        } else {
            "****".to_string()
        };
        format!("Telegram bot token (current: {masked}, press Enter to keep):")
    } else {
        "Telegram bot token (press Enter to skip):".to_string()
    };

    let input = inquire::Text::new(&prompt_msg)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    let trimmed = input.trim();

    if trimmed.is_empty() {
        // Keep current or skip.
        return Ok(existing_token.map(|t| t.to_string()));
    }

    validate_telegram_token(trimmed)?;
    Ok(Some(trimmed.to_string()))
}

// ---------------------------------------------------------------------------
// Chat ID setup (public)
// ---------------------------------------------------------------------------

/// Prompt for Telegram chat IDs during init.
///
/// Returns parsed IDs, or empty vec if the user skips.
pub fn chat_ids_setup() -> miette::Result<Vec<i64>> {
    let input = inquire::Text::new(
        "Your Telegram user ID (send /start to @userinfobot to find it, empty to skip):",
    )
    .prompt()
    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    let ids: Vec<i64> = trimmed
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<i64>()
                .map_err(|e| miette::miette!("invalid chat ID '{}': {e}", s.trim()))
        })
        .collect::<miette::Result<Vec<_>>>()?;

    Ok(ids)
}

// ---------------------------------------------------------------------------
// Combined settings menu (public)
// ---------------------------------------------------------------------------

/// Menu items for the combined (global + per-agent) settings menu.
enum CombinedMenuItem {
    Tunnel(String),
    Agent(String),
    Done,
}

impl fmt::Display for CombinedMenuItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tunnel(display) => write!(f, "{display}"),
            Self::Agent(name) => write!(f, "Agent: {name}"),
            Self::Done => write!(f, "Done"),
        }
    }
}

/// Interactive menu showing both global settings and per-agent settings.
///
/// Bare `rightclaw config` (no subcommand) launches this.
pub fn combined_setting_menu(home: &Path) -> miette::Result<()> {
    loop {
        let config = read_global_config(home)?;

        let tunnel_label = match &config.tunnel {
            Some(t) => format!(
                "Tunnel: {} ({})",
                t.hostname,
                &t.tunnel_uuid[..8.min(t.tunnel_uuid.len())]
            ),
            None => "Tunnel: (not configured)".to_string(),
        };

        let agents_dir = rightclaw::config::agents_dir(home);
        let agents = if agents_dir.exists() {
            discover_agents(&agents_dir).unwrap_or_default()
        } else {
            vec![]
        };

        let mut options: Vec<CombinedMenuItem> = vec![CombinedMenuItem::Tunnel(tunnel_label)];
        for agent in &agents {
            options.push(CombinedMenuItem::Agent(agent.name.clone()));
        }
        options.push(CombinedMenuItem::Done);

        let selection = inquire::Select::new("Settings:", options)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        match selection {
            CombinedMenuItem::Done => break,
            CombinedMenuItem::Tunnel(_) => {
                let tunnel_name = inquire::Text::new("Tunnel name:")
                    .with_default("rightclaw")
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

                let result = tunnel_setup(tunnel_name.trim(), None, true)?;

                let new_config = rightclaw::config::GlobalConfig { tunnel: result };
                write_global_config(home, &new_config)?;
                println!("Global config saved.");
            }
            CombinedMenuItem::Agent(name) => {
                let _ = agent_setting_menu(home, Some(&name))?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Agent settings menu (public)
// ---------------------------------------------------------------------------

/// Interactive menu for editing per-agent settings.
///
/// If `agent_name` is `None`, presents a picker to choose from discovered agents.
/// Returns the chosen agent name so callers can act on it (e.g. sandbox migration).
pub fn agent_setting_menu(home: &Path, agent_name: Option<&str>) -> miette::Result<String> {
    let agents_dir = rightclaw::config::agents_dir(home);

    let chosen_name = match agent_name {
        Some(name) => name.to_string(),
        None => {
            let agents = discover_agents(&agents_dir)?;
            if agents.is_empty() {
                return Err(miette::miette!(
                    "No agents found in {}",
                    agents_dir.display()
                ));
            }
            let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
            inquire::Select::new("Select agent:", names)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?
        }
    };

    let agent_yaml_path = agents_dir.join(&chosen_name).join("agent.yaml");
    if !agent_yaml_path.exists() {
        return Err(miette::miette!(
            "agent.yaml not found at {}",
            agent_yaml_path.display()
        ));
    }

    loop {
        let content = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("read agent.yaml: {e:#}"))?;
        let config: rightclaw::agent::AgentConfig = serde_saphyr::from_str(&content)
            .map_err(|e| miette::miette!("parse agent.yaml: {e:#}"))?;

        let token_display = match &config.telegram_token {
            Some(t) if t.len() > 8 => format!("{}...{}", &t[..4], &t[t.len() - 4..]),
            Some(_) => "****".to_string(),
            None => "(not set)".to_string(),
        };
        let model_display = config.model.as_deref().unwrap_or("(default)");
        let chat_ids_display = if config.allowed_chat_ids.is_empty() {
            "(none — blocks all)".to_string()
        } else {
            config
                .allowed_chat_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let sandbox_display = format!("{}", config.sandbox_mode());
        let network_policy_display = format!("{}", config.network_policy);

        let opt_token = format!("Telegram token: {token_display}");
        let opt_model = format!("Model: {model_display}");
        let opt_chat_ids = format!("Allowed chat IDs: {chat_ids_display}");
        let opt_sandbox = format!("Sandbox mode: {sandbox_display}");
        let opt_network_policy = format!("Network policy: {network_policy_display}");
        let opt_done = "Done".to_string();

        let mut options = vec![
            opt_token.clone(),
            opt_model.clone(),
            opt_chat_ids.clone(),
            opt_sandbox.clone(),
        ];
        // Only show network policy when sandbox is openshell (no sandbox = no policy).
        if matches!(
            config.sandbox_mode(),
            rightclaw::agent::types::SandboxMode::Openshell
        ) {
            options.push(opt_network_policy.clone());
        }
        options.push(opt_done.clone());

        let selection = inquire::Select::new(
            &format!("Agent '{}' settings:", chosen_name),
            options,
        )
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if selection == opt_done {
            break;
        }

        if selection == opt_token {
            let new_token = telegram_setup(config.telegram_token.as_deref(), true)?;
            match new_token {
                Some(token) => {
                    update_agent_yaml_field(&agent_yaml_path, "telegram_token", &format!("\"{token}\""))?;
                }
                None if config.telegram_token.is_some() => {
                    // User cleared the token.
                    remove_agent_yaml_field(&agent_yaml_path, "telegram_token")?;
                }
                None => {
                    // No change.
                }
            }
        } else if selection == opt_model {
            let input = inquire::Text::new("Model (e.g. sonnet, opus, haiku — empty to clear):")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                remove_agent_yaml_field(&agent_yaml_path, "model")?;
            } else {
                update_agent_yaml_field(&agent_yaml_path, "model", &format!("\"{trimmed}\""))?;
            }
        } else if selection == opt_chat_ids {
            let input = inquire::Text::new("Allowed chat IDs (comma-separated, empty to clear):")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                update_agent_yaml_chat_ids(&agent_yaml_path, &[])?;
            } else {
                let ids: Vec<i64> = trimmed
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<i64>()
                            .map_err(|e| miette::miette!("invalid chat ID '{}': {e}", s.trim()))
                    })
                    .collect::<miette::Result<Vec<_>>>()?;
                update_agent_yaml_chat_ids(&agent_yaml_path, &ids)?;
            }
        } else if selection == opt_sandbox {
            let options = vec![
                "OpenShell — run in isolated container (recommended)",
                "None — run directly on host (for computer-use, Chrome, etc.)",
            ];
            let choice = inquire::Select::new("Sandbox mode:", options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let mode = if choice.starts_with("OpenShell") {
                "openshell"
            } else {
                "none"
            };
            update_agent_yaml_sandbox_mode(&agent_yaml_path, mode)?;
        } else if selection == opt_network_policy {
            let options = vec![
                "Restrictive — Anthropic/Claude domains only (recommended)",
                "Permissive — all HTTPS domains allowed (needed for external MCP servers)",
            ];
            let choice = inquire::Select::new("Network policy for sandbox:", options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let policy = if choice.starts_with("Restrictive") {
                "restrictive"
            } else {
                "permissive"
            };
            update_agent_yaml_field(&agent_yaml_path, "network_policy", policy)?;
        }

        println!("Saved.");
    }

    Ok(chosen_name)
}

// ---------------------------------------------------------------------------
// YAML mutation helpers (private)
// ---------------------------------------------------------------------------

/// Update (or append) a top-level scalar field in an agent.yaml file.
fn update_agent_yaml_field(path: &Path, key: &str, value: &str) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let prefix = format!("{key}:");
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                found = true;
                format!("{key}: {value}")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        lines.push(format!("{key}: {value}"));
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Remove a top-level scalar field from an agent.yaml file.
fn remove_agent_yaml_field(path: &Path, key: &str) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let prefix = format!("{key}:");
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect();

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Update the `sandbox: mode:` field in an agent.yaml file.
///
/// Handles the nested `sandbox:` block: updates `mode:` if it exists,
/// or creates the block if absent. When switching to `none`, removes `policy_file`.
/// When switching to `openshell`, adds default `policy_file: policy.yaml`.
fn update_agent_yaml_sandbox_mode(path: &Path, mode: &str) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Remove existing sandbox block (header + indented lines).
    let mut lines: Vec<String> = Vec::new();
    let mut in_sandbox_block = false;
    for line in content.lines() {
        if line == "sandbox:" {
            in_sandbox_block = true;
            continue;
        }
        if in_sandbox_block {
            if line.starts_with("  ") {
                continue;
            }
            in_sandbox_block = false;
        }
        lines.push(line.to_string());
    }

    // Append new sandbox block.
    lines.push("sandbox:".to_string());
    lines.push(format!("  mode: {mode}"));
    if mode == "openshell" {
        lines.push("  policy_file: policy.yaml".to_string());
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Write or update `sandbox.name` in agent.yaml.
pub fn update_agent_yaml_sandbox_name(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let path = agent_dir.join("agent.yaml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Remove existing sandbox block (header + indented lines), preserving
    // all sub-fields except `name:`.
    let mut sandbox_lines: Vec<String> = Vec::new();
    let mut other_lines: Vec<String> = Vec::new();
    let mut in_sandbox_block = false;

    for line in content.lines() {
        if line == "sandbox:" {
            in_sandbox_block = true;
            continue;
        }
        if in_sandbox_block {
            if line.starts_with("  ") {
                // Skip existing name: line — we'll add the new one.
                if !line.trim_start().starts_with("name:") {
                    sandbox_lines.push(line.to_string());
                }
                continue;
            }
            in_sandbox_block = false;
        }
        other_lines.push(line.to_string());
    }

    // Rebuild: other lines first, then sandbox block with name.
    let mut lines = other_lines;
    lines.push("sandbox:".to_string());
    for sl in sandbox_lines {
        lines.push(sl);
    }
    lines.push(format!("  name: \"{sandbox_name}\""));

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(&path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Replace the `allowed_chat_ids` block in an agent.yaml file.
///
/// Removes any existing `allowed_chat_ids:` block (header + indented list items),
/// then appends the new block if `ids` is non-empty.
fn update_agent_yaml_chat_ids(path: &Path, ids: &[i64]) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Filter out the existing allowed_chat_ids block.
    let mut lines: Vec<String> = Vec::new();
    let mut in_chat_ids_block = false;
    for line in content.lines() {
        if line.starts_with("allowed_chat_ids:") {
            in_chat_ids_block = true;
            continue;
        }
        if in_chat_ids_block {
            // List items under allowed_chat_ids are indented.
            if line.starts_with("  - ") || line.starts_with("  -\t") {
                continue;
            }
            // Non-indented line means block ended.
            in_chat_ids_block = false;
        }
        lines.push(line.to_string());
    }

    // Append new block if needed.
    if !ids.is_empty() {
        lines.push("allowed_chat_ids:".to_string());
        for id in ids {
            lines.push(format!("  - {id}"));
        }
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}
