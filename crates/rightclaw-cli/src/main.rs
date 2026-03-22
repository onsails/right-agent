use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use clap::{Parser, Subcommand};

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

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize RightClaw home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
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
        /// Run without OpenShell sandbox (development only)
        #[arg(long)]
        no_sandbox: bool,
    },
    /// Stop all agents and destroy sandboxes
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
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    miette::set_hook(Box::new(|_| Box::new(miette::MietteHandlerOpts::new().build())))?;

    let cli = Cli::parse();

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
        Commands::Init { telegram_token } => cmd_init(&home, telegram_token.as_deref()),
        Commands::List => cmd_list(&home),
        Commands::Doctor => cmd_doctor(&home),
        Commands::Up {
            agents,
            detach,
            no_sandbox,
        } => cmd_up(&home, agents, detach, no_sandbox).await,
        Commands::Down => cmd_down(&home).await,
        Commands::Status => cmd_status(&home).await,
        Commands::Restart { agent } => cmd_restart(&home, &agent).await,
        Commands::Attach => cmd_attach(&home),
    }
}

fn cmd_init(home: &Path, telegram_token: Option<&str>) -> miette::Result<()> {
    // If --telegram-token flag provided, validate it upfront.
    // Otherwise prompt interactively (per D-06, D-07).
    let token = match telegram_token {
        Some(t) => {
            rightclaw::init::validate_telegram_token(t)?;
            Some(t.to_string())
        }
        None => rightclaw::init::prompt_telegram_token()?,
    };

    rightclaw::init::init_rightclaw_home(home, token.as_deref(), None)?;

    println!("Initialized RightClaw at {}", home.display());
    println!(
        "Default agent 'right' created at {}/agents/right/",
        home.display()
    );
    if token.is_some() {
        println!("Telegram channel configured. Install the plugin in Claude Code:");
        println!("  /plugin install telegram@claude-plugins-official");
    }
    Ok(())
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
            let mcp_status = if agent.mcp_config_path.is_some() {
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
) -> miette::Result<()> {
    // Fail fast if required tools are missing.
    rightclaw::runtime::verify_dependencies(no_sandbox)?;

    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let socket_path = run_dir.join("pc.sock");

    // Check for stale socket / already-running instance.
    if socket_path.exists() {
        let client = rightclaw::runtime::PcClient::new(&socket_path)?;
        match client.health_check().await {
            Ok(()) => {
                return Err(miette::miette!(
                    "rightclaw is already running. Use `rightclaw down` first or `rightclaw attach` to connect."
                ));
            }
            Err(_) => {
                // Stale socket -- remove it.
                tracing::debug!("removing stale socket at {}", socket_path.display());
                std::fs::remove_file(&socket_path).map_err(|e| {
                    miette::miette!("failed to remove stale socket: {e:#}")
                })?;
            }
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

    // Generate shell wrappers for each agent.
    for agent in &agents {
        // Generate system prompt if agent has crons/ directory (per D-16, D-21).
        let system_prompt_path = match rightclaw::codegen::generate_system_prompt(agent) {
            Some(content) => {
                let path = run_dir.join(format!("{}-system.md", agent.name));
                std::fs::write(&path, &content).map_err(|e| {
                    miette::miette!(
                        "failed to write system prompt for '{}': {e:#}",
                        agent.name
                    )
                })?;
                tracing::debug!(agent = %agent.name, "wrote system prompt: {}", path.display());
                Some(path.display().to_string())
            }
            None => None,
        };

        let wrapper_content = rightclaw::codegen::generate_wrapper(
            agent,
            no_sandbox,
            system_prompt_path.as_deref(),
        )?;
        let wrapper_path = run_dir.join(format!("{}.sh", agent.name));
        std::fs::write(&wrapper_path, &wrapper_content)
            .map_err(|e| miette::miette!("failed to write wrapper for '{}': {e:#}", agent.name))?;
        std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| {
                miette::miette!("failed to set wrapper permissions for '{}': {e:#}", agent.name)
            })?;
        tracing::debug!(agent = %agent.name, "wrote wrapper: {}", wrapper_path.display());
    }

    // Generate process-compose.yaml.
    let pc_config = rightclaw::codegen::generate_process_compose(&agents, &run_dir)?;
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
                sandbox_name: rightclaw::runtime::sandbox_name_for(&a.name),
            })
            .collect(),
        socket_path: socket_path.display().to_string(),
        started_at: format!("{}Z", now.as_secs()),
        no_sandbox,
    };
    let state_path = run_dir.join("state.json");
    rightclaw::runtime::write_state(&state, &state_path)?;

    // Build process-compose command.
    let mut cmd = tokio::process::Command::new("process-compose");
    cmd.args([
        "up",
        "-f",
        config_path.to_str().unwrap_or_default(),
        "--unix-socket",
        socket_path.to_str().unwrap_or_default(),
    ]);

    if detach {
        cmd.arg("--detached-with-tui");
        let child = cmd.spawn().map_err(|e| {
            miette::miette!("failed to spawn process-compose: {e:#}")
        })?;

        // Wait briefly for process-compose to start.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify it's alive.
        let client = rightclaw::runtime::PcClient::new(&socket_path)?;
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

    let state = rightclaw::runtime::read_state(&state_path).map_err(|_| {
        miette::miette!("No running instance found. Is rightclaw running?")
    })?;

    let socket_path = run_dir.join("pc.sock");

    // Best-effort shutdown via REST API.
    if socket_path.exists() {
        match rightclaw::runtime::PcClient::new(&socket_path) {
            Ok(client) => {
                if let Err(e) = client.shutdown().await {
                    tracing::warn!("process-compose shutdown request failed (may already be stopped): {e:#}");
                }
            }
            Err(e) => {
                tracing::warn!("could not connect to process-compose: {e:#}");
            }
        }
    }

    // Destroy sandboxes unless --no-sandbox was used.
    if !state.no_sandbox {
        rightclaw::runtime::destroy_sandboxes(&state.agents)?;
    }

    // Clean up socket file.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).map_err(|e| {
            miette::miette!("failed to remove socket file: {e:#}")
        })?;
    }

    println!("All agents stopped.");
    Ok(())
}

async fn cmd_status(home: &Path) -> miette::Result<()> {
    let socket_path = home.join("run").join("pc.sock");

    if !socket_path.exists() {
        return Err(miette::miette!(
            "No running instance. Run `rightclaw up` first."
        ));
    }

    let client = rightclaw::runtime::PcClient::new(&socket_path)?;
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

async fn cmd_restart(home: &Path, agent: &str) -> miette::Result<()> {
    let socket_path = home.join("run").join("pc.sock");

    if !socket_path.exists() {
        return Err(miette::miette!(
            "No running instance. Run `rightclaw up` first."
        ));
    }

    let client = rightclaw::runtime::PcClient::new(&socket_path)?;
    client.restart_process(agent).await?;

    println!("Restarted agent: {agent}");
    Ok(())
}

fn cmd_attach(home: &Path) -> miette::Result<()> {
    let socket_path = home.join("run").join("pc.sock");

    if !socket_path.exists() {
        return Err(miette::miette!(
            "No running instance. Run `rightclaw up` first."
        ));
    }

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("process-compose")
        .arg("attach")
        .arg("--unix-socket")
        .arg(&socket_path)
        .exec();

    Err(miette::miette!("Failed to attach: {err}"))
}
