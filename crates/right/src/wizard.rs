use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use right_agent::agent::discover_agents;
use right_agent::config::{TunnelConfig, read_global_config, write_global_config};
use right_agent::init::validate_telegram_token;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What to do when a named tunnel already exists in the Cloudflare account.
enum TunnelExistingAction {
    Reuse,
    Rename,
    DeleteAndRecreate,
}

impl fmt::Display for TunnelExistingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reuse => write!(f, "reuse"),
            Self::Rename => write!(f, "rename"),
            Self::DeleteAndRecreate => write!(f, "delete and recreate"),
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
        .args([
            "tunnel",
            "--loglevel",
            "error",
            "create",
            "-o",
            "json",
            name,
        ])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel create: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "cloudflared tunnel create failed: {stderr}"
        ));
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
        return Err(miette::miette!(
            "cloudflared tunnel delete failed: {stderr}"
        ));
    }

    Ok(())
}

/// Route a DNS CNAME record for the tunnel. Non-fatal: logs a warning on failure.
fn route_dns(cf_bin: &Path, uuid: &str, hostname: &str) {
    let result = Command::new(cf_bin)
        .args([
            "tunnel",
            "--loglevel",
            "error",
            "route",
            "dns",
            "--overwrite-dns",
            uuid,
            hostname,
        ])
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
    let home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    Ok(home.join(".cloudflared").join(format!("{uuid}.json")))
}

// ---------------------------------------------------------------------------
// Tunnel setup (public)
// ---------------------------------------------------------------------------

/// Run the interactive (or non-interactive) tunnel setup flow.
///
/// Returns the configured `TunnelConfig`. Cloudflare Tunnel is mandatory; this
/// function errors if no cloudflared certificate is available.
pub fn tunnel_setup(
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    interactive: bool,
) -> miette::Result<TunnelConfig> {
    if !detect_cloudflared_cert() {
        return Err(miette::miette!(
            help = "run: cloudflared tunnel login",
            "no cloudflared certificate found at ~/.cloudflared/cert.pem"
        ));
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
                handle_existing_tunnel(&cf_bin, &entry, tunnel_name, has_local_creds)?
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
                let theme = right_agent::ui::detect();
                println!(
                    "{}",
                    right_agent::ui::status(right_agent::ui::Glyph::Ok)
                        .noun("tunnel")
                        .verb("recreated")
                        .detail(fresh.name.as_str())
                        .render(theme)
                );
                fresh.id
            }
        }
        None => {
            let entry = create_tunnel(&cf_bin, tunnel_name)?;
            let theme = right_agent::ui::detect();
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            entry.id
        }
    };

    // Resolve hostname.
    let hostname = if let Some(h) = tunnel_hostname {
        h.to_string()
    } else if interactive {
        let input = inquire::Text::new("tunnel hostname (e.g. right.example.com):")
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
            help = "use just the domain, e.g. right.example.com",
            "hostname must be a bare domain, not a url"
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
            help = "Run `right config set` and select Tunnel to reconfigure",
            "tunnel credentials missing at {} — cloudflared cannot start",
            credentials_file.display()
        ));
    }

    Ok(TunnelConfig {
        tunnel_uuid: uuid,
        credentials_file,
        hostname,
    })
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
) -> miette::Result<String> {
    let short_uuid = if existing.id.len() > 8 {
        &existing.id[..8]
    } else {
        &existing.id
    };
    let theme = right_agent::ui::detect();
    println!(
        "{}",
        right_agent::ui::status(right_agent::ui::Glyph::Warn)
            .noun("tunnel")
            .verb(format!("found \"{}\"", existing.name))
            .detail(format!("{}…", short_uuid))
            .render(theme)
    );

    if !has_local_creds {
        println!(
            "{}    note: credentials file missing on this machine. choose \"delete and recreate\" to regenerate.",
            right_agent::ui::Rail::blank(theme)
        );
    }

    let mut options = Vec::new();
    if has_local_creds {
        options.push(TunnelExistingAction::Reuse);
    }
    options.push(TunnelExistingAction::DeleteAndRecreate);
    options.push(TunnelExistingAction::Rename);

    let selection = inquire::Select::new("existing tunnel — choose:", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    match selection {
        TunnelExistingAction::Reuse => Ok(existing.id.clone()),

        TunnelExistingAction::Rename => {
            let new_name = inquire::Text::new("new tunnel name:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return Err(miette::miette!("tunnel name cannot be empty"));
            }
            let entry = create_tunnel(cf_bin, new_name)?;
            let theme = right_agent::ui::detect();
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            Ok(entry.id)
        }

        TunnelExistingAction::DeleteAndRecreate => {
            let confirmed = inquire::Confirm::new(&format!(
                "delete tunnel \"{}\" permanently?",
                existing.name
            ))
            .with_default(false)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if !confirmed {
                return Err(miette::miette!("cancelled"));
            }

            delete_tunnel(cf_bin, &existing.name)?;
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("deleted")
                    .detail(existing.name.as_str())
                    .render(theme)
            );

            let entry = create_tunnel(cf_bin, original_name)?;
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            Ok(entry.id)
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram setup (public)
// ---------------------------------------------------------------------------

/// Outcome of `telegram_setup`. Distinguishes "back" (Esc) from "skip"
/// (Enter on empty when `!required`) so the caller can route navigation
/// vs. value separately.
#[derive(Debug, Clone)]
pub enum TelegramSetupOutcome {
    /// User entered a valid token.
    Token(String),
    /// User pressed Enter on empty (only possible when `!required`).
    Skipped,
    /// User pressed Esc — caller should navigate back.
    Back,
}

/// Run the interactive Telegram bot token setup flow.
///
/// When `required` is true and there is no existing token, an empty input
/// is rejected and the prompt re-runs until a valid token is entered. Esc
/// always navigates back; Ctrl+C triggers a "Cancel setup?" confirm via
/// `inquire_back` and propagates as `Err` if confirmed.
pub fn telegram_setup(
    existing_token: Option<&str>,
    interactive: bool,
    required: bool,
) -> miette::Result<TelegramSetupOutcome> {
    if !interactive {
        return Ok(TelegramSetupOutcome::Skipped);
    }

    let prompt_msg = if let Some(token) = existing_token {
        let masked = if token.len() > 8 {
            format!("{}...{}", &token[..4], &token[token.len() - 4..])
        } else {
            "****".to_string()
        };
        format!("telegram bot token (keeping {masked} — enter new or press enter to keep):")
    } else if required {
        "telegram bot token (required — get one from @BotFather):".to_string()
    } else {
        "telegram bot token (enter to skip):".to_string()
    };

    loop {
        let Some(input) =
            right_agent::init::inquire_back(|| inquire::Text::new(&prompt_msg).prompt())?
        else {
            return Ok(TelegramSetupOutcome::Back);
        };

        let trimmed = input.trim();

        if trimmed.is_empty() {
            if required && existing_token.is_none() {
                eprintln!(
                    "  a token is required. create a bot via @BotFather, paste the token here. esc to go back."
                );
                continue;
            }
            return Ok(match existing_token {
                Some(t) => TelegramSetupOutcome::Token(t.to_string()),
                None => TelegramSetupOutcome::Skipped,
            });
        }

        match validate_telegram_token(trimmed) {
            Ok(()) => return Ok(TelegramSetupOutcome::Token(trimmed.to_string())),
            Err(e) if required => {
                eprintln!("  {e:#}");
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Chat ID setup (public)
// ---------------------------------------------------------------------------

/// Prompt for Telegram chat IDs during init.
///
/// When `required` is true, an empty input is rejected and an unparseable
/// entry re-prompts (instead of erroring out). Returns `Ok(Some(ids))`
/// (possibly empty when `!required`) on submit; `Ok(None)` when the user
/// navigates back (Esc); `Err` on confirmed cancel (Ctrl+C).
pub fn chat_ids_setup(required: bool) -> miette::Result<Option<Vec<i64>>> {
    let prompt_text = if required {
        "your telegram user id (required — /start @userinfobot to find it):"
    } else {
        "your telegram user id (/start @userinfobot to find it, empty to skip):"
    };

    loop {
        let Some(input) =
            right_agent::init::inquire_back(|| inquire::Text::new(prompt_text).prompt())?
        else {
            return Ok(None);
        };

        let trimmed = input.trim();
        if trimmed.is_empty() {
            if required {
                eprintln!(
                    "  at least one chat id is required so the bot knows who can talk to it. /start @userinfobot for your numeric id. esc to go back."
                );
                continue;
            }
            return Ok(Some(vec![]));
        }

        let parsed: Result<Vec<i64>, _> = trimmed
            .split(',')
            .map(|s| {
                s.trim()
                    .parse::<i64>()
                    .map_err(|e| miette::miette!("invalid chat id \"{}\": {e}", s.trim()))
            })
            .collect();

        match parsed {
            Ok(ids) => return Ok(Some(ids)),
            Err(e) if required => {
                eprintln!("  {e:#}");
                continue;
            }
            Err(e) => return Err(e),
        }
    }
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
/// Bare `right config` (no subcommand) launches this.
pub async fn combined_setting_menu(home: &Path) -> miette::Result<()> {
    loop {
        let config = read_global_config(home)?;

        let tunnel_label = format!(
            "Tunnel: {} ({})",
            config.tunnel.hostname,
            &config.tunnel.tunnel_uuid[..8.min(config.tunnel.tunnel_uuid.len())]
        );

        let agents_dir = right_agent::config::agents_dir(home);
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
                let tunnel_name = inquire::Text::new("tunnel name:")
                    .with_default("right")
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

                let result = tunnel_setup(tunnel_name.trim(), None, true)?;

                let mut new_config = read_global_config(home)?;
                new_config.tunnel = result;
                write_global_config(home, &new_config)?;
                println!("Global config saved.");
            }
            CombinedMenuItem::Agent(name) => {
                let _ = agent_setting_menu(home, Some(&name)).await?;
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
pub async fn agent_setting_menu(home: &Path, agent_name: Option<&str>) -> miette::Result<String> {
    let agents_dir = right_agent::config::agents_dir(home);

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
        let config: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content)
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
        let memory_display = format_memory_display(&config.memory, &chosen_name);

        let opt_token = format!("Telegram token: {token_display}");
        let opt_model = format!("Model: {model_display}");
        let opt_chat_ids = format!("Allowed chat IDs: {chat_ids_display}");
        let opt_sandbox = format!("Sandbox mode: {sandbox_display}");
        let opt_network_policy = format!("Network policy: {network_policy_display}");
        let opt_memory = format!("Memory: {memory_display}");
        let stt_display = if config.stt.enabled {
            format!("on ({})", config.stt.model.yaml_str())
        } else {
            "off".to_string()
        };
        let opt_stt = format!("STT: {stt_display}");
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
            right_agent::agent::types::SandboxMode::Openshell
        ) {
            options.push(opt_network_policy.clone());
        }
        options.push(opt_stt.clone());
        options.push(opt_memory.clone());
        options.push(opt_done.clone());

        let selection =
            inquire::Select::new(&format!("Agent '{}' settings:", chosen_name), options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if selection == opt_done {
            break;
        }

        if selection == opt_token {
            match telegram_setup(config.telegram_token.as_deref(), true, false)? {
                TelegramSetupOutcome::Token(token) => {
                    update_agent_yaml_field(
                        &agent_yaml_path,
                        "telegram_token",
                        &format!("\"{token}\""),
                    )?;
                }
                TelegramSetupOutcome::Skipped | TelegramSetupOutcome::Back => {
                    // Enter on empty (keep existing) or Esc — no change.
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
                            .map_err(|e| miette::miette!("invalid chat id \"{}\": {e}", s.trim()))
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
        } else if selection == opt_memory {
            match memory_setup(config.memory.as_ref(), &chosen_name).await? {
                Some(new_cfg) => {
                    if matches!(
                        new_cfg.provider,
                        right_agent::agent::types::MemoryProvider::File
                    ) {
                        remove_agent_yaml_memory(&agent_yaml_path)?;
                    } else {
                        update_agent_yaml_memory(&agent_yaml_path, &new_cfg)?;
                    }
                }
                None => {
                    // User cancelled — no change.
                }
            }
        } else if selection == opt_stt {
            match stt_setup()? {
                Some((enabled, model)) => {
                    let stt = right_agent::agent::types::SttConfig { enabled, model };
                    update_agent_yaml_stt(&agent_yaml_path, &stt)?;
                }
                None => {
                    // User cancelled — no change.
                }
            }
        }

        println!("Saved.");
    }

    Ok(chosen_name)
}

/// Compact one-line summary of memory config for the settings menu.
fn format_memory_display(
    memory: &Option<right_agent::agent::types::MemoryConfig>,
    agent_name: &str,
) -> String {
    use right_agent::agent::types::MemoryProvider;
    match memory.as_ref().map(|m| &m.provider) {
        None | Some(MemoryProvider::File) => "file".to_string(),
        Some(MemoryProvider::Hindsight) => {
            let cfg = memory.as_ref().unwrap();
            let bank = cfg.bank_id.as_deref().unwrap_or(agent_name);
            format!("hindsight (bank: {bank}, budget: {})", cfg.recall_budget)
        }
    }
}

/// Interactive memory config submenu. Returns `Ok(None)` when the user
/// cancels without committing a change.
///
/// On switch between providers, warns that memory does not migrate. On
/// Hindsight, optionally uses `HINDSIGHT_API_KEY` from the environment
/// and validates the key against `GET /v1/default/banks`.
async fn memory_setup(
    current: Option<&right_agent::agent::types::MemoryConfig>,
    agent_name: &str,
) -> miette::Result<Option<right_agent::agent::types::MemoryConfig>> {
    use right_agent::agent::types::{MemoryConfig, MemoryProvider};
    use right_agent::init::{
        DEFAULT_RECALL_BUDGET, DEFAULT_RECALL_MAX_TOKENS, ValidationResult,
        prompt_hindsight_bank_id, prompt_memory_provider, prompt_recall_budget,
        prompt_recall_max_tokens,
    };

    let current_provider = current
        .map(|m| m.provider.clone())
        .unwrap_or(MemoryProvider::File);

    let Some(new_provider) = prompt_memory_provider()? else {
        return Ok(None);
    };

    if new_provider != current_provider {
        let confirm = inquire::Confirm::new(
            "Switching memory provider will not migrate existing memory. Continue?",
        )
        .with_default(false)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        if !confirm {
            return Ok(None);
        }
    }

    if matches!(new_provider, MemoryProvider::File) {
        return Ok(Some(MemoryConfig {
            provider: MemoryProvider::File,
            api_key: None,
            bank_id: None,
            recall_budget: DEFAULT_RECALL_BUDGET,
            recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
        }));
    }

    // Hindsight: api_key. If HINDSIGHT_API_KEY is set, offer to use it.
    let env_key = std::env::var("HINDSIGHT_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let api_key: Option<String> = if env_key.is_some() {
        let opts = vec![
            "Use HINDSIGHT_API_KEY env var (recommended)",
            "Enter a key to save in agent.yaml",
        ];
        let choice = inquire::Select::new("Hindsight API key source:", opts)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        if choice.starts_with("Use HINDSIGHT_API_KEY") {
            None
        } else {
            let input = inquire::Text::new("Hindsight API key:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Some(trimmed.to_string())
        }
    } else {
        let input = inquire::Text::new(
            "Hindsight API key (empty to rely on HINDSIGHT_API_KEY env var at runtime):",
        )
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    let bank_id = match prompt_hindsight_bank_id(agent_name)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let budget = match prompt_recall_budget()? {
        Some(b) => b,
        None => return Ok(None),
    };
    let max_tokens = match prompt_recall_max_tokens()? {
        Some(t) => t,
        None => return Ok(None),
    };

    // Validation uses whatever key we can resolve right now:
    // - explicit key if user entered one
    // - HINDSIGHT_API_KEY env var as fallback
    // If neither is available, skip validation with a note.
    let resolved_key = api_key.clone().or_else(|| {
        std::env::var("HINDSIGHT_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    });

    if let Some(k) = resolved_key.as_deref() {
        println!("Validating key against Hindsight...");
        match right_agent::init::validate_hindsight_key(k).await {
            ValidationResult::Valid { banks } => {
                println!("\u{2713} Key valid — {banks} bank(s) accessible.");
            }
            ValidationResult::Invalid { status } => {
                let proceed = inquire::Confirm::new(&format!(
                    "Hindsight rejected the key (HTTP {status}). Save anyway?"
                ))
                .with_default(false)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                if !proceed {
                    return Ok(None);
                }
            }
            ValidationResult::Unreachable { detail } => {
                println!("\u{26a0} Could not validate (Hindsight unreachable): {detail}");
                let proceed = inquire::Confirm::new("Save config anyway?")
                    .with_default(true)
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                if !proceed {
                    return Ok(None);
                }
            }
        }
    } else {
        println!(
            "\u{26a0} No key available to validate (none entered, HINDSIGHT_API_KEY unset). Saving without validation."
        );
    }

    Ok(Some(MemoryConfig {
        provider: MemoryProvider::Hindsight,
        api_key,
        bank_id,
        recall_budget: budget,
        recall_max_tokens: max_tokens,
    }))
}

// ---------------------------------------------------------------------------
// STT setup helpers (public)
// ---------------------------------------------------------------------------

/// macOS: detect brew, prompt to install ffmpeg, run, re-check.
/// Linux: print install instructions only.
/// Returns true iff ffmpeg is in PATH after this call.
pub fn prompt_ffmpeg_install() -> miette::Result<bool> {
    if right_agent::stt::ffmpeg_available() {
        return Ok(true);
    }

    match std::env::consts::OS {
        "macos" => {
            if which::which("brew").is_err() {
                println!("ffmpeg required, but Homebrew (brew) is not installed.");
                println!("Install Homebrew first: https://brew.sh");
                println!("Then run: brew install ffmpeg");
                return Ok(false);
            }
            let install = inquire::Confirm::new(
                "ffmpeg required for voice transcription. Install via 'brew install ffmpeg'?",
            )
            .with_default(true)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            if !install {
                println!("STT will be disabled. Install ffmpeg later: brew install ffmpeg");
                return Ok(false);
            }
            // Spawn brew install with stdout/stderr inherited so user sees output.
            let status = std::process::Command::new("brew")
                .args(["install", "ffmpeg"])
                .status()
                .map_err(|e| miette::miette!("spawn brew: {e:#}"))?;
            if !status.success() {
                println!("brew install ffmpeg exited with {status}; STT disabled.");
                return Ok(false);
            }
            if !right_agent::stt::ffmpeg_available() {
                println!(
                    "brew completed but ffmpeg not yet in PATH — restart shell or check PATH; STT disabled."
                );
                return Ok(false);
            }
            tracing::info!("ffmpeg installed via brew");
            Ok(true)
        }
        "linux" => {
            println!("ffmpeg required for voice transcription. Install:");
            println!("  Debian/Ubuntu:  sudo apt install ffmpeg");
            println!("  NixOS / devenv: add 'pkgs.ffmpeg' to your packages");
            println!("Then re-run this command.");
            Ok(false)
        }
        other => {
            println!("ffmpeg required, but auto-install is not supported on '{other}'.");
            println!("Install ffmpeg from https://ffmpeg.org/download.html, then re-run.");
            Ok(false)
        }
    }
}

/// Wizard step: ask enable/disable + model selection, run ffmpeg detection
/// + install prompt as needed. Returns Some((enabled, model)) on completion,
///   None if the user pressed Esc on either prompt (caller decides where to go back).
pub fn stt_setup() -> miette::Result<Option<(bool, right_agent::agent::types::WhisperModel)>> {
    use right_agent::agent::types::WhisperModel;

    // Step 1: enable y/n
    let Some(enable) = right_agent::init::inquire_back(|| {
        inquire::Confirm::new("Enable voice transcription?")
            .with_default(true)
            .with_help_message(
                "Telegram voice messages and video notes will be transcribed locally via whisper.cpp.",
            )
            .prompt()
    })?
    else {
        return Ok(None);
    };

    if !enable {
        return Ok(Some((false, WhisperModel::Small)));
    }

    // Step 2: model select
    let Some(picked) = right_agent::init::inquire_back(|| {
        inquire::Select::new(
            "Choose whisper model:",
            vec![
                "tiny     — ~75 MB,   fastest, OK for short commands",
                "base     — ~150 MB,  decent",
                "small    — ~470 MB,  recommended (default)",
                "medium   — ~1.5 GB,  very good",
                "large-v3 — ~3.0 GB,  best quality, slow",
            ],
        )
        .with_starting_cursor(2) // small
        .prompt()
    })?
    else {
        // Caller routes the back navigation (skips two steps from the user's perspective).
        return Ok(None);
    };
    let model = if picked.starts_with("tiny") {
        WhisperModel::Tiny
    } else if picked.starts_with("base") {
        WhisperModel::Base
    } else if picked.starts_with("small") {
        WhisperModel::Small
    } else if picked.starts_with("medium") {
        WhisperModel::Medium
    } else if picked.starts_with("large-v3") {
        WhisperModel::LargeV3
    } else {
        unreachable!("unexpected whisper option label: {picked}")
    };

    // Step 3: ffmpeg check + optional install
    let ffmpeg_ok = prompt_ffmpeg_install()?;
    Ok(Some((ffmpeg_ok, model)))
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

/// Remove the entire `memory:` block from an agent.yaml file.
///
/// Used when switching from Hindsight to File: omitting the block lets the
/// config parser fall back to its default (File provider).
fn remove_agent_yaml_memory(path: &Path) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let mut lines: Vec<String> = Vec::new();
    let mut in_memory_block = false;
    for line in content.lines() {
        if line == "memory:" {
            in_memory_block = true;
            continue;
        }
        if in_memory_block {
            if line.starts_with("  ") {
                continue;
            }
            in_memory_block = false;
        }
        lines.push(line.to_string());
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Replace the `memory:` block in an agent.yaml file with a freshly
/// serialized version of `cfg`. Emits only non-default recall fields to
/// keep the yaml tidy.
fn update_agent_yaml_memory(
    path: &Path,
    cfg: &right_agent::agent::types::MemoryConfig,
) -> miette::Result<()> {
    use right_agent::agent::types::MemoryProvider;
    use right_agent::init::{DEFAULT_RECALL_BUDGET, DEFAULT_RECALL_MAX_TOKENS};

    // Strip any pre-existing memory block first.
    remove_agent_yaml_memory(path)?;

    let mut block = String::from("\nmemory:\n");
    let provider_str = match cfg.provider {
        MemoryProvider::File => "file",
        MemoryProvider::Hindsight => "hindsight",
    };
    block.push_str(&format!("  provider: {provider_str}\n"));
    if let Some(ref k) = cfg.api_key {
        block.push_str(&format!("  api_key: \"{k}\"\n"));
    }
    if let Some(ref b) = cfg.bank_id {
        block.push_str(&format!("  bank_id: \"{b}\"\n"));
    }
    if cfg.recall_budget != DEFAULT_RECALL_BUDGET {
        block.push_str(&format!("  recall_budget: {}\n", cfg.recall_budget));
    }
    if cfg.recall_max_tokens != DEFAULT_RECALL_MAX_TOKENS {
        block.push_str(&format!("  recall_max_tokens: {}\n", cfg.recall_max_tokens));
    }

    let mut content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&block);

    std::fs::write(path, &content)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod memory_yaml_tests {
    use super::*;
    use right_agent::agent::types::{MemoryConfig, MemoryProvider, RecallBudget};
    use tempfile::tempdir;

    fn write_yaml(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("agent.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn parse_agent_config(path: &std::path::Path) -> right_agent::agent::AgentConfig {
        let content = std::fs::read_to_string(path).unwrap();
        serde_saphyr::from_str(&content).unwrap()
    }

    #[test]
    fn update_agent_yaml_memory_full_hindsight_block_roundtrips() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("hs_abc123".into()),
            bank_id: Some("my-bank".into()),
            recall_budget: RecallBudget::High,
            recall_max_tokens: 8192,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let parsed = parse_agent_config(&path);
        let m = parsed.memory.expect("memory block must be present");
        assert_eq!(m.provider, MemoryProvider::Hindsight);
        assert_eq!(m.api_key.as_deref(), Some("hs_abc123"));
        assert_eq!(m.bank_id.as_deref(), Some("my-bank"));
        assert_eq!(m.recall_budget, RecallBudget::High);
        assert_eq!(m.recall_max_tokens, 8192);
    }

    #[test]
    fn update_agent_yaml_memory_omits_default_recall_fields() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("hs_x".into()),
            bank_id: None,
            recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("recall_budget"),
            "default recall_budget must not be emitted, got:\n{content}"
        );
        assert!(
            !content.contains("recall_max_tokens"),
            "default recall_max_tokens must not be emitted, got:\n{content}"
        );
        assert!(
            !content.contains("bank_id"),
            "absent bank_id must not be emitted, got:\n{content}"
        );
    }

    #[test]
    fn update_agent_yaml_memory_replaces_existing_block() {
        let dir = tempdir().unwrap();
        let initial = "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"old\"\n  bank_id: \"old-bank\"\n";
        let path = write_yaml(dir.path(), initial);
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("new".into()),
            bank_id: Some("new-bank".into()),
            recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("\"old\""), "old api_key must be gone");
        assert!(!content.contains("old-bank"), "old bank_id must be gone");
        assert!(content.contains("\"new\""), "new api_key must be present");
        assert!(content.contains("new-bank"), "new bank_id must be present");

        let parsed = parse_agent_config(&path);
        let m = parsed.memory.unwrap();
        assert_eq!(m.api_key.as_deref(), Some("new"));
        assert_eq!(m.bank_id.as_deref(), Some("new-bank"));
    }

    #[test]
    fn remove_agent_yaml_memory_strips_entire_block() {
        let dir = tempdir().unwrap();
        let initial = "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"x\"\n  bank_id: \"b\"\n\nnetwork_policy: permissive\n";
        let path = write_yaml(dir.path(), initial);
        remove_agent_yaml_memory(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("memory:"),
            "memory block must be gone, got:\n{content}"
        );
        assert!(
            !content.contains("api_key"),
            "api_key must be gone, got:\n{content}"
        );
        assert!(
            content.contains("network_policy: permissive"),
            "unrelated fields must survive, got:\n{content}"
        );

        let parsed = parse_agent_config(&path);
        assert!(parsed.memory.is_none(), "memory must be None after removal");
    }

    #[test]
    fn update_agent_yaml_memory_appends_when_absent() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("k".into()),
            bank_id: None,
            recall_budget: RecallBudget::Low,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("model: \"sonnet\""),
            "pre-existing field survives"
        );
        assert!(content.contains("memory:"), "memory block added");
        assert!(
            content.contains("recall_budget: low"),
            "non-default budget emitted"
        );
    }
}

/// Replace the `stt:` block in an agent.yaml file.
///
/// Removes any existing `stt:` block (header + indented body), then appends
/// the new block.  Matches the style of the other `update_agent_yaml_*`
/// helpers: line-by-line block stripping + unconditional append.
fn update_agent_yaml_stt(
    path: &Path,
    stt: &right_agent::agent::types::SttConfig,
) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Strip existing stt: block (header + indented lines).
    let mut lines: Vec<String> = Vec::new();
    let mut in_stt_block = false;
    for line in content.lines() {
        if line == "stt:" {
            in_stt_block = true;
            continue;
        }
        if in_stt_block {
            if line.starts_with("  ") {
                continue;
            }
            in_stt_block = false;
        }
        lines.push(line.to_string());
    }

    // Append new stt block.
    lines.push("stt:".to_string());
    lines.push(format!("  enabled: {}", stt.enabled));
    lines.push(format!("  model: {}", stt.model.yaml_str()));

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
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

#[cfg(test)]
mod stt_yaml_tests {
    use super::*;
    use right_agent::agent::types::{SttConfig, WhisperModel};

    #[test]
    fn append_stt_when_block_missing() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "telegram_token: \"x\"\n").unwrap();

        let stt = SttConfig {
            enabled: true,
            model: WhisperModel::Small,
        };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("stt:"));
        assert!(content.contains("enabled: true"));
        assert!(content.contains("model: small"));
    }

    #[test]
    fn replace_stt_when_block_present() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "telegram_token: \"x\"\nstt:\n  enabled: true\n  model: tiny\n",
        )
        .unwrap();

        let stt = SttConfig {
            enabled: false,
            model: WhisperModel::Small,
        };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        // Block replaced, not duplicated:
        assert_eq!(content.matches("stt:").count(), 1, "exactly one stt: block");
        assert!(content.contains("enabled: false"));
        assert!(content.contains("model: small"));
        assert!(!content.contains("model: tiny"));
    }
}
