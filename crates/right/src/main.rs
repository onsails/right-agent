use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub(crate) mod aggregator;
pub(crate) mod internal_api;
mod memory_server;
pub(crate) mod right_backend;
mod wizard;

/// Source-of-truth list for every interactive prompt label rendered from
/// `crates/right/src/main.rs`. Mirrors `wizard::PROMPT_LABELS` /
/// `right_agent::init::PROMPT_LABELS` — the brand voice regression test
/// (`voice_pass_main`) walks this list and the `voice_pass.rs` integration
/// test on `right-agent` does the same for those crates. When you add or
/// edit an `inquire` prompt in this file, update this array — failure to do
/// so is caught by the test.
#[allow(dead_code)]
pub(crate) const MAIN_PROMPT_LABELS: &[&str] = &[
    // cmd_agent_init: existing-agent override path
    "how to initialize this agent?",
    "create fresh",
    "restore from backup",
    "backup directory path:",
    // prompt_dependencies: missing-binary install confirms
    "install openshell now?",
    "start openshell gateway now?",
    // cmd_agent_destroy
    "create backup before destroying?",
    // cmd_agent_destroy: dynamic confirm — agent_name varies, prefix is the static portion
    "permanently destroy agent '",
    // cmd_agent_rebootstrap: dynamic confirm — agent_name varies, prefix is the static portion
    "rebootstrap agent '",
    // cmd_agent_config: sandbox migration confirm
    "migrate sandbox now? (backup old, create new, restore data)",
];

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod voice_pass_main {
    //! Brand voice regression for `MAIN_PROMPT_LABELS`. Mirrors the inline
    //! `wizard::voice_pass` block and the integration test
    //! `crates/right-agent/tests/voice_pass.rs`.

    use super::MAIN_PROMPT_LABELS;

    const ALLOWED_PROPER_NOUNS: &[&str] = &[
        "HINDSIGHT_API_KEY",
        "RIGHT_TG_TOKEN",
        "MEMORY.md",
        "@BotFather",
        "@userinfobot",
    ];

    #[test]
    fn main_prompt_labels_are_lowercase_first() {
        for label in MAIN_PROMPT_LABELS {
            let trimmed = label.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            let first = trimmed.chars().next().unwrap();
            if !first.is_alphabetic() {
                continue;
            }
            let starts_with_proper = ALLOWED_PROPER_NOUNS
                .iter()
                .any(|noun| trimmed.starts_with(noun));
            if starts_with_proper {
                continue;
            }
            assert!(
                first.is_lowercase(),
                "prompt label must be lowercase-first (or start with an allowed proper noun): {label:?}"
            );
        }
    }

    #[test]
    fn main_prompt_labels_have_no_exclamation_marks() {
        for label in MAIN_PROMPT_LABELS {
            assert!(
                !label.contains('!'),
                "prompt label must not contain '!': {label:?}"
            );
        }
    }
}

#[derive(Parser)]
#[command(
    name = "right",
    version,
    about = "Multi-agent runtime for Claude Code"
)]
pub struct Cli {
    /// Path to Right Agent home directory
    #[arg(long, env = "RIGHT_HOME")]
    pub home: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Disable color output. Equivalent to setting NO_COLOR=1 for this run.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Subcommands for `right config`.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
    /// Read a config value by key (e.g. tunnel.hostname)
    Get {
        /// Config key (e.g. tunnel.hostname, tunnel.uuid, tunnel.credentials-file)
        key: String,
    },
    /// Set a config value by key
    Set {
        /// Config key
        key: String,
        /// New value
        value: String,
    },
}

/// Subcommands for `right agent`.
#[derive(Subcommand)]
pub enum AgentCommands {
    /// Initialize a new agent
    Init {
        /// Agent name (alphanumeric + hyphens)
        name: String,
        /// Non-interactive mode
        #[arg(short = 'y', long)]
        yes: bool,
        /// If agent exists, wipe and re-create (confirms unless -y)
        #[arg(long)]
        force_recreate: bool,
        /// With --force-recreate: re-run wizard instead of reusing existing config
        #[arg(long, requires = "force_recreate")]
        fresh: bool,
        /// Network policy: restrictive or permissive
        #[arg(long)]
        network_policy: Option<right_agent::agent::types::NetworkPolicy>,
        /// Sandbox mode: openshell or none
        #[arg(long)]
        sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
        /// Restore agent from a backup directory
        #[arg(long, conflicts_with_all = ["fresh", "network_policy", "sandbox_mode"])]
        from_backup: Option<std::path::PathBuf>,
    },
    /// Configure an agent interactively (or get/set a specific setting)
    Config {
        /// Agent name (interactive selection if omitted)
        name: Option<String>,
        /// Setting key (e.g. telegram-token)
        key: Option<String>,
        /// New value (omit to print current)
        value: Option<String>,
    },
    /// List discovered agents
    List,
    /// SSH into an agent's sandbox
    Ssh {
        /// Agent name
        name: String,
        /// Command to run inside the sandbox (optional)
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Back up an agent's sandbox and configuration
    Backup {
        /// Agent name
        name: String,
        /// Only back up sandbox files (skip agent.yaml, data.db, policy.yaml)
        #[arg(long)]
        sandbox_only: bool,
    },
    /// Destroy an agent (stop, optionally backup, delete sandbox and files)
    Destroy {
        /// Agent name
        name: String,
        /// Create backup before destroying
        #[arg(long)]
        backup: bool,
        /// Skip interactive prompts
        #[arg(long)]
        force: bool,
    },
    /// Re-enter bootstrap mode (debug only). Backs up identity files,
    /// deletes them from host and sandbox, recreates BOOTSTRAP.md, and
    /// deactivates active sessions. Sandbox, credentials, memory bank,
    /// and data.db rows are preserved.
    Rebootstrap {
        /// Agent name
        name: String,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Add a trusted user to this agent's allowlist
    Allow {
        /// Agent name
        name: String,
        /// Telegram user ID (positive integer)
        user_id: i64,
        /// Optional label (first_name or username)
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a trusted user from this agent's allowlist
    Deny {
        /// Agent name
        name: String,
        /// Telegram user ID
        user_id: i64,
    },
    /// Open a group for all members (non-trusted senders may address the bot)
    #[command(name = "allow_all")]
    AllowAll {
        /// Agent name
        name: String,
        /// Telegram group chat ID (negative integer for regular groups)
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// Optional label (group title)
        #[arg(long)]
        label: Option<String>,
    },
    /// Close an opened group
    #[command(name = "deny_all")]
    DenyAll {
        /// Agent name
        name: String,
        /// Telegram group chat ID
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
    },
    /// Dump the current allowlist
    Allowed {
        /// Agent name
        name: String,
        /// Emit as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `right memory`.
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

/// Subcommands for `right mcp`.
#[derive(Subcommand)]
pub enum McpCommands {
    /// Show MCP OAuth auth status for all agents (or a single agent)
    Status {
        /// Filter to a single agent by name
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize Right Agent home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Comma-separated list of Telegram chat IDs allowed to use this bot
        /// (e.g. --telegram-allowed-chat-ids 12345678,100200300)
        #[arg(long, value_delimiter = ',')]
        telegram_allowed_chat_ids: Vec<i64>,
        /// Cloudflare Named Tunnel name (created if not exists; requires cloudflared login)
        #[arg(long, default_value = "right")]
        tunnel_name: String,
        /// Public hostname for the tunnel (e.g. right.example.com)
        #[arg(long)]
        tunnel_hostname: Option<String>,
        /// Non-interactive mode — skip all prompts (requires --tunnel-hostname when cloudflared login detected)
        #[arg(short = 'y', long)]
        yes: bool,
        /// Network policy: restrictive (Anthropic/Claude only) or permissive (all HTTPS)
        #[arg(long)]
        network_policy: Option<right_agent::agent::types::NetworkPolicy>,
        /// Sandbox mode: openshell or none
        #[arg(long)]
        sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
        /// Recreate sandbox if it already exists (without prompting)
        #[arg(long)]
        force: bool,
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
        /// Enable debug logging (writes to $RIGHT_HOME/run/<agent>-debug.log)
        #[arg(long)]
        debug: bool,
    },
    /// Stop all agents
    Down,
    /// Re-sync agent codegen and hot-update running process-compose
    Reload {
        /// Only re-run codegen for specific agents (comma-separated).
        /// process-compose.yaml always includes all agents.
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
    },
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
    /// Manage Right Agent configuration (interactive wizard if no subcommand)
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    /// Manage agents
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Inspect and manage agent memory databases
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Run MCP memory server (stdio transport, launched by Claude Code)
    MemoryServer,
    /// Run MCP Aggregator HTTP server (multi-agent, Bearer token auth)
    McpServer {
        /// Port to listen on
        #[arg(long, default_value = "8100")]
        port: u16,
        /// Path to agent-tokens.json (agent name → Bearer token map)
        #[arg(long)]
        token_map: PathBuf,
    },
    /// Inspect MCP OAuth token status
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// Run the per-agent Telegram bot (long-polling, teloxide)
    Bot {
        /// Agent name (resolves to $RIGHT_HOME/agents/<name>/)
        #[arg(long)]
        agent: String,
        /// Pass --verbose to CC subprocess and log CC stderr at debug level
        #[arg(long)]
        debug: bool,
    },
}

/// Intercept `BlockAlreadyRendered`: exit code 1, no miette formatting.
/// Used when a command has already rendered a brand-conformant rail block
/// explaining the failure.
fn handle_dispatch(result: miette::Result<()>) -> miette::Result<()> {
    if let Err(ref e) = result
        && e.downcast_ref::<right_agent::ui::BlockAlreadyRendered>().is_some()
    {
        std::process::exit(1);
    }
    result
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    miette::set_hook(Box::new(|_| {
        Box::new(miette::MietteHandlerOpts::new().build())
    }))?;

    let cli = Cli::parse();

    if cli.no_color {
        // SAFETY: main is still single-threaded at this point — no readers of NO_COLOR yet.
        unsafe { std::env::set_var("NO_COLOR", "1"); }
    }

    // Brand-conformant inquire prompt chrome — replaces the default
    // LightGreen `?` and LightCyan answers/highlighted-options with subtle
    // DarkGrey (or no styling at all on Mono/Ascii themes).
    right_agent::ui::install_prompt_render_config();

    // memory-server manages its own tracing (stderr-only for MCP compatibility).
    // Dispatch BEFORE the default tracing_subscriber init which writes to stdout.
    if matches!(cli.command, Commands::MemoryServer) {
        return memory_server::run_memory_server().await;
    }

    let filter = if cli.verbose {
        "right=debug,right_agent=debug,right_bot=debug"
    } else {
        "right=info,right_agent=info,right_bot=info"
    };

    // Set up tracing with console + per-process file log.
    // Bot writes console to stderr (stdout reserved for JSON), aggregator to stdout (colored).
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let setup_file_log = |name: &str| {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".right")
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let file_appender = tracing_appender::rolling::daily(&log_dir, format!("{name}.log"));
        tracing_appender::non_blocking(file_appender)
    };

    let _log_guard = match &cli.command {
        Commands::Bot { agent, .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log(agent);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false),
                )
                .init();
            Some(guard)
        }
        Commands::McpServer { .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log("mcp-aggregator");
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false),
                )
                .init();
            Some(guard)
        }
        _ => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(env_filter)
                .init();
            None
        }
    };

    let home = right_agent::config::resolve_home(
        cli.home.as_deref(),
        std::env::var("RIGHT_HOME").ok().as_deref(),
    )?;

    let result = match cli.command {
        Commands::Init {
            telegram_token,
            telegram_allowed_chat_ids,
            tunnel_name,
            tunnel_hostname,
            yes,
            network_policy,
            sandbox_mode,
            force,
        } => cmd_init(
            &home,
            telegram_token.as_deref(),
            &telegram_allowed_chat_ids,
            &tunnel_name,
            tunnel_hostname.as_deref(),
            yes,
            network_policy,
            sandbox_mode,
            force,
        ),
        Commands::List => cmd_list(&home),
        Commands::Doctor => cmd_doctor(&home),
        Commands::Up {
            agents,
            detach,
            debug,
        } => cmd_up(&home, agents, detach, debug).await,
        Commands::Down => cmd_down(&home).await,
        Commands::Reload { agents } => cmd_reload(&home, agents).await,
        Commands::Status => cmd_status(&home).await,
        Commands::Restart { agent } => cmd_restart(&home, &agent).await,
        Commands::Attach => cmd_attach(&home),
        Commands::Pair { agent } => cmd_pair(&home, agent.as_deref()),
        Commands::Config { command } => match command {
            None => {
                crate::wizard::combined_setting_menu(&home).await?;
                Ok(())
            }
            Some(ConfigCommands::StrictSandbox) => cmd_config_strict_sandbox(),
            Some(ConfigCommands::Get { key }) => {
                let config = right_agent::config::read_global_config(&home)?;
                match key.as_str() {
                    "tunnel.hostname" => println!("{}", config.tunnel.hostname),
                    "tunnel.uuid" => println!("{}", config.tunnel.tunnel_uuid),
                    "tunnel.credentials-file" => {
                        println!("{}", config.tunnel.credentials_file.display())
                    }
                    other => return Err(miette::miette!("Unknown config key: {other}")),
                }
                Ok(())
            }
            Some(ConfigCommands::Set { key, value }) => Err(miette::miette!(
                "Direct set not yet implemented for key '{key}' with value '{value}'. Use `right config` for interactive mode."
            )),
        },
        Commands::Agent { command } => match command {
            AgentCommands::Init {
                name,
                yes,
                force_recreate,
                fresh,
                network_policy,
                sandbox_mode,
                from_backup,
            } => {
                if let Some(backup_path) = from_backup {
                    cmd_agent_restore(&home, &name, &backup_path).await
                } else {
                    cmd_agent_init(
                        &home,
                        &name,
                        yes,
                        force_recreate,
                        fresh,
                        network_policy,
                        sandbox_mode,
                    )
                }
            }
            AgentCommands::List => cmd_list(&home),
            AgentCommands::Config { name, key, value } => {
                match (key, value) {
                    (None, None) => {
                        let agent_name =
                            crate::wizard::agent_setting_menu(&home, name.as_deref()).await?;
                        maybe_migrate_sandbox(&home, &agent_name).await?;
                    }
                    (Some(_key), _) => {
                        return Err(miette::miette!(
                            "Direct get/set not yet implemented. Use `right agent config` for interactive mode."
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(miette::miette!("Cannot set a value without a key"));
                    }
                }
                Ok(())
            }
            AgentCommands::Ssh { name, command } => cmd_agent_ssh(&home, &name, &command).await,
            AgentCommands::Backup { name, sandbox_only } => {
                cmd_agent_backup(&home, &name, sandbox_only).await
            }
            AgentCommands::Destroy {
                name,
                backup,
                force,
            } => cmd_agent_destroy(&home, &name, backup, force).await,
            AgentCommands::Rebootstrap { name, yes } => {
                cmd_agent_rebootstrap(&home, &name, yes).await
            }
            AgentCommands::Allow {
                name,
                user_id,
                label,
            } => {
                if user_id < 0 {
                    miette::bail!(
                        "user_id cannot be negative (groups/channels use `right agent allow_all`)"
                    );
                }
                let dir = right_agent::config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AddOutcome, AllowedUser, AllowlistState};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<AddOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.add_user(AllowedUser {
                        id: user_id,
                        label: label.clone(),
                        added_by: None,
                        added_at: chrono::Utc::now(),
                    });
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    AddOutcome::Inserted => println!("added user {user_id}"),
                    AddOutcome::AlreadyPresent => println!("user {user_id} already allowed"),
                }
                Ok(())
            }
            AgentCommands::Deny { name, user_id } => {
                let dir = right_agent::config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, RemoveOutcome};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<RemoveOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.remove_user(user_id);
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    RemoveOutcome::Removed => println!("removed user {user_id}"),
                    RemoveOutcome::NotFound => println!("user {user_id} not in allowlist"),
                }
                Ok(())
            }
            AgentCommands::AllowAll {
                name,
                chat_id,
                label,
            } => {
                let dir = right_agent::config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AddOutcome, AllowedGroup, AllowlistState};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<AddOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.add_group(AllowedGroup {
                        id: chat_id,
                        label: label.clone(),
                        opened_by: None,
                        opened_at: chrono::Utc::now(),
                    });
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    AddOutcome::Inserted => println!("opened group {chat_id}"),
                    AddOutcome::AlreadyPresent => println!("group {chat_id} already opened"),
                }
                Ok(())
            }
            AgentCommands::DenyAll { name, chat_id } => {
                let dir = right_agent::config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist::{self, AllowlistState, RemoveOutcome};
                let outcome = allowlist::with_lock(&dir, |d| -> Result<RemoveOutcome, String> {
                    let file = allowlist::read_file(d)?.unwrap_or_default();
                    let mut state = AllowlistState::from_file(file);
                    let outcome = state.remove_group(chat_id);
                    allowlist::write_file_inner(d, &state.to_file())?;
                    Ok(outcome)
                })
                .map_err(|e| miette::miette!("{e}"))?;
                match outcome {
                    RemoveOutcome::Removed => println!("closed group {chat_id}"),
                    RemoveOutcome::NotFound => println!("group {chat_id} was not opened"),
                }
                Ok(())
            }
            AgentCommands::Allowed { name, json } => {
                let dir = right_agent::config::agents_dir(&home).join(&name);
                if !dir.exists() {
                    return Err(miette::miette!("agent not found: {}", dir.display()));
                }
                use right_agent::agent::allowlist;
                let file = allowlist::read_file(&dir)
                    .map_err(|e| miette::miette!("{e}"))?
                    .unwrap_or_default();
                if json {
                    let out = serde_json::json!({
                        "users": file.users,
                        "groups": file.groups,
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&out).map_err(|e| miette::miette!("{e:#}"))?
                    );
                } else {
                    println!("Trusted users:");
                    if file.users.is_empty() {
                        println!("  (none)");
                    }
                    for u in &file.users {
                        println!(
                            "  - {} {} (added {})",
                            u.id,
                            u.label.as_deref().unwrap_or(""),
                            u.added_at.format("%Y-%m-%d")
                        );
                    }
                    println!("Opened groups:");
                    if file.groups.is_empty() {
                        println!("  (none)");
                    }
                    for g in &file.groups {
                        println!(
                            "  - {} {} (opened {})",
                            g.id,
                            g.label.as_deref().unwrap_or(""),
                            g.opened_at.format("%Y-%m-%d")
                        );
                    }
                }
                Ok(())
            }
        },
        Commands::Memory { command } => match command {
            MemoryCommands::List {
                agent,
                limit,
                offset,
                json,
            } => cmd_memory_list(&home, &agent, limit, offset, json),
            MemoryCommands::Search {
                agent,
                query,
                limit,
                offset,
                json,
            } => cmd_memory_search(&home, &agent, &query, limit, offset, json),
            MemoryCommands::Delete { agent, id } => cmd_memory_delete(&home, &agent, id),
            MemoryCommands::Stats { agent, json } => cmd_memory_stats(&home, &agent, json),
        },
        Commands::Mcp { command } => match command {
            McpCommands::Status { agent } => cmd_mcp_status(&home, agent.as_deref()),
        },
        // Unreachable: MemoryServer is dispatched before reaching here.
        Commands::MemoryServer => unreachable!("MemoryServer dispatched before tracing init"),
        Commands::McpServer {
            port,
            ref token_map,
        } => {
            let agents_dir = right_agent::config::agents_dir(&home);
            let token_map_path = token_map.clone();
            let allowed_hosts = right_agent::config::read_global_config(&home)?
                .aggregator
                .allowed_hosts;
            let token_map_content = std::fs::read_to_string(token_map)
                .map_err(|e| miette::miette!("failed to read token map: {e:#}"))?;
            let token_entries: std::collections::HashMap<String, String> =
                serde_json::from_str(&token_map_content)
                    .map_err(|e| miette::miette!("failed to parse token map: {e:#}"))?;

            let token_map = {
                let mut map = std::collections::HashMap::new();
                for (agent_name, token) in &token_entries {
                    let agent_dir = agents_dir.join(agent_name);
                    map.insert(
                        token.clone(),
                        aggregator::AgentInfo {
                            name: agent_name.clone(),
                            dir: agent_dir,
                        },
                    );
                }
                std::sync::Arc::new(tokio::sync::RwLock::new(map))
            };

            let dispatcher = std::sync::Arc::new(aggregator::ToolDispatcher {
                agents: dashmap::DashMap::new(),
            });

            // Register agents in dispatcher, restoring proxy backends from SQLite.
            // Also create per-agent refresh schedulers and spawn reconnect tasks.
            let mut refresh_senders_map = std::collections::HashMap::new();
            let mut reconnect_managers_map: std::collections::HashMap<
                String,
                tokio::sync::Mutex<right_agent::mcp::reconnect::ReconnectManager>,
            > = std::collections::HashMap::new();
            let http_client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());

            for agent_name in token_entries.keys() {
                let agent_dir = agents_dir.join(agent_name);
                let agent_config = right_agent::agent::discovery::parse_agent_config(&agent_dir)
                    .ok()
                    .flatten();
                let mtls_dir = match &agent_config {
                    Some(config)
                        if *config.sandbox_mode() == right_agent::agent::SandboxMode::Openshell =>
                    {
                        match right_agent::openshell::preflight_check() {
                            right_agent::openshell::OpenShellStatus::Ready(dir) => Some(dir),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let right = right_backend::RightBackend::new(agents_dir.clone(), mtls_dir);

                // Load existing MCP servers from SQLite and create ProxyBackends.
                // Collect OAuth entries for refresh scheduling.
                let mut proxies = std::collections::HashMap::new();
                // Local to this block; extracting a named type alias is out of scope.
                #[allow(clippy::type_complexity)]
                let mut oauth_entries: Vec<(
                    String,
                    right_agent::mcp::refresh::OAuthServerState,
                    std::sync::Arc<tokio::sync::RwLock<Option<String>>>,
                )> = Vec::new();
                let mut oauth_server_names = std::collections::HashSet::<String>::new();

                match right_agent::memory::open_connection(&agent_dir, true) {
                    Ok(conn) => match right_agent::mcp::credentials::db_list_servers(&conn) {
                        Ok(servers) => {
                            for s in servers {
                                let auth_method = right_agent::mcp::proxy::AuthMethod::from_db(
                                    s.auth_type.as_deref(),
                                    s.auth_header.as_deref(),
                                );
                                let token = std::sync::Arc::new(tokio::sync::RwLock::new(
                                    s.auth_token.clone(),
                                ));

                                // Collect OAuth entries before moving token into ProxyBackend.
                                if s.auth_type.as_deref() == Some("oauth") {
                                    oauth_server_names.insert(s.name.clone());
                                    if let (Some(te), Some(cid), Some(exp)) =
                                        (&s.token_endpoint, &s.client_id, &s.expires_at)
                                    {
                                        let expires_at = chrono::DateTime::parse_from_rfc3339(exp)
                                            .map(|dt| dt.with_timezone(&chrono::Utc))
                                            .unwrap_or_else(|_| chrono::Utc::now());
                                        oauth_entries.push((
                                            s.name.clone(),
                                            right_agent::mcp::refresh::OAuthServerState {
                                                refresh_token: s.refresh_token.clone(),
                                                token_endpoint: te.clone(),
                                                client_id: cid.clone(),
                                                client_secret: s.client_secret.clone(),
                                                expires_at,
                                                server_url: s.url.clone(),
                                            },
                                            token.clone(),
                                        ));
                                    }
                                }

                                let backend = right_agent::mcp::proxy::ProxyBackend::new(
                                    s.name.clone(),
                                    agent_dir.clone(),
                                    s.url.clone(),
                                    token,
                                    auth_method,
                                );
                                proxies.insert(s.name, std::sync::Arc::new(backend));
                            }
                        }
                        Err(e) => tracing::error!(
                            agent = agent_name.as_str(),
                            "failed to list MCP servers: {e:#}"
                        ),
                    },
                    Err(e) => tracing::error!(
                        agent = agent_name.as_str(),
                        "failed to open DB for MCP restore: {e:#}"
                    ),
                }

                // Clone proxies for reconnect tasks before moving into registry.
                let proxies_snapshot: Vec<(
                    String,
                    std::sync::Arc<right_agent::mcp::proxy::ProxyBackend>,
                )> = proxies
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                // Wire up HindsightBackend if memory provider is configured.
                let hindsight = match &agent_config {
                    Some(agent_config) => {
                        if let Some(ref mem_config) = agent_config.memory {
                            if mem_config.provider
                                == right_agent::agent::types::MemoryProvider::Hindsight
                            {
                                let resolved_key = std::env::var("HINDSIGHT_API_KEY")
                                    .ok()
                                    .or_else(|| mem_config.api_key.clone());
                                if let Some(ref api_key) = resolved_key {
                                    let bank_id = mem_config
                                        .bank_id
                                        .as_deref()
                                        .unwrap_or(agent_name.as_str());
                                    let budget = mem_config.recall_budget.to_string();
                                    let client = right_agent::memory::hindsight::HindsightClient::new(
                                        api_key,
                                        bank_id,
                                        &budget,
                                        mem_config.recall_max_tokens,
                                        None,
                                    );
                                    let wrapper = std::sync::Arc::new(
                                        right_agent::memory::ResilientHindsight::new(
                                            client,
                                            agent_dir.clone(),
                                            "aggregator",
                                        ),
                                    );
                                    Some(std::sync::Arc::new(aggregator::HindsightBackend::new(
                                        wrapper,
                                    )))
                                } else {
                                    tracing::warn!(
                                        agent = agent_name.as_str(),
                                        "Hindsight provider configured but no API key found (set HINDSIGHT_API_KEY or memory.api_key in agent.yaml) — memory tools disabled"
                                    );
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                let registry = aggregator::BackendRegistry {
                    right,
                    proxies: std::sync::Arc::new(tokio::sync::RwLock::new(proxies)),
                    agent_dir: agent_dir.clone(),
                    hindsight,
                };
                dispatcher.agents.insert(agent_name.clone(), registry);

                // Spawn per-agent refresh scheduler.
                let (refresh_tx, refresh_rx) =
                    tokio::sync::mpsc::channel::<right_agent::mcp::refresh::RefreshMessage>(32);
                tokio::spawn(right_agent::mcp::refresh::run_refresh_scheduler(
                    agent_dir.clone(),
                    refresh_rx,
                ));

                // Build oauth_map for reconnect loop.
                let oauth_map: std::collections::HashMap<String, _> = oauth_entries
                    .into_iter()
                    .map(|(name, state, token_arc)| (name, (state, token_arc)))
                    .collect();

                // Send NewEntry for non-expired OAuth servers. Expired tokens
                // are handled by the reconnect task which sends NewEntry after refresh.
                for (name, (state, token_arc)) in &oauth_map {
                    if state.refresh_token.is_some() {
                        let due_in = right_agent::mcp::refresh::refresh_due_in(state);
                        if due_in > std::time::Duration::ZERO {
                            let msg = right_agent::mcp::refresh::RefreshMessage::NewEntry {
                                server_name: name.clone(),
                                state: state.clone(),
                                token: token_arc.clone(),
                            };
                            if let Err(e) = refresh_tx.send(msg).await {
                                tracing::warn!(
                                    agent = agent_name.as_str(),
                                    server = name.as_str(),
                                    "failed to send refresh entry: {e:#}",
                                );
                            }
                        }
                    }
                }

                // Spawn background reconnect tasks (fire-and-forget).
                let mut reconnect_mgr = right_agent::mcp::reconnect::ReconnectManager::new(
                    refresh_tx.clone(),
                    agent_dir.clone(),
                );
                for (server_name, backend) in proxies_snapshot {
                    let http = http_client.clone();
                    let agent_name_owned = agent_name.clone();

                    if let Some((oauth_state, token_arc)) = oauth_map.get(&server_name) {
                        // OAuth server — check token expiry before connecting.
                        let due_in = right_agent::mcp::refresh::refresh_due_in(oauth_state);
                        tracing::info!(
                            agent = agent_name.as_str(),
                            server = server_name.as_str(),
                            due_secs = due_in.as_secs(),
                            expires_at = %oauth_state.expires_at,
                            has_refresh_token = oauth_state.refresh_token.is_some(),
                            "reconnect: checking OAuth token",
                        );
                        if due_in == std::time::Duration::ZERO {
                            // Token expired — try refresh or mark NeedsAuth.
                            if oauth_state.refresh_token.is_some() {
                                reconnect_mgr.start_reconnect(
                                    server_name.clone(),
                                    backend,
                                    oauth_state.clone(),
                                    token_arc.clone(),
                                    http,
                                );
                            } else {
                                // No refresh_token — cannot refresh.
                                let b = backend.clone();
                                tokio::spawn(async move {
                                    b.set_status(right_agent::mcp::proxy::BackendStatus::NeedsAuth)
                                        .await;
                                });
                            }
                        } else {
                            // Token still valid — just connect.
                            tokio::spawn(async move {
                                if let Err(e) = backend.connect(http).await {
                                    tracing::warn!(
                                        agent = agent_name_owned.as_str(),
                                        server = server_name.as_str(),
                                        "reconnect failed: {e:#}",
                                    );
                                }
                            });
                        }
                    } else if oauth_server_names.contains(&server_name) {
                        // OAuth server with incomplete DB fields — cannot refresh.
                        tracing::warn!(
                            agent = agent_name.as_str(),
                            server = server_name.as_str(),
                            "OAuth server missing token_endpoint/client_id/expires_at — marking NeedsAuth",
                        );
                        let b = backend.clone();
                        tokio::spawn(async move {
                            b.set_status(right_agent::mcp::proxy::BackendStatus::NeedsAuth)
                                .await;
                        });
                    } else {
                        // Non-OAuth server — just connect.
                        tokio::spawn(async move {
                            if let Err(e) = backend.connect(http).await {
                                tracing::warn!(
                                    agent = agent_name_owned.as_str(),
                                    server = server_name.as_str(),
                                    "reconnect failed: {e:#}",
                                );
                            }
                        });
                    }
                }

                reconnect_managers_map
                    .insert(agent_name.clone(), tokio::sync::Mutex::new(reconnect_mgr));
                refresh_senders_map.insert(agent_name.clone(), refresh_tx);
            }

            let refresh_senders: aggregator::RefreshSenders =
                std::sync::Arc::new(refresh_senders_map);
            let reconnect_managers: aggregator::ReconnectManagers =
                std::sync::Arc::new(reconnect_managers_map);

            aggregator::run_aggregator_http(
                port,
                token_map,
                token_map_path,
                dispatcher,
                agents_dir,
                home,
                refresh_senders,
                reconnect_managers,
                allowed_hosts,
            )
            .await
        }
        Commands::Bot { agent, debug } => {
            let needs_restart = right_bot::run(right_bot::BotArgs {
                agent,
                home: cli.home,
                debug,
            })
            .await?;
            if needs_restart {
                std::process::exit(right_bot::CONFIG_RESTART_EXIT_CODE);
            }
            Ok(())
        }
    };
    handle_dispatch(result)
}

/// Filter agents by name, or clone all if no filter provided.
fn filter_agents(
    all_agents: &[right_agent::agent::AgentDef],
    filter: Option<&[String]>,
) -> miette::Result<Vec<right_agent::agent::AgentDef>> {
    let Some(names) = filter else {
        return Ok(all_agents.to_vec());
    };
    let mut filtered = Vec::new();
    for name in names {
        let found = all_agents
            .iter()
            .find(|a| a.name == *name)
            .cloned()
            .ok_or_else(|| {
                let available: Vec<&str> = all_agents.iter().map(|a| a.name.as_str()).collect();
                miette::miette!(
                    "agent '{}' not found. Available agents: {}",
                    name,
                    available.join(", ")
                )
            })?;
        filtered.push(found);
    }
    Ok(filtered)
}

#[allow(clippy::too_many_arguments)]
fn cmd_init(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    yes: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
    force: bool,
) -> miette::Result<()> {
    let interactive = !yes;

    // Brand: splash + dependency probe.
    {
        let theme = right_agent::ui::detect();
        let version = env!("CARGO_PKG_VERSION");
        println!(
            "{}",
            right_agent::ui::splash(theme, version, "sandboxed multi-agent runtime")
        );
        println!("{}", right_agent::ui::section(theme, "dependencies"));
        println!("{}", right_agent::ui::Rail::blank(theme));

        let mut block = right_agent::ui::Block::new();
        let mut fatal = false;

        // process-compose (fatal)
        match which::which("process-compose") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("process-compose")
                    .verb("in PATH"),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_agent::ui::status(right_agent::ui::Glyph::Err)
                        .noun("process-compose")
                        .verb("not in PATH")
                        .fix("https://f1bonacc1.github.io/process-compose/installation/"),
                );
            }
        }

        // claude (fatal)
        match which::which("claude") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("claude")
                    .verb("in PATH"),
            ),
            Err(_) => {
                fatal = true;
                block.push(
                    right_agent::ui::status(right_agent::ui::Glyph::Err)
                        .noun("claude")
                        .verb("not in PATH")
                        .fix("https://docs.anthropic.com/en/docs/claude-code"),
                );
            }
        }

        // openshell (warn)
        match which::which("openshell") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("openshell")
                    .verb("in PATH"),
            ),
            Err(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Warn)
                    .noun("openshell")
                    .verb("not in PATH (optional, sandbox mode)"),
            ),
        }

        // cloudflared (warn)
        match which::which("cloudflared") {
            Ok(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("cloudflared")
                    .verb("in PATH"),
            ),
            Err(_) => block.push(
                right_agent::ui::status(right_agent::ui::Glyph::Warn)
                    .noun("cloudflared")
                    .verb("not in PATH (optional, tunnel)"),
            ),
        }

        println!("{}", block.render(theme));
        println!("{}", right_agent::ui::Rail::blank(theme));

        if fatal {
            return Err(right_agent::ui::BlockAlreadyRendered.into());
        }
    }

    // Validate CLI-passed token up front so we fail fast before any prompt —
    // both in interactive and non-interactive mode.
    if let Some(t) = telegram_token {
        right_agent::init::validate_telegram_token(t)?;
    }

    // Non-interactive: use CLI flags or defaults.
    // Interactive: wizard with Esc-to-go-back between steps.
    let (
        sandbox,
        network_policy_val,
        token,
        chat_ids,
        memory_provider,
        memory_api_key,
        memory_bank_id,
        memory_recall_budget,
        memory_recall_max_tokens,
    );

    if !interactive {
        sandbox = sandbox_mode.unwrap_or(right_agent::agent::types::SandboxMode::Openshell);
        network_policy_val =
            network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
        token = telegram_token.map(|t| t.to_string());
        chat_ids = telegram_allowed_chat_ids.to_vec();
        memory_provider = right_agent::agent::types::MemoryProvider::Hindsight;
        memory_api_key = None;
        memory_bank_id = None;
        memory_recall_budget = right_agent::init::DEFAULT_RECALL_BUDGET;
        memory_recall_max_tokens = right_agent::init::DEFAULT_RECALL_MAX_TOKENS;
    } else {
        // Wizard state machine: Esc goes back to previous step.
        #[derive(Clone, Copy)]
        enum Step {
            Sandbox,
            Network,
            Telegram,
            ChatIds,
            Memory,
            Done,
        }

        let theme = right_agent::ui::detect();
        println!("{}", right_agent::ui::section(theme, "agent"));
        println!("{}", right_agent::ui::Rail::blank(theme));

        let mut step = if sandbox_mode.is_some() {
            Step::Network
        } else {
            Step::Sandbox
        };
        let mut w_sandbox = sandbox_mode.unwrap_or(right_agent::agent::types::SandboxMode::Openshell);
        let mut w_network =
            network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
        let mut w_token: Option<String> = telegram_token.map(|t| t.to_string());
        let mut w_chat_ids: Vec<i64> = telegram_allowed_chat_ids.to_vec();
        let mut w_mem = (
            right_agent::agent::types::MemoryProvider::Hindsight,
            None::<String>,
            None::<String>,
            right_agent::init::DEFAULT_RECALL_BUDGET,
            right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        );

        loop {
            match step {
                Step::Sandbox => {
                    if let Some(m) = sandbox_mode {
                        w_sandbox = m;
                        step = Step::Network;
                    } else if let Some(s) = right_agent::init::prompt_sandbox_mode()? {
                        w_sandbox = s;
                        step = Step::Network;
                    } else {
                        // Esc on first step — abort.
                        return Err(miette::miette!("cancelled"));
                    }
                }
                Step::Network => {
                    if matches!(w_sandbox, right_agent::agent::types::SandboxMode::Openshell) {
                        if let Some(p) = network_policy {
                            w_network = p;
                            step = Step::Telegram;
                        } else if let Some(p) = right_agent::init::prompt_network_policy()? {
                            w_network = p;
                            step = Step::Telegram;
                        } else {
                            step = Step::Sandbox; // back
                        }
                    } else {
                        w_network = network_policy
                            .unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
                        step = Step::Telegram;
                    }
                }
                Step::Telegram => {
                    if telegram_token.is_some() {
                        w_token = telegram_token.map(|t| t.to_string());
                        step = if w_token.is_some() {
                            Step::ChatIds
                        } else {
                            Step::Memory
                        };
                    } else {
                        use crate::wizard::TelegramSetupOutcome;
                        match crate::wizard::telegram_setup(None, true, false)? {
                            TelegramSetupOutcome::Token(t) => {
                                w_token = Some(t);
                                step = Step::ChatIds;
                            }
                            TelegramSetupOutcome::Skipped => {
                                w_token = None;
                                step = Step::Memory;
                            }
                            TelegramSetupOutcome::Back => {
                                step = Step::Network;
                            }
                        }
                    }
                }
                Step::ChatIds => {
                    if !telegram_allowed_chat_ids.is_empty() {
                        w_chat_ids = telegram_allowed_chat_ids.to_vec();
                        step = Step::Memory;
                    } else {
                        match crate::wizard::chat_ids_setup(false)? {
                            Some(ids) => {
                                w_chat_ids = ids;
                                step = Step::Memory;
                            }
                            None => {
                                step = Step::Telegram;
                            } // back
                        }
                    }
                }
                Step::Memory => match right_agent::init::prompt_memory_config("right")? {
                    Some((p, k, b, rb, rt)) => {
                        w_mem = (p, k, b, rb, rt);
                        step = Step::Done;
                    }
                    None => {
                        step = if w_token.is_some() {
                            Step::ChatIds
                        } else {
                            Step::Telegram
                        };
                    }
                },
                Step::Done => break,
            }
        }

        sandbox = w_sandbox;
        network_policy_val = w_network;
        token = w_token;
        chat_ids = w_chat_ids;
        memory_provider = w_mem.0;
        memory_api_key = w_mem.1;
        memory_bank_id = w_mem.2;
        memory_recall_budget = w_mem.3;
        memory_recall_max_tokens = w_mem.4;
    }

    // Compute memory_detail before init_right_home consumes memory_provider.
    let memory_detail = match memory_provider {
        right_agent::agent::types::MemoryProvider::Hindsight => "hindsight".to_string(),
        right_agent::agent::types::MemoryProvider::File => "file".to_string(),
    };

    right_agent::init::init_right_home(
        home,
        token.as_deref(),
        &chat_ids,
        &network_policy_val,
        &sandbox,
        memory_provider,
        memory_api_key,
        memory_bank_id,
        memory_recall_budget,
        memory_recall_max_tokens,
    )?;

    // Tunnel setup BEFORE codegen — codegen reads config.yaml (mandatory tunnel),
    // so we must write it first.
    {
        let theme = right_agent::ui::detect();
        println!("{}", right_agent::ui::section(theme, "tunnel"));
        println!("{}", right_agent::ui::Rail::blank(theme));
    }
    let tunnel_cfg = crate::wizard::tunnel_setup(tunnel_name, tunnel_hostname, interactive)?;
    let aggregator = if home.join("config.yaml").exists() {
        right_agent::config::read_global_config(home)?.aggregator
    } else {
        right_agent::config::AggregatorConfig::default()
    };
    let global_config = right_agent::config::GlobalConfig {
        tunnel: tunnel_cfg,
        aggregator,
    };
    right_agent::config::write_global_config(home, &global_config)?;

    // Run codegen for the default "right" agent.
    // Per-agent codegen was moved to bot startup (59243d0) but init needs it
    // for schemas and settings before sandbox staging upload.
    {
        let agent_dir = home.join("agents/right");
        let self_exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
        let agent_def = right_agent::agent::AgentDef {
            name: "right".to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: right_agent::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        right_agent::codegen::run_agent_codegen(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        right_agent::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;

        // Create sandbox if openshell mode.
        if matches!(sandbox, right_agent::agent::types::SandboxMode::Openshell) {
            let staging = agent_dir.join("staging");
            right_agent::openshell::prepare_staging_dir(&agent_dir, &staging)?;

            let policy_path = agent_dir.join("policy.yaml");
            // Must match `sandbox.name: right-{agent}` written by init_agent into agent.yaml.
            let sb_name = format!("right-{}", "right");
            let force_recreate = if force {
                true
            } else {
                prompt_sandbox_recreate_if_exists(&sb_name, interactive)?
            };
            let theme = right_agent::ui::detect();
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Info)
                    .noun("sandbox")
                    .verb("creating")
                    .render(theme)
            );
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    right_agent::openshell::ensure_sandbox(
                        &sb_name,
                        &policy_path,
                        Some(&staging),
                        force_recreate,
                    )
                    .await
                })
            })?;
            println!(
                "{}",
                right_agent::ui::status(right_agent::ui::Glyph::Ok)
                    .noun("sandbox")
                    .verb("ready")
                    .detail(&sb_name)
                    .render(theme)
            );

            let run_dir = home.join("run");
            std::fs::create_dir_all(run_dir.join("ssh"))
                .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(right_agent::openshell::generate_ssh_config(
                        &sb_name,
                        &run_dir.join("ssh"),
                    ))
            })?;
        }
    }

    let theme = right_agent::ui::detect();
    let mode = format!("{} ({})", sandbox, network_policy_val);
    let chat_ids_detail = if chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", chat_ids.len())
    };
    let telegram_detail = if token.is_some() {
        "configured".to_string()
    } else {
        "not configured".to_string()
    };

    let mut recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("right ({mode})"))
        .ok("tunnel", &global_config.tunnel.hostname);
    recap = if token.is_some() {
        recap.ok("telegram", &telegram_detail)
    } else {
        recap.warn("telegram", &telegram_detail)
    };
    recap = recap
        .ok("chat ids", &chat_ids_detail)
        .ok("memory", &memory_detail)
        .next("right up");
    println!("{}", recap.render(theme));

    Ok(())
}

fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force_recreate: bool,
    fresh: bool,
    network_policy: Option<right_agent::agent::types::NetworkPolicy>,
    sandbox_mode: Option<right_agent::agent::types::SandboxMode>,
) -> miette::Result<()> {
    let interactive = !yes;
    let agents_parent = right_agent::config::agents_dir(home);
    let agent_dir = agents_parent.join(name);
    let agent_existed = agent_dir.exists();

    // Reject if exists and --force-recreate not given.
    if agent_dir.exists() && !force_recreate {
        return Err(miette::miette!(
            help =
                "Use --force-recreate to wipe and re-create, or `right agent config` to change settings",
            "Agent directory already exists at {}",
            agent_dir.display()
        ));
    }

    // --- Force wipe logic ---
    let saved_overrides = if force_recreate && agent_dir.exists() {
        // Read existing config before deletion (unless --fresh).
        let saved = if fresh {
            None
        } else {
            let yaml_path = agent_dir.join("agent.yaml");
            let yaml_str = std::fs::read_to_string(&yaml_path).map_err(|e| {
                miette::miette!(
                    help = "Use --fresh to reconfigure from scratch",
                    "Could not read existing agent.yaml: {e:#}"
                )
            })?;
            let config: right_agent::agent::types::AgentConfig = serde_saphyr::from_str(&yaml_str)
                .map_err(|e| {
                    miette::miette!(
                        help = "Use --fresh to reconfigure from scratch",
                        "Could not parse existing agent.yaml: {e:#}"
                    )
                })?;
            Some(config)
        };

        // Check agent is not running.
        // NOTE: This check uses `runtime-state.json` (not `state.json`) and is
        // a no-op in practice — the file never exists under that name.
        // Pre-existing bug unrelated to the runtime-isolation fix; touching it
        // breaks `test_agent_init_force_*` which depend on the no-op path.
        let state_path = home.join("run/runtime-state.json");
        if state_path.exists() {
            let state = right_agent::runtime::read_state(&state_path)?;
            if state.agents.iter().any(|a| a.name == name) {
                return Err(miette::miette!(
                    help = "Run `right down` first",
                    "Agent '{name}' is currently running"
                ));
            }
        }

        // Confirm with user.
        if interactive {
            use std::io::{self, Write};
            println!("Agent \"{name}\" already exists at {}", agent_dir.display());
            println!("This will permanently delete:");
            println!("  - All agent files (identity, memory, skills, config)");
            let display_sb = saved
                .as_ref()
                .map(|c| right_agent::openshell::resolve_sandbox_name(name, c))
                .unwrap_or_else(|| right_agent::openshell::sandbox_name(name));
            println!("  - OpenShell sandbox \"{}\" (if exists)", display_sb);
            print!("Continue? [y/N] ");
            io::stdout()
                .flush()
                .map_err(|e| miette::miette!("stdout flush: {e}"))?;
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| miette::miette!("failed to read input: {e}"))?;
            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                return Err(miette::miette!("Aborted"));
            }
        }

        // Delete sandbox (best-effort, async).
        let sb_name = saved
            .as_ref()
            .map(|c| right_agent::openshell::resolve_sandbox_name(name, c))
            .unwrap_or_else(|| right_agent::openshell::sandbox_name(name));
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                right_agent::openshell::delete_sandbox(&sb_name).await;
            });
        });

        // Delete SSH config.
        let ssh_config = home.join(format!("run/ssh/{}.ssh-config", sb_name));
        if ssh_config.exists() {
            std::fs::remove_file(&ssh_config).ok();
        }

        // Delete agent directory.
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            miette::miette!(
                "Failed to delete agent directory {}: {e:#}",
                agent_dir.display()
            )
        })?;

        tracing::info!(agent = name, "wiped agent directory and sandbox");

        saved
    } else {
        None
    };

    let theme = right_agent::ui::detect();
    println!("{}", right_agent::ui::section(theme, &format!("agent init: {name}")));
    println!("{}", right_agent::ui::Rail::blank(theme));

    // --- Build overrides ---
    let overrides = if let Some(config) = saved_overrides {
        // Reuse saved config from old agent.yaml.
        right_agent::init::InitOverrides {
            sandbox_mode: *config.sandbox_mode(),
            network_policy: config.network_policy,
            telegram_token: config.telegram_token,
            allowed_chat_ids: config.allowed_chat_ids,
            model: config.model,
            env: config.env,
            memory_provider: config
                .memory
                .as_ref()
                .map(|m| m.provider.clone())
                .unwrap_or_default(),
            memory_api_key: config.memory.as_ref().and_then(|m| m.api_key.clone()),
            memory_bank_id: config.memory.as_ref().and_then(|m| m.bank_id.clone()),
            memory_recall_budget: config
                .memory
                .as_ref()
                .map(|m| m.recall_budget.clone())
                .unwrap_or(right_agent::init::DEFAULT_RECALL_BUDGET),
            memory_recall_max_tokens: config
                .memory
                .as_ref()
                .map(|m| m.recall_max_tokens)
                .unwrap_or(right_agent::init::DEFAULT_RECALL_MAX_TOKENS),
            stt: config.stt,
        }
    } else {
        // Fresh init: optionally restore from backup or run wizard.
        if interactive && !force_recreate {
            let options = vec!["create fresh", "restore from backup"];
            let choice = inquire::Select::new("how to initialize this agent?", options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if choice == "restore from backup" {
                let backup_path_str = inquire::Text::new("backup directory path:")
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                let backup_path = std::path::PathBuf::from(backup_path_str.trim());
                if !backup_path.exists() {
                    return Err(miette::miette!(
                        "Backup directory does not exist: {}",
                        backup_path.display()
                    ));
                }
                return tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(cmd_agent_restore(
                        home,
                        name,
                        &backup_path,
                    ))
                });
            }
        }

        // Run wizard or use CLI flags. Esc goes back to previous step.
        if !interactive {
            let ffmpeg_ok = right_agent::stt::ffmpeg_available();
            let stt = right_agent::agent::types::SttConfig {
                enabled: ffmpeg_ok,
                model: right_agent::agent::types::WhisperModel::Small,
            };
            if !ffmpeg_ok {
                eprintln!(
                    "warning: STT disabled — ffmpeg not in PATH. \
                     Install (macOS): brew install ffmpeg, then enable via \
                     `right agent config {name}`."
                );
            }
            right_agent::init::InitOverrides {
                sandbox_mode: sandbox_mode
                    .unwrap_or(right_agent::agent::types::SandboxMode::Openshell),
                network_policy: network_policy
                    .unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive),
                telegram_token: None,
                allowed_chat_ids: vec![],
                model: None,
                env: std::collections::HashMap::new(),
                memory_provider: right_agent::agent::types::MemoryProvider::Hindsight,
                memory_api_key: None,
                memory_bank_id: None,
                memory_recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
                memory_recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
                stt,
            }
        } else {
            #[derive(Clone, Copy)]
            enum Step {
                Sandbox,
                Network,
                Telegram,
                ChatIds,
                Stt,
                Memory,
                Done,
            }

            let mut step = if sandbox_mode.is_some() {
                Step::Network
            } else {
                Step::Sandbox
            };
            let mut w_sandbox =
                sandbox_mode.unwrap_or(right_agent::agent::types::SandboxMode::Openshell);
            let mut w_network =
                network_policy.unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
            let mut w_token: Option<String> = None;
            let mut w_chat_ids: Vec<i64> = vec![];
            let mut w_stt: right_agent::agent::types::SttConfig =
                right_agent::agent::types::SttConfig::default();
            let mut w_mem = (
                right_agent::agent::types::MemoryProvider::Hindsight,
                None::<String>,
                None::<String>,
                right_agent::init::DEFAULT_RECALL_BUDGET,
                right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
            );

            loop {
                match step {
                    Step::Sandbox => {
                        if let Some(s) = right_agent::init::prompt_sandbox_mode()? {
                            w_sandbox = s;
                            step = Step::Network;
                        } else {
                            return Err(miette::miette!("Setup cancelled."));
                        }
                    }
                    Step::Network => {
                        if matches!(w_sandbox, right_agent::agent::types::SandboxMode::Openshell) {
                            if let Some(p) = network_policy {
                                w_network = p;
                                step = Step::Telegram;
                            } else if let Some(p) = right_agent::init::prompt_network_policy()? {
                                w_network = p;
                                step = Step::Telegram;
                            } else {
                                step = Step::Sandbox;
                            }
                        } else {
                            w_network = network_policy
                                .unwrap_or(right_agent::agent::types::NetworkPolicy::Permissive);
                            step = Step::Telegram;
                        }
                    }
                    Step::Telegram => {
                        use crate::wizard::TelegramSetupOutcome;
                        match crate::wizard::telegram_setup(None, true, true)? {
                            TelegramSetupOutcome::Token(t) => {
                                w_token = Some(t);
                                step = Step::ChatIds;
                            }
                            TelegramSetupOutcome::Skipped => {
                                unreachable!("required=true, telegram_setup never skips")
                            }
                            TelegramSetupOutcome::Back => {
                                step = Step::Network;
                            }
                        }
                    }
                    Step::ChatIds => match crate::wizard::chat_ids_setup(true)? {
                        Some(ids) => {
                            w_chat_ids = ids;
                            step = Step::Stt;
                        }
                        None => {
                            step = Step::Telegram;
                        }
                    },
                    Step::Stt => match crate::wizard::stt_setup() {
                        Ok(Some((enabled, model))) => {
                            w_stt = right_agent::agent::types::SttConfig { enabled, model };
                            step = Step::Memory;
                        }
                        Ok(None) => {
                            step = if w_token.is_some() {
                                Step::ChatIds
                            } else {
                                Step::Telegram
                            };
                        }
                        Err(e) => return Err(e),
                    },
                    Step::Memory => match right_agent::init::prompt_memory_config(name)? {
                        Some((p, k, b, rb, rt)) => {
                            w_mem = (p, k, b, rb, rt);
                            step = Step::Done;
                        }
                        None => {
                            step = Step::Stt;
                        }
                    },
                    Step::Done => break,
                }
            }

            right_agent::init::InitOverrides {
                sandbox_mode: w_sandbox,
                network_policy: w_network,
                telegram_token: w_token,
                allowed_chat_ids: w_chat_ids,
                model: None,
                env: std::collections::HashMap::new(),
                memory_provider: w_mem.0,
                memory_api_key: w_mem.1,
                memory_bank_id: w_mem.2,
                memory_recall_budget: w_mem.3,
                memory_recall_max_tokens: w_mem.4,
                stt: w_stt,
            }
        }
    };

    let agent_dir = right_agent::init::init_agent(&agents_parent, name, Some(&overrides))?;

    // Run codegen so settings, schemas, skills are generated.
    // Per-agent codegen was moved to bot startup (59243d0) but init/agent-init
    // need it for schemas and settings before sandbox staging upload.
    {
        let self_exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
        let agent_def = right_agent::agent::AgentDef {
            name: name.to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: right_agent::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        right_agent::codegen::run_agent_codegen(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        right_agent::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;
    }

    // Create sandbox for openshell agents.
    if matches!(
        overrides.sandbox_mode,
        right_agent::agent::types::SandboxMode::Openshell
    ) {
        let staging = agent_dir.join("staging");
        right_agent::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        let policy_path = agent_dir.join("policy.yaml");
        // Must match `sandbox.name: right-{agent}` written by init_agent into agent.yaml.
        let sb_name = format!("right-{name}");
        // --force-recreate always recreates; fresh agent (didn't exist before) always creates;
        // otherwise prompt if stale sandbox exists.
        let recreate_sandbox = if force_recreate || !agent_existed {
            // Check if sandbox exists — if so, we need to recreate. If not, false is fine
            // (ensure_sandbox will create fresh).
            let exists = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { check_sandbox_exists_async(&sb_name).await })
            });
            exists.unwrap_or(false)
        } else {
            prompt_sandbox_recreate_if_exists(&sb_name, interactive)?
        };
        println!("Creating OpenShell sandbox...");
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                right_agent::openshell::ensure_sandbox(
                    &sb_name,
                    &policy_path,
                    Some(&staging),
                    recreate_sandbox,
                )
                .await
            })
        })?;

        println!("  Sandbox '{sb_name}' ready");

        // Generate SSH config.
        let run_dir = home.join("run");
        std::fs::create_dir_all(run_dir.join("ssh"))
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(right_agent::openshell::generate_ssh_config(
                    &sb_name,
                    &run_dir.join("ssh"),
                ))
        })?;
    }

    let cfg = right_agent::agent::discovery::parse_agent_config(&agent_dir)?
        .ok_or_else(|| miette::miette!("agent.yaml missing after init"))?;

    let sandbox_str = format!("{}", cfg.sandbox_mode());
    let sandbox_with_policy = if matches!(
        cfg.sandbox_mode(),
        right_agent::agent::types::SandboxMode::Openshell
    ) {
        format!("{} ({})", sandbox_str, cfg.network_policy)
    } else {
        sandbox_str
    };

    let chat_ids_detail = if cfg.allowed_chat_ids.is_empty() {
        "0 allowed (blocks all)".to_string()
    } else {
        format!("{} allowed", cfg.allowed_chat_ids.len())
    };

    let stt_detail = if cfg.stt.enabled {
        cfg.stt.model.yaml_str().to_string()
    } else {
        "off".to_string()
    };

    let memory_detail = match cfg.memory.as_ref().map(|m| &m.provider) {
        Some(right_agent::agent::types::MemoryProvider::Hindsight) => "hindsight",
        _ => "file",
    };

    // If PC is already running, hot-add the new agent's bot via reload.
    // No PC ⇒ pc_running: false ⇒ recap ends with `next: right up`.
    let register_outcome = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            right_agent::agent::register_with_running_pc(
                home,
                right_agent::agent::RegisterOptions {
                    agent_name: name.to_string(),
                    recreated: agent_existed && force_recreate,
                },
            )
            .await
        })
    });

    let mut recap = right_agent::ui::Recap::new("ready")
        .ok("agent", &format!("{name} created"))
        .ok("sandbox", &sandbox_with_policy)
        .ok("telegram", if cfg.telegram_token.is_some() { "configured" } else { "not configured" })
        .ok("chat ids", &chat_ids_detail)
        .ok("stt", &stt_detail)
        .ok("memory", memory_detail);

    recap = match register_outcome {
        Ok(right_agent::agent::RegisterResult { pc_running: false }) => {
            recap.next("right up")
        }
        Ok(right_agent::agent::RegisterResult { pc_running: true }) => {
            recap.next("send /start to your bot in Telegram")
        }
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), "PC reload failed during agent init");
            recap
                .warn("reload", "failed to add to running right")
                .next("right restart")
        }
    };
    println!("{}", recap.render(theme));

    Ok(())
}

/// Check if a sandbox exists via gRPC. Returns Ok(bool).
async fn check_sandbox_exists_async(sandbox_name: &str) -> miette::Result<bool> {
    let mtls_dir = match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return Ok(false), // OpenShell not available — no sandbox
    };
    let mut client = right_agent::openshell::connect_grpc(&mtls_dir).await?;
    right_agent::openshell::is_sandbox_ready(&mut client, sandbox_name).await
}

/// If a sandbox already exists, prompt the user to recreate or abort.
/// Returns `true` if sandbox exists and should be recreated.
/// Returns `false` if sandbox doesn't exist (fresh create).
/// Errors if user declines recreate.
fn prompt_sandbox_recreate_if_exists(
    sandbox_name: &str,
    interactive: bool,
) -> miette::Result<bool> {
    let exists = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(check_sandbox_exists_async(sandbox_name))
    })?;

    if !exists {
        return Ok(false); // No existing sandbox — fresh create
    }

    if !interactive {
        // Non-interactive (-y): refuse to silently destroy a sandbox.
        return Err(miette::miette!(
            help = "Run interactively to confirm, or pass --force-recreate (agent init) / --force (init)",
            "Sandbox '{sandbox_name}' already exists"
        ));
    }

    use std::io::{self, Write};
    println!();
    println!("⚠ Sandbox '{sandbox_name}' already exists.");
    println!("  1. Recreate — delete and create fresh sandbox");
    println!("  2. Cancel — use `right agent config` to update existing agent");
    loop {
        print!("Choose [1/2]: ");
        io::stdout()
            .flush()
            .map_err(|e| miette::miette!("stdout flush: {e}"))?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| miette::miette!("failed to read input: {e}"))?;
        match input.trim() {
            "1" => return Ok(true),
            "2" => return Err(miette::miette!("Sandbox creation cancelled")),
            _ => continue,
        }
    }
}

fn cmd_doctor(home: &Path) -> miette::Result<()> {
    let theme = right_agent::ui::detect();
    let checks = right_agent::doctor::run_doctor(home);

    println!("{}", right_agent::ui::section(theme, "diagnostics"));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let mut block = right_agent::ui::Block::new();
    for check in &checks {
        block.push(check.to_ui_line());
    }
    println!("{}", block.render(theme));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let pass = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Pass))
        .count();
    let warn = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Warn))
        .count();
    let fail = checks
        .iter()
        .filter(|c| matches!(c.status, right_agent::doctor::CheckStatus::Fail))
        .count();
    let total = checks.len();

    let summary = if warn == 0 && fail == 0 {
        format!("{pass}/{total} checks passed")
    } else {
        let mut parts = Vec::new();
        if warn > 0 {
            parts.push(format!("{warn} warn"));
        }
        if fail > 0 {
            parts.push(format!("{fail} fail"));
        }
        format!("{pass}/{total} checks passed ({})", parts.join(", "))
    };
    println!(
        "{}{}",
        right_agent::ui::Rail::prefix(theme),
        summary
    );

    if fail > 0 {
        return Err(miette::miette!("checks failed — see above for fixes"));
    }
    Ok(())
}

fn cmd_list(home: &Path) -> miette::Result<()> {
    let agents_dir = right_agent::config::agents_dir(home);
    if !agents_dir.exists() {
        println!("No agents directory found. Run `right init` first.");
        return Ok(());
    }

    let agents = right_agent::agent::discover_agents(&agents_dir)?;
    if agents.is_empty() {
        println!("No agents found in {}", agents_dir.display());
    } else {
        println!("Discovered {} agent(s):", agents.len());
        for agent in &agents {
            let config_status = if agent.config.is_some() { "yes" } else { "no" };
            let mcp_status = if agent.path.join("mcp.json").exists() {
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
    debug: bool,
) -> miette::Result<()> {
    let t_total = std::time::Instant::now();
    let mut t_phase = std::time::Instant::now();

    // Fail fast if required tools are missing.
    right_agent::runtime::verify_dependencies()?;
    tracing::info!(elapsed_ms = t_phase.elapsed().as_millis() as u64, "up: verify_dependencies");
    t_phase = std::time::Instant::now();

    let run_dir = home.join("run");

    // Pre-flight: check for stale processes holding required ports.
    // `from_home` returns None with an isolated --home tempdir, skipping the
    // probe. `check_port_available` below still catches a PC started by
    // another home that happens to bind the port we need.
    if let Some(client) = right_agent::runtime::PcClient::from_home(home)?
        && client.health_check().await.is_ok()
    {
        return Err(miette::miette!(
            "right is already running. Use `right down` first or `right attach` to connect."
        ));
    }
    check_port_available(right_agent::runtime::MCP_HTTP_PORT).await?;
    tracing::info!(elapsed_ms = t_phase.elapsed().as_millis() as u64, "up: pc_health + port_check");
    t_phase = std::time::Instant::now();

    // Discover agents.
    let agents_dir = right_agent::config::agents_dir(home);
    let all_agents = right_agent::agent::discover_agents(&agents_dir)?;

    let agents = filter_agents(&all_agents, agents_filter.as_deref())?;

    if agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `right agent init <name>` to create one."
        ));
    }
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        agents = agents.len(),
        "up: discover_agents"
    );
    t_phase = std::time::Instant::now();

    // Pre-flight: when any agent needs sandbox, verify OpenShell is ready.
    // The bot process needs mTLS certs to connect to the gateway's gRPC API —
    // without them it will crash in a loop. Diagnose the specific issue and
    // offer to fix it interactively.
    let any_sandboxed = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| {
                matches!(
                    c.sandbox_mode(),
                    right_agent::agent::types::SandboxMode::Openshell
                )
            })
            .unwrap_or(true) // default is openshell
    });
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        any_sandboxed,
        "up: sandbox_mode_scan"
    );
    t_phase = std::time::Instant::now();

    if any_sandboxed {
        match right_agent::openshell::preflight_check() {
            right_agent::openshell::OpenShellStatus::Ready(_) => {}
            right_agent::openshell::OpenShellStatus::NotInstalled => {
                println!("OpenShell is not installed. Sandbox mode requires OpenShell.");
                println!();
                let install = inquire::Confirm::new("install openshell now?")
                    .with_default(true)
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                if install {
                    println!("Installing OpenShell...");
                    let status = std::process::Command::new("sh")
                        .args(["-c", "curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh"])
                        .status()
                        .map_err(|e| miette::miette!("failed to run installer: {e:#}"))?;
                    if !status.success() {
                        return Err(miette::miette!(
                            help = "Install manually: https://github.com/NVIDIA/OpenShell",
                            "OpenShell installer failed"
                        ));
                    }
                    // After install, still need a gateway — fall through to gateway check.
                    println!();
                    match right_agent::openshell::preflight_check() {
                        right_agent::openshell::OpenShellStatus::Ready(_) => {}
                        right_agent::openshell::OpenShellStatus::NoGateway(_) => {
                            start_openshell_gateway()?;
                        }
                        other => return Err(openshell_status_error(other)),
                    }
                } else {
                    return Err(miette::miette!(
                        help = "Install from https://github.com/NVIDIA/OpenShell, or set `sandbox: mode: none` in agent.yaml",
                        "OpenShell is required for sandbox mode"
                    ));
                }
            }
            right_agent::openshell::OpenShellStatus::NoGateway(_) => {
                start_openshell_gateway()?;
            }
            status @ right_agent::openshell::OpenShellStatus::BrokenGateway(_) => {
                return Err(openshell_status_error(status));
            }
        }
    }
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: openshell_preflight"
    );
    t_phase = std::time::Instant::now();

    // Download any whisper models needed by STT-enabled agents.
    {
        use right_agent::agent::types::WhisperModel;
        use std::collections::HashSet;

        let mut models: HashSet<WhisperModel> = HashSet::new();
        for agent in &agents {
            if let Some(cfg) = agent.config.as_ref()
                && cfg.stt.enabled
            {
                models.insert(cfg.stt.model);
            }
        }
        if !models.is_empty() {
            println!("Ensuring whisper models are cached...");
            if let Err(e) = right_agent::stt::ensure_models_cached(home, &models).await {
                eprintln!("warning: model cache step failed: {e:#}");
            }
        }
    }
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: whisper_models_cache"
    );
    t_phase = std::time::Instant::now();

    // Clear rightcron init locks so the bootstrap hook fires on this session.
    for agent in &agents {
        let lock = agent.path.join(".rightcron-init-done");
        let _ = std::fs::remove_file(&lock);
    }

    // Resolve current executable path once — written into each agent's mcp.json so the
    // right MCP server can be found even when right is not on PATH (process-compose).
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    // Run cross-agent codegen: token map, policy validation,
    // cloudflared config, process-compose.yaml, and runtime state.
    right_agent::codegen::run_agent_codegen(home, &agents, &self_exe, debug)?;
    tracing::info!(
        elapsed_ms = t_phase.elapsed().as_millis() as u64,
        "up: cross_agent_codegen"
    );

    // Check that at least one agent has a Telegram token configured.
    let has_bot_agents = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false)
    });
    if !has_bot_agents {
        eprintln!("right: no agents have Telegram tokens configured — nothing to start");
        return Err(miette::miette!("no agents have Telegram tokens configured"));
    }

    // Build process-compose command.
    let config_path = run_dir.join("process-compose.yaml");
    let mut cmd = tokio::process::Command::new("process-compose");
    // Use TCP API (avoids --use-uds which crashes TUI).
    let pc_port = right_agent::runtime::PC_PORT.to_string();
    cmd.args([
        "up",
        "-f",
        config_path.to_str().unwrap_or_default(),
        "--port",
        &pc_port,
    ]);

    // Read the API token from state.json (just written by codegen) and inject
    // as PC_API_TOKEN env var. process-compose then rejects any unauthenticated
    // REST API request — prevents stray HTTP callers from stopping production bots.
    let state_path = run_dir.join("state.json");
    if let Ok(state) = right_agent::runtime::read_state(&state_path)
        && let Some(token) = &state.pc_api_token
    {
        cmd.env("PC_API_TOKEN", token);
    }

    tracing::info!(
        total_pre_pc_ms = t_total.elapsed().as_millis() as u64,
        detach,
        "up: spawning process-compose"
    );

    if detach {
        cmd.arg("--detached");
        let child = cmd
            .spawn()
            .map_err(|e| miette::miette!("failed to spawn process-compose: {e:#}"))?;

        // Wait briefly for process-compose to start.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify it's alive. `run_agent_codegen` above wrote state.json, so
        // `from_home` resolves to Some(_); missing state here would be a bug.
        let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
            miette::miette!("runtime state missing after codegen — refusing to health-check")
        })?;
        client.health_check().await.map_err(|e| {
            miette::miette!("process-compose started but health check failed: {e:#}")
        })?;

        println!(
            "right started in background ({} agent(s)). Use `right attach` to view TUI.",
            agents.len()
        );

        // Drop child handle without killing -- it's detached.
        drop(child);
    } else {
        let status = cmd
            .status()
            .await
            .map_err(|e| miette::miette!("failed to run process-compose: {e:#}"))?;

        if !status.success() {
            return Err(miette::miette!(
                "process-compose exited with status: {}",
                status
            ));
        }
    }

    Ok(())
}

/// Prompt the user to start an OpenShell gateway, then verify it came up.
fn start_openshell_gateway() -> miette::Result<()> {
    println!("OpenShell gateway is not running. Sandbox mode requires a running gateway.");
    println!("Note: OpenShell requires Docker to be installed and running.");
    println!();
    let start = inquire::Confirm::new("start openshell gateway now?")
        .with_default(true)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
    if !start {
        return Err(miette::miette!(
            help = "Run `openshell gateway start` manually, or set `sandbox: mode: none` in agent.yaml",
            "OpenShell gateway is required for sandbox mode"
        ));
    }
    println!("Starting OpenShell gateway (this may take a minute on first run)...");
    let status = std::process::Command::new("openshell")
        .args(["gateway", "start"])
        .status()
        .map_err(|e| miette::miette!("failed to run `openshell gateway start`: {e:#}"))?;
    if !status.success() {
        return Err(miette::miette!(
            help = "Check `openshell doctor check` for prerequisites (Docker must be running)",
            "`openshell gateway start` failed"
        ));
    }
    // Verify certs are now present.
    match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(_) => {
            println!("OpenShell gateway started successfully.");
            Ok(())
        }
        status => Err(openshell_status_error(status)),
    }
}

/// Convert an `OpenShellStatus` into a user-facing miette error.
fn openshell_status_error(status: right_agent::openshell::OpenShellStatus) -> miette::Report {
    match status {
        right_agent::openshell::OpenShellStatus::Ready(_) => unreachable!(),
        right_agent::openshell::OpenShellStatus::NotInstalled => miette::miette!(
            help = "Install from https://github.com/NVIDIA/OpenShell, or set `sandbox: mode: none` in agent.yaml",
            "OpenShell is not installed"
        ),
        right_agent::openshell::OpenShellStatus::NoGateway(_) => miette::miette!(
            help = "Run `openshell gateway start`, or set `sandbox: mode: none` in agent.yaml",
            "OpenShell gateway is not running"
        ),
        right_agent::openshell::OpenShellStatus::BrokenGateway(mtls_dir) => miette::miette!(
            help = "Try `openshell gateway destroy && openshell gateway start` to recreate,\n  \
                    or set `sandbox: mode: none` in agent.yaml",
            "OpenShell gateway exists but mTLS certificates are missing at {}\n\n  \
             The gateway may be in a broken state.",
            mtls_dir.display()
        ),
    }
}

/// Fail fast if a required port is already occupied by a stale process.
async fn check_port_available(port: u16) -> miette::Result<()> {
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(_listener) => Ok(()), // bound successfully → port is free
        Err(_) => Err(miette::miette!(
            help = "A previous right session may still be running. Kill it first:\n  \
                    killall right  # or: right down",
            "port {port} is already in use"
        )),
    }
}

async fn cmd_down(home: &Path) -> miette::Result<()> {
    let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
        miette::miette!(
            help = "Start right first with `right up`",
            "No running instance found. Is right running?"
        )
    })?;

    client
        .health_check()
        .await
        .map_err(|_| miette::miette!("No running instance found. Is right running?"))?;

    client.shutdown().await.map_err(|e| {
        miette::miette!("Shutdown request failed (process-compose may already be stopped): {e:#}")
    })?;

    println!("All agents stopped.");
    Ok(())
}

async fn cmd_reload(home: &Path, _agents_filter: Option<Vec<String>>) -> miette::Result<()> {
    let client = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
        miette::miette!(
            help = "Start right first with `right up`",
            "nothing running — cannot reload"
        )
    })?;
    client.health_check().await.map_err(|_| {
        miette::miette!(
            help = "Start right first with `right up`",
            "nothing running — cannot reload"
        )
    })?;

    let agents_dir = right_agent::config::agents_dir(home);
    let all_agents = right_agent::agent::discover_agents(&agents_dir)?;

    if all_agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `right agent init <name>` to create one."
        ));
    }

    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    right_agent::codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

    client.reload_configuration().await?;

    // Notify aggregator to pick up new agents from updated token map
    let socket_path = home.join("run/internal.sock");
    let internal = right_agent::mcp::internal_client::InternalClient::new(&socket_path);
    match internal.reload().await {
        Ok(resp) => {
            if !resp.added.is_empty() {
                println!(
                    "Registered {} new agent(s) in aggregator: {}",
                    resp.added.len(),
                    resp.added.join(", "),
                );
            }
            if !resp.removed.is_empty() {
                println!(
                    "Removed {} agent(s) from aggregator: {}",
                    resp.removed.len(),
                    resp.removed.join(", "),
                );
            }
        }
        Err(e) => {
            eprintln!("warning: failed to reload aggregator: {e:#}");
        }
    }

    let has_bot = all_agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false)
    });
    if !has_bot {
        eprintln!("right: warning: no agents have Telegram tokens — nothing will run");
    }

    println!("Reloaded. Active agents:");
    for agent in &all_agents {
        let has_token = agent
            .config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false);
        let status = if has_token {
            "bot"
        } else {
            "no token (skipped)"
        };
        println!("  {:<20} {}", agent.name, status);
    }

    Ok(())
}

async fn cmd_status(home: &Path) -> miette::Result<()> {
    let theme = right_agent::ui::detect();

    println!("{}", right_agent::ui::section(theme, "status"));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let Some(client) = right_agent::runtime::PcClient::from_home(home)? else {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    };

    if client.health_check().await.is_err() {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("not running")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    }

    let processes = client.list_processes().await?;

    if processes.is_empty() {
        let line = right_agent::ui::status(right_agent::ui::Glyph::Err)
            .noun("right agent")
            .verb("no processes")
            .fix("right up")
            .render(theme);
        println!("{line}");
        return Err(right_agent::ui::BlockAlreadyRendered.into());
    }

    let mut block = right_agent::ui::Block::new();
    for p in &processes {
        let glyph = match p.status.as_str() {
            "Running" => right_agent::ui::Glyph::Ok,
            "Restarting" | "Pending" => right_agent::ui::Glyph::Warn,
            _ => right_agent::ui::Glyph::Err,
        };
        let verb = format!("{:<6} {}", p.pid, p.system_time);
        block.push(
            right_agent::ui::status(glyph)
                .noun(&p.name)
                .verb(verb),
        );
    }
    println!("{}", block.render(theme));
    println!("{}", right_agent::ui::Rail::blank(theme));

    let warn = processes
        .iter()
        .filter(|p| matches!(p.status.as_str(), "Restarting" | "Pending"))
        .count();
    let fail = processes
        .iter()
        .filter(|p| !matches!(p.status.as_str(), "Running" | "Restarting" | "Pending"))
        .count();
    let total = processes.len();
    let summary = if warn == 0 && fail == 0 {
        format!("{total} processes")
    } else {
        let mut parts = Vec::new();
        if warn > 0 {
            parts.push(format!("{warn} warn"));
        }
        if fail > 0 {
            parts.push(format!("{fail} fail"));
        }
        format!("{total} processes ({})", parts.join(", "))
    };
    println!("{}{}", right_agent::ui::Rail::prefix(theme), summary);

    Ok(())
}

async fn cmd_restart(_home: &Path, _agent: &str) -> miette::Result<()> {
    // process-compose crashes on programmatic restart (both REST API and CLI client).
    // This is a known process-compose bug. Direct users to the TUI instead.
    Err(miette::miette!(
        help = "Use the process-compose TUI: select the agent and press Ctrl+R to restart",
        "Programmatic restart is not supported (process-compose bug). Use `right attach` and Ctrl+R instead."
    ))
}

fn cmd_attach(home: &Path) -> miette::Result<()> {
    use std::os::unix::process::CommandExt;

    // Read the recorded PC port for this home. With an isolated --home and no
    // prior `right up`, there is nothing to attach to — fail loudly.
    let state_path = home.join("run").join("state.json");
    let state = right_agent::runtime::read_state(&state_path).map_err(|e| {
        miette::miette!(
            help = "Start right first with `right up`",
            "No running instance recorded at {} ({e:#})",
            state_path.display(),
        )
    })?;

    let err = std::process::Command::new("process-compose")
        .arg("attach")
        .arg("--port")
        .arg(state.pc_port.to_string())
        .exec();

    Err(miette::miette!("Failed to attach: {err}"))
}

async fn cmd_agent_restore(
    home: &Path,
    agent_name: &str,
    backup_path: &Path,
) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    // 1. Validate preconditions.
    let agents_dir = right_agent::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);

    if agent_dir.exists() {
        return Err(miette::miette!(
            help = "Remove the existing agent first, or choose a different name",
            "Agent '{}' already exists at {}",
            agent_name,
            agent_dir.display()
        ));
    }

    let tar_path = backup_path.join("sandbox.tar.gz");
    if !tar_path.exists() {
        return Err(miette::miette!(
            "sandbox.tar.gz not found in backup directory {}",
            backup_path.display()
        ));
    }

    let agent_yaml_src = backup_path.join("agent.yaml");
    if !agent_yaml_src.exists() {
        return Err(miette::miette!(
            help = "Full backups (not --sandbox-only) include agent.yaml",
            "agent.yaml not found in backup directory {}",
            backup_path.display()
        ));
    }

    // 2. Create agent dir and restore config files.
    std::fs::create_dir_all(&agent_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!("failed to create agent dir {}: {e:#}", agent_dir.display())
        })?;

    for filename in &["agent.yaml", "policy.yaml", "data.db"] {
        let src = backup_path.join(filename);
        if src.exists() {
            let dest = agent_dir.join(filename);
            std::fs::copy(&src, &dest)
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to copy {filename}: {e:#}"))?;
            println!("{filename} restored");
        }
    }

    // 3. Parse restored config to determine sandbox mode.
    let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

    if is_sandboxed {
        // 4. Sandboxed restore: create new sandbox, upload tar contents.
        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let new_sandbox_name = format!("right-{agent_name}-{timestamp}");

        // We need codegen for staging dir. Create a minimal IDENTITY.md placeholder
        // so discover_single_agent succeeds (the real one is inside the tar).
        let identity_path = agent_dir.join("IDENTITY.md");
        if !identity_path.exists() {
            std::fs::write(&identity_path, "# Placeholder (restoring from backup)\n")
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to write placeholder IDENTITY.md: {e:#}"))?;
        }

        let agent_def = right_agent::agent::discover_single_agent(&agent_dir)?;
        let self_exe = std::env::current_exe()
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to resolve self exe: {e:#}"))?;

        right_agent::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;

        // Prepare staging dir.
        let staging = agent_dir.join("staging");
        right_agent::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        // Resolve policy path.
        let policy_path = config
            .as_ref()
            .and_then(|c| c.sandbox.as_ref())
            .and_then(|s| s.policy_file.as_ref())
            .map(|p| agent_dir.join(p))
            .unwrap_or_else(|| agent_dir.join("policy.yaml"));

        if !policy_path.exists() {
            return Err(miette::miette!(
                "policy file not found at {} — cannot create sandbox",
                policy_path.display()
            ));
        }

        // Verify OpenShell is reachable.
        let mtls_dir = match right_agent::openshell::preflight_check() {
            right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
            right_agent::openshell::OpenShellStatus::NotInstalled => {
                return Err(miette::miette!(
                    "openshell not installed — required for sandboxed agent restore"
                ));
            }
            right_agent::openshell::OpenShellStatus::NoGateway(_) => {
                return Err(miette::miette!(
                    "openshell gateway not started — start it before restoring"
                ));
            }
            right_agent::openshell::OpenShellStatus::BrokenGateway(_) => {
                return Err(miette::miette!(
                    "openshell mTLS certs missing or corrupt — try reinstalling openshell"
                ));
            }
        };

        // Spawn sandbox.
        println!("Creating sandbox '{new_sandbox_name}'...");
        let mut child =
            right_agent::openshell::spawn_sandbox(&new_sandbox_name, &policy_path, Some(&staging))?;

        let mut grpc = right_agent::openshell::connect_grpc(&mtls_dir).await?;

        // Wait for READY (race with child exit).
        tokio::select! {
            result = right_agent::openshell::wait_for_ready(&mut grpc, &new_sandbox_name, 120, 2) => {
                result?;
                drop(child);
            }
            status = child.wait() => {
                let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
                if !status.success() {
                    return Err(miette::miette!(
                        "openshell sandbox create for '{}' exited with {status} before reaching READY",
                        new_sandbox_name
                    ));
                }
            }
        }

        // Wait for SSH transport.
        let sandbox_id =
            right_agent::openshell::resolve_sandbox_id(&mut grpc, &new_sandbox_name).await?;
        right_agent::openshell::wait_for_ssh(&mut grpc, &sandbox_id, 60, 2).await?;

        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let ssh_config_path =
            right_agent::openshell::generate_ssh_config(&new_sandbox_name, &ssh_config_dir).await?;

        let ssh_host = right_agent::openshell::ssh_host_for_sandbox(&new_sandbox_name);

        // Upload backup tar.
        println!("Uploading sandbox backup...");
        right_agent::openshell::ssh_tar_upload(&ssh_config_path, &ssh_host, &tar_path, 600).await?;
        println!("Sandbox files restored");

        // Write sandbox.name into agent.yaml.
        crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox_name)?;
        println!("sandbox.name set to '{new_sandbox_name}' in agent.yaml");

        // Clean up staging dir and placeholder.
        let _ = std::fs::remove_dir_all(&staging);
    } else {
        // 5. No-sandbox restore: unpack tar directly.
        // The tar was created with `-C <agents_parent> <agent_name>`, so we
        // strip the top-level directory to restore into potentially different name.
        println!("Extracting sandbox.tar.gz...");
        let status = std::process::Command::new("tar")
            .args([
                "xzpf",
                tar_path
                    .to_str()
                    .ok_or_else(|| miette::miette!("non-UTF-8 tar path"))?,
                "--strip-components=1",
                "-C",
                agent_dir
                    .to_str()
                    .ok_or_else(|| miette::miette!("non-UTF-8 agent dir"))?,
            ])
            .status()
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!(
                "tar extraction failed with status {status}"
            ));
        }
        println!("Agent files restored");
    }

    println!(
        "Restore complete: agent '{}' at {}",
        agent_name,
        agent_dir.display()
    );
    Ok(())
}

async fn cmd_agent_backup(home: &Path, agent_name: &str, sandbox_only: bool) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    // 1. Discover agent and parse config
    let agents_dir = right_agent::config::agents_dir(home);
    let agents = right_agent::agent::discover_agents(&agents_dir)?;
    let _agent = agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!(
                "Agent '{}' not found. Available: {}",
                agent_name,
                available.join(", ")
            )
        })?;

    let agent_dir = agents_dir.join(agent_name);
    let config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;

    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

    // 2. Create backup directory: ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_base = right_agent::config::backups_dir(home, agent_name);
    let backup_dir = backup_base.join(&timestamp);
    std::fs::create_dir_all(&backup_dir)
        .into_diagnostic()
        .map_err(|e| {
            miette::miette!(
                "failed to create backup dir {}: {e:#}",
                backup_dir.display()
            )
        })?;

    tracing::info!(agent = agent_name, backup_dir = %backup_dir.display(), "starting backup");

    // 3. Sandbox tar download (if sandboxed)
    if is_sandboxed {
        let sb_name = config
            .as_ref()
            .map(|c| right_agent::openshell::resolve_sandbox_name(agent_name, c))
            .unwrap_or_else(|| right_agent::openshell::sandbox_name(agent_name));

        // Verify OpenShell is reachable
        let mtls_dir = match right_agent::openshell::preflight_check() {
            right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
            right_agent::openshell::OpenShellStatus::NotInstalled => {
                return Err(miette::miette!(
                    "openshell not installed — required for sandboxed agent backup"
                ));
            }
            right_agent::openshell::OpenShellStatus::NoGateway(_) => {
                return Err(miette::miette!(
                    "openshell gateway not started — start it before backing up"
                ));
            }
            right_agent::openshell::OpenShellStatus::BrokenGateway(_) => {
                return Err(miette::miette!(
                    "openshell mTLS certs missing or corrupt — try reinstalling openshell"
                ));
            }
        };

        let mut grpc = right_agent::openshell::connect_grpc(&mtls_dir).await?;

        let ready = right_agent::openshell::is_sandbox_ready(&mut grpc, &sb_name).await?;
        if !ready {
            return Err(miette::miette!(
                help = "Start the agent with: right up",
                "Sandbox '{}' is not ready — agent must be running to back up sandbox files",
                sb_name,
            ));
        }

        let ssh_config = home
            .join("run")
            .join("ssh")
            .join(format!("{}.ssh-config", sb_name));
        if !ssh_config.exists() {
            return Err(miette::miette!(
                help = "Try restarting the agent",
                "SSH config not found at {}",
                ssh_config.display(),
            ));
        }

        let ssh_host = right_agent::openshell::ssh_host_for_sandbox(&sb_name);
        let dest_tar = backup_dir.join("sandbox.tar.gz");

        tracing::info!(sandbox = %sb_name, dest = %dest_tar.display(), "downloading sandbox via SSH tar");
        right_agent::openshell::ssh_tar_download(&ssh_config, &ssh_host, "sandbox", &dest_tar, 300)
            .await?;
        println!(
            "sandbox.tar.gz written ({} bytes)",
            std::fs::metadata(&dest_tar).map(|m| m.len()).unwrap_or(0)
        );
    } else {
        // No-sandbox: tar the agent dir (excluding data.db — backed up separately via VACUUM)
        let dest_tar = backup_dir.join("sandbox.tar.gz");
        tracing::info!(agent_dir = %agent_dir.display(), dest = %dest_tar.display(), "archiving agent directory");
        let status = std::process::Command::new("tar")
            .args([
                "czpf",
                dest_tar
                    .to_str()
                    .ok_or_else(|| miette::miette!("non-UTF-8 backup path"))?,
                "--exclude=data.db",
                "-C",
                agent_dir
                    .parent()
                    .ok_or_else(|| miette::miette!("agent_dir has no parent"))?
                    .to_str()
                    .ok_or_else(|| miette::miette!("non-UTF-8 agents_dir"))?,
                agent_name,
            ])
            .status()
            .into_diagnostic()
            .map_err(|e| miette::miette!("failed to spawn tar: {e:#}"))?;
        if !status.success() {
            return Err(miette::miette!("tar exited with status {status}"));
        }
        println!(
            "sandbox.tar.gz written ({} bytes)",
            std::fs::metadata(&dest_tar).map(|m| m.len()).unwrap_or(0)
        );
    }

    // 4. Config files (unless --sandbox-only)
    if !sandbox_only {
        for filename in &["agent.yaml", "policy.yaml"] {
            let src = agent_dir.join(filename);
            if src.exists() {
                let dest = backup_dir.join(filename);
                std::fs::copy(&src, &dest)
                    .into_diagnostic()
                    .map_err(|e| miette::miette!("failed to copy {filename}: {e:#}"))?;
                println!("{filename} copied");
            }
        }

        let db_path = agent_dir.join("data.db");
        if db_path.exists() {
            let backup_db = backup_dir.join("data.db");
            let conn = rusqlite::Connection::open(&db_path)
                .into_diagnostic()
                .map_err(|e| miette::miette!("failed to open data.db: {e:#}"))?;
            conn.execute(
                &format!(
                    "VACUUM INTO '{}'",
                    backup_db.display().to_string().replace('\'', "''")
                ),
                [],
            )
            .into_diagnostic()
            .map_err(|e| miette::miette!("VACUUM INTO failed: {e:#}"))?;
            println!(
                "data.db vacuumed ({} bytes)",
                std::fs::metadata(&backup_db).map(|m| m.len()).unwrap_or(0)
            );
        }
    }

    println!("Backup complete: {}", backup_dir.display());
    Ok(())
}

async fn cmd_agent_destroy(
    home: &Path,
    agent_name: &str,
    backup_flag: bool,
    force: bool,
) -> miette::Result<()> {
    use inquire::ui::{Color, RenderConfig, Styled};

    // Validate agent exists
    let agents_dir = right_agent::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.exists() {
        return Err(miette::miette!("Agent '{}' not found", agent_name));
    }

    let config = right_agent::agent::parse_agent_config(&agent_dir)?;
    let is_sandboxed = config.as_ref().map(|c| c.is_sandboxed()).unwrap_or(true);

    let do_backup = if force {
        backup_flag
    } else {
        // Show summary of what will be destroyed
        println!("Agent: {agent_name}");
        println!("  Directory: {}", agent_dir.display());
        if let Ok(size) = dir_size(&agent_dir) {
            println!("  Size: {}", format_bytes(size));
        }
        if is_sandboxed {
            let sb_name = config
                .as_ref()
                .map(|c| right_agent::openshell::resolve_sandbox_name(agent_name, c))
                .unwrap_or_else(|| right_agent::openshell::sandbox_name(agent_name));
            println!("  Sandbox: {sb_name}");
        } else {
            println!("  Sandbox: none");
        }
        let db_path = agent_dir.join("data.db");
        if db_path.exists()
            && let Ok(meta) = std::fs::metadata(&db_path)
        {
            println!("  data.db: {}", format_bytes(meta.len()));
        }

        // Check if PC is running and agent is active. `from_home` returns
        // None when this --home has no recorded runtime state, in which case
        // there is no PC to contact. See ARCHITECTURE.md "Runtime isolation".
        let pc_running = match right_agent::runtime::PcClient::from_home(home)? {
            Some(pc_client) => pc_client.health_check().await.is_ok(),
            None => false,
        };
        if pc_running {
            println!("  Process: running (will be stopped)");
        } else {
            println!("  Process: not running");
        }

        println!();

        // Backup prompt
        let do_backup = if backup_flag {
            true
        } else {
            inquire::Confirm::new("create backup before destroying?")
                .with_default(false)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?
        };

        // Final confirmation — red styled
        let red_config =
            RenderConfig::default().with_prompt_prefix(Styled::new("⚠").with_fg(Color::LightRed));

        let confirmed = inquire::Confirm::new(&format!(
            "permanently destroy agent '{agent_name}'? this cannot be undone."
        ))
        .with_default(false)
        .with_render_config(red_config)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }

        do_backup
    };

    let options = right_agent::agent::DestroyOptions {
        agent_name: agent_name.to_string(),
        backup: do_backup,
    };

    let result = right_agent::agent::destroy_agent(home, &options).await?;

    // Print summary
    println!();
    println!("Destroyed agent '{agent_name}':");
    if result.agent_stopped {
        println!("  ✓ Stopped process");
    }
    if let Some(ref path) = result.backup_path {
        println!("  ✓ Backup saved to {}", path.display());
    }
    if result.sandbox_deleted {
        println!("  ✓ Deleted sandbox");
    }
    if result.dir_removed {
        println!("  ✓ Removed agent directory");
    }
    if result.pc_reloaded {
        println!("  ✓ Reloaded process-compose");
    }

    Ok(())
}

async fn cmd_agent_rebootstrap(
    home: &Path,
    agent_name: &str,
    yes: bool,
) -> miette::Result<()> {
    use right_agent::ui::{Block, Glyph, Rail, detect, section, status};

    let plan = right_agent::rebootstrap::plan(home, agent_name)?;
    let theme = detect();
    let pc_process = format!("{agent_name}-bot");

    if !yes {
        println!(
            "{}",
            section(theme, &format!("rebootstrap: {agent_name}"))
        );
        println!("{}", Rail::blank(theme));

        let sandbox_detail = match &plan.sandbox_name {
            Some(name) => format!("{} ({name})", plan.sandbox_mode),
            None => plan.sandbox_mode.to_string(),
        };
        let mut plan_block = Block::new();
        plan_block.push(
            status(Glyph::Info)
                .noun("directory")
                .verb(plan.agent_dir.display().to_string()),
        );
        plan_block.push(status(Glyph::Info).noun("sandbox").verb(sandbox_detail));
        plan_block.push(
            status(Glyph::Info)
                .noun("backup")
                .verb(plan.backup_dir.display().to_string()),
        );
        println!("{}", plan_block.render(theme));
        println!("{}", Rail::blank(theme));

        println!("{}", section(theme, "effects"));
        println!("{}", Rail::blank(theme));
        let mut effects = Block::new();
        effects.push(
            status(Glyph::Info)
                .noun("back up")
                .verb("IDENTITY.md, SOUL.md, USER.md (host + sandbox)"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("remove")
                .verb("same files from host and sandbox"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("recreate")
                .verb("BOOTSTRAP.md on host"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("deactivate")
                .verb("active sessions in data.db"),
        );
        effects.push(
            status(Glyph::Info)
                .noun("bounce")
                .verb(format!("{pc_process} if running")),
        );
        println!("{}", effects.render(theme));
        println!("{}", Rail::blank(theme));
        println!(
            "{}preserved: sandbox, credentials, hindsight memory, data.db rows",
            Rail::prefix(theme)
        );
        println!("{}", Rail::blank(theme));

        let confirmed = inquire::Confirm::new(&format!(
            "rebootstrap '{agent_name}'? this rewinds onboarding state"
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if !confirmed {
            println!(
                "{}",
                status(Glyph::Warn)
                    .noun("aborted")
                    .verb("no changes made")
                    .render(theme)
            );
            return Ok(());
        }
    }

    // Three states, NOT two — we must not silently skip the bot bounce when
    // state.json is present but PC's API is unreachable. That's how the
    // 2026-04-29 incident ("rebootstrap ran but my bot kept serving the old
    // persona") happened: the previous code treated 401-on-/live as
    // equivalent to "PC not running", continued with the file-side rewind,
    // and left the still-running bot serving stale identity.
    //
    //   None              — no state.json: PC was never started from this
    //                       home. No live bot to bounce; file ops are safe.
    //   Some(Some(pc))    — state.json + healthy PC. Stop now, restart later.
    //   error             — state.json present but PC unreachable. We REFUSE
    //                       to do file ops because the bot would keep serving
    //                       the old persona.
    let stopped_pc = match right_agent::runtime::PcClient::from_home(home)? {
        None => {
            println!(
                "{}",
                status(Glyph::Info)
                    .noun(pc_process.as_str())
                    .verb("not running, skipping bot stop")
                    .render(theme)
            );
            None
        }
        Some(pc) => {
            pc.health_check().await.map_err(|e| {
                miette::miette!(
                    "process-compose API unreachable: {e:#}\n\
                     Refusing to rebootstrap: cannot bounce {pc_process}, and proceeding \
                     would leave the running bot serving the old identity.\n\
                     Verify `right up` is healthy (or stop it cleanly) and retry."
                )
            })?;
            pc.stop_process(&pc_process).await.map_err(|e| {
                miette::miette!(
                    "failed to stop {pc_process} (not safe to proceed with bot up): {e:#}"
                )
            })?;
            println!(
                "{}",
                status(Glyph::Ok)
                    .noun(pc_process.as_str())
                    .verb("stopped")
                    .render(theme)
            );
            Some(pc)
        }
    };

    let report = right_agent::rebootstrap::execute(&plan).await?;

    if let Some(pc) = &stopped_pc {
        pc.start_process(&pc_process).await?;
        println!(
            "{}",
            status(Glyph::Ok)
                .noun(pc_process.as_str())
                .verb("started")
                .render(theme)
        );
    }

    println!(
        "{}",
        section(theme, &format!("rebootstrapped: {agent_name}"))
    );
    println!("{}", Rail::blank(theme));
    let mut recap_block = Block::new();
    recap_block.push(
        status(Glyph::Ok)
            .noun("backup")
            .verb(report.backup_dir.display().to_string()),
    );
    let host_detail = if report.host_backed_up.is_empty() {
        "none (agent had not bootstrapped)".to_string()
    } else {
        report.host_backed_up.join(", ")
    };
    recap_block.push(status(Glyph::Ok).noun("host").verb(host_detail));
    if !report.sandbox_backed_up.is_empty() {
        recap_block.push(
            status(Glyph::Ok)
                .noun("sandbox")
                .verb(report.sandbox_backed_up.join(", ")),
        );
    }
    recap_block.push(
        status(Glyph::Ok)
            .noun("sessions")
            .verb(format!("{} deactivated", report.sessions_deactivated)),
    );
    if let right_agent::rebootstrap::SandboxStatus::Skipped(reason) = &report.sandbox_status {
        recap_block.push(
            status(Glyph::Warn)
                .noun("sandbox cleanup")
                .verb(format!("skipped ({reason})"))
                .fix(format!(
                    "re-run `right agent rebootstrap {agent_name}` once openshell is healthy"
                )),
        );
    }
    println!("{}", recap_block.render(theme));
    println!("{}", Rail::blank(theme));

    if stopped_pc.is_none() {
        println!("{}next: right up", Rail::prefix(theme));
    }

    Ok(())
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

async fn cmd_agent_ssh(home: &Path, agent_name: &str, command: &[String]) -> miette::Result<()> {
    use std::os::unix::process::CommandExt;

    // 1. Discover agent
    let agents = right_agent::agent::discover_agents(&right_agent::config::agents_dir(home))?;
    let agent = agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!(
                "Agent '{}' not found. Available: {}",
                agent_name,
                available.join(", ")
            )
        })?;

    // 2. Check sandbox mode
    if !matches!(
        agent.sandbox_mode(),
        right_agent::agent::types::SandboxMode::Openshell
    ) {
        return Err(miette::miette!(
            "Agent '{}' runs without sandbox, SSH not available",
            agent_name
        ));
    }

    // 3. Check agent is running via process-compose
    let pc = right_agent::runtime::PcClient::from_home(home)?.ok_or_else(|| {
        miette::miette!(
            help = "Start it with: right up",
            "Agent '{}' is not running",
            agent_name,
        )
    })?;
    pc.health_check().await.map_err(|_| {
        miette::miette!(
            help = "Start it with: right up",
            "Agent '{}' is not running",
            agent_name,
        )
    })?;

    let processes = pc.list_processes().await?;
    let pc_process_name = format!("{}-bot", agent_name);
    let proc = processes.iter().find(|p| p.name == pc_process_name);
    match proc {
        Some(p) if p.status != "Running" => {
            return Err(miette::miette!(
                help = "Start it with: right up",
                "Agent '{}' is not running (status: {})",
                agent_name,
                p.status,
            ));
        }
        None => {
            return Err(miette::miette!(
                help = "Start it with: right up",
                "Agent '{}' is not running",
                agent_name,
            ));
        }
        Some(_) => {} // Running — continue
    }

    // 4. Locate SSH config
    let sb_name = agent
        .config
        .as_ref()
        .map(|c| right_agent::openshell::resolve_sandbox_name(agent_name, c))
        .unwrap_or_else(|| right_agent::openshell::sandbox_name(agent_name));
    let ssh_config = home.join(format!("run/ssh/{}.ssh-config", sb_name));
    if !ssh_config.exists() {
        return Err(miette::miette!(
            help = "Try restarting the agent",
            "SSH config not found at {}. Try restarting the agent.",
            ssh_config.display(),
        ));
    }

    // 5. exec into SSH
    let ssh_host = right_agent::openshell::ssh_host_for_sandbox(&sb_name);
    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-F").arg(&ssh_config);
    cmd.arg(&ssh_host);
    if !command.is_empty() {
        cmd.arg(command.join(" "));
    }

    let err = cmd.exec();
    Err(miette::miette!("Failed to exec ssh: {err}"))
}

// Tests are placed mid-file historically; moving them is a structural
// change out of scope for this cleanup pass.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::{
        ConfigCommands, MemoryCommands, resolve_agent_db, truncate_content, write_managed_settings,
    };
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
        // create agent dir but no data.db
        let agent_dir = tmp.path().join("agents").join("testagent");
        fs::create_dir_all(&agent_dir).unwrap();
        let result = resolve_agent_db(tmp.path(), "testagent");
        let err = result.expect_err("should fail when data.db missing");
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
        assert_eq!(
            char_count, 61,
            "truncated string should be 61 chars (60 + ellipsis), got {char_count}"
        );
        assert!(
            result.ends_with('…'),
            "truncated string should end with ellipsis"
        );
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
        assert_eq!(
            char_count, 5,
            "should be 5 chars (4 + ellipsis), got {char_count}"
        );
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

        write_managed_settings(dir.to_str().unwrap(), path.to_str().unwrap())
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
        assert!(msg.contains("sudo"), "error must mention sudo, got: {msg}");
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

        assert!(
            !agent_dir.join(".git").exists(),
            "pre-condition: no .git yet"
        );

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

        assert!(
            agent_dir.join(".git").exists(),
            ".git/ should exist after init"
        );
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

        assert!(
            agent_dir.join(".git").exists(),
            ".git/ still present after idempotent run"
        );
    }

    // ---- settings.local.json tests ----

    #[test]
    fn settings_local_json_created_when_absent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-settings");
        let claude_dir = agent_dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_local = claude_dir.join("settings.local.json");
        assert!(
            !settings_local.exists(),
            "pre-condition: no settings.local.json"
        );

        if !settings_local.exists() {
            fs::write(&settings_local, "{}").unwrap();
        }

        assert!(
            settings_local.exists(),
            "settings.local.json should be created"
        );
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
        assert_eq!(
            after, original_content,
            "pre-existing content must not be overwritten"
        );
    }

    // ---- skills install tests ----

    #[test]
    fn skills_install_creates_builtin_skill_dirs() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = make_agent_dir(&tmp, "agent-skills");

        right_agent::codegen::install_builtin_skills(
            &agent_dir,
            &right_agent::agent::types::MemoryProvider::File,
        )
        .expect("install_builtin_skills should succeed");

        let skills_dir = agent_dir.join(".claude").join("skills");
        let skills_skill = skills_dir.join("rightskills").join("SKILL.md");
        assert!(
            skills_skill.exists(),
            "rightskills/SKILL.md should be installed"
        );
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

        assert!(
            !stale.exists(),
            "stale clawhub dir should be removed after cleanup"
        );
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

        assert!(
            !stale.exists(),
            "stale skills dir should be removed after cleanup"
        );
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

    // ---- McpCommands variant existence (compile-time) ----

    #[test]
    fn mcp_commands_status_variant_exists() {
        use super::McpCommands;
        let _ = McpCommands::Status { agent: None };
        let _ = McpCommands::Status {
            agent: Some("right".to_string()),
        };
    }

    // ---- cmd_mcp_status error paths ----

    #[test]
    fn cmd_mcp_status_errors_on_nonexistent_agent() {
        use super::cmd_mcp_status;
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let result = cmd_mcp_status(tmp.path(), Some("nonexistent"));
        let err = result.expect_err("should fail when agent not found");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("agent not found"),
            "error must mention 'agent not found', got: {msg}"
        );
    }

    #[test]
    fn cmd_mcp_status_returns_ok_with_no_mcp_json() {
        use super::cmd_mcp_status;
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("myagent");
        fs::create_dir_all(&agent_dir).unwrap();
        // CLI commands expect a pre-migrated DB (aggregator migrates at startup).
        right_agent::memory::open_db(&agent_dir, true).unwrap();

        let result = cmd_mcp_status(tmp.path(), Some("myagent"));
        assert!(result.is_ok(), "should succeed when mcp.json absent");
    }
}

const MANAGED_SETTINGS_DIR: &str = "/etc/claude-code";
const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";

/// Write managed-settings.json to the given dir/path (extracted for testability).
fn write_managed_settings(dir: &str, path: &str) -> miette::Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo right config strict-sandbox",
            "Permission denied creating {dir}: {e:#}"
        )
    })?;
    std::fs::write(path, "{\"allowManagedDomainsOnly\": true}\n").map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo right config strict-sandbox",
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
    let agent_path = right_agent::config::agents_dir(home).join(agent);
    if !agent_path.exists() {
        return Err(miette::miette!(
            "agent '{}' not found at {}",
            agent,
            agent_path.display()
        ));
    }
    let db_path = agent_path.join("data.db");
    if !db_path.exists() {
        return Err(miette::miette!(
            "no memory database for agent '{}' — run `right up` first",
            agent
        ));
    }
    right_agent::memory::open_connection(&agent_path, false)
        .map_err(|e| miette::miette!("failed to open data.db for '{}': {e:#}", agent))
}

fn cmd_memory_list(
    home: &Path,
    agent: &str,
    limit: i64,
    offset: i64,
    json: bool,
) -> miette::Result<()> {
    let conn = resolve_agent_db(home, agent)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, content, tags, stored_by, created_at \
             FROM memories \
             WHERE deleted_at IS NULL \
             ORDER BY created_at DESC, id DESC \
             LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?;
    // Local SQLite row projection; extracting a named alias is out of scope.
    #[allow(clippy::type_complexity)]
    let entries: Vec<(i64, String, Option<String>, Option<String>, String)> = stmt
        .query_map(rusqlite::params![limit, offset], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| miette::miette!("failed to list memories: {e:#}"))?;

    if json {
        for (id, content, tags, stored_by, created_at) in &entries {
            let obj = serde_json::json!({
                "id": id,
                "content": content,
                "tags": tags,
                "stored_by": stored_by,
                "created_at": created_at,
            });
            println!(
                "{}",
                serde_json::to_string(&obj)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories for agent '{agent}'.");
        return Ok(());
    }

    println!(
        "{:<6} {:<61} {:<20} CREATED_AT",
        "ID", "CONTENT", "STORED_BY"
    );
    for (id, content, _tags, stored_by, created_at) in &entries {
        let truncated = truncate_content(content, 60);
        let stored_by = stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            id, truncated, stored_by, created_at
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
    // resolve_agent_db validates agent dir and data.db existence before opening.
    let conn = resolve_agent_db(home, agent)?;

    // db_path needed only for fs metadata (file size) — derive from home, not conn.
    let db_path = right_agent::config::agents_dir(home)
        .join(agent)
        .join("data.db");
    let db_size = std::fs::metadata(&db_path)
        .map_err(|e| miette::miette!("failed to stat data.db: {e:#}"))?
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
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.content, m.tags, m.stored_by, m.created_at \
             FROM memories m \
             JOIN memories_fts f ON m.id = f.rowid \
             WHERE memories_fts MATCH ?1 \
               AND m.deleted_at IS NULL \
             ORDER BY bm25(memories_fts) \
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| miette::miette!(
            help = "FTS5 syntax: use simple words or phrases. Avoid special chars like * at start.",
            "search failed: {e:#}"
        ))?;
    // Local SQLite row projection; extracting a named alias is out of scope.
    #[allow(clippy::type_complexity)]
    let entries: Vec<(i64, String, Option<String>, Option<String>, String)> = stmt
        .query_map(rusqlite::params![query, limit, offset], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .map_err(|e| miette::miette!(
            help = "FTS5 syntax: use simple words or phrases. Avoid special chars like * at start.",
            "search failed: {e:#}"
        ))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| miette::miette!(
            help = "FTS5 syntax: use simple words or phrases. Avoid special chars like * at start.",
            "search failed: {e:#}"
        ))?;

    if json {
        for (id, content, tags, stored_by, created_at) in &entries {
            let obj = serde_json::json!({
                "id": id,
                "content": content,
                "tags": tags,
                "stored_by": stored_by,
                "created_at": created_at,
            });
            println!(
                "{}",
                serde_json::to_string(&obj)
                    .map_err(|e| miette::miette!("JSON serialization failed: {e:#}"))?
            );
        }
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memories match '{query}' for agent '{agent}'.");
        return Ok(());
    }

    println!(
        "{:<6} {:<61} {:<20} CREATED_AT",
        "ID", "CONTENT", "STORED_BY"
    );
    for (id, content, _tags, stored_by, created_at) in &entries {
        let truncated = truncate_content(content, 60);
        let stored_by = stored_by.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<6} {:<61} {:<20} {}",
            id, truncated, stored_by, created_at
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
            return Err(miette::miette!(
                "memory entry {id} not found for agent '{agent}'"
            ));
        }
        Some((content, stored_by)) => {
            println!("  id:        {id}");
            println!("  content:   {}", truncate_content(&content, 60));
            println!(
                "  stored_by: {}",
                stored_by.as_deref().unwrap_or("(unknown)")
            );
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

    let deleted = conn
        .execute("DELETE FROM memories WHERE id = ?1", [id])
        .map_err(|e| miette::miette!("failed to delete memory: {e:#}"))?;
    if deleted == 0 {
        return Err(miette::miette!(
            "memory entry {id} not found for agent '{agent}'"
        ));
    }

    println!("Deleted memory entry {id}.");
    Ok(())
}

fn cmd_pair(home: &Path, agent_name: Option<&str>) -> miette::Result<()> {
    let agent_name = agent_name.unwrap_or("right");

    let agents_dir = right_agent::config::agents_dir(home);
    let all_agents = right_agent::agent::discover_agents(&agents_dir)?;

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

    // Ensure schemas exist (function may run without prior cmd_up).
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent_name))?;
    std::fs::write(
        claude_dir.join("reply-schema.json"),
        right_agent::codegen::REPLY_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write reply-schema.json for '{}': {e:#}",
            agent_name
        )
    })?;
    std::fs::write(
        claude_dir.join("cron-schema.json"),
        right_agent::codegen::CRON_SCHEMA_JSON,
    )
    .map_err(|e| {
        miette::miette!(
            "failed to write cron-schema.json for '{}': {e:#}",
            agent_name
        )
    })?;

    // Assemble system prompt on host.
    let sandbox_mode = agent
        .config
        .as_ref()
        .map(|c| *c.sandbox_mode())
        .unwrap_or_default();
    let base_prompt = right_agent::codegen::generate_system_prompt(
        &agent.name,
        &sandbox_mode,
        &agent.path.to_string_lossy(),
    );
    let mut prompt = base_prompt;
    prompt.push_str("\n## Operating Instructions\n");
    prompt.push_str(right_agent::codegen::OPERATING_INSTRUCTIONS);
    prompt.push('\n');
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
        if let Ok(content) = std::fs::read_to_string(agent.path.join(file)) {
            prompt.push_str(&format!("\n{header}\n"));
            prompt.push_str(&content);
            prompt.push('\n');
        }
    }
    let prompt_path = claude_dir.join("composite-system-prompt.md");
    std::fs::write(&prompt_path, &prompt).map_err(|e| {
        miette::miette!("failed to write system prompt for '{}': {e:#}", agent_name)
    })?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)"))?;

    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(claude_bin)
        .arg("--system-prompt-file")
        .arg(&prompt_path)
        .arg("--dangerously-skip-permissions")
        .current_dir(&agent.path)
        .exec();

    Err(miette::miette!("failed to launch claude: {err}"))
}

fn cmd_mcp_status(home: &Path, agent_filter: Option<&str>) -> miette::Result<()> {
    let agents_dir = right_agent::config::agents_dir(home);
    // Collect agent dirs -- either all or filtered to one
    let entries: Vec<std::path::PathBuf> = if let Some(name) = agent_filter {
        let dir = agents_dir.join(name);
        if !dir.is_dir() {
            return Err(miette::miette!("agent not found: {name}"));
        }
        vec![dir]
    } else {
        let rd = std::fs::read_dir(&agents_dir)
            .map_err(|e| miette::miette!("cannot read agents dir: {e:#}"))?;
        let mut dirs: Vec<_> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs
    };

    let mut any = false;
    for agent_dir in &entries {
        let agent_name = agent_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let conn = right_agent::memory::open_connection(agent_dir, false)
            .map_err(|e| miette::miette!("open data.db for {agent_name}: {e:#}"))?;
        let servers = right_agent::mcp::credentials::db_list_servers(&conn)
            .map_err(|e| miette::miette!("db_list_servers for {agent_name}: {e:#}"))?;
        for s in &servers {
            println!("{agent_name}  {} [{}]", s.name, s.url);
            any = true;
        }
    }
    if !any {
        println!("No MCP servers configured.");
    }
    Ok(())
}

/// Check if sandbox migration is needed after config changes and perform it.
///
/// Compares the active sandbox policy (via gRPC) with the on-disk policy.yaml.
/// If filesystem/landlock sections differ, triggers a full sandbox migration
/// (backup -> create new -> restore -> delete old). Network-only changes are
/// applied automatically on next bot restart via hot-reload.
async fn maybe_migrate_sandbox(home: &Path, agent_name: &str) -> miette::Result<()> {
    let agents_dir = right_agent::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);

    // Load config from disk.
    let config = match right_agent::agent::discovery::parse_agent_config(&agent_dir)? {
        Some(c) => c,
        None => return Ok(()), // No agent.yaml — nothing to check.
    };

    // Only relevant for sandboxed agents.
    if !config.is_sandboxed() {
        return Ok(());
    }

    // Check OpenShell availability.
    let mtls_dir = match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => {
            println!("OpenShell not available — skipping sandbox migration check.");
            return Ok(());
        }
    };

    let sb_name = right_agent::openshell::resolve_sandbox_name(agent_name, &config);

    let mut grpc = match right_agent::openshell::connect_grpc(&mtls_dir).await {
        Ok(g) => g,
        Err(_) => {
            println!("Cannot connect to OpenShell gRPC — skipping sandbox migration check.");
            return Ok(());
        }
    };

    // Check if sandbox exists and is READY.
    let ready = right_agent::openshell::is_sandbox_ready(&mut grpc, &sb_name).await?;
    if !ready {
        // Sandbox doesn't exist or isn't ready — no migration needed.
        return Ok(());
    }

    // Get active policy from sandbox.
    let active_policy = match right_agent::openshell::get_active_policy(&mut grpc, &sb_name).await? {
        Some(p) => p,
        None => {
            println!(
                "Warning: cannot retrieve active policy for sandbox '{}'. \
                 If you changed filesystem policy, manually back up and recreate the sandbox.",
                sb_name
            );
            return Ok(());
        }
    };

    // Read new policy from disk.
    let policy_path = config
        .sandbox
        .as_ref()
        .and_then(|s| s.policy_file.as_ref())
        .map(|p| agent_dir.join(p))
        .unwrap_or_else(|| agent_dir.join("policy.yaml"));

    if !policy_path.exists() {
        // No policy file on disk — can't compare.
        return Ok(());
    }

    let policy_yaml = std::fs::read_to_string(&policy_path)
        .map_err(|e| miette::miette!("read {}: {e:#}", policy_path.display()))?;
    let new_policy = right_agent::openshell::parse_policy_yaml_filesystem(&policy_yaml)?;

    if right_agent::openshell::filesystem_policy_changed(&active_policy, &new_policy) {
        println!("\nFilesystem policy changed — sandbox migration required.");
        let confirmed =
            inquire::Confirm::new("migrate sandbox now? (backup old, create new, restore data)")
                .with_default(true)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if confirmed {
            perform_migration(home, agent_name, &sb_name, &mtls_dir).await?;
        } else {
            println!(
                "Migration skipped. Filesystem policy changes will NOT take effect until the sandbox is recreated."
            );
        }
    } else {
        println!("Network-only changes will apply on next bot restart.");
    }

    Ok(())
}

/// Perform sandbox migration: backup old sandbox, create new one, restore data, delete old.
///
/// `old_sandbox` and `mtls_dir` are pre-resolved by the caller to avoid redundant
/// config parsing and preflight checks.
async fn perform_migration(
    home: &Path,
    agent_name: &str,
    old_sandbox: &str,
    mtls_dir: &Path,
) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    let agents_dir = right_agent::config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);

    // --- Step 1/6: Backup ---
    println!("Step 1/6: Backing up sandbox '{old_sandbox}'...");

    let old_ssh_config = home
        .join("run")
        .join("ssh")
        .join(format!("{old_sandbox}.ssh-config"));
    if !old_ssh_config.exists() {
        return Err(miette::miette!(
            help = "Try restarting the agent first so SSH config is generated",
            "SSH config not found at {} — cannot back up sandbox",
            old_ssh_config.display(),
        ));
    }

    let old_ssh_host = right_agent::openshell::ssh_host_for_sandbox(old_sandbox);
    let backup_base = right_agent::config::backups_dir(home, agent_name);
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let backup_dir = backup_base.join(&timestamp);
    std::fs::create_dir_all(&backup_dir)
        .into_diagnostic()
        .map_err(|e| miette::miette!("failed to create backup dir: {e:#}"))?;

    let backup_tar = backup_dir.join("sandbox.tar.gz");
    right_agent::openshell::ssh_tar_download(
        &old_ssh_config,
        &old_ssh_host,
        "sandbox",
        &backup_tar,
        300,
    )
    .await?;

    let tar_size = std::fs::metadata(&backup_tar).map(|m| m.len()).unwrap_or(0);
    println!(
        "  Backup complete ({tar_size} bytes) at {}",
        backup_dir.display()
    );

    // --- Step 2/6: Create new sandbox ---
    let new_sandbox = format!("right-{agent_name}-{timestamp}");
    println!("Step 2/6: Creating new sandbox '{new_sandbox}'...");

    // Run codegen for staging dir.
    let agent_def = right_agent::agent::discover_single_agent(&agent_dir)?;
    let self_exe = std::env::current_exe()
        .into_diagnostic()
        .map_err(|e| miette::miette!("failed to resolve self exe: {e:#}"))?;
    right_agent::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;

    let staging = agent_dir.join("staging");
    right_agent::openshell::prepare_staging_dir(&agent_dir, &staging)?;

    let migration_config = right_agent::agent::discovery::parse_agent_config(&agent_dir)?;
    let policy_path = migration_config
        .as_ref()
        .and_then(|c| c.sandbox.as_ref())
        .and_then(|s| s.policy_file.as_ref())
        .map(|p| agent_dir.join(p))
        .unwrap_or_else(|| agent_dir.join("policy.yaml"));

    let mut child =
        right_agent::openshell::spawn_sandbox(&new_sandbox, &policy_path, Some(&staging))?;

    let mut grpc = right_agent::openshell::connect_grpc(mtls_dir).await?;

    // Wait for READY (race with child exit).
    tokio::select! {
        result = right_agent::openshell::wait_for_ready(&mut grpc, &new_sandbox, 120, 2) => {
            result?;
            drop(child);
        }
        status = child.wait() => {
            let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
            if !status.success() {
                return Err(miette::miette!(
                    "openshell sandbox create for '{}' exited with {status} before reaching READY",
                    new_sandbox
                ));
            }
        }
    }

    println!("  Sandbox '{new_sandbox}' is READY.");

    // --- Step 3/6: Wait for SSH ---
    println!("Step 3/6: Waiting for SSH transport...");
    let sandbox_id = right_agent::openshell::resolve_sandbox_id(&mut grpc, &new_sandbox).await?;
    right_agent::openshell::wait_for_ssh(&mut grpc, &sandbox_id, 60, 2).await?;
    println!("  SSH transport ready.");

    // --- Step 4/6: Generate SSH config ---
    println!("Step 4/6: Generating SSH config...");
    let ssh_config_dir = home.join("run").join("ssh");
    std::fs::create_dir_all(&ssh_config_dir)
        .into_diagnostic()
        .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
    let new_ssh_config =
        right_agent::openshell::generate_ssh_config(&new_sandbox, &ssh_config_dir).await?;
    println!("  SSH config written to {}", new_ssh_config.display());

    // --- Step 5/6: Restore data ---
    println!("Step 5/6: Restoring sandbox data...");
    let new_ssh_host = right_agent::openshell::ssh_host_for_sandbox(&new_sandbox);
    if let Err(e) =
        right_agent::openshell::ssh_tar_upload(&new_ssh_config, &new_ssh_host, &backup_tar, 600).await
    {
        // Rollback: delete new sandbox, keep old, report error.
        eprintln!("Restore failed — rolling back: deleting new sandbox '{new_sandbox}'...");
        right_agent::openshell::delete_sandbox(&new_sandbox).await;
        let _ = right_agent::openshell::wait_for_deleted(&mut grpc, &new_sandbox, 60, 2).await;
        // Remove new SSH config (best-effort).
        let _ = std::fs::remove_file(&new_ssh_config);
        return Err(miette::miette!(
            "Sandbox restore failed (old sandbox '{}' preserved): {:#}",
            old_sandbox,
            e
        ));
    }
    println!("  Sandbox data restored.");

    // --- Step 6/6: Update agent.yaml and cleanup ---
    println!("Step 6/6: Updating agent.yaml and cleaning up...");
    crate::wizard::update_agent_yaml_sandbox_name(&agent_dir, &new_sandbox)?;
    println!("  sandbox.name set to '{new_sandbox}' in agent.yaml");

    // Tear down the old sandbox's ControlMaster before we remove its config.
    // Best-effort — the master may already be dead if the bot exited cleanly.
    let old_socket = right_agent::openshell::control_master_socket_path(
        &home.join("run").join("ssh"),
        old_sandbox,
    );
    right_agent::openshell::tear_down_control_master(
        &old_ssh_config,
        &old_ssh_host,
        &old_socket,
    )
    .await;

    // Delete old sandbox (best-effort).
    println!("  Deleting old sandbox '{old_sandbox}'...");
    right_agent::openshell::delete_sandbox(old_sandbox).await;
    let _ = right_agent::openshell::wait_for_deleted(&mut grpc, old_sandbox, 60, 2).await;

    // Remove old SSH config (best-effort).
    let _ = std::fs::remove_file(&old_ssh_config);

    // Clean up staging dir.
    let _ = std::fs::remove_dir_all(&staging);

    println!("\nMigration complete. New sandbox: {new_sandbox}");
    println!("Restart the agent with `right up` to use the new sandbox.");

    Ok(())
}
