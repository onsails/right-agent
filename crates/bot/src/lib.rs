pub mod cron;
pub mod error;
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
    /// Disable OpenShell sandbox — invoke claude -p directly.
    pub no_sandbox: bool,
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
        }
    });

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
            "allowed_chat_ids is empty -- all incoming messages will be dropped"
        );
    }

    // CRON-01: spawn cron task alongside Telegram dispatcher.
    // Build bot here so cron can send replies; run_telegram builds its own independent instance.
    let cron_bot = telegram::bot::build_bot(token.clone());
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_chat_ids = config.allowed_chat_ids.clone();
    tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_bot, cron_chat_ids).await;
    });

    // Build shared OAuth PendingAuth map
    use std::collections::HashMap;
    use std::sync::Arc;
    use telegram::oauth_callback::{OAuthCallbackState, PendingAuthMap, run_oauth_callback_server, run_pending_auth_cleanup};

    let pending_auth: PendingAuthMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let notify_bot = teloxide::Bot::new(token.clone());
    let notify_chat_ids = config.allowed_chat_ids.clone();
    let agent_name = args.agent.clone();

    let mcp_json_path = agent_dir.join(".mcp.json");

    // Create refresh scheduler channels
    let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshEntry>(32);
    let (notify_refresh_tx, mut notify_refresh_rx) = tokio::sync::mpsc::channel::<String>(32);

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
    let mcp_json_path_for_refresh = agent_dir.join("staging").join(".mcp.json");
    let sandbox_for_refresh = if !args.no_sandbox {
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
    let ssh_config_path: Option<std::path::PathBuf> = if !args.no_sandbox {
        // Read policy path from env (generated by cmd_up).
        let policy_path = std::env::var("RC_SANDBOX_POLICY")
            .map(std::path::PathBuf::from)
            .map_err(|_| miette::miette!("RC_SANDBOX_POLICY not set — required for sandbox mode, run `rightclaw up` first"))?;

        let sandbox = rightclaw::openshell::sandbox_name(&args.agent);

        // mTLS certs dir.
        let mtls_dir = std::env::var("OPENSHELL_MTLS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/etc"))
                    .join("openshell/gateways/openshell/mtls")
            });

        // Check if sandbox already exists and is READY.
        let mut grpc_client = rightclaw::openshell::connect_grpc(&mtls_dir).await?;
        let sandbox_exists = rightclaw::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if sandbox_exists {
            // Reuse existing sandbox — just apply updated policy.
            tracing::info!(agent = %args.agent, "reusing existing sandbox");
            rightclaw::openshell::apply_policy(&sandbox, &policy_path).await?;
        } else {
            // Sandbox doesn't exist — create with curated staging dir.
            tracing::info!(agent = %args.agent, "creating new sandbox");
            let upload_dir = agent_dir.join("staging");
            prepare_staging_dir(&agent_dir, &upload_dir)?;

            let mut child = rightclaw::openshell::spawn_sandbox(&sandbox, &policy_path, Some(&upload_dir))?;

            tokio::select! {
                result = rightclaw::openshell::wait_for_ready(&mut grpc_client, &sandbox, 120, 2) => {
                    result?;
                    // `openshell sandbox create` is a long-lived monitor process.
                    // Drop the handle — kill_on_drop(false) keeps it alive.
                    drop(child);
                }
                status = child.wait() => {
                    let status = status.map_err(|e| miette::miette!("sandbox create child wait failed: {e:#}"))?;
                    if !status.success() {
                        return Err(miette::miette!(
                            "openshell sandbox create for '{}' exited with {status} before reaching READY",
                            args.agent
                        ));
                    }
                }
            }
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

    // Spawn background sync task for sandbox file sync.
    if !args.no_sandbox {
        let sync_agent_dir = agent_dir.clone();
        let sync_sandbox = rightclaw::openshell::sandbox_name(&args.agent);
        tokio::spawn(sync::run_sync_task(sync_agent_dir, sync_sandbox));
    }

    let pc_port: u16 = std::env::var("RC_PC_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(rightclaw::runtime::pc_client::PC_PORT);

    let result = tokio::select! {
        result = telegram::run_telegram(
            token,
            config.allowed_chat_ids,
            agent_dir,
            args.debug,
            Arc::clone(&pending_auth),
            home.clone(),
            ssh_config_path,
            pc_port,
        ) => result,
        result = axum_handle => result
            .map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
    };

    result
}

/// Prepare the staging directory for sandbox upload.
///
/// Copies curated files from the agent directory into staging/,
/// excluding credentials (sandbox gets its own via login flow) and plugins (not used).
fn prepare_staging_dir(agent_dir: &std::path::Path, upload_dir: &std::path::Path) -> miette::Result<()> {
    let staging_claude = upload_dir.join(".claude");
    if staging_claude.exists() {
        std::fs::remove_dir_all(&staging_claude)
            .map_err(|e| miette::miette!("failed to clean staging/.claude: {e:#}"))?;
    }
    std::fs::create_dir_all(&staging_claude)
        .map_err(|e| miette::miette!("failed to create staging/.claude: {e:#}"))?;

    let src_claude = agent_dir.join(".claude");

    // Copy individual items — NOT credentials, NOT plugins, NOT shell-snapshots
    let copy_items: &[&str] = &[
        "settings.json",
        "reply-schema.json",
        "agents", // agent definition directory
    ];

    for item in copy_items {
        let src = src_claude.join(item);
        let dst = staging_claude.join(item);
        if !src.exists() {
            continue;
        }
        let meta = std::fs::metadata(&src)
            .map_err(|e| miette::miette!("failed to stat {}: {e:#}", src.display()))?;
        if meta.is_dir() {
            copy_dir_resolve_symlinks(&src, &dst)
                .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
        } else {
            std::fs::copy(&src, &dst)
                .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
        }
    }

    // Copy only rightclaw builtin skills (not entire skills/ tree)
    let skills_src = src_claude.join("skills");
    if skills_src.exists() {
        for builtin in &["rightskills", "cronsync"] {
            let skill_src = skills_src.join(builtin);
            let skill_dst = staging_claude.join("skills").join(builtin);
            if skill_src.exists() {
                copy_dir_resolve_symlinks(&skill_src, &skill_dst)
                    .map_err(|e| miette::miette!("failed to copy skill {builtin} to staging: {e:#}"))?;
            }
        }
    }

    // Copy .claude.json (trust/onboarding — at agent root, not inside .claude/)
    let claude_json_src = agent_dir.join(".claude.json");
    let claude_json_dst = upload_dir.join(".claude.json");
    if claude_json_src.exists() {
        std::fs::copy(&claude_json_src, &claude_json_dst)
            .map_err(|e| miette::miette!("failed to copy .claude.json to staging: {e:#}"))?;
    }

    // Copy .mcp.json
    let mcp_json_src = agent_dir.join(".mcp.json");
    let mcp_json_dst = upload_dir.join(".mcp.json");
    if mcp_json_src.exists() {
        std::fs::copy(&mcp_json_src, &mcp_json_dst)
            .map_err(|e| miette::miette!("failed to copy .mcp.json to staging: {e:#}"))?;
    }

    tracing::info!("prepared staging dir for sandbox upload");
    Ok(())
}

/// Recursively copy a directory, resolving symlinks to regular files.
/// Skips entries that fail to read (e.g. broken symlinks).
fn copy_dir_resolve_symlinks(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        // Resolve symlinks by reading metadata (follows symlinks).
        let meta = match std::fs::metadata(&src_path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %src_path.display(), "skipping unresolvable entry: {e}");
                continue;
            }
        };

        if meta.is_dir() {
            copy_dir_resolve_symlinks(&src_path, &dst_path)?;
        } else if meta.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
