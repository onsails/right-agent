use std::path::Path;

use clap::{Parser, Subcommand};

mod memory_server;

#[derive(Parser)]
#[command(name = "rightclaw", version, about = "Multi-agent runtime for Claude Code")]
pub struct Cli {
    /// Path to RightClaw home directory
    #[arg(long, env = "RIGHTCLAW_HOME")]
    pub home: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Subcommands for `rightclaw config`.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
}

/// Subcommands for `rightclaw memory`.
#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Show paginated memory table (newest first)
    List {
        /// Agent name
        agent: String,
        /// Max entries to show (default: 10)
        #[arg(long, default_value = "10")]
        limit: i64,
        /// Skip first N entries (for pagination)
        #[arg(long, default_value = "0")]
        offset: i64,
        /// Emit newline-delimited JSON instead of table
        #[arg(long)]
        json: bool,
    },
    /// Full-text search memories (FTS5 BM25)
    Search {
        /// Agent name
        agent: String,
        /// FTS5 search query
        query: String,
        /// Max entries to show (default: 10)
        #[arg(long, default_value = "10")]
        limit: i64,
        /// Skip first N entries (for pagination)
        #[arg(long, default_value = "0")]
        offset: i64,
        /// Emit newline-delimited JSON instead of table
        #[arg(long)]
        json: bool,
    },
    /// Hard-delete a memory entry (operator bypass of soft-delete)
    Delete {
        /// Agent name
        agent: String,
        /// Memory entry ID to delete
        id: i64,
    },
    /// Show memory database statistics
    Stats {
        /// Agent name
        agent: String,
        /// Emit JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize RightClaw home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Comma-separated list of Telegram chat IDs allowed to use this bot
        /// (e.g. --telegram-allowed-chat-ids 12345678,100200300)
        #[arg(long, value_delimiter = ',')]
        telegram_allowed_chat_ids: Vec<i64>,
        /// Cloudflare Named Tunnel name (created if not exists; requires cloudflared login)
        #[arg(long, default_value = "rightclaw")]
        tunnel_name: String,
        /// Public hostname for the tunnel (e.g. right.example.com)
        #[arg(long)]
        tunnel_hostname: Option<String>,
        /// Non-interactive mode — skip all prompts (requires --tunnel-hostname when cloudflared login detected)
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// List discovered agents and their status
    List,
    /// Validate dependencies and agent configuration
    Doctor,
    /// Launch agents with process-compose
    Up {
        /// Only launch specific agents (comma-separated)
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Launch in background with TUI server
        #[arg(short, long)]
        detach: bool,
        /// Disable sandbox enforcement (development only)
        #[arg(long)]
        no_sandbox: bool,
        /// Enable debug logging (writes to $RIGHTCLAW_HOME/run/<agent>-debug.log)
        #[arg(long)]
        debug: bool,
    },
    /// Stop all agents
    Down,
    /// Show running agent status
    Status,
    /// Restart a single agent
    Restart {
        /// Agent name to restart
        agent: String,
    },
    /// Attach to running process-compose TUI
    Attach,
    /// Launch an agent interactively for setup (Telegram pairing, onboarding)
    Pair {
        /// Agent name (defaults to "right")
        agent: Option<String>,
    },
    /// Manage RightClaw configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Inspect and manage agent memory databases
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Run MCP memory server (stdio transport, launched by Claude Code)
    MemoryServer,
    /// Run the per-agent Telegram bot (long-polling, teloxide)
    Bot {
        /// Agent name (resolves to $RIGHTCLAW_HOME/agents/<name>/)
        #[arg(long)]
        agent: String,
        /// Pass --verbose to CC subprocess and log CC stderr at debug level
        #[arg(long)]
        debug: bool,
    },
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    miette::set_hook(Box::new(|_| Box::new(miette::MietteHandlerOpts::new().build())))?;

    let cli = Cli::parse();

    // memory-server manages its own tracing (stderr-only for MCP compatibility).
    // Dispatch BEFORE the default tracing_subscriber init which writes to stdout.
    if matches!(cli.command, Commands::MemoryServer) {
        return memory_server::run_memory_server().await;
    }

    let filter = if cli.verbose {
        "rightclaw=debug"
    } else {
        "rightclaw=info"
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .init();

    let home = rightclaw::config::resolve_home(
        cli.home.as_deref(),
        std::env::var("RIGHTCLAW_HOME").ok().as_deref(),
    )?;

    match cli.command {
        Commands::Init { telegram_token, telegram_allowed_chat_ids, tunnel_name, tunnel_hostname, yes } => {
            cmd_init(&home, telegram_token.as_deref(), &telegram_allowed_chat_ids, &tunnel_name, tunnel_hostname.as_deref(), yes)
        }
        Commands::List => cmd_list(&home),
        Commands::Doctor => cmd_doctor(&home),
        Commands::Up {
            agents,
            detach,
            no_sandbox,
            debug,
        } => cmd_up(&home, agents, detach, no_sandbox, debug).await,
        Commands::Down => cmd_down(&home).await,
        Commands::Status => cmd_status(&home).await,
        Commands::Restart { agent } => cmd_restart(&home, &agent).await,
        Commands::Attach => cmd_attach(&home),
        Commands::Pair { agent } => cmd_pair(&home, agent.as_deref()),
        Commands::Config { command } => match command {
            ConfigCommands::StrictSandbox => cmd_config_strict_sandbox(),
        },
        Commands::Memory { command } => match command {
            MemoryCommands::List { agent, limit, offset, json } =>
                cmd_memory_list(&home, &agent, limit, offset, json),
            MemoryCommands::Search { agent, query, limit, offset, json } =>
                cmd_memory_search(&home, &agent, &query, limit, offset, json),
            MemoryCommands::Delete { agent, id } =>
                cmd_memory_delete(&home, &agent, id),
            MemoryCommands::Stats { agent, json } =>
                cmd_memory_stats(&home, &agent, json),
        },
        // Unreachable: MemoryServer is dispatched before reaching here.
        Commands::MemoryServer => unreachable!("MemoryServer dispatched before tracing init"),
        Commands::Bot { agent, debug } => {
            rightclaw_bot::run(rightclaw_bot::BotArgs {
                agent,
                home: cli.home,
                debug,
            })
            .await
        }
    }
}

fn cmd_init(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    yes: bool,
) -> miette::Result<()> {
    // If --telegram-token flag provided, validate it upfront.
    // Otherwise prompt interactively.
    let token = match telegram_token {
        Some(t) => {
            rightclaw::init::validate_telegram_token(t)?;
            Some(t.to_string())
        }
        None if yes => None,
        None => rightclaw::init::prompt_telegram_token()?,
    };

    rightclaw::init::init_rightclaw_home(home, token.as_deref(), telegram_allowed_chat_ids, None)?;

    println!("Initialized RightClaw at {}", home.display());
    println!(
        "Default agent 'right' created at {}/agents/right/",
        home.display()
    );
    if token.is_some() {
        println!("Telegram channel configured and plugin auto-enabled.");
    }
    if !telegram_allowed_chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }

    // Auto-tunnel setup: detect cloudflared login via cert.pem.
    if !detect_cloudflared_cert() {
        println!("No cloudflared login found. Run `cloudflared login` to enable tunnel support.");
        return Ok(());
    }

    let cf_bin = which::which("cloudflared")
        .map_err(|_| miette::miette!("cloudflared not found in PATH — install it first"))?;

    // Find or create the Named Tunnel.
    let existing = find_tunnel_by_name(&cf_bin, tunnel_name)?;
    let uuid = match existing {
        Some(ref t) => {
            if tunnel_hostname.is_some() || yes {
                // Silent reuse in non-interactive mode.
                t.id.clone()
            } else {
                let msg = format!("Tunnel '{}' already exists. Reuse it?", tunnel_name);
                if prompt_yes_no(&msg, true)? {
                    t.id.clone()
                } else {
                    return Err(miette::miette!("tunnel setup cancelled"));
                }
            }
        }
        None => {
            let created = create_tunnel(&cf_bin, tunnel_name)?;
            created.id
        }
    };

    // Resolve hostname.
    let hostname = match tunnel_hostname {
        Some(h) => h.to_string(),
        None if yes => {
            return Err(miette::miette!(
                "--tunnel-hostname is required when using -y"
            ));
        }
        None => prompt_hostname()?,
    };

    // Validate hostname is a bare domain, not a URL.
    if hostname.starts_with("https://") || hostname.starts_with("http://") {
        return Err(miette::miette!(
            "--tunnel-hostname must be a bare domain (e.g. example.com), not a URL"
        ));
    }

    // DNS CNAME record (non-fatal).
    route_dns(&cf_bin, &uuid, &hostname);

    // Credentials file is always at ~/.cloudflared/<uuid>.json — no copy needed.
    let credentials_file = cloudflared_credentials_path(&uuid)?;
    if !credentials_file.exists() {
        tracing::warn!(
            path = %credentials_file.display(),
            "credentials file not found — tunnel may have been created on a different machine"
        );
    }

    let tunnel_config = rightclaw::config::TunnelConfig {
        tunnel_uuid: uuid.clone(),
        credentials_file,
        hostname: hostname.clone(),
    };
    let config = rightclaw::config::GlobalConfig {
        tunnel: Some(tunnel_config),
    };
    rightclaw::config::write_global_config(home, &config)?;
    println!("Tunnel config written. UUID: {uuid}, hostname: {hostname}");

    Ok(())
}

// ---- Auto-tunnel helpers (Phase 39) ----

/// An entry returned by `cloudflared tunnel list -o json` or `cloudflared tunnel create -o json`.
#[derive(serde::Deserialize)]
struct TunnelListEntry {
    id: String,
    name: String,
}

/// Testable variant of detect_cloudflared_cert — takes an explicit home dir.
fn detect_cloudflared_cert_with_home(home: &std::path::Path) -> bool {
    home.join(".cloudflared").join("cert.pem").exists()
}

/// Returns true if `~/.cloudflared/cert.pem` exists (cloudflared login has been run).
fn detect_cloudflared_cert() -> bool {
    dirs::home_dir()
        .map(|h| detect_cloudflared_cert_with_home(&h))
        .unwrap_or(false)
}

/// Testable variant — builds `<home>/.cloudflared/<uuid>.json` path.
fn cloudflared_credentials_path_for_home(home: &std::path::Path, uuid: &str) -> std::path::PathBuf {
    home.join(".cloudflared").join(format!("{uuid}.json"))
}

/// Returns `~/.cloudflared/<uuid>.json` for the given tunnel UUID.
fn cloudflared_credentials_path(uuid: &str) -> miette::Result<std::path::PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    Ok(cloudflared_credentials_path_for_home(&home, uuid))
}

/// Query cloudflared for existing tunnels and find one by name.
fn find_tunnel_by_name(cf_bin: &std::path::Path, name: &str) -> miette::Result<Option<TunnelListEntry>> {
    let output = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "list", "-o", "json"])
        .output()
        .map_err(|e| miette::miette!("cloudflared list failed: {e:#}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel list failed: {stderr}"));
    }
    let tunnels: Vec<TunnelListEntry> = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("parse cloudflared list output: {e:#}"))?;
    Ok(tunnels.into_iter().find(|t| t.name == name))
}

/// Create a new Named Tunnel via cloudflared and return the created entry.
fn create_tunnel(cf_bin: &std::path::Path, name: &str) -> miette::Result<TunnelListEntry> {
    let output = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "create", "-o", "json", name])
        .output()
        .map_err(|e| miette::miette!("cloudflared create failed: {e:#}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel create failed: {stderr}"));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("parse cloudflared create output: {e:#}"))
}

/// Create a DNS CNAME record for the tunnel. Non-fatal — logs warn on failure.
fn route_dns(cf_bin: &std::path::Path, uuid: &str, hostname: &str) {
    let result = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "route", "dns", uuid, hostname])
        .output();
    match result {
        Ok(output) if output.status.success() => {
            println!("DNS CNAME record created for {hostname}");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("cloudflared route dns failed (non-fatal): {stderr}");
        }
        Err(e) => {
            tracing::warn!("cloudflared route dns invocation failed (non-fatal): {e:#}");
        }
    }
}

/// Prompt user with a Y/n question. Returns true if yes (default when `default_yes`).
fn prompt_yes_no(msg: &str, default_yes: bool) -> miette::Result<bool> {
    use std::io::{self, Write};
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{msg} {hint}: ");
    io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("read input: {e}"))?;
    let answer = input.trim().to_lowercase();
    if answer.is_empty() {
        return Ok(default_yes);
    }
    Ok(answer == "y" || answer == "yes")
}

/// Prompt user to enter a public hostname for the tunnel.
fn prompt_hostname() -> miette::Result<String> {
    use std::io::{self, Write};
    print!("Public hostname for tunnel (e.g. right.example.com): ");
    io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("read input: {e}"))?;
    Ok(input.trim().to_string())
}

fn cmd_doctor(home: &Path) -> miette::Result<()> {
    let checks = rightclaw::doctor::run_doctor(home);
    let mut has_failure = false;

    for check in &checks {
        if matches!(check.status, rightclaw::doctor::CheckStatus::Fail) {
            has_failure = true;
        }
        println!("{check}");
    }

    let pass_count = checks
        .iter()
        .filter(|c| matches!(c.status, rightclaw::doctor::CheckStatus::Pass))
        .count();
    let total = checks.len();
    println!("\n  {pass_count}/{total} checks passed");

    if has_failure {
        return Err(miette::miette!(
            "Some checks failed. See above for fix instructions."
        ));
    }
    Ok(())
}

fn cmd_list(home: &Path) -> miette::Result<()> {
    let agents_dir = home.join("agents");
    if !agents_dir.exists() {
        println!("No agents directory found. Run `rightclaw init` first.");
        return Ok(());
    }

    let agents = rightclaw::agent::discover_agents(&agents_dir)?;
    if agents.is_empty() {
        println!("No agents found in {}", agents_dir.display());
    } else {
        println!("Discovered {} agent(s):", agents.len());
        for agent in &agents {
            let config_status = if agent.config.is_some() { "yes" } else { "no" };
            let mcp_status = if agent.path.join(".mcp.json").exists() {
                "yes"
            } else {
                "no"
            };
            println!(
                "  {:<20} {}    config: {}    mcp: {}",
                agent.name,
                agent.path.display(),
                config_status,
                mcp_status,
            );
        }
    }
    Ok(())
}

async fn cmd_up(
    home: &Path,
    agents_filter: Option<Vec<String>>,
    detach: bool,
    no_sandbox: bool,
    debug: bool,
) -> miette::Result<()> {
    // Fail fast if required tools are missing.
    rightclaw::runtime::verify_dependencies()?;

    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let socket_path = run_dir.join("pc.sock");

    // Check for already-running instance via TCP health check.
    {
        let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
        if client.health_check().await.is_ok() {
            return Err(miette::miette!(
                "rightclaw is already running. Use `rightclaw down` first or `rightclaw attach` to connect."
            ));
        }
    }

    // Discover agents.
    let agents_dir = home.join("agents");
    let all_agents = rightclaw::agent::discover_agents(&agents_dir)?;

    // Apply --agents filter if provided.
    let agents = if let Some(ref filter) = agents_filter {
        let mut filtered = Vec::new();
        for name in filter {
            let found = all_agents.iter().find(|a| a.name == *name);
            match found {
                Some(agent) => filtered.push(agent.clone()),
                None => {
                    let available: Vec<&str> = all_agents.iter().map(|a| a.name.as_str()).collect();
                    return Err(miette::miette!(
                        "agent '{}' not found. Available agents: {}",
                        name,
                        available.join(", ")
                    ));
                }
            }
        }
        filtered
    } else {
        all_agents
    };

    if agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `rightclaw init` to create a default agent."
        ));
    }

    // Clear rightcron init locks so the bootstrap hook fires on this session.
    for agent in &agents {
        let lock = agent.path.join(".rightcron-init-done");
        let _ = std::fs::remove_file(&lock);
    }

    // Resolve host HOME before per-agent loop (must be done before any HOME env override).
    let host_home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;

    // Resolve current executable path once — written into each agent's .mcp.json so the
    // rightmemory MCP server can be found even when rightclaw is not on PATH (process-compose).
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    // Resolve system rg path once — passed to each agent's settings generation (D-01, SBOX-01).
    // When absent: warn, pass None; agents will fail at CC sandbox check (failIfUnavailable: true, D-02).
    let rg_path = match which::which("rg") {
        Ok(p) => {
            tracing::debug!(rg = %p.display(), "system ripgrep found");
            Some(p)
        }
        Err(_) => {
            tracing::warn!(
                "ripgrep (rg) not found in PATH — CC sandbox will fail to initialize. \
                 Install ripgrep: nix profile install nixpkgs#ripgrep / apt install ripgrep / brew install ripgrep"
            );
            None
        }
    };

    // Write agent definition, reply-schema.json, and settings.json for each agent.
    for agent in &agents {
        // Generate .claude/settings.json with sandbox config (Phase 6).
        let settings = rightclaw::codegen::generate_settings(agent, no_sandbox, &host_home, rg_path.clone())?;
        let claude_dir = agent.path.join(".claude");
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name))?;

        // Generate agent definition .md from present identity files (AGDEF-01).
        let agent_def_content = rightclaw::codegen::generate_agent_definition(agent)?;
        let agents_dir = claude_dir.join("agents");
        std::fs::create_dir_all(&agents_dir)
            .map_err(|e| miette::miette!("failed to create .claude/agents dir for '{}': {e:#}", agent.name))?;
        std::fs::write(agents_dir.join(format!("{}.md", agent.name)), &agent_def_content)
            .map_err(|e| miette::miette!("failed to write agent definition for '{}': {e:#}", agent.name))?;

        // Write reply-schema.json (D-01).
        std::fs::write(claude_dir.join("reply-schema.json"), rightclaw::codegen::REPLY_SCHEMA_JSON)
            .map_err(|e| miette::miette!("failed to write reply-schema.json for '{}': {e:#}", agent.name))?;

        tracing::debug!(agent = %agent.name, "wrote agent definition + reply-schema.json");
        // Pre-create shell-snapshots dir so CC Bash tool doesn't error on first run.
        std::fs::create_dir_all(claude_dir.join("shell-snapshots"))
            .map_err(|e| miette::miette!("failed to create shell-snapshots dir for '{}': {e:#}", agent.name))?;
        std::fs::write(
            claude_dir.join("settings.json"),
            serde_json::to_string_pretty(&settings)
                .map_err(|e| miette::miette!("failed to serialize settings for '{}': {e:#}", agent.name))?,
        )
        .map_err(|e| miette::miette!("failed to write settings.json for '{}': {e:#}", agent.name))?;
        tracing::debug!(agent = %agent.name, "wrote settings.json");

        // Generate per-agent .claude.json with trust entries (Phase 8, HOME-02).
        rightclaw::codegen::generate_agent_claude_json(agent)?;

        // Create credential symlink for OAuth under HOME override (Phase 8, HOME-03).
        rightclaw::codegen::create_credential_symlink(agent, &host_home)?;

        // Symlink host ~/.claude/plugins/ so agents share installed plugins (e.g. telegram).
        // Without this, CC starts with an empty plugin registry in the isolated HOME.
        rightclaw::codegen::create_plugins_symlink(agent, &host_home)?;

        // 6. git init if .git/ missing (Phase 9, AENV-01).
        // Non-fatal: log warning and continue if git binary absent.
        if !agent.path.join(".git").exists() {
            match std::process::Command::new("git")
                .arg("init")
                .current_dir(&agent.path)
                .status()
            {
                Ok(s) if s.success() => {
                    tracing::debug!(agent = %agent.name, "git init done");
                }
                Ok(s) => {
                    tracing::warn!(agent = %agent.name, "git init exited with status {}", s);
                }
                Err(e) => {
                    tracing::warn!(agent = %agent.name, "git binary not found, skipping git init: {e}");
                }
            }
        }

        // 7. (removed) Telegram channel config removed in Phase 26 — CC channels replaced by bot process.

        // 8. Reinstall built-in skills (Phase 9, AENV-03).
        // Always overwrites built-in skill dirs; user skill dirs untouched (D-10).
        // Remove stale clawhub dir from agents upgraded from pre-v2.2 (SKILLS-05, D-01, D-02).
        let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/clawhub"));
        // Remove stale skills/ dir from agents upgraded from Phase 12 intermediate state (CLEANUP-02).
        let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/skills"));
        rightclaw::codegen::install_builtin_skills(&agent.path)?;

        // 9. Write settings.local.json only if absent (Phase 9, AENV-03).
        // CC and agents may write runtime state here — never overwrite (D-11).
        let settings_local = agent.path.join(".claude").join("settings.local.json");
        if !settings_local.exists() {
            std::fs::write(&settings_local, "{}")
                .map_err(|e| miette::miette!("failed to write settings.local.json for '{}': {e:#}", agent.name))?;
        }

        // 10. Initialize per-agent memory database (Phase 16, DB-01).
        rightclaw::memory::open_db(&agent.path)
            .map_err(|e| miette::miette!("failed to open memory database for '{}': {e:#}", agent.name))?;
        tracing::debug!(agent = %agent.name, "memory.db initialized");

        // 11. Generate .mcp.json with rightmemory MCP server entry (Phase 17, SKILL-05).
        rightclaw::codegen::generate_mcp_config(&agent.path, &self_exe, &agent.name)?;
        tracing::debug!(agent = %agent.name, "wrote .mcp.json with rightmemory entry");
    }

    // Generate cloudflared config and wrapper script when tunnel is configured (Phase 38).
    let global_cfg = rightclaw::config::read_global_config(home)?;

    // Pre-flight: if tunnel is configured, cloudflared binary must be in PATH.
    // Check before generating any files to avoid leaving stale artifacts.
    if global_cfg.tunnel.is_some() {
        which::which("cloudflared").map_err(|_| {
            miette::miette!(
                "TunnelConfig is present but `cloudflared` is not in PATH — install cloudflared first"
            )
        })?;
    }

    let cloudflared_script_path: Option<std::path::PathBuf> = if let Some(tunnel_cfg) = global_cfg.tunnel {
        let agent_pairs: Vec<(String, std::path::PathBuf)> = agents
            .iter()
            .map(|a| (a.name.clone(), a.path.clone()))
            .collect();

        let creds = rightclaw::codegen::cloudflared::CloudflaredCredentials {
            tunnel_uuid: tunnel_cfg.tunnel_uuid.clone(),
            credentials_file: tunnel_cfg.credentials_file.clone(),
        };

        let cf_config = rightclaw::codegen::cloudflared::generate_cloudflared_config(
            &agent_pairs,
            &tunnel_cfg.hostname,
            Some(&creds),
        )?;
        let cf_config_path = home.join("cloudflared-config.yml");
        std::fs::write(&cf_config_path, &cf_config)
            .map_err(|e| miette::miette!("write cloudflared config: {e:#}"))?;
        tracing::info!(path = %cf_config_path.display(), "cloudflared config written");

        // Write DNS routing wrapper script.
        // route dns is non-fatal (|| true) — DNS record persists across restarts;
        // cert.pem expiry should not prevent cloudflared from running.
        let scripts_dir = home.join("scripts");
        std::fs::create_dir_all(&scripts_dir)
            .map_err(|e| miette::miette!("create scripts dir: {e:#}"))?;
        let uuid = &tunnel_cfg.tunnel_uuid;
        let hostname = &tunnel_cfg.hostname;
        let cf_config_path_str = cf_config_path.display();
        let script_content = format!(
            "#!/bin/sh\ncloudflared tunnel route dns {uuid} {hostname} || true\nexec cloudflared tunnel --config {cf_config_path_str} run\n"
        );
        let script_path = scripts_dir.join("cloudflared-start.sh");
        std::fs::write(&script_path, &script_content)
            .map_err(|e| miette::miette!("write cloudflared-start.sh: {e:#}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| miette::miette!("chmod cloudflared-start.sh: {e:#}"))?;
        }
        tracing::info!(path = %script_path.display(), "cloudflared wrapper script written");
        Some(script_path)
    } else {
        None
    };
    // Generate process-compose.yaml (bot-only entries, Phase 26).
    let has_bot_agents = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some() || c.telegram_token_file.is_some())
            .unwrap_or(false)
    });
    if !has_bot_agents {
        eprintln!("rightclaw: no agents have Telegram tokens configured — nothing to start");
        return Err(miette::miette!("no agents have Telegram tokens configured"));
    }
    let pc_config = rightclaw::codegen::generate_process_compose(
        &agents,
        &self_exe,
        debug,
        cloudflared_script_path.as_deref(),
    )?;
    let config_path = run_dir.join("process-compose.yaml");
    std::fs::write(&config_path, &pc_config)
        .map_err(|e| miette::miette!("failed to write process-compose.yaml: {e:#}"))?;
    tracing::debug!("wrote process-compose config: {}", config_path.display());

    // Write runtime state for `down` command.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| miette::miette!("system time error: {e:#}"))?;
    let state = rightclaw::runtime::RuntimeState {
        agents: agents
            .iter()
            .map(|a| rightclaw::runtime::AgentState {
                name: a.name.clone(),
            })
            .collect(),
        socket_path: socket_path.display().to_string(),
        started_at: format!("{}Z", now.as_secs()),
    };
    let state_path = run_dir.join("state.json");
    rightclaw::runtime::write_state(&state, &state_path)?;

    // Build process-compose command.
    let mut cmd = tokio::process::Command::new("process-compose");
    // Use TCP API (avoids --use-uds which crashes TUI).
    let pc_port = rightclaw::runtime::PC_PORT.to_string();
    cmd.args([
        "up",
        "-f",
        config_path.to_str().unwrap_or_default(),
        "--port",
        &pc_port,
    ]);

    if detach {
        cmd.arg("--detached");
        let child = cmd.spawn().map_err(|e| {
            miette::miette!("failed to spawn process-compose: {e:#}")
        })?;

        // Wait briefly for process-compose to start.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify it's alive.
        let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
        client.health_check().await.map_err(|e| {
            miette::miette!("process-compose started but health check failed: {e:#}")
        })?;

        println!(
            "rightclaw started in background ({} agent(s)). Use `rightclaw attach` to view TUI.",
            agents.len()
        );

        // Drop child handle without killing -- it's detached.
        drop(child);
    } else {
        let status = cmd.status().await.map_err(|e| {
            miette::miette!("failed to run process-compose: {e:#}")
        })?;

        if !status.success() {
            return Err(miette::miette!(
                "process-compose exited with status: {}",
                status
            ));
        }
    }

    Ok(())
}

async fn cmd_down(home: &Path) -> miette::Result<()> {
    let run_dir = home.join("run");
    let state_path = run_dir.join("state.json");

    let _state = rightclaw::runtime::read_state(&state_path).map_err(|_| {
        miette::miette!("No running instance found. Is rightclaw running?")
    })?;

    // Best-effort shutdown via REST API.
    match rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT) {
        Ok(client) => {
            if let Err(e) = client.shutdown().await {
                tracing::warn!("process-compose shutdown request failed (may already be stopped): {e:#}");
            }
        }
        Err(e) => {
            tracing::warn!("could not connect to process-compose: {e:#}");
        }
    }

    println!("All agents stopped.");
    Ok(())
}

async fn cmd_status(_home: &Path) -> miette::Result<()> {
    let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
    let processes = client.list_processes().await?;

    if processes.is_empty() {
        println!("No processes running.");
    } else {
        println!(
            "{:<20} {:<12} {:<10} UPTIME",
            "NAME", "STATUS", "PID"
        );
        for p in &processes {
            println!(
                "{:<20} {:<12} {:<10} {}",
                p.name, p.status, p.pid, p.system_time
            );
        }
    }

    Ok(())
}

async fn cmd_restart(_home: &Path, _agent: &str) -> miette::Result<()> {
    // process-compose crashes on programmatic restart (both REST API and CLI client).
    // This is a known process-compose bug. Direct users to the TUI instead.
    Err(miette::miette!(
        help = "Use the process-compose TUI: select the agent and press Ctrl+R to restart",
        "Programmatic restart is not supported (process-compose bug). Use `rightclaw attach` and Ctrl+R instead."
    ))
}

fn cmd_attach(_home: &Path) -> miette::Result<()> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("process-compose")
        .arg("attach")
        .arg("--port")
        .arg(rightclaw::runtime::PC_PORT.to_string())
        .exec();

    Err(miette::miette!("Failed to attach: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{cloudflared_credentials_path_for_home, detect_cloudflared_cert_with_home, resolve_agent_db, truncate_content, write_managed_settings, ConfigCommands, MemoryCommands, TunnelListEntry};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ---- memory commands variant existence (compile-time) ----

    #[test]
    fn memory_commands_list_variant_exists() {
        let _ = MemoryCommands::List {
            agent: "x".to_string(),
            limit: 10,
            offset: 0,
            json: false,
        };
    }

    #[test]
    fn memory_commands_stats_variant_exists() {
        let _ = MemoryCommands::Stats {
            agent: "x".to_string(),
            json: false,
        };
    }

    // ---- resolve_agent_db error paths ----

    #[test]
    fn resolve_agent_db_errors_on_missing_agent_dir() {
        let tmp = TempDir::new().unwrap();
        // home exists but agents/nonexistent does not
        let result = resolve_agent_db(tmp.path(), "nonexistent");
        let err = result.expect_err("should fail when agent dir missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("not found at"),
            "error must mention 'not found at', got: {msg}"
        );
    }

    #[test]
    fn resolve_agent_db_errors_on_missing_memory_db() {
        let tmp = TempDir::new().unwrap();
        // create agent dir but no memory.db
        let agent_dir = tmp.path().join("agents").join("testagent");
        fs::create_dir_all(&agent_dir).unwrap();
        let result = resolve_agent_db(tmp.path(), "testagent");
        let err = result.expect_err("should fail when memory.db missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("no memory database"),
            "error must mention 'no memory database', got: {msg}"
        );
    }

    // ---- Task 2: search/delete variant existence (compile-time) ----

    #[test]
    fn memory_commands_search_variant_exists() {
        let _ = MemoryCommands::Search {
            agent: "x".to_string(),
            query: "q".to_string(),
            limit: 10,
            offset: 0,
            json: false,
        };
    }

    #[test]
    fn memory_commands_delete_variant_exists() {
        let _ = MemoryCommands::Delete {
            agent: "x".to_string(),
            id: 1,
        };
    }

    // ---- truncate_content tests ----

    #[test]
    fn truncate_content_truncates_long_string() {
        let s = "a".repeat(65);
        let result = truncate_content(&s, 60);
        let char_count: usize = result.chars().count();
        assert_eq!(char_count, 61, "truncated string should be 61 chars (60 + ellipsis), got {char_count}");
        assert!(result.ends_with('…'), "truncated string should end with ellipsis");
    }

    #[test]
    fn truncate_content_preserves_short_string() {
        let result = truncate_content("hello", 60);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_content_handles_multibyte() {
        // "你好世界test" = 4 CJK + 4 ASCII = 8 chars total
        let result = truncate_content("你好世界test", 4);
        // should not panic; 4 chars taken + ellipsis = 5 chars
        let char_count: usize = result.chars().count();
        assert_eq!(char_count, 5, "should be 5 chars (4 + ellipsis), got {char_count}");
        assert!(result.ends_with('…'));
    }

    // ---- format_size tests ----

    #[test]
    fn format_size_bytes() {
        use super::format_size;
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        use super::format_size;
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_mb() {
        use super::format_size;
        assert_eq!(format_size(2_097_152), "2.0 MB");
    }

    // ---- config strict-sandbox tests ----

    #[test]
    fn config_commands_strict_sandbox_variant_exists() {
        // Compile-time check: ConfigCommands::StrictSandbox must exist.
        // If it doesn't compile, the test fails.
        let _cmd = ConfigCommands::StrictSandbox;
    }

    #[test]
    fn write_managed_settings_writes_correct_json_to_writable_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("etc").join("claude-code");
        let path = dir.join("managed-settings.json");

        write_managed_settings(
            dir.to_str().unwrap(),
            path.to_str().unwrap(),
        )
        .expect("should succeed in writable temp dir");

        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"allowManagedDomainsOnly\": true"),
            "file must contain allowManagedDomainsOnly:true, got: {content}"
        );
    }

    #[test]
    fn write_managed_settings_returns_error_with_sudo_hint_on_nonexistent_path() {
        // /nonexistent cannot be created without root.
        let result = write_managed_settings(
            "/nonexistent-rightclaw-test-dir",
            "/nonexistent-rightclaw-test-dir/managed-settings.json",
        );
        let err = result.expect_err("should fail on unwritable path");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("sudo"),
            "error must mention sudo, got: {msg}"
        );
    }

    #[test]
    fn write_managed_settings_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("etc").join("claude-code");
        let path = dir.join("managed-settings.json");

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
            .expect("first call should succeed");

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
            .expect("second call should also succeed (idempotent)");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"allowManagedDomainsOnly\": true"));
    }

    /// Creates a minimal agent directory with IDENTITY.md so discover_agents accepts it.
    fn make_agent_dir(base: &TempDir, name: &str) -> PathBuf {
        let agent_dir = base.path().join(name);
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("IDENTITY.md"), format!("# {name}\n")).unwrap();
        agent_dir
    }

    // ---- git init tests ----

    #[test]
    fn git_init_creates_dot_git_when_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-git-test");

        assert!(!agent_dir.join(".git").exists(), "pre-condition: no .git yet");

        // Run git init logic (same block as in cmd_up).
        if !agent_dir.join(".git").exists() {
            let status = std::process::Command::new("git")
                .arg("init")
                .current_dir(&agent_dir)
                .status();
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => panic!("git init failed with status {s}"),
                Err(e) => panic!("git not found: {e}"),
            }
        }

        assert!(agent_dir.join(".git").exists(), ".git/ should exist after init");
    }

    #[test]
    fn git_init_is_idempotent_when_dot_git_exists() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-idempotent");

        // First init.
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&agent_dir)
            .status()
            .expect("first git init should succeed");

        assert!(agent_dir.join(".git").exists());

        // Second run of the conditional block — should NOT re-init.
        let was_skipped = agent_dir.join(".git").exists();
        if was_skipped {
            // Condition false — nothing happens.
        } else {
            std::process::Command::new("git")
                .arg("init")
                .current_dir(&agent_dir)
                .status()
                .unwrap();
        }

        assert!(agent_dir.join(".git").exists(), ".git/ still present after idempotent run");
    }

    // ---- settings.local.json tests ----

    #[test]
    fn settings_local_json_created_when_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-settings");
        let claude_dir = agent_dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_local = claude_dir.join("settings.local.json");
        assert!(!settings_local.exists(), "pre-condition: no settings.local.json");

        if !settings_local.exists() {
            fs::write(&settings_local, "{}").unwrap();
        }

        assert!(settings_local.exists(), "settings.local.json should be created");
        assert_eq!(fs::read_to_string(&settings_local).unwrap(), "{}");
    }

    #[test]
    fn settings_local_json_not_overwritten_when_exists() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-settings-preserve");
        let claude_dir = agent_dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_local = claude_dir.join("settings.local.json");
        let original_content = r#"{"theme":"dark","customKey":42}"#;
        fs::write(&settings_local, original_content).unwrap();

        // cmd_up conditional: only write if absent.
        if !settings_local.exists() {
            fs::write(&settings_local, "{}").unwrap();
        }

        let after = fs::read_to_string(&settings_local).unwrap();
        assert_eq!(after, original_content, "pre-existing content must not be overwritten");
    }

    // ---- skills install tests ----

    #[test]
    fn skills_install_creates_builtin_skill_dirs() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-skills");

        rightclaw::codegen::install_builtin_skills(&agent_dir)
            .expect("install_builtin_skills should succeed");

        let skills_dir = agent_dir.join(".claude").join("skills");
        let skills_skill = skills_dir.join("rightskills").join("SKILL.md");
        assert!(skills_skill.exists(), "rightskills/SKILL.md should be installed");
    }

    #[test]
    fn cmd_up_removes_stale_clawhub_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-stale");

        // Simulate pre-v2.2 state: clawhub dir exists
        let stale = agent_dir.join(".claude").join("skills").join("clawhub");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SKILL.md"), "old content").unwrap();
        assert!(stale.exists(), "stale dir should exist before cleanup");

        // Run cleanup (same logic as cmd_up inserts)
        let _ = std::fs::remove_dir_all(agent_dir.join(".claude/skills/clawhub"));

        assert!(!stale.exists(), "stale clawhub dir should be removed after cleanup");
    }

    #[test]
    fn stale_cleanup_is_idempotent_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-no-stale");
        // No clawhub dir — cleanup should not error
        let result = std::fs::remove_dir_all(agent_dir.join(".claude/skills/clawhub"));
        // Either Ok or NotFound error — never panics
        assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::NotFound);
    }

    #[test]
    fn cmd_up_removes_stale_skills_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-stale-skills");

        // Simulate Phase 12 intermediate state: skills/ dir exists
        let stale = agent_dir.join(".claude").join("skills").join("skills");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SKILL.md"), "old content").unwrap();
        assert!(stale.exists(), "stale dir should exist before cleanup");

        // Run cleanup (same logic as cmd_up inserts)
        let _ = std::fs::remove_dir_all(agent_dir.join(".claude/skills/skills"));

        assert!(!stale.exists(), "stale skills dir should be removed after cleanup");
    }

    #[test]
    fn stale_skills_cleanup_is_idempotent_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-no-stale-skills");
        // No skills/ dir — cleanup should not error
        let result = std::fs::remove_dir_all(agent_dir.join(".claude/skills/skills"));
        // Either Ok or NotFound error — never panics
        assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::NotFound);
    }

    // ---- auto-tunnel helper tests (Phase 39) ----

    #[test]
    fn detect_cloudflared_cert_returns_false_when_absent() {
        let tmp = TempDir::new().unwrap();
        let result = detect_cloudflared_cert_with_home(tmp.path());
        assert!(!result, "should return false when cert.pem is absent");
    }

    #[test]
    fn detect_cloudflared_cert_returns_true_when_present() {
        let tmp = TempDir::new().unwrap();
        let cloudflared_dir = tmp.path().join(".cloudflared");
        fs::create_dir_all(&cloudflared_dir).unwrap();
        fs::write(cloudflared_dir.join("cert.pem"), "dummy cert content").unwrap();
        let result = detect_cloudflared_cert_with_home(tmp.path());
        assert!(result, "should return true when cert.pem exists");
    }

    #[test]
    fn cloudflared_credentials_path_constructs_expected_path() {
        let result = cloudflared_credentials_path_for_home(std::path::Path::new("/home/user"), "abc-123");
        assert_eq!(result, PathBuf::from("/home/user/.cloudflared/abc-123.json"));
    }

    #[test]
    fn parse_tunnel_list_finds_tunnel_by_name() {
        let json = r#"[{"id":"abc-123","name":"rightclaw","created_at":"2026-04-05T10:00:00Z","deleted_at":"0001-01-01T00:00:00Z","connections":[]}]"#;
        let tunnels: Vec<TunnelListEntry> = serde_json::from_str(json).expect("parse should succeed");
        let found = tunnels.into_iter().find(|t| t.name == "rightclaw");
        assert!(found.is_some(), "should find tunnel by name");
        assert_eq!(found.unwrap().id, "abc-123");
    }

    #[test]
    fn parse_tunnel_list_returns_none_for_missing_name() {
        let json = r#"[{"id":"abc-123","name":"rightclaw","created_at":"2026-04-05T10:00:00Z","deleted_at":"0001-01-01T00:00:00Z","connections":[]}]"#;
        let tunnels: Vec<TunnelListEntry> = serde_json::from_str(json).expect("parse should succeed");
        let found = tunnels.into_iter().find(|t| t.name == "other-tunnel");
        assert!(found.is_none(), "should return None for unknown tunnel name");
    }

    #[test]
    fn parse_tunnel_list_ignores_unknown_fields() {
        let json = r#"[{"id":"x","name":"n","future_field":"whatever"}]"#;
        let result: Result<Vec<TunnelListEntry>, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "parse must succeed with unknown fields present");
    }
}

const MANAGED_SETTINGS_DIR: &str = "/etc/claude-code";
const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";

/// Write managed-settings.json to the given dir/path (extracted for testability).
fn write_managed_settings(dir: &str, path: &str) -> miette::Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo rightclaw config strict-sandbox",
            "Permission denied creating {dir}: {e:#}"
        )
    })?;
    std::fs::write(path, "{\"allowManagedDomainsOnly\": true}\n").map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo rightclaw config strict-sandbox",
            "Permission denied writing {path}: {e:#}"
        )
    })?;
    Ok(())
}

fn cmd_config_strict_sandbox() -> miette::Result<()> {
    write_managed_settings(MANAGED_SETTINGS_DIR, MANAGED_SETTINGS_PATH)?;
    println!("Wrote {MANAGED_SETTINGS_PATH} — machine-wide domain blocking enabled.");
    Ok(())
}

/// Truncate content to at most `max_chars` characters, appending '…' if truncated.
/// Uses char-safe slicing (avoids byte-boundary panic on multi-byte UTF-8).
fn truncate_content(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Auto-scale byte count to human-readable size string.
fn format_size(bytes: u64) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_048_576 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

/// Resolve agent directory and open its memory database.
///
/// Returns a live `Connection` or a fatal miette error.
fn resolve_agent_db(home: &Path, agent: &str) -> miette::Result<rusqlite::Connection> {
    let agent_path = home.join("agents").join(agent);
    if !agent_path.exists() {
        return Err(miette::miette!(
            "agent '{}' not found at {}",
            agent,
            agent_path.display()
        ));
    }
    let db_path = agent_path.join("memory.db");
    if !db_path.exists() {
        return Err(miette::miette!(
            "no memory database for agent '{}' — run `rightclaw up` first",
            agent
        ));
    }
    rightclaw::memory::open_connection(&agent_path)
        .map_err(|e| miette::miette!("failed to open memory.db for '{}': {e:#}", agent))
}

fn cmd_memory_list(
    home: &Path,
    agent: &str,
    limit: i64,
    offset: i64,
    json: bool,
) -> miette::Result<()> {
    let conn = resolve_agent_db(home, agent)?;
    let entries = rightclaw::memory::list_memories(&conn, limit, offset)
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?;

    if json {
        for entry in &entries {
            println!(
                "{}",
                serde_json::to_string(entry)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories for agent '{agent}'.");
        return Ok(());
    }

    println!("{:<6} {:<61} {:<20} CREATED_AT", "ID", "CONTENT", "STORED_BY");
    for entry in &entries {
        let truncated = truncate_content(&entry.content, 60);
        let stored_by = entry.stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            entry.id, truncated, stored_by, entry.created_at
        );
    }

    // Pagination footer (text mode only, when result count == limit)
    if entries.len() as i64 == limit {
        let total: i64 = conn
            .query_row(
                "SELECT count(*) FROM memories WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .map_err(|e| miette::miette!("failed to count memories: {e:#}"))?;
        println!(
            "\n{} of {} entries shown  (--offset {} for next page)",
            limit,
            total,
            offset + limit
        );
    }

    Ok(())
}

fn cmd_memory_stats(home: &Path, agent: &str, json: bool) -> miette::Result<()> {
    // resolve_agent_db validates agent dir and memory.db existence before opening.
    let conn = resolve_agent_db(home, agent)?;

    // db_path needed only for fs metadata (file size) — derive from home, not conn.
    let db_path = home.join("agents").join(agent).join("memory.db");
    let db_size = std::fs::metadata(&db_path)
        .map_err(|e| miette::miette!("failed to stat memory.db: {e:#}"))?
        .len();

    let (total_entries, oldest, newest): (i64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT count(*), min(created_at), max(created_at) \
             FROM memories WHERE deleted_at IS NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| miette::miette!("failed to query stats: {e:#}"))?;

    if json {
        let obj = serde_json::json!({
            "agent": agent,
            "db_size_bytes": db_size,
            "total_entries": total_entries,
            "oldest": oldest,
            "newest": newest,
        });
        println!("{obj}");
        return Ok(());
    }

    println!("Agent:         {agent}");
    println!("DB size:       {}", format_size(db_size));
    println!("Total entries: {total_entries}");
    println!("Oldest:        {}", oldest.as_deref().unwrap_or("(none)"));
    println!("Newest:        {}", newest.as_deref().unwrap_or("(none)"));

    Ok(())
}

fn cmd_memory_search(
    home: &Path,
    agent: &str,
    query: &str,
    limit: i64,
    offset: i64,
    json: bool,
) -> miette::Result<()> {
    let conn = resolve_agent_db(home, agent)?;
    let entries = rightclaw::memory::search_memories_paged(&conn, query, limit, offset)
        .map_err(|e| {
            // FTS5 query syntax errors are common — give a helpful hint.
            miette::miette!(
                help = "FTS5 syntax: use simple words or phrases. Avoid special chars like * at start.",
                "search failed: {e:#}"
            )
        })?;

    if json {
        for entry in &entries {
            println!(
                "{}",
                serde_json::to_string(entry)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories match '{query}' for agent '{agent}'.");
        return Ok(());
    }

    println!("{:<6} {:<61} {:<20} CREATED_AT", "ID", "CONTENT", "STORED_BY");
    for entry in &entries {
        let truncated = truncate_content(&entry.content, 60);
        let stored_by = entry.stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            entry.id, truncated, stored_by, entry.created_at
        );
    }

    // Pagination footer (text mode only)
    if entries.len() as i64 == limit {
        println!(
            "\n{} results shown  (--offset {} for next page)",
            limit,
            offset + limit
        );
    }

    Ok(())
}

fn cmd_memory_delete(home: &Path, agent: &str, id: i64) -> miette::Result<()> {
    use rusqlite::OptionalExtension;
    use std::io::{self, Write};

    let conn = resolve_agent_db(home, agent)?;

    // Check soft-deleted rows too (hard-delete works on any existing row).
    let any_row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT content, stored_by FROM memories WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| miette::miette!("DB query failed: {e:#}"))?;

    match any_row {
        None => {
            return Err(miette::miette!("memory entry {id} not found for agent '{agent}'"));
        }
        Some((content, stored_by)) => {
            println!("  id:        {id}");
            println!("  content:   {}", truncate_content(&content, 60));
            println!("  stored_by: {}", stored_by.as_deref().unwrap_or("(unknown)"));
        }
    }

    print!("Hard-delete this entry? [y/N]: ");
    io::stdout()
        .flush()
        .map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("failed to read input: {e}"))?;

    if input.trim().to_lowercase() != "y" {
        println!("Aborted.");
        return Ok(());
    }

    rightclaw::memory::hard_delete_memory(&conn, id).map_err(|e| match e {
        rightclaw::memory::MemoryError::NotFound(n) => {
            miette::miette!("memory entry {n} not found for agent '{agent}'")
        }
        other => miette::miette!("failed to delete memory: {other:#}"),
    })?;

    println!("Deleted memory entry {id}.");
    Ok(())
}

fn cmd_pair(home: &Path, agent_name: Option<&str>) -> miette::Result<()> {
    let agent_name = agent_name.unwrap_or("right");

    let agents_dir = home.join("agents");
    let all_agents = rightclaw::agent::discover_agents(&agents_dir)?;

    let agent = all_agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = all_agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!(
                "agent '{}' not found. Available agents: {}",
                agent_name,
                available.join(", ")
            )
        })?;

    // Generate agent definition .md before exec (function may run without prior cmd_up).
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent_name))?;
    let agent_def_content = rightclaw::codegen::generate_agent_definition(agent)?;
    let agents_dir = claude_dir.join("agents");
    std::fs::create_dir_all(&agents_dir)
        .map_err(|e| miette::miette!("failed to create .claude/agents dir for '{}': {e:#}", agent_name))?;
    std::fs::write(agents_dir.join(format!("{}.md", agent.name)), &agent_def_content)
        .map_err(|e| miette::miette!("failed to write agent definition for '{}': {e:#}", agent_name))?;

    // Write reply-schema.json (D-01).
    std::fs::write(claude_dir.join("reply-schema.json"), rightclaw::codegen::REPLY_SCHEMA_JSON)
        .map_err(|e| miette::miette!("failed to write reply-schema.json for '{}': {e:#}", agent_name))?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| {
            miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)")
        })?;

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(claude_bin)
        .arg("--agent")
        .arg(&agent.name)
        .arg("--dangerously-skip-permissions")
        .arg("-p")
        .arg(&agent.path)
        .exec();

    Err(miette::miette!("failed to launch claude: {err}"))
}
