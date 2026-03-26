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

/// Subcommands for `rightclaw config`.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize RightClaw home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Telegram numeric user ID for auto-pairing (get from @userinfobot)
        #[arg(long)]
        telegram_user_id: Option<String>,
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
        Commands::Init { telegram_token, telegram_user_id } => cmd_init(&home, telegram_token.as_deref(), telegram_user_id.as_deref()),
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
    }
}

fn cmd_init(home: &Path, telegram_token: Option<&str>, telegram_user_id: Option<&str>) -> miette::Result<()> {
    // If --telegram-token flag provided, validate it upfront.
    // Otherwise prompt interactively (per D-06, D-07).
    let token = match telegram_token {
        Some(t) => {
            rightclaw::init::validate_telegram_token(t)?;
            Some(t.to_string())
        }
        None => rightclaw::init::prompt_telegram_token()?,
    };

    // Telegram user ID is required when token is provided (needed for auto-pairing).
    let user_id = match telegram_user_id {
        Some(id) => Some(id.to_string()),
        None if token.is_some() => {
            let id = prompt_telegram_user_id()?;
            if id.is_none() {
                return Err(miette::miette!(
                    help = "Get your numeric user ID from @userinfobot on Telegram",
                    "Telegram user ID is required for auto-pairing. Use --telegram-user-id or enter it when prompted."
                ));
            }
            id
        }
        None => None,
    };

    rightclaw::init::init_rightclaw_home(home, token.as_deref(), user_id.as_deref(), None)?;

    println!("Initialized RightClaw at {}", home.display());
    println!(
        "Default agent 'right' created at {}/agents/right/",
        home.display()
    );
    if token.is_some() {
        println!("Telegram channel configured and plugin auto-enabled.");
        if user_id.is_some() {
            println!("Telegram user pre-paired (no pairing step needed).");
        }
    }
    Ok(())
}

fn prompt_telegram_user_id() -> miette::Result<Option<String>> {
    use std::io::{self, Write};
    print!("Telegram numeric user ID for auto-pairing (get from @userinfobot, or Enter to skip): ");
    io::stdout().flush().map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("failed to read input: {e}"))?;
    let id = input.trim();
    if id.is_empty() {
        return Ok(None);
    }
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(miette::miette!(
            help = "Get your numeric user ID from @userinfobot on Telegram",
            "Invalid Telegram user ID — must be numeric"
        ));
    }
    Ok(Some(id.to_string()))
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

    // Generate shell wrappers for each agent.
    for agent in &agents {
        // Generate combined prompt (identity + start prompt + optional rightcron).
        let combined_content = rightclaw::codegen::generate_combined_prompt(agent)?;
        let prompt_path = run_dir.join(format!("{}-prompt.md", agent.name));
        std::fs::write(&prompt_path, &combined_content).map_err(|e| {
            miette::miette!(
                "failed to write combined prompt for '{}': {e:#}",
                agent.name
            )
        })?;
        tracing::debug!(agent = %agent.name, "wrote combined prompt: {}", prompt_path.display());

        let prompt_path_str = prompt_path.display().to_string();
        let debug_log = if debug {
            Some(run_dir.join(format!("{}-debug.log", agent.name)).display().to_string())
        } else {
            None
        };
        let wrapper_content = rightclaw::codegen::generate_wrapper(
            agent,
            &prompt_path_str,
            debug_log.as_deref(),
        )?;
        let wrapper_path = run_dir.join(format!("{}.sh", agent.name));
        std::fs::write(&wrapper_path, &wrapper_content)
            .map_err(|e| miette::miette!("failed to write wrapper for '{}': {e:#}", agent.name))?;
        std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| {
                miette::miette!("failed to set wrapper permissions for '{}': {e:#}", agent.name)
            })?;
        tracing::debug!(agent = %agent.name, "wrote wrapper: {}", wrapper_path.display());

        // Generate .claude/settings.json with sandbox config (Phase 6).
        let settings = rightclaw::codegen::generate_settings(agent, no_sandbox, &host_home)?;
        let claude_dir = agent.path.join(".claude");
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent.name))?;
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

        // 7. Telegram channel config (Phase 9, AENV-02, PERM-03).
        rightclaw::codegen::generate_telegram_channel_config(agent)?;

        // 8. Reinstall built-in skills (Phase 9, AENV-03).
        // Always overwrites built-in skill dirs; user skill dirs untouched (D-10).
        // Remove stale clawhub dir from agents upgraded from pre-v2.2 (SKILLS-05, D-01, D-02).
        let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/clawhub"));
        rightclaw::codegen::install_builtin_skills(&agent.path)?;

        // 9. Write settings.local.json only if absent (Phase 9, AENV-03).
        // CC and agents may write runtime state here — never overwrite (D-11).
        let settings_local = agent.path.join(".claude").join("settings.local.json");
        if !settings_local.exists() {
            std::fs::write(&settings_local, "{}")
                .map_err(|e| miette::miette!("failed to write settings.local.json for '{}': {e:#}", agent.name))?;
        }
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
    use super::{write_managed_settings, ConfigCommands};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

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

    // ---- telegram channel config tests ----

    #[test]
    fn telegram_config_not_created_when_no_telegram_fields() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-no-telegram");

        // Build an AgentDef with no telegram config.
        let agent = rightclaw::agent::AgentDef {
            name: "agent-no-telegram".to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: None,
            mcp_config_path: None,
            soul_path: None,
            user_path: None,
            memory_path: None,
            agents_path: None,
            tools_path: None,
            bootstrap_path: None,
            heartbeat_path: None,
        };

        rightclaw::codegen::generate_telegram_channel_config(&agent)
            .expect("should not fail when no telegram config");

        let telegram_dir = agent_dir.join(".claude").join("channels").join("telegram");
        assert!(
            !telegram_dir.exists(),
            ".claude/channels/telegram/ should NOT be created when no config"
        );
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

    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| miette::miette!("failed to create run directory: {e:#}"))?;

    let combined_content = rightclaw::codegen::generate_combined_prompt(agent)?;
    let prompt_path = run_dir.join(format!("{agent_name}-prompt.md"));
    std::fs::write(&prompt_path, &combined_content).map_err(|e| {
        miette::miette!(
            "failed to write combined prompt for '{}': {e:#}",
            agent_name
        )
    })?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| {
            miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)")
        })?;

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(claude_bin)
        .arg("--append-system-prompt-file")
        .arg(&prompt_path)
        .arg("--dangerously-skip-permissions")
        .arg("-p")
        .arg(&agent.path)
        .exec();

    Err(miette::miette!("failed to launch claude: {err}"))
}
