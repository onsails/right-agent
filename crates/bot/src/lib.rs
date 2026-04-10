mod config_watcher;
pub mod cron;
pub mod error;
pub mod login;
pub mod sync;
pub mod telegram;

pub use error::BotError;

/// Arguments passed from the CLI `rightclaw bot` subcommand.
#[derive(Debug, Clone)]
pub struct BotArgs {
    /// Agent name (directory name under $RIGHTCLAW_HOME/agents/).
    pub agent: String,
    /// Override for RIGHTCLAW_HOME (from --home flag).
    pub home: Option<String>,
    /// Pass --verbose to CC subprocess and log CC stderr at debug level.
    pub debug: bool,
}

/// Entry point called from rightclaw-cli.
///
/// Resolves agent directory, opens memory.db, resolves token, and starts
/// the teloxide long-polling dispatcher with graceful shutdown wiring.
///
/// This is an async function. The caller (rightclaw-cli) runs inside a
/// `#[tokio::main]` runtime and simply `.await`s this call. No nested
/// runtime construction needed.
pub async fn run(args: BotArgs) -> miette::Result<()> {
    run_async(args).await
}

async fn run_async(args: BotArgs) -> miette::Result<()> {
    use rightclaw::{
        agent::discovery::{parse_agent_config, validate_agent_name},
        config::resolve_home,
        memory::open_connection,
    };
    use std::path::PathBuf;

    // Resolve RIGHTCLAW_HOME
    let home = resolve_home(
        args.home.as_deref(),
        std::env::var("RIGHTCLAW_HOME").ok().as_deref(),
    )?;

    // Validate agent name
    validate_agent_name(&args.agent).map_err(|e| miette::miette!("{e}"))?;

    // RC_AGENT_DIR override (used by process-compose in Phase 26)
    let agent_dir: PathBuf = if let Ok(dir) = std::env::var("RC_AGENT_DIR") {
        PathBuf::from(dir)
    } else {
        let dir = home.join("agents").join(&args.agent);
        if !dir.exists() {
            return Err(miette::miette!(
                "agent directory not found: {}",
                dir.display()
            ));
        }
        dir
    };

    // Create inbox/outbox directories for attachment handling
    for subdir in &["inbox", "outbox", "tmp/inbox", "tmp/outbox"] {
        let dir = agent_dir.join(subdir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| miette::miette!("failed to create {}: {e:#}", dir.display()))?;
    }

    // Parse agent config
    let config = parse_agent_config(&agent_dir)?.unwrap_or_else(|| {
        rightclaw::agent::types::AgentConfig {
            allowed_chat_ids: vec![],
            telegram_token: None,
            restart: Default::default(),
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            env: Default::default(),
            secret: None,
            attachments: Default::default(),
            network_policy: Default::default(),
        }
    });

    let is_sandboxed = matches!(config.sandbox_mode(), rightclaw::agent::types::SandboxMode::Openshell);

    let bootstrap_pending = agent_dir.join("BOOTSTRAP.md").exists();
    tracing::info!(
        agent = %args.agent,
        sandbox_mode = ?config.sandbox_mode(),
        model = config.model.as_deref().unwrap_or("inherit"),
        restart = ?config.restart,
        network_policy = %config.network_policy,
        bootstrap_pending,
        "bot starting"
    );

    // Open memory.db (creates if absent, applies migrations)
    let _conn = open_connection(&agent_dir)
        .map_err(|e| miette::miette!("failed to open memory.db: {:#}", e))?;
    tracing::info!(agent = %args.agent, "memory.db opened");

    // Resolve Telegram token
    let token = telegram::resolve_token(&agent_dir, &config)?;

    // PC-04: Clear any prior Telegram webhook before starting long-polling.
    // Fatal if this fails -- competing with an active webhook causes silent message drops.
    {
        use teloxide::requests::Requester as _;
        let webhook_bot = teloxide::Bot::new(token.clone());
        webhook_bot
            .delete_webhook()
            .await
            .map_err(|e| miette::miette!("deleteWebhook failed -- long polling would compete with active webhook: {e:#}"))?;

        // Log bot identity -- helps detect token conflicts with other running CC sessions
        match webhook_bot.get_me().await {
            Ok(me) => tracing::info!(
                agent = %args.agent,
                bot_id = me.id.0,
                bot_username = %me.username(),
                "deleteWebhook succeeded -- bot identity confirmed"
            ),
            // getMe is diagnostic-only; a transient API failure here does not block operation.
            // Intentional FAIL FAST exception -- deleteWebhook already confirmed connectivity.
            Err(e) => tracing::warn!(agent = %args.agent, "getMe failed (non-fatal, bot identity unknown): {e:#}"),
        }
    }

    // Warn about unauthenticated MCP servers at startup (UAT gap -- test 8).
    match rightclaw::mcp::detect::mcp_auth_status(&agent_dir) {
        Ok(statuses) => {
            for s in &statuses {
                if s.state != rightclaw::mcp::detect::AuthState::Present {
                    tracing::warn!(
                        agent = %args.agent,
                        server = %s.name,
                        state = %s.state,
                        "MCP server needs auth"
                    );
                }
            }
        }
        Err(e) => tracing::warn!(agent = %args.agent, "mcp_auth_status check failed: {e:#}"),
    }

    // Warn if allowed_chat_ids is empty (D-05)
    if config.allowed_chat_ids.is_empty() {
        tracing::warn!(
            agent = %args.agent,
            "allowed_chat_ids is empty -- all incoming messages will be dropped. \
             Run `rightclaw agent config {}` to add your Telegram chat ID",
            args.agent,
        );
    }

    // Graceful restart: config watcher cancels this token when agent.yaml changes.
    use tokio_util::sync::CancellationToken;
    let shutdown = CancellationToken::new();
    let agent_yaml_path = agent_dir.join("agent.yaml");
    config_watcher::spawn_config_watcher(&agent_yaml_path, shutdown.clone())?;

    // CRON-01: spawn cron task alongside Telegram dispatcher.
    // Build bot here so cron can send replies; run_telegram builds its own independent instance.
    let cron_bot = telegram::bot::build_bot(token.clone());
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_chat_ids = config.allowed_chat_ids.clone();
    let cron_shutdown = shutdown.clone();
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_bot, cron_chat_ids, cron_shutdown).await;
    });

    // Build shared OAuth PendingAuth map
    use std::collections::HashMap;
    use std::sync::Arc;
    use telegram::oauth_callback::{OAuthCallbackState, PendingAuthMap, run_oauth_callback_server, run_pending_auth_cleanup};

    let pending_auth: PendingAuthMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let notify_bot = teloxide::Bot::new(token.clone());
    let notify_chat_ids = config.allowed_chat_ids.clone();
    let agent_name = args.agent.clone();

    let mcp_json_path = agent_dir.join("mcp.json");

    // Create refresh scheduler channels
    let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshMessage>(32);
    let (notify_refresh_tx, mut notify_refresh_rx) = tokio::sync::mpsc::channel::<String>(32);

    let refresh_tx_for_handler = refresh_tx.clone();

    let oauth_state = OAuthCallbackState {
        pending_auth: Arc::clone(&pending_auth),
        mcp_json_path,
        agent_name: agent_name.clone(),
        bot: notify_bot,
        notify_chat_ids,
        refresh_tx,
    };

    // Spawn cleanup task
    tokio::spawn(run_pending_auth_cleanup(Arc::clone(&pending_auth)));

    // Spawn axum OAuth callback server and wait for it to bind before starting teloxide
    let socket_path = agent_dir.join("oauth-callback.sock");
    let (axum_ready_tx, axum_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let axum_socket = socket_path.clone();
    let axum_handle = tokio::spawn(async move {
        run_oauth_callback_server(axum_socket, oauth_state, Some(axum_ready_tx)).await
    });
    // Wait for axum to bind before starting teloxide (ensures callback socket is ready)
    let _ = axum_ready_rx.await;

    // Spawn OAuth refresh scheduler
    let oauth_state_path = agent_dir.join("oauth-state.json");
    let mcp_json_path_for_refresh = agent_dir.join("mcp.json");
    let sandbox_for_refresh = if is_sandboxed {
        Some(rightclaw::openshell::sandbox_name(&agent_name))
    } else {
        None
    };
    tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
        oauth_state_path,
        mcp_json_path_for_refresh,
        sandbox_for_refresh,
        refresh_rx,
        notify_refresh_tx,
    ));

    // Forward refresh error notifications to Telegram
    let bot_for_notify = teloxide::Bot::new(token.clone());
    let ids_for_notify: Vec<i64> = config.allowed_chat_ids.clone();
    tokio::spawn(async move {
        use teloxide::requests::Requester as _;
        while let Some(msg) = notify_refresh_rx.recv().await {
            for &chat_id in &ids_for_notify {
                let _ = bot_for_notify.send_message(teloxide::types::ChatId(chat_id), &msg).await;
            }
        }
    });

    // --- OpenShell sandbox lifecycle (when sandbox mode is active) ---
    let ssh_config_path: Option<std::path::PathBuf> = if is_sandboxed {
        // Resolve policy path from agent.yaml sandbox config.
        let policy_path = config.resolve_policy_path(&agent_dir)?
            .ok_or_else(|| miette::miette!(
                "sandbox mode is openshell but no policy path resolved — check sandbox.policy_file in agent.yaml"
            ))?;

        let sandbox = rightclaw::openshell::sandbox_name(&args.agent);

        // Verify OpenShell is ready before attempting gRPC connection.
        let mtls_dir = match rightclaw::openshell::preflight_check() {
            rightclaw::openshell::OpenShellStatus::Ready(dir) => dir,
            rightclaw::openshell::OpenShellStatus::NotInstalled => {
                return Err(miette::miette!(
                    help = "Install from https://github.com/NVIDIA/OpenShell, or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell is not installed"
                ));
            }
            rightclaw::openshell::OpenShellStatus::NoGateway(_) => {
                return Err(miette::miette!(
                    help = "Run `openshell gateway start`, or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell gateway is not running"
                ));
            }
            rightclaw::openshell::OpenShellStatus::BrokenGateway(dir) => {
                return Err(miette::miette!(
                    help = "Try `openshell gateway destroy && openshell gateway start`,\n  \
                            or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell gateway exists but mTLS certificates are missing at {}",
                    dir.display()
                ));
            }
        };

        // Check if sandbox already exists and is READY.
        let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
        let sandbox_exists = rightclaw::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if sandbox_exists {
            // Reuse existing sandbox — just apply updated policy.
            tracing::info!(agent = %args.agent, "reusing existing sandbox");
            rightclaw::openshell::apply_policy(&sandbox, &policy_path).await?;
        }

        if !sandbox_exists {
            return Err(miette::miette!(
                help = format!("Run `rightclaw init` or `rightclaw agent init {}` to create the sandbox", args.agent),
                "Sandbox '{}' not found",
                sandbox
            ));
        }

        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let config_path = rightclaw::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;
        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        Some(config_path)
    } else {
        None
    };

    // Create inbox/outbox inside sandbox for attachment handling
    if is_sandboxed
        && let Some(ref cfg_path) = ssh_config_path
    {
        let ssh_host = rightclaw::openshell::ssh_host(&args.agent);
        rightclaw::openshell::ssh_exec(
            cfg_path, &ssh_host,
            &["mkdir", "-p", "/sandbox/inbox", "/sandbox/outbox"],
            10,
        ).await
            .map_err(|e| miette::miette!("failed to create sandbox attachment dirs: {e:#}"))?;
    }

    // Sync config files to sandbox before starting teloxide.
    // Blocks until first sync completes — ensures sandbox has correct .claude.json,
    // settings.json, etc. before any claude -p invocations.
    let sync_handle = if is_sandboxed {
        let sync_sandbox = rightclaw::openshell::sandbox_name(&args.agent);
        sync::initial_sync(&agent_dir, &sync_sandbox).await?;
        let sync_agent_dir = agent_dir.clone();
        let sync_sandbox_bg = sync_sandbox;
        let sync_shutdown = shutdown.clone();
        Some(tokio::spawn(sync::run_sync_task(sync_agent_dir, sync_sandbox_bg, sync_shutdown)))
    } else {
        None
    };

    // Spawn periodic attachment cleanup task
    {
        let cleanup_agent_dir = agent_dir.clone();
        let cleanup_ssh_config = ssh_config_path.clone();
        let cleanup_agent_name = args.agent.clone();
        let cleanup_retention = config.attachments.retention_days;
        telegram::attachments::spawn_cleanup_task(
            cleanup_agent_dir,
            cleanup_ssh_config,
            cleanup_agent_name,
            cleanup_retention,
        );
    }

    let result = tokio::select! {
        result = telegram::run_telegram(
            token,
            config.allowed_chat_ids,
            agent_dir,
            args.debug,
            Arc::clone(&pending_auth),
            home.clone(),
            ssh_config_path,
            refresh_tx_for_handler,
            shutdown.clone(),
        ) => result,
        result = axum_handle => result
            .map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
    };

    // Signal cron/sync tasks to stop. The teloxide dispatcher handles SIGTERM
    // internally but doesn't cancel this token, so we must do it here.
    shutdown.cancel();

    tracing::info!("waiting for cron to finish");
    let _ = cron_handle.await;
    if let Some(handle) = sync_handle {
        tracing::info!("waiting for sync to finish");
        let _ = handle.await;
    }
    tracing::info!("graceful shutdown complete");

    result
}

