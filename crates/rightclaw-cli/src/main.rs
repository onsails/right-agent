use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub(crate) mod aggregator;
pub(crate) mod internal_api;
mod memory_server;
pub(crate) mod right_backend;
mod wizard;

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

/// Subcommands for `rightclaw agent`.
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
        force: bool,
        /// With --force: re-run wizard instead of reusing existing config
        #[arg(long, requires = "force")]
        fresh: bool,
        /// Network policy: restrictive or permissive
        #[arg(long)]
        network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
        /// Sandbox mode: openshell or none
        #[arg(long)]
        sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
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

/// Subcommands for `rightclaw mcp`.
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
    /// Initialize RightClaw home directory with default agent
    Init {
        /// Telegram bot token for channel setup (skip with Enter if interactive)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Comma-separated list of Telegram chat IDs allowed to use this bot
        /// (e.g. --telegram-allowed-chat-ids 85743491,100200300)
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
        /// Network policy: restrictive (Anthropic/Claude only) or permissive (all HTTPS)
        #[arg(long)]
        network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
        /// Sandbox mode: openshell or none
        #[arg(long)]
        sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
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
        /// Enable debug logging (writes to $RIGHTCLAW_HOME/run/<agent>-debug.log)
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
    /// Manage RightClaw configuration (interactive wizard if no subcommand)
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

    // Set up tracing with console + per-process file log.
    // Bot writes console to stderr (stdout reserved for JSON), aggregator to stdout (colored).
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let setup_file_log = |name: &str| {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".rightclaw")
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let file_appender = tracing_appender::rolling::daily(&log_dir, format!("{name}.log"));
        tracing_appender::non_blocking(file_appender)
    };

    let _log_guard;
    match &cli.command {
        Commands::Bot { agent, .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log(agent);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .with(tracing_subscriber::fmt::layer().with_writer(non_blocking).with_ansi(false))
                .init();
            _log_guard = Some(guard);
        }
        Commands::McpServer { .. } => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            let (non_blocking, guard) = setup_file_log("mcp-aggregator");
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
                .with(tracing_subscriber::fmt::layer().with_writer(non_blocking).with_ansi(false))
                .init();
            _log_guard = Some(guard);
        }
        _ => {
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter));
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .init();
            _log_guard = None;
        }
    };

    let home = rightclaw::config::resolve_home(
        cli.home.as_deref(),
        std::env::var("RIGHTCLAW_HOME").ok().as_deref(),
    )?;

    match cli.command {
        Commands::Init { telegram_token, telegram_allowed_chat_ids, tunnel_name, tunnel_hostname, yes, network_policy, sandbox_mode, force } => {
            cmd_init(&home, telegram_token.as_deref(), &telegram_allowed_chat_ids, &tunnel_name, tunnel_hostname.as_deref(), yes, network_policy, sandbox_mode, force)
        }
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
                crate::wizard::combined_setting_menu(&home)?;
                Ok(())
            }
            Some(ConfigCommands::StrictSandbox) => cmd_config_strict_sandbox(),
            Some(ConfigCommands::Get { key }) => {
                let config = rightclaw::config::read_global_config(&home)?;
                match key.as_str() {
                    "tunnel.hostname" => println!(
                        "{}",
                        config.tunnel.as_ref().map(|t| t.hostname.as_str()).unwrap_or("(not set)")
                    ),
                    "tunnel.uuid" => println!(
                        "{}",
                        config.tunnel.as_ref().map(|t| t.tunnel_uuid.as_str()).unwrap_or("(not set)")
                    ),
                    "tunnel.credentials-file" => println!(
                        "{}",
                        config.tunnel.as_ref().map(|t| t.credentials_file.display().to_string()).unwrap_or("(not set)".to_string())
                    ),
                    other => return Err(miette::miette!("Unknown config key: {other}")),
                }
                Ok(())
            }
            Some(ConfigCommands::Set { key, value }) => {
                Err(miette::miette!(
                    "Direct set not yet implemented for key '{key}' with value '{value}'. Use `rightclaw config` for interactive mode."
                ))
            }
        },
        Commands::Agent { command } => match command {
            AgentCommands::Init { name, yes, force, fresh, network_policy, sandbox_mode } => {
                cmd_agent_init(&home, &name, yes, force, fresh, network_policy, sandbox_mode)
            }
            AgentCommands::List => cmd_list(&home),
            AgentCommands::Config { name, key, value } => {
                match (key, value) {
                    (None, None) => crate::wizard::agent_setting_menu(&home, name.as_deref())?,
                    (Some(_key), _) => {
                        return Err(miette::miette!(
                            "Direct get/set not yet implemented. Use `rightclaw agent config` for interactive mode."
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(miette::miette!("Cannot set a value without a key"));
                    }
                }
                Ok(())
            }
            AgentCommands::Ssh { name, command } => {
                cmd_agent_ssh(&home, &name, &command).await
            }
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
        Commands::Mcp { command } => match command {
            McpCommands::Status { agent } => cmd_mcp_status(&home, agent.as_deref()),
        },
        // Unreachable: MemoryServer is dispatched before reaching here.
        Commands::MemoryServer => unreachable!("MemoryServer dispatched before tracing init"),
        Commands::McpServer { port, ref token_map } => {
            let agents_dir = rightclaw::config::agents_dir(&home);
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
                tokio::sync::Mutex<rightclaw::mcp::reconnect::ReconnectManager>,
            > = std::collections::HashMap::new();
            let http_client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());

            for (agent_name, _token) in &token_entries {
                let agent_dir = agents_dir.join(agent_name);
                let mtls_dir = match rightclaw::agent::discovery::parse_agent_config(&agent_dir) {
                    Ok(Some(config))
                        if *config.sandbox_mode() == rightclaw::agent::SandboxMode::Openshell =>
                    {
                        match rightclaw::openshell::preflight_check() {
                            rightclaw::openshell::OpenShellStatus::Ready(dir) => Some(dir),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let right = right_backend::RightBackend::new(agents_dir.clone(), mtls_dir);

                // Load existing MCP servers from SQLite and create ProxyBackends.
                // Collect OAuth entries for refresh scheduling.
                let mut proxies = std::collections::HashMap::new();
                let mut oauth_entries: Vec<(
                    String,
                    rightclaw::mcp::refresh::OAuthServerState,
                    std::sync::Arc<tokio::sync::RwLock<Option<String>>>,
                )> = Vec::new();
                let mut oauth_server_names = std::collections::HashSet::<String>::new();

                match rightclaw::memory::open_connection(&agent_dir, true) {
                    Ok(conn) => match rightclaw::mcp::credentials::db_list_servers(&conn) {
                        Ok(servers) => for s in servers {
                            let auth_method = rightclaw::mcp::proxy::AuthMethod::from_db(
                                s.auth_type.as_deref(),
                                s.auth_header.as_deref(),
                            );
                            let token = std::sync::Arc::new(
                                tokio::sync::RwLock::new(s.auth_token.clone()),
                            );

                            // Collect OAuth entries before moving token into ProxyBackend.
                            if s.auth_type.as_deref() == Some("oauth") {
                                oauth_server_names.insert(s.name.clone());
                                if let (Some(te), Some(cid), Some(exp)) =
                                    (&s.token_endpoint, &s.client_id, &s.expires_at)
                                {
                                    let expires_at =
                                        chrono::DateTime::parse_from_rfc3339(exp)
                                            .map(|dt| dt.with_timezone(&chrono::Utc))
                                            .unwrap_or_else(|_| chrono::Utc::now());
                                    oauth_entries.push((
                                        s.name.clone(),
                                        rightclaw::mcp::refresh::OAuthServerState {
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

                            let backend = rightclaw::mcp::proxy::ProxyBackend::new(
                                s.name.clone(),
                                agent_dir.clone(),
                                s.url.clone(),
                                token,
                                auth_method,
                            );
                            proxies.insert(s.name, std::sync::Arc::new(backend));
                        },
                        Err(e) => tracing::error!(agent = agent_name.as_str(), "failed to list MCP servers: {e:#}"),
                    },
                    Err(e) => tracing::error!(agent = agent_name.as_str(), "failed to open DB for MCP restore: {e:#}"),
                }

                // Clone proxies for reconnect tasks before moving into registry.
                let proxies_snapshot: Vec<(String, std::sync::Arc<rightclaw::mcp::proxy::ProxyBackend>)> =
                    proxies.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                let registry = aggregator::BackendRegistry {
                    right,
                    proxies: std::sync::Arc::new(tokio::sync::RwLock::new(proxies)),
                    agent_dir: agent_dir.clone(),
                };
                dispatcher.agents.insert(agent_name.clone(), registry);

                // Spawn per-agent refresh scheduler.
                let (refresh_tx, refresh_rx) =
                    tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshMessage>(32);
                tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
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
                        let due_in = rightclaw::mcp::refresh::refresh_due_in(state);
                        if due_in > std::time::Duration::ZERO {
                            let msg = rightclaw::mcp::refresh::RefreshMessage::NewEntry {
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
                let mut reconnect_mgr = rightclaw::mcp::reconnect::ReconnectManager::new(
                    refresh_tx.clone(),
                    agent_dir.clone(),
                );
                for (server_name, backend) in proxies_snapshot {
                    let http = http_client.clone();
                    let agent_name_owned = agent_name.clone();

                    if let Some((oauth_state, token_arc)) = oauth_map.get(&server_name) {
                        // OAuth server — check token expiry before connecting.
                        let due_in = rightclaw::mcp::refresh::refresh_due_in(oauth_state);
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
                                    b.set_status(
                                        rightclaw::mcp::proxy::BackendStatus::NeedsAuth,
                                    ).await;
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
                            b.set_status(
                                rightclaw::mcp::proxy::BackendStatus::NeedsAuth,
                            ).await;
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

                reconnect_managers_map.insert(
                    agent_name.clone(),
                    tokio::sync::Mutex::new(reconnect_mgr),
                );
                refresh_senders_map.insert(agent_name.clone(), refresh_tx);
            }

            let refresh_senders: aggregator::RefreshSenders =
                std::sync::Arc::new(refresh_senders_map);
            let reconnect_managers: aggregator::ReconnectManagers =
                std::sync::Arc::new(reconnect_managers_map);

            aggregator::run_aggregator_http(
                port, token_map, dispatcher, agents_dir, home, refresh_senders, reconnect_managers,
            ).await
        }
        Commands::Bot { agent, debug } => {
            let needs_restart = rightclaw_bot::run(rightclaw_bot::BotArgs {
                agent,
                home: cli.home,
                debug,
            })
            .await?;
            if needs_restart {
                std::process::exit(rightclaw_bot::CONFIG_RESTART_EXIT_CODE);
            }
            Ok(())
        }
    }
}

/// Filter agents by name, or clone all if no filter provided.
fn filter_agents(
    all_agents: &[rightclaw::agent::AgentDef],
    filter: Option<&[String]>,
) -> miette::Result<Vec<rightclaw::agent::AgentDef>> {
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
                miette::miette!("agent '{}' not found. Available agents: {}", name, available.join(", "))
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
    network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
    sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
    force: bool,
) -> miette::Result<()> {
    let interactive = !yes;

    // Telegram token: CLI flag > interactive prompt > skip.
    let token = match telegram_token {
        Some(t) => {
            rightclaw::init::validate_telegram_token(t)?;
            Some(t.to_string())
        }
        None if !interactive => None,
        None => crate::wizard::telegram_setup(None, true)?,
    };

    // Chat IDs: CLI flag > interactive prompt (only when token is set) > empty.
    let chat_ids: Vec<i64> = if !telegram_allowed_chat_ids.is_empty() {
        telegram_allowed_chat_ids.to_vec()
    } else if interactive && token.is_some() {
        crate::wizard::chat_ids_setup()?
    } else {
        vec![]
    };

    // Network policy: CLI flag > interactive prompt > restrictive (default for --yes).
    let network_policy = match network_policy {
        Some(p) => p,
        None if !interactive => rightclaw::agent::types::NetworkPolicy::Restrictive,
        None => rightclaw::init::prompt_network_policy()?,
    };

    // Sandbox mode: CLI flag > interactive prompt > openshell (default for --yes).
    let sandbox = match sandbox_mode {
        Some(m) => m,
        None if !interactive => rightclaw::agent::types::SandboxMode::Openshell,
        None => rightclaw::init::prompt_sandbox_mode()?,
    };

    rightclaw::init::init_rightclaw_home(home, token.as_deref(), &chat_ids, &network_policy, &sandbox)?;

    // Run codegen for the default "right" agent.
    // Per-agent codegen was moved to bot startup (59243d0) but init needs it
    // for schemas and settings before sandbox staging upload.
    {
        let agent_dir = home.join("agents/right");
        let self_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("rightclaw"));
        let agent_def = rightclaw::agent::AgentDef {
            name: "right".to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: rightclaw::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            agents_path: if agent_dir.join("AGENTS.md").exists() {
                Some(agent_dir.join("AGENTS.md"))
            } else {
                None
            },
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        rightclaw::codegen::run_agent_codegen(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        rightclaw::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;

        // Create sandbox if openshell mode.
        if matches!(sandbox, rightclaw::agent::types::SandboxMode::Openshell) {
            let staging = agent_dir.join("staging");
            rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

            let policy_path = agent_dir.join("policy.yaml");
            let sb_name = rightclaw::openshell::sandbox_name("right");
            let force_recreate = if force {
                true
            } else {
                prompt_sandbox_recreate_if_exists(&sb_name, interactive)?
            };
            println!("Creating OpenShell sandbox...");
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    rightclaw::openshell::ensure_sandbox(
                        "right",
                        &policy_path,
                        Some(&staging),
                        force_recreate,
                    )
                    .await
                })
            })?;
            println!("  Sandbox '{sb_name}' ready");

            let run_dir = home.join("run");
            std::fs::create_dir_all(run_dir.join("ssh"))
                .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    rightclaw::openshell::generate_ssh_config(
                        &rightclaw::openshell::sandbox_name("right"),
                        &run_dir.join("ssh"),
                    )
                    .await
                })
            })?;
        }
    }

    println!("Initialized RightClaw at {}", home.display());
    println!(
        "Default agent 'right' created at {}/agents/right/",
        home.display()
    );
    if token.is_some() {
        println!("Telegram channel configured.");
    }
    if !chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
    println!("Network policy: {network_policy}");

    // Tunnel setup via wizard.
    let tunnel_cfg = crate::wizard::tunnel_setup(tunnel_name, tunnel_hostname, interactive)?;

    let config = rightclaw::config::GlobalConfig {
        tunnel: tunnel_cfg,
    };
    rightclaw::config::write_global_config(home, &config)?;

    println!();
    println!("Setup complete. Next steps:");
    println!("  rightclaw up        Launch agents");
    println!("  rightclaw config    Change global settings");
    println!("  rightclaw doctor    Check configuration");

    Ok(())
}

fn cmd_agent_init(
    home: &Path,
    name: &str,
    yes: bool,
    force: bool,
    fresh: bool,
    network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
    sandbox_mode: Option<rightclaw::agent::types::SandboxMode>,
) -> miette::Result<()> {
    let interactive = !yes;
    let agents_parent = rightclaw::config::agents_dir(home);
    let agent_dir = agents_parent.join(name);
    let agent_existed = agent_dir.exists();

    // Reject if exists and --force not given.
    if agent_dir.exists() && !force {
        return Err(miette::miette!(
            help = "Use --force to wipe and re-create, or `rightclaw agent config` to change settings",
            "Agent directory already exists at {}",
            agent_dir.display()
        ));
    }

    // --- Force wipe logic ---
    let saved_overrides = if force && agent_dir.exists() {
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
            let config: rightclaw::agent::types::AgentConfig =
                serde_saphyr::from_str(&yaml_str).map_err(|e| {
                    miette::miette!(
                        help = "Use --fresh to reconfigure from scratch",
                        "Could not parse existing agent.yaml: {e:#}"
                    )
                })?;
            Some(config)
        };

        // Check agent is not running.
        let state_path = home.join("run/runtime-state.json");
        if state_path.exists() {
            let state = rightclaw::runtime::read_state(&state_path)?;
            if state.agents.iter().any(|a| a.name == name) {
                return Err(miette::miette!(
                    help = "Run `rightclaw down` first",
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
            println!(
                "  - OpenShell sandbox \"{}\" (if exists)",
                rightclaw::openshell::sandbox_name(name)
            );
            print!("Continue? [y/N] ");
            io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| miette::miette!("failed to read input: {e}"))?;
            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                return Err(miette::miette!("Aborted"));
            }
        }

        // Delete sandbox (best-effort, async).
        let sb_name = rightclaw::openshell::sandbox_name(name);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::delete_sandbox(&sb_name).await;
            });
        });

        // Delete SSH config.
        let ssh_config = home.join(format!("run/ssh/{}.ssh-config", sb_name));
        if ssh_config.exists() {
            std::fs::remove_file(&ssh_config).ok();
        }

        // Delete agent directory.
        std::fs::remove_dir_all(&agent_dir).map_err(|e| {
            miette::miette!("Failed to delete agent directory {}: {e:#}", agent_dir.display())
        })?;

        tracing::info!(agent = name, "wiped agent directory and sandbox");

        saved
    } else {
        None
    };

    // --- Build overrides ---
    let overrides = if let Some(config) = saved_overrides {
        // Reuse saved config from old agent.yaml.
        rightclaw::init::InitOverrides {
            sandbox_mode: config.sandbox_mode().clone(),
            network_policy: config.network_policy,
            telegram_token: config.telegram_token,
            allowed_chat_ids: config.allowed_chat_ids,
            model: config.model,
            env: config.env,
        }
    } else {
        // Fresh init: run wizard or use CLI flags.
        let sandbox = match sandbox_mode {
            Some(mode) => mode,
            None if !interactive => rightclaw::agent::types::SandboxMode::Openshell,
            None => rightclaw::init::prompt_sandbox_mode()?,
        };

        let network_policy =
            if matches!(sandbox, rightclaw::agent::types::SandboxMode::Openshell) {
                match network_policy {
                    Some(p) => p,
                    None if !interactive => {
                        rightclaw::agent::types::NetworkPolicy::Restrictive
                    }
                    None => rightclaw::init::prompt_network_policy()?,
                }
            } else {
                network_policy.unwrap_or(rightclaw::agent::types::NetworkPolicy::Permissive)
            };

        let token = if interactive {
            crate::wizard::telegram_setup(None, true)?
        } else {
            None
        };

        let chat_ids: Vec<i64> = if interactive && token.is_some() {
            crate::wizard::chat_ids_setup()?
        } else {
            vec![]
        };

        rightclaw::init::InitOverrides {
            sandbox_mode: sandbox,
            network_policy,
            telegram_token: token,
            allowed_chat_ids: chat_ids,
            model: None,
            env: std::collections::HashMap::new(),
        }
    };

    let agent_dir = rightclaw::init::init_agent(&agents_parent, name, Some(&overrides))?;

    // Run codegen so settings, schemas, skills are generated.
    // Per-agent codegen was moved to bot startup (59243d0) but init/agent-init
    // need it for schemas and settings before sandbox staging upload.
    {
        let self_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("rightclaw"));
        let agent_def = rightclaw::agent::AgentDef {
            name: name.to_string(),
            path: agent_dir.clone(),
            identity_path: agent_dir.join("IDENTITY.md"),
            config: rightclaw::agent::discovery::parse_agent_config(&agent_dir)?,
            soul_path: None,
            user_path: None,
            agents_path: if agent_dir.join("AGENTS.md").exists() {
                Some(agent_dir.join("AGENTS.md"))
            } else {
                None
            },
            tools_path: None,
            bootstrap_path: if agent_dir.join("BOOTSTRAP.md").exists() {
                Some(agent_dir.join("BOOTSTRAP.md"))
            } else {
                None
            },
            heartbeat_path: None,
        };
        rightclaw::codegen::run_agent_codegen(
            home,
            std::slice::from_ref(&agent_def),
            &self_exe,
            false,
        )?;
        rightclaw::codegen::run_single_agent_codegen(home, &agent_def, &self_exe, false)?;
    }

    // Create sandbox for openshell agents.
    if matches!(overrides.sandbox_mode, rightclaw::agent::types::SandboxMode::Openshell) {
        let staging = agent_dir.join("staging");
        rightclaw::openshell::prepare_staging_dir(&agent_dir, &staging)?;

        let policy_path = agent_dir.join("policy.yaml");
        let sb_name = rightclaw::openshell::sandbox_name(name);
        // --force always recreates; fresh agent (didn't exist before) always creates;
        // otherwise prompt if stale sandbox exists.
        let force_recreate = if force || !agent_existed {
            // Check if sandbox exists — if so, we need to recreate. If not, false is fine
            // (ensure_sandbox will create fresh).
            let exists = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    check_sandbox_exists_async(&sb_name).await
                })
            });
            exists.unwrap_or(false)
        } else {
            prompt_sandbox_recreate_if_exists(&sb_name, interactive)?
        };
        println!("Creating OpenShell sandbox...");
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::ensure_sandbox(
                    name,
                    &policy_path,
                    Some(&staging),
                    force_recreate,
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
            tokio::runtime::Handle::current().block_on(async {
                rightclaw::openshell::generate_ssh_config(
                    &rightclaw::openshell::sandbox_name(name),
                    &run_dir.join("ssh"),
                )
                .await
            })
        })?;
    }

    println!("Agent '{name}' created at {}", agent_dir.display());
    if overrides.telegram_token.is_some() {
        println!("Telegram channel configured.");
    }
    if !overrides.allowed_chat_ids.is_empty() {
        println!("Telegram chat ID allowlist configured.");
    }
    println!();
    println!("If rightclaw is running, apply changes with:");
    println!("  rightclaw reload");

    Ok(())
}

/// Check if a sandbox exists via gRPC. Returns Ok(bool).
async fn check_sandbox_exists_async(sandbox_name: &str) -> miette::Result<bool> {
    let mtls_dir = match rightclaw::openshell::preflight_check() {
        rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
        _ => return Ok(false), // OpenShell not available — no sandbox
    };
    let mut client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
    rightclaw::openshell::is_sandbox_ready(&mut client, sandbox_name).await
}

/// If a sandbox already exists, prompt the user to recreate or abort.
/// Returns `true` if sandbox exists and should be recreated.
/// Returns `false` if sandbox doesn't exist (fresh create).
/// Errors if user declines recreate.
fn prompt_sandbox_recreate_if_exists(sandbox_name: &str, interactive: bool) -> miette::Result<bool> {
    let exists = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(check_sandbox_exists_async(sandbox_name))
    })?;

    if !exists {
        return Ok(false); // No existing sandbox — fresh create
    }

    if !interactive {
        // Non-interactive (-y): refuse to silently destroy a sandbox.
        return Err(miette::miette!(
            help = "Run interactively to confirm, or use `--force`",
            "Sandbox '{sandbox_name}' already exists"
        ));
    }

    use std::io::{self, Write};
    println!();
    println!("⚠ Sandbox '{sandbox_name}' already exists.");
    println!("  1. Recreate — delete and create fresh sandbox");
    println!("  2. Cancel — use `rightclaw agent config` to update existing agent");
    loop {
        print!("Choose [1/2]: ");
        io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
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
    let agents_dir = rightclaw::config::agents_dir(home);
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
    // Fail fast if required tools are missing.
    rightclaw::runtime::verify_dependencies()?;

    let run_dir = home.join("run");

    // Pre-flight: check for stale processes holding required ports.
    {
        let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
        if client.health_check().await.is_ok() {
            return Err(miette::miette!(
                "rightclaw is already running. Use `rightclaw down` first or `rightclaw attach` to connect."
            ));
        }
    }
    check_port_available(rightclaw::runtime::MCP_HTTP_PORT).await?;

    // Discover agents.
    let agents_dir = rightclaw::config::agents_dir(home);
    let all_agents = rightclaw::agent::discover_agents(&agents_dir)?;

    let agents = filter_agents(&all_agents, agents_filter.as_deref())?;

    if agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `rightclaw agent init <name>` to create one."
        ));
    }

    // Pre-flight: when any agent needs sandbox, verify OpenShell is ready.
    // The bot process needs mTLS certs to connect to the gateway's gRPC API —
    // without them it will crash in a loop. Diagnose the specific issue and
    // offer to fix it interactively.
    let any_sandboxed = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| matches!(c.sandbox_mode(), rightclaw::agent::types::SandboxMode::Openshell))
            .unwrap_or(true) // default is openshell
    });
    if any_sandboxed {
        match rightclaw::openshell::preflight_check() {
            rightclaw::openshell::OpenShellStatus::Ready(_) => {}
            rightclaw::openshell::OpenShellStatus::NotInstalled => {
                println!("OpenShell is not installed. Sandbox mode requires OpenShell.");
                println!();
                let install = inquire::Confirm::new("Install OpenShell now?")
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
                    match rightclaw::openshell::preflight_check() {
                        rightclaw::openshell::OpenShellStatus::Ready(_) => {}
                        rightclaw::openshell::OpenShellStatus::NoGateway(_) => {
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
            rightclaw::openshell::OpenShellStatus::NoGateway(_) => {
                start_openshell_gateway()?;
            }
            status @ rightclaw::openshell::OpenShellStatus::BrokenGateway(_) => {
                return Err(openshell_status_error(status));
            }
        }
    }

    // Clear rightcron init locks so the bootstrap hook fires on this session.
    for agent in &agents {
        let lock = agent.path.join(".rightcron-init-done");
        let _ = std::fs::remove_file(&lock);
    }

    // Resolve current executable path once — written into each agent's mcp.json so the
    // right MCP server can be found even when rightclaw is not on PATH (process-compose).
    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    // Run cross-agent codegen: token map, policy validation,
    // cloudflared config, process-compose.yaml, and runtime state.
    rightclaw::codegen::run_agent_codegen(home, &agents, &self_exe, debug)?;

    // Check that at least one agent has a Telegram token configured.
    let has_bot_agents = agents.iter().any(|a| {
        a.config
            .as_ref()
            .map(|c| c.telegram_token.is_some())
            .unwrap_or(false)
    });
    if !has_bot_agents {
        eprintln!("rightclaw: no agents have Telegram tokens configured — nothing to start");
        return Err(miette::miette!("no agents have Telegram tokens configured"));
    }

    // Build process-compose command.
    let config_path = run_dir.join("process-compose.yaml");
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

/// Prompt the user to start an OpenShell gateway, then verify it came up.
fn start_openshell_gateway() -> miette::Result<()> {
    println!("OpenShell gateway is not running. Sandbox mode requires a running gateway.");
    println!("Note: OpenShell requires Docker to be installed and running.");
    println!();
    let start = inquire::Confirm::new("Start OpenShell gateway now?")
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
    match rightclaw::openshell::preflight_check() {
        rightclaw::openshell::OpenShellStatus::Ready(_) => {
            println!("OpenShell gateway started successfully.");
            Ok(())
        }
        status => Err(openshell_status_error(status)),
    }
}

/// Convert an `OpenShellStatus` into a user-facing miette error.
fn openshell_status_error(status: rightclaw::openshell::OpenShellStatus) -> miette::Report {
    match status {
        rightclaw::openshell::OpenShellStatus::Ready(_) => unreachable!(),
        rightclaw::openshell::OpenShellStatus::NotInstalled => miette::miette!(
            help = "Install from https://github.com/NVIDIA/OpenShell, or set `sandbox: mode: none` in agent.yaml",
            "OpenShell is not installed"
        ),
        rightclaw::openshell::OpenShellStatus::NoGateway(_) => miette::miette!(
            help = "Run `openshell gateway start`, or set `sandbox: mode: none` in agent.yaml",
            "OpenShell gateway is not running"
        ),
        rightclaw::openshell::OpenShellStatus::BrokenGateway(mtls_dir) => miette::miette!(
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
            help = "A previous rightclaw session may still be running. Kill it first:\n  \
                    killall rightclaw  # or: rightclaw down",
            "port {port} is already in use"
        )),
    }
}

async fn cmd_down(_home: &Path) -> miette::Result<()> {
    let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)
        .map_err(|_| miette::miette!("No running instance found. Is rightclaw running?"))?;

    client
        .health_check()
        .await
        .map_err(|_| miette::miette!("No running instance found. Is rightclaw running?"))?;

    client.shutdown().await.map_err(|e| {
        miette::miette!("Shutdown request failed (process-compose may already be stopped): {e:#}")
    })?;

    println!("All agents stopped.");
    Ok(())
}

async fn cmd_reload(home: &Path, _agents_filter: Option<Vec<String>>) -> miette::Result<()> {
    let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
    client.health_check().await.map_err(|_| {
        miette::miette!(
            help = "Start rightclaw first with `rightclaw up`",
            "nothing running — cannot reload"
        )
    })?;

    let agents_dir = rightclaw::config::agents_dir(home);
    let all_agents = rightclaw::agent::discover_agents(&agents_dir)?;

    if all_agents.is_empty() {
        return Err(miette::miette!(
            "no agents found. Run `rightclaw agent init <name>` to create one."
        ));
    }

    let self_exe = std::env::current_exe()
        .map_err(|e| miette::miette!("failed to resolve current executable path: {e:#}"))?;

    rightclaw::codegen::run_agent_codegen(home, &all_agents, &self_exe, false)?;

    client.reload_configuration().await?;

    let has_bot = all_agents.iter().any(|a| {
        a.config.as_ref().map(|c| c.telegram_token.is_some()).unwrap_or(false)
    });
    if !has_bot {
        eprintln!("rightclaw: warning: no agents have Telegram tokens — nothing will run");
    }

    println!("Reloaded. Active agents:");
    for agent in &all_agents {
        let has_token = agent.config.as_ref().map(|c| c.telegram_token.is_some()).unwrap_or(false);
        let status = if has_token { "bot" } else { "no token (skipped)" };
        println!("  {:<20} {}", agent.name, status);
    }

    Ok(())
}

async fn cmd_status(_home: &Path) -> miette::Result<()> {
    let client = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;

    client
        .health_check()
        .await
        .map_err(|_| miette::miette!("No running instance found. Is rightclaw running?"))?;

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

async fn cmd_agent_ssh(home: &Path, agent_name: &str, command: &[String]) -> miette::Result<()> {
    use std::os::unix::process::CommandExt;

    // 1. Discover agent
    let agents = rightclaw::agent::discover_agents(&rightclaw::config::agents_dir(home))?;
    let agent = agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            let available: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
            miette::miette!("Agent '{}' not found. Available: {}", agent_name, available.join(", "))
        })?;

    // 2. Check sandbox mode
    if !matches!(agent.sandbox_mode(), rightclaw::agent::types::SandboxMode::Openshell) {
        return Err(miette::miette!(
            "Agent '{}' runs without sandbox, SSH not available",
            agent_name
        ));
    }

    // 3. Check agent is running via process-compose
    let pc = rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT)?;
    pc.health_check()
        .await
        .map_err(|_| miette::miette!(
            help = "Start it with: rightclaw up",
            "Agent '{}' is not running",
            agent_name,
        ))?;

    let processes = pc.list_processes().await?;
    let pc_process_name = format!("{}-bot", agent_name);
    let proc = processes.iter().find(|p| p.name == pc_process_name);
    match proc {
        Some(p) if p.status != "Running" => {
            return Err(miette::miette!(
                help = "Start it with: rightclaw up",
                "Agent '{}' is not running (status: {})",
                agent_name,
                p.status,
            ));
        }
        None => {
            return Err(miette::miette!(
                help = "Start it with: rightclaw up",
                "Agent '{}' is not running",
                agent_name,
            ));
        }
        Some(_) => {} // Running — continue
    }

    // 4. Locate SSH config
    let sb_name = rightclaw::openshell::sandbox_name(agent_name);
    let ssh_config = home.join(format!("run/ssh/{}.ssh-config", sb_name));
    if !ssh_config.exists() {
        return Err(miette::miette!(
            help = "Try restarting the agent",
            "SSH config not found at {}. Try restarting the agent.",
            ssh_config.display(),
        ));
    }

    // 5. exec into SSH
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    let mut cmd = std::process::Command::new("ssh");
    cmd.arg("-F").arg(&ssh_config);
    cmd.arg(&ssh_host);
    if !command.is_empty() {
        cmd.arg(command.join(" "));
    }

    let err = cmd.exec();
    Err(miette::miette!("Failed to exec ssh: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{resolve_agent_db, truncate_content, write_managed_settings, ConfigCommands, MemoryCommands};
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

    // ---- McpCommands variant existence (compile-time) ----

    #[test]
    fn mcp_commands_status_variant_exists() {
        use super::McpCommands;
        let _ = McpCommands::Status { agent: None };
        let _ = McpCommands::Status { agent: Some("right".to_string()) };
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
        rightclaw::memory::open_db(&agent_dir, true).unwrap();

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
    let agent_path = rightclaw::config::agents_dir(home).join(agent);
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
            "no memory database for agent '{}' — run `rightclaw up` first",
            agent
        ));
    }
    rightclaw::memory::open_connection(&agent_path, false)
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
    // resolve_agent_db validates agent dir and data.db existence before opening.
    let conn = resolve_agent_db(home, agent)?;

    // db_path needed only for fs metadata (file size) — derive from home, not conn.
    let db_path = rightclaw::config::agents_dir(home).join(agent).join("data.db");
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

    let agents_dir = rightclaw::config::agents_dir(home);
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

    // Ensure schemas exist (function may run without prior cmd_up).
    let claude_dir = agent.path.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir for '{}': {e:#}", agent_name))?;
    std::fs::write(claude_dir.join("reply-schema.json"), rightclaw::codegen::REPLY_SCHEMA_JSON)
        .map_err(|e| miette::miette!("failed to write reply-schema.json for '{}': {e:#}", agent_name))?;
    std::fs::write(claude_dir.join("cron-schema.json"), rightclaw::codegen::CRON_SCHEMA_JSON)
        .map_err(|e| miette::miette!("failed to write cron-schema.json for '{}': {e:#}", agent_name))?;

    // Assemble system prompt on host.
    let sandbox_mode = agent.config.as_ref()
        .map(|c| c.sandbox_mode().clone())
        .unwrap_or_default();
    let base_prompt = rightclaw::codegen::generate_system_prompt(
        &agent.name,
        &sandbox_mode,
        &agent.path.to_string_lossy(),
    );
    let mut prompt = base_prompt;
    prompt.push_str("\n## Operating Instructions\n");
    prompt.push_str(rightclaw::codegen::OPERATING_INSTRUCTIONS);
    prompt.push('\n');
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("AGENTS.md", "## Agent Configuration"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
        if let Ok(content) = std::fs::read_to_string(agent.path.join(file)) {
            prompt.push_str(&format!("\n{header}\n"));
            prompt.push_str(&content);
            prompt.push('\n');
        }
    }
    let prompt_path = claude_dir.join("composite-system-prompt.md");
    std::fs::write(&prompt_path, &prompt)
        .map_err(|e| miette::miette!("failed to write system prompt for '{}': {e:#}", agent_name))?;

    let claude_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| {
            miette::miette!("claude CLI not found in PATH (tried: claude, claude-bun)")
        })?;

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
    let agents_dir = rightclaw::config::agents_dir(home);
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
        let conn = rightclaw::memory::open_connection(agent_dir, false)
            .map_err(|e| miette::miette!("open data.db for {agent_name}: {e:#}"))?;
        let servers = rightclaw::mcp::credentials::db_list_servers(&conn)
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
