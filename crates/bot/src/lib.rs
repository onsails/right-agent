mod config_watcher;
pub mod cron;
pub mod cron_delivery;
pub mod error;
mod keepalive;
pub mod login;
pub mod reflection;
mod stt;
pub mod sync;
pub mod telegram;
mod upgrade;

pub use error::BotError;

use rightclaw::agent::allowlist::{self, AllowlistHandle, AllowlistState};

/// Load `allowlist.yaml` for this agent, migrating from the legacy
/// `agent.yaml::allowed_chat_ids` field on first boot. Returns a shareable
/// `AllowlistHandle` ready for the routing filter and command handlers.
fn load_or_migrate_allowlist(
    agent_dir: &std::path::Path,
    legacy: &[i64],
) -> miette::Result<AllowlistHandle> {
    let now = chrono::Utc::now();
    let existed_before = allowlist::allowlist_path(agent_dir).exists();
    let report = allowlist::migrate_from_legacy(agent_dir, legacy, now)
        .map_err(|e| miette::miette!("allowlist migration: {e:#}"))?;
    if !existed_before
        && !report.already_present
        && (report.migrated_users + report.migrated_groups) > 0
    {
        tracing::info!(
            users = report.migrated_users,
            groups = report.migrated_groups,
            "migrated {} users, {} groups from agent.yaml::allowed_chat_ids; consider removing the legacy field",
            report.migrated_users,
            report.migrated_groups,
        );
    }
    if report.already_present && !legacy.is_empty() {
        tracing::warn!(
            "legacy allowed_chat_ids field in agent.yaml is ignored; source of truth is allowlist.yaml"
        );
    }
    let file = allowlist::read_file(agent_dir)
        .map_err(|e| miette::miette!("read allowlist: {e:#}"))?
        .unwrap_or_default();
    Ok(AllowlistHandle::new(AllowlistState::from_file(file)))
}

/// Exit code returned when bot shuts down due to config change.
/// process-compose's `on_failure` policy will restart the bot.
pub const CONFIG_RESTART_EXIT_CODE: i32 = 2;

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
/// Resolves agent directory, opens data.db, resolves token, and starts
/// the teloxide long-polling dispatcher with graceful shutdown wiring.
///
/// This is an async function. The caller (rightclaw-cli) runs inside a
/// `#[tokio::main]` runtime and simply `.await`s this call. No nested
/// runtime construction needed.
/// Returns `true` when the bot exited due to a config change and should be
/// restarted (the caller is expected to exit with [`CONFIG_RESTART_EXIT_CODE`]).
pub async fn run(args: BotArgs) -> miette::Result<bool> {
    run_async(args).await
}

async fn run_async(args: BotArgs) -> miette::Result<bool> {
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
        let dir = rightclaw::config::agents_dir(&home).join(&args.agent);
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

    // Per-agent codegen: regenerate all derived files from agent.yaml + identity files.
    // This ensures policy.yaml, settings.json, mcp.json, etc. reflect the current config
    // even after a config change triggered restart.
    let self_exe =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("rightclaw"));
    let agent_def = rightclaw::agent::discover_single_agent(&agent_dir)?;
    rightclaw::codegen::run_single_agent_codegen(&home, &agent_def, &self_exe, args.debug)?;
    tracing::info!(agent = %args.agent, "per-agent codegen complete");

    // Parse config after codegen (secret may have been generated in agent.yaml).
    let config =
        parse_agent_config(&agent_dir)?.unwrap_or_else(|| rightclaw::agent::types::AgentConfig {
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
            show_thinking: true,
            memory: None,
            stt: Default::default(),
        });

    // Load (or migrate from legacy) the bot-managed allowlist, and spawn a
    // notify-based watcher so external edits hot-reload into the in-memory
    // handle without requiring a bot restart.
    let allowlist = load_or_migrate_allowlist(&agent_dir, &config.allowed_chat_ids)?;
    let _allowlist_watcher = allowlist::spawn_watcher(&agent_dir, allowlist.clone())
        .map_err(|e| miette::miette!("allowlist watcher: {e:#}"))?;

    // Memory: initialize ResilientHindsight wrapper + prefetch cache if configured.
    let memory_provider = config
        .memory
        .as_ref()
        .map(|m| &m.provider)
        .cloned()
        .unwrap_or_default();

    let (hindsight_wrapper, prefetch_cache): (
        Option<Arc<rightclaw::memory::ResilientHindsight>>,
        Option<rightclaw::memory::prefetch::PrefetchCache>,
    ) = match &memory_provider {
        rightclaw::agent::types::MemoryProvider::Hindsight => {
            let mem_config = config.memory.as_ref().unwrap();
            let api_key = std::env::var("HINDSIGHT_API_KEY")
                .ok()
                .or_else(|| mem_config.api_key.clone())
                .ok_or_else(|| {
                    miette::miette!(
                        help = "Set HINDSIGHT_API_KEY env var, add `memory.api_key` to agent.yaml, or switch to `memory.provider: file`",
                        "Hindsight memory provider requires an API key"
                    )
                })?;
            let bank_id = mem_config
                .bank_id
                .as_deref()
                .unwrap_or(&args.agent)
                .to_string();
            let budget = mem_config.recall_budget.to_string();
            let client = rightclaw::memory::hindsight::HindsightClient::new(
                &api_key,
                &bank_id,
                &budget,
                mem_config.recall_max_tokens,
                None,
            );

            let wrapper = Arc::new(rightclaw::memory::ResilientHindsight::new(
                client,
                agent_dir.clone(),
                "bot",
            ));

            match wrapper
                .get_or_create_bank(rightclaw::memory::resilient::POLICY_STARTUP_BANK)
                .await
            {
                Ok(profile) => tracing::info!(
                    agent = %args.agent,
                    bank_id = %profile.bank_id,
                    "Hindsight bank ready"
                ),
                Err(rightclaw::memory::ResilientError::Upstream(e)) => match e.classify() {
                    rightclaw::memory::ErrorKind::Auth => tracing::error!(
                        agent = %args.agent,
                        "Hindsight AUTH failed at startup: {e:#} — booting in degraded mode"
                    ),
                    rightclaw::memory::ErrorKind::Client => tracing::error!(
                        agent = %args.agent,
                        "Hindsight 4xx at startup: {e:#} — payload or API-drift bug"
                    ),
                    _ => {
                        tracing::warn!(
                            agent = %args.agent,
                            "Hindsight transient at startup: {e:#} — will retry in background"
                        );
                        let w_bg = wrapper.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                                match w_bg
                                    .get_or_create_bank(
                                        rightclaw::memory::resilient::POLICY_STARTUP_BANK,
                                    )
                                    .await
                                {
                                    Ok(p) => {
                                        tracing::info!(
                                            bank_id = %p.bank_id,
                                            "background bank probe succeeded"
                                        );
                                        return;
                                    }
                                    Err(e) => tracing::warn!("background bank probe failed: {e:#}"),
                                }
                            }
                        });
                    }
                },
                Err(rightclaw::memory::ResilientError::CircuitOpen { .. }) => {
                    tracing::warn!("unexpected CircuitOpen at startup");
                }
            }

            let cache = rightclaw::memory::prefetch::PrefetchCache::new();
            (Some(wrapper), Some(cache))
        }
        rightclaw::agent::types::MemoryProvider::File => (None, None),
    };

    // Spawn background drain task if wrapper is present.
    // Periodically drains pending_retains from SQLite, calling drain_retain_item
    // on each row. Skips when wrapper is non-Healthy (breaker open or auth failed).
    //
    // `drain_tick` holds `&rusqlite::Connection` across an `.await`, and
    // `Connection` is `!Sync` -- so the future is `!Send` and cannot be handed
    // to `tokio::spawn`. We drive it via a `LocalSet` from a dedicated
    // `spawn_blocking` thread; async upstream calls (e.g. Hindsight HTTP) still
    // run on the shared runtime through the `Handle` captured inside `LocalSet`.
    if let Some(ref w) = hindsight_wrapper {
        let w = w.clone();
        let agent_db = agent_dir.clone();
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let local = tokio::task::LocalSet::new();
            handle.block_on(local.run_until(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                interval.tick().await; // first tick is immediate
                loop {
                    interval.tick().await;
                    if !matches!(w.status(), rightclaw::memory::MemoryStatus::Healthy) {
                        continue;
                    }
                    let conn = match rightclaw::memory::open_connection(&agent_db, false) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!("drain: open_connection failed: {e:#}");
                            continue;
                        }
                    };
                    let w_call = w.clone();
                    let report = rightclaw::memory::retain_queue::drain_tick(&conn, |items| {
                        let w = w_call.clone();
                        async move {
                            let item = rightclaw::memory::hindsight::RetainItem {
                                content: items[0].content.clone(),
                                context: items[0].context.clone(),
                                document_id: items[0].document_id.clone(),
                                update_mode: items[0].update_mode.clone(),
                                tags: items[0].tags.clone(),
                            };
                            w.drain_retain_item(&item).await
                        }
                    })
                    .await;
                    if report.deleted
                        + report.dropped_age
                        + report.dropped_client
                        + report.bumped_attempts
                        > 0
                    {
                        tracing::debug!(?report, "drain tick");
                    }
                }
            }));
        });
    }

    // Re-install skills with correct memory variant.
    rightclaw::codegen::skills::install_builtin_skills(&agent_dir, &memory_provider)?;

    let is_sandboxed = matches!(
        config.sandbox_mode(),
        rightclaw::agent::types::SandboxMode::Openshell
    );

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

    // Open data.db (creates if absent, applies migrations)
    let _conn = open_connection(&agent_dir, true)
        .map_err(|e| miette::miette!("failed to open data.db: {:#}", e))?;
    tracing::info!(agent = %args.agent, "data.db opened");

    // Resolve Telegram token
    let token = telegram::resolve_token(&config)?;

    // PC-04: Clear any prior Telegram webhook before starting long-polling.
    // Fatal if this fails -- competing with an active webhook causes silent message drops.
    {
        use teloxide::requests::Requester as _;
        let webhook_bot = teloxide::Bot::new(token.clone());
        webhook_bot.delete_webhook().await.map_err(|e| {
            miette::miette!(
                "deleteWebhook failed -- long polling would compete with active webhook: {e:#}"
            )
        })?;

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
            Err(e) => {
                tracing::warn!(agent = %args.agent, "getMe failed (non-fatal, bot identity unknown): {e:#}")
            }
        }
    }

    // Log registered MCP servers at startup.
    {
        let conn = rightclaw::memory::open_connection(&agent_dir, false)
            .map_err(|e| miette::miette!("failed to open data.db for MCP check: {e:#}"))?;
        match rightclaw::mcp::credentials::db_list_servers(&conn) {
            Ok(servers) => {
                for s in &servers {
                    tracing::info!(
                        agent = %args.agent,
                        server = %s.name,
                        url = %s.url,
                        "registered MCP server"
                    );
                }
            }
            Err(e) => tracing::warn!(agent = %args.agent, "db_list_servers check failed: {e:#}"),
        }
    }

    // Warn when the trusted-users set is empty — DMs will be silently dropped.
    {
        let r = allowlist.0.read().expect("allowlist lock poisoned");
        if r.users().is_empty() {
            tracing::warn!(
                agent = %args.agent,
                "allowlist.yaml has no trusted users — DMs will be silently dropped until you add one via `rightclaw agent allow` or a first-run wizard",
            );
        }
    }

    // Graceful restart: config watcher cancels this token when agent.yaml changes.
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;
    let shutdown = CancellationToken::new();
    let config_changed = Arc::new(AtomicBool::new(false));
    let agent_yaml_path = agent_dir.join("agent.yaml");
    config_watcher::spawn_config_watcher(
        &agent_yaml_path,
        shutdown.clone(),
        Arc::clone(&config_changed),
    )?;

    // Build shared OAuth PendingAuth map
    use std::collections::HashMap;
    use std::sync::Arc;
    use telegram::oauth_callback::{
        OAuthCallbackState, PendingAuthMap, run_oauth_callback_server, run_pending_auth_cleanup,
    };

    let pending_auth: PendingAuthMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let notify_bot = teloxide::Bot::new(token.clone());
    let agent_name = args.agent.clone();

    // Internal API client for bot→aggregator IPC (MCP add/remove/set-token)
    let internal_socket = home.join("run/internal.sock");
    let internal_client = Arc::new(rightclaw::mcp::internal_client::InternalClient::new(
        internal_socket,
    ));

    let oauth_state = OAuthCallbackState {
        pending_auth: Arc::clone(&pending_auth),
        agent_name: agent_name.clone(),
        bot: notify_bot,
        allowlist: allowlist.clone(),
        internal_client: Arc::clone(&internal_client),
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

    // One-time migration: oauth-state.json → SQLite
    migrate_oauth_state_to_db(&agent_dir);

    // Resolve sandbox name once — used throughout the bot lifetime.
    // None when running without sandbox (mode: none).
    let resolved_sandbox: Option<String> = if is_sandboxed {
        Some(rightclaw::openshell::resolve_sandbox_name(
            &args.agent,
            &config,
        ))
    } else {
        None
    };

    // --- OpenShell sandbox lifecycle (when sandbox mode is active) ---
    let (ssh_config_path, sandbox_ctx): (
        Option<std::path::PathBuf>,
        Option<(std::path::PathBuf, String)>,
    ) = if is_sandboxed {
        // Resolve policy path from agent.yaml sandbox config.
        let policy_path = config.resolve_policy_path(&agent_dir)?
            .ok_or_else(|| miette::miette!(
                "sandbox mode is openshell but no policy path resolved — check sandbox.policy_file in agent.yaml"
            ))?;

        // SAFETY: resolved_sandbox is always Some when is_sandboxed is true.
        let sandbox = resolved_sandbox.clone().unwrap();

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
                    help =
                        "Run `openshell gateway start`, or set `sandbox: mode: none` in agent.yaml",
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
        let sandbox_exists =
            rightclaw::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if !sandbox_exists {
            return Err(miette::miette!(
                help = format!(
                    "Run `rightclaw init` or `rightclaw agent init {}` to create the sandbox",
                    args.agent
                ),
                "Sandbox '{}' not found",
                sandbox
            ));
        }

        // Resolve host IP from inside sandbox for policy allowed_ips.
        let sandbox_id =
            rightclaw::openshell::resolve_sandbox_id(&mut grpc_client, &sandbox).await?;
        let host_ip = rightclaw::openshell::resolve_host_ip(&mut grpc_client, &sandbox_id).await?;

        // Regenerate policy with resolved host IP and apply.
        let network_policy = config.network_policy;
        let policy_content = rightclaw::codegen::policy::generate_policy(
            rightclaw::runtime::MCP_HTTP_PORT,
            &network_policy,
            host_ip,
        );
        // Drift check BEFORE write+apply: `openshell policy set --wait` rejects
        // landlock changes on a live sandbox with InvalidArgument, so applying
        // a drifted policy crash-loops the bot. FAIL FAST: if drift status
        // cannot be determined (parse failure, gRPC error, missing payload),
        // behave as if drift IS present — skip apply and log WARN. A bot that
        // runs with stale policy is better than one that crash-loops.
        let desired_filesystem =
            match rightclaw::openshell::parse_policy_yaml_filesystem(&policy_content) {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!(agent = %args.agent, "could not parse generated policy.yaml for drift check: {e:#}");
                    None
                }
            };
        let active_filesystem =
            match rightclaw::openshell::get_active_policy(&mut grpc_client, &sandbox).await {
                Ok(Some(a)) => Some(a),
                Ok(None) => {
                    tracing::warn!(agent = %args.agent, "active policy has no payload; skipping drift check");
                    None
                }
                Err(e) => {
                    tracing::warn!(agent = %args.agent, "could not fetch active policy for drift check: {e:#}");
                    None
                }
            };
        let drifted = match (active_filesystem, desired_filesystem) {
            (Some(active), Some(desired)) => {
                rightclaw::openshell::filesystem_policy_changed(&active, &desired)
            }
            _ => true,
        };

        if drifted {
            // Still write so a later `rightclaw agent config`-triggered
            // migration sees the fresh policy; skip apply to avoid crash-loop.
            rightclaw::codegen::contract::write_regenerated(&policy_path, &policy_content)?;
            tracing::warn!(
                agent = %args.agent,
                "Filesystem policy drift detected for '{}'. Landlock rules in the running sandbox do not match policy.yaml. Run `rightclaw agent config {}` (accept defaults) to trigger sandbox migration, or `rightclaw agent backup {} --sandbox-only` first if you want a recovery point.",
                args.agent, args.agent, args.agent,
            );
        } else {
            tracing::info!(agent = %args.agent, "reusing existing sandbox, applying policy with host_ip={:?}", host_ip);
            rightclaw::codegen::contract::write_and_apply_sandbox_policy(
                &sandbox,
                &policy_path,
                &policy_content,
            )
            .await?;
        }

        // Generate SSH config.
        let ssh_config_dir = home.join("run").join("ssh");
        std::fs::create_dir_all(&ssh_config_dir)
            .map_err(|e| miette::miette!("failed to create ssh config dir: {e:#}"))?;
        let config_path =
            rightclaw::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;
        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        (Some(config_path), Some((mtls_dir, sandbox_id)))
    } else {
        (None, None)
    };

    // Create inbox/outbox inside sandbox for attachment handling
    if is_sandboxed && let Some(ref cfg_path) = ssh_config_path {
        let ssh_host =
            rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.as_deref().unwrap());
        rightclaw::openshell::ssh_exec(
            cfg_path,
            &ssh_host,
            &["mkdir", "-p", "/sandbox/inbox", "/sandbox/outbox"],
            10,
        )
        .await
        .map_err(|e| miette::miette!("failed to create sandbox attachment dirs: {e:#}"))?;
    }

    // Sync config files to sandbox before starting teloxide.
    // Blocks until first sync completes — ensures sandbox has correct .claude.json,
    // settings.json, etc. before any claude -p invocations.
    let sync_handle = if let Some((ref mtls_dir, ref sandbox_id)) = sandbox_ctx {
        let sandbox = resolved_sandbox.clone().unwrap();
        let sbox = rightclaw::sandbox_exec::SandboxExec::new(
            mtls_dir.clone(),
            sandbox,
            sandbox_id.clone(),
        );
        sync::initial_sync(&agent_dir, &sbox).await?;
        let sync_agent_dir = agent_dir.clone();
        let sync_shutdown = shutdown.clone();
        Some(tokio::spawn(sync::run_sync_task(
            sync_agent_dir,
            sbox,
            sync_shutdown,
        )))
    } else {
        None
    };

    // Spawn periodic attachment cleanup task
    {
        let cleanup_agent_dir = agent_dir.clone();
        let cleanup_ssh_config = ssh_config_path.clone();
        let cleanup_sandbox = resolved_sandbox.clone();
        let cleanup_retention = config.attachments.retention_days;
        telegram::attachments::spawn_cleanup_task(
            cleanup_agent_dir,
            cleanup_ssh_config,
            cleanup_sandbox,
            cleanup_retention,
        );
    }

    // Upgrade lock: upgrade (write) vs CC sessions (read).
    let upgrade_lock = Arc::new(tokio::sync::RwLock::new(()));

    // Startup upgrade: runs before cron/telegram — no lock contention.
    if let Some(ref cfg_path) = ssh_config_path {
        upgrade::run_startup_upgrade(cfg_path, &args.agent).await;
    }

    // CRON-01: spawn cron task alongside Telegram dispatcher.
    // Cron results are persisted to DB; Telegram delivery is handled separately.
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_model = config.model.clone();
    let cron_ssh_config = ssh_config_path.clone();
    let cron_internal_client = Arc::clone(&internal_client);
    let cron_shutdown = shutdown.clone();
    let cron_sandbox = resolved_sandbox.clone();
    let cron_upgrade_lock = Arc::clone(&upgrade_lock);
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(
            cron_agent_dir,
            cron_agent_name,
            cron_model,
            cron_ssh_config,
            cron_internal_client,
            cron_shutdown,
            cron_sandbox,
            cron_upgrade_lock,
        )
        .await;
    });

    // Shared idle timestamp: tracks last handler/worker interaction for cron delivery gating.
    use crate::telegram::handler::IdleTimestamp;
    let idle_timestamp = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
        chrono::Utc::now().timestamp(),
    ))));

    // Cron delivery loop: delivers pending cron results through main CC session when idle
    let delivery_agent_dir = agent_dir.clone();
    let delivery_agent_name = args.agent.clone();
    let delivery_bot = telegram::bot::build_bot(token.clone());
    let delivery_allowlist = allowlist.clone();
    let delivery_idle_ts = Arc::clone(&idle_timestamp);
    let delivery_ssh_config = ssh_config_path.clone();
    let delivery_internal_client = Arc::clone(&internal_client);
    let delivery_shutdown = shutdown.clone();
    let delivery_sandbox = resolved_sandbox.clone();
    let delivery_upgrade_lock = Arc::clone(&upgrade_lock);
    let delivery_handle = tokio::spawn(async move {
        cron_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_bot,
            delivery_allowlist,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_internal_client,
            delivery_shutdown,
            delivery_sandbox,
            delivery_upgrade_lock,
        )
        .await;
    });

    // Spawn periodic claude upgrade task (sandbox-only).
    let upgrade_handle = ssh_config_path.as_ref().map(|cfg_path| {
        upgrade::spawn_upgrade_task(
            cfg_path.clone(),
            args.agent.clone(),
            shutdown.clone(),
            Arc::clone(&upgrade_lock),
        )
    });

    // Token keepalive: periodic `claude -p "hi"` to prevent OAuth token expiration.
    let keepalive_handle = keepalive::spawn_keepalive(
        agent_dir.clone(),
        ssh_config_path.clone(),
        resolved_sandbox.clone(),
        shutdown.clone(),
    );

    // Build STT context once at startup — shared across all worker sessions via Arc.
    let stt: Option<Arc<crate::stt::SttContext>> = if config.stt.enabled {
        let model_path = rightclaw::stt::model_cache_path(&home, config.stt.model);
        let transcriber = crate::stt::Transcriber::new(model_path);
        let ffmpeg_available = rightclaw::stt::ffmpeg_available();
        if !ffmpeg_available {
            tracing::warn!(
                "ffmpeg not found in PATH — voice messages will be answered with an error marker. \
                 Install: brew install ffmpeg / apt install ffmpeg."
            );
        }
        Some(Arc::new(crate::stt::SttContext {
            transcriber,
            ffmpeg_available,
        }))
    } else {
        None
    };

    let result = tokio::select! {
        result = telegram::run_telegram(
            token,
            allowlist,
            agent_dir,
            args.debug,
            Arc::clone(&pending_auth),
            home.clone(),
            ssh_config_path,
            config.show_thinking,
            config.model.clone(),
            shutdown.clone(),
            Arc::clone(&idle_timestamp),
            Arc::clone(&internal_client),
            resolved_sandbox,
            hindsight_wrapper,
            prefetch_cache,
            upgrade_lock,
            stt,
        ) => result,
        result = axum_handle => result
            .map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
    };

    // Signal cron/sync tasks to stop. The teloxide dispatcher handles SIGTERM
    // internally but doesn't cancel this token, so we must do it here.
    shutdown.cancel();

    tracing::info!("waiting for cron to finish");
    let _ = cron_handle.await;
    tracing::info!("waiting for cron delivery to finish");
    let _ = delivery_handle.await;
    if let Some(handle) = sync_handle {
        tracing::info!("waiting for sync to finish");
        let _ = handle.await;
    }
    // Await keepalive/upgrade so their in-flight Interval::tick() futures
    // resolve before the tokio runtime is dropped. Without this, the runtime
    // drop panics: "A Tokio 1.x context was found, but it is being shutdown."
    let _ = keepalive_handle.await;
    if let Some(handle) = upgrade_handle {
        let _ = handle.await;
    }
    tracing::info!("graceful shutdown complete");

    // Propagate any dispatcher/axum error first, then signal config restart.
    result?;

    if config_changed.load(Ordering::Acquire) {
        tracing::info!("config change detected — requesting restart");
        return Ok(true);
    }

    Ok(false)
}

/// Migrate OAuth state from oauth-state.json to SQLite (one-time).
/// Non-fatal — logs warnings and continues on error.
fn migrate_oauth_state_to_db(agent_dir: &std::path::Path) {
    let json_path = agent_dir.join("oauth-state.json");
    if !json_path.exists() {
        return;
    }

    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to read oauth-state.json for migration: {e:#}");
            return;
        }
    };
    let state: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to parse oauth-state.json: {e:#}");
            return;
        }
    };

    let conn = match rightclaw::memory::open_connection(agent_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to open DB for oauth-state migration: {e:#}");
            return;
        }
    };

    let mut all_succeeded = true;
    if let Some(servers) = state.get("servers").and_then(|s| s.as_object()) {
        for (name, entry) in servers {
            let token_endpoint = entry
                .get("token_endpoint")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let client_id = entry
                .get("client_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let client_secret = entry.get("client_secret").and_then(|v| v.as_str());
            let refresh_token = entry.get("refresh_token").and_then(|v| v.as_str());
            let expires_at = entry
                .get("expires_at")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if let Err(e) = rightclaw::mcp::credentials::db_set_oauth_state(
                &conn,
                name,
                "",
                refresh_token,
                token_endpoint,
                client_id,
                client_secret,
                expires_at,
            ) {
                tracing::warn!(server = %name, "skipping oauth-state migration: {e:#}");
                all_succeeded = false;
            }
        }
    }

    if !all_succeeded {
        tracing::warn!("keeping oauth-state.json — some server migrations failed");
        return;
    }

    if let Err(e) = std::fs::remove_file(&json_path) {
        tracing::warn!("failed to remove oauth-state.json after migration: {e:#}");
    } else {
        tracing::info!("migrated oauth-state.json to SQLite and removed file");
    }
}
