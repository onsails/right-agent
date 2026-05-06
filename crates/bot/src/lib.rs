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

use right_agent::agent::allowlist::{self, AllowlistHandle, AllowlistState};

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

/// Register the Telegram webhook with retry-and-backoff.
///
/// Calls `setWebhook` with the derived URL, secret, and allowed updates.
/// Retries with capped exponential backoff (2s → 60s, jittered) on transient
/// errors. Exits with code 2 on `ApiError::InvalidToken` (invalid bot token).
/// Cancels on shutdown.
async fn webhook_register_loop(
    bot: telegram::BotType,
    url: url::Url,
    secret: String,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    use std::sync::atomic::Ordering;
    use teloxide::ApiError;
    use teloxide::RequestError;
    use teloxide::payloads::SetWebhookSetters as _;
    use teloxide::requests::Requester as _;
    use tokio::time::Duration;

    let allowed = telegram::webhook::webhook_allowed_updates();
    let mut delay = Duration::from_secs(2);

    loop {
        if shutdown.is_cancelled() {
            return;
        }

        let req = bot
            .set_webhook(url.clone())
            .secret_token(secret.clone())
            .allowed_updates(allowed.clone())
            .max_connections(40);

        match req.await {
            Ok(_) => {
                webhook_set.store(true, Ordering::Relaxed);
                tracing::info!(target: "bot::webhook", url = %url, "webhook registered");
                return;
            }
            Err(e) => {
                if matches!(&e, RequestError::Api(ApiError::InvalidToken)) {
                    tracing::error!(target: "bot::webhook", "bot token invalid; exiting");
                    std::process::exit(2);
                }
                let jitter_ms = (rand::random::<u64>() % 1000) as i64 - 500;
                let with_jitter_ms = (delay.as_millis() as i64 + jitter_ms).max(500) as u64;
                tracing::warn!(
                    target: "bot::webhook",
                    error = %format!("{e:#}"),
                    retry_in_ms = with_jitter_ms,
                    "setWebhook failed",
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(with_jitter_ms)) => {}
                    _ = shutdown.cancelled() => return,
                }
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// Exit code returned when bot shuts down due to config change.
/// process-compose's `on_failure` policy will restart the bot.
pub const CONFIG_RESTART_EXIT_CODE: i32 = 2;

/// Arguments passed from the CLI `right bot` subcommand.
#[derive(Debug, Clone)]
pub struct BotArgs {
    /// Agent name (directory name under $RIGHT_HOME/agents/).
    pub agent: String,
    /// Override for RIGHT_HOME (from --home flag).
    pub home: Option<String>,
    /// Pass --verbose to CC subprocess and log CC stderr at debug level.
    pub debug: bool,
}

/// Entry point called from the right CLI.
///
/// Resolves agent directory, opens data.db, resolves token, and starts
/// the teloxide webhook dispatcher with graceful shutdown wiring.
///
/// This is an async function. The caller (right CLI) runs inside a
/// `#[tokio::main]` runtime and simply `.await`s this call. No nested
/// runtime construction needed.
/// Returns `true` when the bot exited due to a config change and should be
/// restarted (the caller is expected to exit with [`CONFIG_RESTART_EXIT_CODE`]).
pub async fn run(args: BotArgs) -> miette::Result<bool> {
    run_async(args).await
}

async fn run_async(args: BotArgs) -> miette::Result<bool> {
    use right_agent::{
        agent::discovery::{parse_agent_config, validate_agent_name},
        config::resolve_home,
        memory::open_connection,
    };
    use std::path::PathBuf;

    // Resolve RIGHT_HOME
    let home = resolve_home(
        args.home.as_deref(),
        std::env::var("RIGHT_HOME").ok().as_deref(),
    )?;

    // Validate agent name
    validate_agent_name(&args.agent).map_err(|e| miette::miette!("{e}"))?;

    // RC_AGENT_DIR override (used by process-compose in Phase 26)
    let agent_dir: PathBuf = if let Ok(dir) = std::env::var("RC_AGENT_DIR") {
        PathBuf::from(dir)
    } else {
        let dir = right_agent::config::agents_dir(&home).join(&args.agent);
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
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
    let agent_def = right_agent::agent::discover_single_agent(&agent_dir)?;
    right_agent::codegen::run_single_agent_codegen(&home, &agent_def, &self_exe, args.debug)?;
    tracing::info!(agent = %args.agent, "per-agent codegen complete");

    // Parse config after codegen (secret may have been generated in agent.yaml).
    let config =
        parse_agent_config(&agent_dir)?.unwrap_or_else(|| right_agent::agent::types::AgentConfig {
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
        Option<Arc<right_agent::memory::ResilientHindsight>>,
        Option<right_agent::memory::prefetch::PrefetchCache>,
    ) = match &memory_provider {
        right_agent::agent::types::MemoryProvider::Hindsight => {
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
            let client = right_agent::memory::hindsight::HindsightClient::new(
                &api_key,
                &bank_id,
                &budget,
                mem_config.recall_max_tokens,
                None,
            );

            let wrapper = Arc::new(right_agent::memory::ResilientHindsight::new(
                client,
                agent_dir.clone(),
                "bot",
            ));

            match wrapper
                .get_or_create_bank(right_agent::memory::resilient::POLICY_STARTUP_BANK)
                .await
            {
                Ok(profile) => tracing::info!(
                    agent = %args.agent,
                    bank_id = %profile.bank_id,
                    "Hindsight bank ready"
                ),
                Err(right_agent::memory::ResilientError::Upstream(e)) => match e.classify() {
                    right_agent::memory::ErrorKind::Auth => tracing::error!(
                        agent = %args.agent,
                        "Hindsight AUTH failed at startup: {e:#} — booting in degraded mode"
                    ),
                    right_agent::memory::ErrorKind::Client => tracing::error!(
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
                                        right_agent::memory::resilient::POLICY_STARTUP_BANK,
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
                Err(right_agent::memory::ResilientError::CircuitOpen { .. }) => {
                    tracing::warn!("unexpected CircuitOpen at startup");
                }
            }

            let cache = right_agent::memory::prefetch::PrefetchCache::new();
            (Some(wrapper), Some(cache))
        }
        right_agent::agent::types::MemoryProvider::File => (None, None),
    };

    // Graceful shutdown token, created before any long-lived background task is
    // spawned. The drop_guard ensures `shutdown.cancel()` runs on every exit
    // path of `run_async` (early `?` errors, panics, normal return). Without
    // this, an early Err leaves the drain task polling `tokio::time::interval`
    // when the runtime begins to drop, which panics with
    // "A Tokio 1.x context was found, but it is being shutdown."
    use tokio_util::sync::CancellationToken;
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();

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
        let drain_shutdown = shutdown.clone();
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let local = tokio::task::LocalSet::new();
            handle.block_on(local.run_until(run_drain_loop(w, agent_db, drain_shutdown)));
        });
    }

    // Re-install skills with correct memory variant.
    right_agent::codegen::skills::install_builtin_skills(&agent_dir, &memory_provider)?;

    let is_sandboxed = matches!(
        config.sandbox_mode(),
        right_agent::agent::types::SandboxMode::Openshell
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
    let conn = open_connection(&agent_dir, true)
        .map_err(|e| miette::miette!("failed to open data.db: {:#}", e))?;
    tracing::info!(agent = %args.agent, "data.db opened");

    // One-time data migration: rewrite legacy `@immediate` + `X-FORK-FROM:`
    // bg-continuation rows into the new `@bg:<uuid>` schedule encoding.
    // Idempotent — safe to re-run on every startup.
    //
    // Deliberate FAIL FAST exception: a transient DB error here only causes
    // legacy rows to keep their old encoding for one more boot; the cron
    // engine ignores them (`@immediate` no longer maps to a fork target),
    // so they sit dormant rather than corrupting state. Halting bot startup
    // would block agents whose data.db has zero legacy rows for a problem
    // affecting nobody. Logged at error so the next boot retries and the
    // condition stays visible.
    match crate::cron::migrate_legacy_bg_continuation(&conn) {
        Ok(0) => {}
        Ok(n) => {
            tracing::info!(agent = %args.agent, "migrated {n} legacy bg-continuation rows")
        }
        Err(e) => {
            tracing::error!(agent = %args.agent, "legacy bg-continuation migration failed: {e:#}")
        }
    }

    // Resolve Telegram token
    let token = telegram::resolve_token(&config)?;

    // Log bot identity at startup -- helps detect token conflicts with other
    // running CC sessions. Webhook registration happens later via the register
    // loop (after the UDS bind), not here.
    {
        use teloxide::requests::Requester as _;
        let probe_bot = teloxide::Bot::new(token.clone());
        match probe_bot.get_me().await {
            Ok(me) => tracing::info!(
                agent = %args.agent,
                bot_id = me.id.0,
                bot_username = %me.username(),
                "bot identity confirmed"
            ),
            Err(e) => tracing::warn!(
                agent = %args.agent,
                "getMe failed (non-fatal, bot identity unknown): {e:#}"
            ),
        }
    }

    // Log registered MCP servers at startup.
    {
        let conn = right_agent::memory::open_connection(&agent_dir, false)
            .map_err(|e| miette::miette!("failed to open data.db for MCP check: {e:#}"))?;
        match right_agent::mcp::credentials::db_list_servers(&conn) {
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
                "allowlist.yaml has no trusted users — DMs will be silently dropped until you add one via `right agent allow` or a first-run wizard",
            );
        }
    }

    // Graceful restart: config watcher cancels the shutdown token (created
    // earlier, alongside the memory-drain task) when agent.yaml changes.
    // Model-only changes are hot-reloaded into model_arc without restart.
    use std::sync::atomic::{AtomicBool, Ordering};
    let config_changed = Arc::new(AtomicBool::new(false));
    let agent_yaml_path = agent_dir.join("agent.yaml");
    // Create the model swap cell here so both the watcher and the telegram
    // dispatcher share the same Arc. The watcher writes; the dispatcher reads.
    let model_arc: Arc<arc_swap::ArcSwap<Option<String>>> =
        Arc::new(arc_swap::ArcSwap::from_pointee(config.model.clone()));
    config_watcher::spawn_config_watcher(
        &agent_yaml_path,
        shutdown.clone(),
        Arc::clone(&config_changed),
        Arc::clone(&model_arc),
    )?;

    // Build shared OAuth PendingAuth map
    use std::collections::HashMap;
    use std::sync::Arc;
    use telegram::oauth_callback::{
        OAuthCallbackState, PendingAuthMap, run_bot_uds_server, run_pending_auth_cleanup,
    };

    let pending_auth: PendingAuthMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let notify_bot = teloxide::Bot::new(token.clone());
    let agent_name = args.agent.clone();

    // Internal API client for bot→aggregator IPC (MCP add/remove/set-token)
    let internal_socket = home.join("run/internal.sock");
    let internal_client = Arc::new(right_agent::mcp::internal_client::InternalClient::new(
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

    // Spawn axum bot UDS server and wait for it to bind before starting teloxide
    let socket_path = agent_dir.join("bot.sock");
    let started_at = std::time::Instant::now();

    // Build webhook URL from global tunnel hostname.
    //
    // No trailing slash: axum's `nest("/tg/<agent>", router)` matches
    // `/tg/<agent>` exactly (inner sees `/`) but does NOT rewrite
    // `/tg/<agent>/` to `/`, so a trailing slash here would yield 404.
    // The cloudflared ingress rule is anchored to match this exact path.
    let global_cfg = right_agent::config::read_global_config(&home)?;
    let webhook_url = url::Url::parse(&format!(
        "https://{}/tg/{}",
        global_cfg.tunnel.hostname.trim_end_matches('/'),
        args.agent
    ))
    .map_err(|e| miette::miette!("invalid webhook URL: {e:#}"))?;

    // Derive webhook secret from the agent secret.
    let agent_secret = config
        .secret
        .clone()
        .ok_or_else(|| miette::miette!("agent.yaml missing required `secret:` field"))?;
    let webhook_secret = right_agent::mcp::derive_token(&agent_secret, "tg-webhook")?;

    // Build the webhook listener + router. The listener is consumed by
    // run_telegram → dispatcher.dispatch_with_listener; the router is mounted
    // on the bot.sock UDS axum app so cloudflared can POST updates.
    let (update_listener, _webhook_stop, webhook_router) =
        telegram::webhook::build_webhook_router(webhook_secret.clone(), webhook_url.clone());

    // Shared flag for healthz "webhook_set"; flipped by Task 10's register loop.
    let webhook_set_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let (axum_ready_tx, axum_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let axum_socket = socket_path.clone();
    let agent_name_for_uds = args.agent.clone();
    let webhook_set_for_axum = webhook_set_flag.clone();
    let axum_handle = tokio::spawn(async move {
        run_bot_uds_server(
            axum_socket,
            oauth_state,
            webhook_router,
            agent_name_for_uds,
            started_at,
            webhook_set_for_axum,
            Some(axum_ready_tx),
        )
        .await
    });
    // Wait for axum to bind before starting teloxide (ensures callback socket is ready)
    let _ = axum_ready_rx.await;

    // Register Telegram webhook in the background. Retries with backoff;
    // flips webhook_set_flag (visible via /healthz) on first success.
    let webhook_url_for_loop = webhook_url.clone();
    let webhook_secret_for_loop = webhook_secret.clone();
    let bot_for_webhook = telegram::bot::build_bot(token.clone());
    let shutdown_for_webhook = shutdown.clone();
    let webhook_set_for_loop = webhook_set_flag.clone();
    let _webhook_register_handle = tokio::spawn(async move {
        webhook_register_loop(
            bot_for_webhook,
            webhook_url_for_loop,
            webhook_secret_for_loop,
            webhook_set_for_loop,
            shutdown_for_webhook,
        )
        .await
    });

    // One-time migration: oauth-state.json → SQLite
    migrate_oauth_state_to_db(&agent_dir);

    // Resolve sandbox name once — used throughout the bot lifetime.
    // None when running without sandbox (mode: none).
    let resolved_sandbox: Option<String> = if is_sandboxed {
        Some(right_agent::openshell::resolve_sandbox_name(
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
        let mtls_dir = match right_agent::openshell::preflight_check() {
            right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
            right_agent::openshell::OpenShellStatus::NotInstalled => {
                return Err(miette::miette!(
                    help = "Install from https://github.com/NVIDIA/OpenShell, or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell is not installed"
                ));
            }
            right_agent::openshell::OpenShellStatus::NoGateway(_) => {
                return Err(miette::miette!(
                    help =
                        "Run `openshell gateway start`, or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell gateway is not running"
                ));
            }
            right_agent::openshell::OpenShellStatus::BrokenGateway(dir) => {
                return Err(miette::miette!(
                    help = "Try `openshell gateway destroy && openshell gateway start`,\n  \
                            or set `sandbox: mode: none` in agent.yaml",
                    "OpenShell gateway exists but mTLS certificates are missing at {}",
                    dir.display()
                ));
            }
        };

        // Check if sandbox already exists and is READY.
        let mut grpc_client = right_agent::openshell::connect_grpc(&mtls_dir).await?;
        let sandbox_exists =
            right_agent::openshell::is_sandbox_ready(&mut grpc_client, &sandbox).await?;

        if !sandbox_exists {
            return Err(miette::miette!(
                help = format!(
                    "Run `right init` or `right agent init {}` to create the sandbox",
                    args.agent
                ),
                "Sandbox '{}' not found",
                sandbox
            ));
        }

        // Resolve host IP from inside sandbox for policy allowed_ips.
        let sandbox_id =
            right_agent::openshell::resolve_sandbox_id(&mut grpc_client, &sandbox).await?;
        let host_ip = right_agent::openshell::resolve_host_ip(&mut grpc_client, &sandbox_id).await?;

        // Regenerate policy with resolved host IP and apply.
        let network_policy = config.network_policy;
        let policy_content = right_agent::codegen::policy::generate_policy(
            right_agent::runtime::MCP_HTTP_PORT,
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
            match right_agent::openshell::parse_policy_yaml_filesystem(&policy_content) {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!(agent = %args.agent, "could not parse generated policy.yaml for drift check: {e:#}");
                    None
                }
            };
        let active_filesystem =
            match right_agent::openshell::get_active_policy(&mut grpc_client, &sandbox).await {
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
                right_agent::openshell::filesystem_policy_changed(&active, &desired)
            }
            _ => true,
        };

        if drifted {
            // Still write so a later `right agent config`-triggered
            // migration sees the fresh policy; skip apply to avoid crash-loop.
            right_agent::codegen::contract::write_regenerated(&policy_path, &policy_content)?;
            tracing::warn!(
                agent = %args.agent,
                "Filesystem policy drift detected for '{}'. Landlock rules in the running sandbox do not match policy.yaml. Run `right agent config {}` (accept defaults) to trigger sandbox migration, or `right agent backup {} --sandbox-only` first if you want a recovery point.",
                args.agent, args.agent, args.agent,
            );
        } else {
            tracing::info!(agent = %args.agent, "reusing existing sandbox, applying policy with host_ip={:?}", host_ip);
            right_agent::codegen::contract::write_and_apply_sandbox_policy(
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
            right_agent::openshell::generate_ssh_config(&sandbox, &ssh_config_dir).await?;

        // Clean up stale ControlMaster socket from a SIGKILL'd previous bot.
        // The next ssh call (inbox/outbox mkdir below) implicitly establishes
        // a fresh master via ControlMaster=auto in the config we just wrote.
        let cm_socket =
            right_agent::openshell::control_master_socket_path(&ssh_config_dir, &sandbox);
        let cm_host = right_agent::openshell::ssh_host_for_sandbox(&sandbox);
        right_agent::openshell::clean_stale_control_master(&config_path, &cm_host, &cm_socket)
            .await?;

        tracing::info!(agent = %args.agent, "OpenShell sandbox ready");

        (Some(config_path), Some((mtls_dir, sandbox_id)))
    } else {
        (None, None)
    };

    // Snapshot for shutdown teardown — the originals are moved into run_telegram below.
    let shutdown_ssh_config = ssh_config_path.clone();
    let shutdown_sandbox = resolved_sandbox.clone();

    // Create inbox/outbox inside sandbox for attachment handling.
    // This is also the first ssh -F <config> call, which establishes the
    // ControlMaster (see clean_stale_control_master above + SSH config
    // appended directives in generate_ssh_config).
    if is_sandboxed && let Some(ref cfg_path) = ssh_config_path {
        let ssh_host =
            right_agent::openshell::ssh_host_for_sandbox(resolved_sandbox.as_deref().unwrap());
        right_agent::openshell::ssh_exec(
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
        let sbox = right_agent::sandbox_exec::SandboxExec::new(
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
        // SAFETY: ssh_config_path is Some only when is_sandboxed is true, and
        // resolved_sandbox is always Some when is_sandboxed is true.
        let sandbox = resolved_sandbox.as_deref().unwrap();
        upgrade::run_startup_upgrade(cfg_path, &args.agent, sandbox).await;
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

    // Per-main-session mutex map and per-(chat,thread) bg-request flags.
    // Shared across worker, delivery, and callback handlers.
    let session_locks: crate::telegram::SessionLocks = Arc::new(dashmap::DashMap::new());
    let bg_requests: crate::telegram::BgRequests = Arc::new(dashmap::DashMap::new());

    // Periodic sweeper: drop orphan mutex entries (entries whose only Arc holder
    // is the map itself). Without this, the map grows unboundedly on long-lived
    // agents — every unique session UUID adds an entry forever.
    const SESSION_LOCK_SWEEP_INTERVAL_SECS: u64 = 3600;
    {
        let session_locks = Arc::clone(&session_locks);
        let sweep_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_secs(
                SESSION_LOCK_SWEEP_INTERVAL_SECS,
            ));
            iv.tick().await;
            loop {
                tokio::select! {
                    _ = iv.tick() => {
                        session_locks.retain(|_, arc| Arc::strong_count(arc) > 1);
                    }
                    _ = sweep_shutdown.cancelled() => break,
                }
            }
        });
    }

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
    let delivery_session_locks = Arc::clone(&session_locks);
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
            delivery_session_locks,
        )
        .await;
    });

    // Spawn periodic claude upgrade task (sandbox-only).
    let upgrade_handle = ssh_config_path.as_ref().map(|cfg_path| {
        // SAFETY: ssh_config_path is Some only when is_sandboxed is true, and
        // resolved_sandbox is always Some when is_sandboxed is true.
        let sandbox = resolved_sandbox.clone().unwrap();
        upgrade::spawn_upgrade_task(
            cfg_path.clone(),
            args.agent.clone(),
            sandbox,
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
        let model_path = right_agent::stt::model_cache_path(&home, config.stt.model);
        let transcriber = crate::stt::Transcriber::new(model_path);
        let ffmpeg_available = right_agent::stt::ffmpeg_available();
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
            model_arc,
            shutdown.clone(),
            Arc::clone(&idle_timestamp),
            Arc::clone(&internal_client),
            resolved_sandbox,
            hindsight_wrapper,
            prefetch_cache,
            upgrade_lock,
            stt,
            Arc::clone(&session_locks),
            Arc::clone(&bg_requests),
            update_listener,
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

    // Without this, the master ssh process outlives the bot and only gets
    // cleaned up by `clean_stale_control_master` on the next start.
    if let (Some(cfg_path), Some(sandbox_name)) = (shutdown_ssh_config, shutdown_sandbox) {
        let ssh_config_dir = home.join("run").join("ssh");
        let socket =
            right_agent::openshell::control_master_socket_path(&ssh_config_dir, &sandbox_name);
        let host = right_agent::openshell::ssh_host_for_sandbox(&sandbox_name);
        right_agent::openshell::tear_down_control_master(&cfg_path, &host, &socket).await;
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

/// Memory drain loop. Periodically flushes `pending_retains` to Hindsight,
/// skipping ticks when the resilient wrapper is in a non-Healthy state.
///
/// Holds `&rusqlite::Connection` across `.await`, so the returned future is
/// `!Send` and must be driven from a `LocalSet`. Honours `shutdown` so the
/// loop exits cleanly before the runtime starts tearing down its time driver.
async fn run_drain_loop(
    wrapper: std::sync::Arc<right_agent::memory::ResilientHindsight>,
    agent_db: std::path::PathBuf,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await; // first tick is immediate

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.cancelled() => return,
        }
        if !matches!(
            wrapper.status(),
            right_agent::memory::MemoryStatus::Healthy
        ) {
            continue;
        }
        let conn = match right_agent::memory::open_connection(&agent_db, false) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("drain: open_connection failed: {e:#}");
                continue;
            }
        };
        let w_call = wrapper.clone();
        let report = right_agent::memory::retain_queue::drain_tick(&conn, |items| {
            let w = w_call.clone();
            async move {
                let item = right_agent::memory::hindsight::RetainItem {
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

    let conn = match right_agent::memory::open_connection(agent_dir, false) {
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

            if let Err(e) = right_agent::mcp::credentials::db_set_oauth_state(
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

#[cfg(test)]
mod tests {
    //! Regression test for the drain-loop shutdown pattern.
    //!
    //! Before the fix, the drain task ran a `tokio::time::interval` inside
    //! `Handle::block_on(LocalSet::run_until(...))` on a `spawn_blocking`
    //! thread with no shutdown branch. When `run_async` returned an early
    //! `Err` (e.g. sandbox-not-found), the runtime began to drop, the time
    //! driver shut down, and the still-polling `interval.tick()` tripped
    //! `RUNTIME_SHUTTING_DOWN_ERROR` in tokio.
    //!
    //! The fix wraps the tick in `tokio::select!` against
    //! `shutdown.cancelled()`, and a `DropGuard` in `run_async` cancels the
    //! token on every exit path. This test verifies the structural pattern:
    //! a cancellation must cause the blocking task to return cleanly.
    //! Without the `select!` branch, the loop would never exit and the test
    //! would hang to timeout.
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_loop_pattern_exits_on_shutdown() {
        let shutdown = CancellationToken::new();
        let s = shutdown.clone();

        let handle = tokio::task::spawn_blocking(move || {
            let local = tokio::task::LocalSet::new();
            let h = tokio::runtime::Handle::current();
            h.block_on(local.run_until(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(20));
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = s.cancelled() => return,
                    }
                }
            }));
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown.cancel();

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("drain loop must exit when shutdown is cancelled")
            .expect("blocking thread must not panic on shutdown");
    }
}
