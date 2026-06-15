#![warn(unreachable_pub)]

pub(crate) mod async_delivery;
pub(crate) mod background;
pub(crate) mod cc;
mod config_watcher;
pub(crate) mod cron;
pub(crate) mod idle_compaction;
mod keepalive;
pub(crate) mod learning_curator;
pub(crate) mod learning_pipeline;
pub(crate) mod learning_prefilter;
pub(crate) mod learning_probe_writer;
pub(crate) mod lifecycle;
pub(crate) mod login;
pub(crate) mod reflection;
pub(crate) mod sandbox_copy;
pub mod sandbox_runtime;
pub(crate) mod sandbox_supervisor;
mod stt;
pub(crate) mod sync;
pub mod telegram;
mod upgrade;

use right_agent::agent::allowlist::{self, AllowlistHandle, AllowlistState};

/// Snapshot the current value of an `ArcSwap<Option<String>>` into an owned `Option<String>`.
///
/// The double-deref dance (`(**cell.load()).clone()`) reads the `Guard`, then derefs to the
/// inner `Arc`, then clones the `Option<String>` payload. The intermediate guard and arc are
/// dropped before the return — no lock-like resource is held.
pub(crate) fn snapshot_model(cell: &arc_swap::ArcSwap<Option<String>>) -> Option<String> {
    (**cell.load()).clone()
}

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

/// Backoff delays (ms) between retry attempts on the bot-startup
/// `resolve_host_ips` call. The first attempt is immediate; failures
/// sleep `BACKOFFS_MS[attempt - 1]` before the next attempt. Length of
/// the slice is `attempts - 1`.
pub(crate) const RESOLVE_HOST_IPS_BACKOFFS_MS: &[u64] = &[200, 500, 1000];

/// Drive `attempt_fn` until it succeeds or `backoffs_ms.len() + 1` attempts
/// have failed. Logs a `warn` per failed attempt and a final `error` if all
/// attempts fail. The last error is propagated unchanged — FAIL FAST after
/// retry budget is exhausted.
///
/// Only used at the bot-startup callsite of `resolve_host_ips`: a transient
/// DNS hiccup, sandbox NSS warmup race, or OpenShell-alias rename should
/// not brick the bot. Init/restore/migration callsites in `right` are
/// interactive and intentionally fail fast.
pub(crate) async fn run_with_backoff<T>(
    op_name: &str,
    agent: &str,
    backoffs_ms: &[u64],
    mut attempt_fn: impl AsyncFnMut() -> miette::Result<T>,
) -> miette::Result<T> {
    let max_attempts = backoffs_ms.len() + 1;
    let mut last_err: Option<miette::Report> = None;
    for attempt in 1..=max_attempts {
        match attempt_fn().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                tracing::warn!(
                    agent = %agent,
                    attempt,
                    max_attempts,
                    "{op_name} failed: {e:#}",
                );
                last_err = Some(e);
                if attempt < max_attempts {
                    let delay_ms = backoffs_ms[attempt - 1];
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }
    let err = last_err.expect("loop runs at least once");
    tracing::error!(
        agent = %agent,
        attempts = max_attempts,
        "{op_name} exhausted retries: {err:#}",
    );
    Err(err)
}

async fn run_async(args: BotArgs) -> miette::Result<bool> {
    use right_agent::agent::discovery::{parse_agent_config, validate_agent_name};
    use right_config::resolve_home;
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
        let dir = right_config::agents_dir(&home).join(&args.agent);
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
    let self_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("right"));
    let agent_def = right_agent::agent::discover_single_agent(&agent_dir)?;
    right_codegen::run_single_agent_codegen(&home, &agent_def, &self_exe, args.debug).await?;
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
            debug: None,
            sandbox: None,
            env: Default::default(),
            secret: None,
            attachments: Default::default(),
            network_policy: Default::default(),
            show_thinking: true,
            learning: Default::default(),
            memory: None,
            stt: Default::default(),
        });

    config.learning.warn_on_deprecated(&args.agent);

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
        Option<Arc<right_memory::ResilientHindsight>>,
        Option<right_memory::prefetch::PrefetchCache>,
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
            let client = right_memory::hindsight::HindsightClient::new(
                &api_key,
                &bank_id,
                &budget,
                mem_config.recall_max_tokens,
                None,
            );

            let wrapper = Arc::new(right_memory::ResilientHindsight::new(
                client,
                agent_dir.clone(),
                "bot",
            ));

            match wrapper
                .get_or_create_bank(right_memory::resilient::POLICY_STARTUP_BANK)
                .await
            {
                Ok(profile) => tracing::info!(
                    agent = %args.agent,
                    bank_id = %profile.bank_id,
                    "Hindsight bank ready"
                ),
                Err(right_memory::ResilientError::Upstream(e)) => match e.classify() {
                    right_memory::ErrorKind::Auth => tracing::error!(
                        agent = %args.agent,
                        "Hindsight AUTH failed at startup: {e:#} — booting in degraded mode"
                    ),
                    right_memory::ErrorKind::Quota => tracing::error!(
                        agent = %args.agent,
                        "Hindsight 402 (out of credits) at startup: {e:#} — \
                         booting in QuotaExhausted mode; user must top up at \
                         https://hindsight.vectorize.io"
                    ),
                    right_memory::ErrorKind::Client => tracing::error!(
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
                                        right_memory::resilient::POLICY_STARTUP_BANK,
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
                Err(right_memory::ResilientError::CircuitOpen { .. }) => {
                    tracing::warn!("unexpected CircuitOpen at startup");
                }
            }

            let cache = right_memory::prefetch::PrefetchCache::new();
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
    // `drain_tick` holds `&right_db::Connection` across an `.await`, and
    // the local connection wrapper is not shared between threads -- so the
    // future is `!Send` and cannot be handed
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
    right_codegen::skills::install_builtin_skills(&agent_dir, &memory_provider)?;

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
    let conn = right_db::open_connection(&agent_dir, true)
        .await
        .map_err(|e| miette::miette!("failed to open data.db: {:#}", e))?;
    tracing::info!(agent = %args.agent, "data.db opened");

    let interrupted_handoffs = crate::background::mark_interrupted_handoffs(&conn)
        .await
        .map_err(|e| {
            miette::miette!("failed to recover interrupted background handoffs: {:#}", e)
        })?;
    if interrupted_handoffs > 0 {
        tracing::info!(
            agent = %args.agent,
            count = interrupted_handoffs,
            "recovered interrupted background handoffs"
        );
    }

    drop(conn);

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
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .map_err(|e| miette::miette!("failed to open data.db for MCP check: {e:#}"))?;
        match right_mcp::credentials::db_list_servers(&conn).await {
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

    // Hot-reloadable debug flag. yaml takes precedence; CLI --debug is the fallback.
    let initial_debug = config.debug.unwrap_or(args.debug);
    let debug_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(initial_debug));

    // Create the model swap cell here so both the watcher and the telegram
    // dispatcher share the same Arc. The watcher writes; the dispatcher reads.
    let model_arc: Arc<arc_swap::ArcSwap<Option<String>>> =
        Arc::new(arc_swap::ArcSwap::from_pointee(config.model.clone()));
    let (providers_tx, providers_rx) =
        tokio::sync::mpsc::unbounded_channel::<Box<right_agent::agent::types::AgentConfig>>();
    config_watcher::spawn_config_watcher(
        &agent_yaml_path,
        shutdown.clone(),
        Arc::clone(&config_changed),
        Arc::clone(&model_arc),
        Arc::clone(&debug_flag),
        args.debug,
        providers_tx,
    )?;

    // Build shared OAuth PendingAuth map
    use dashmap::DashMap;
    use std::collections::HashMap;
    use std::sync::Arc;
    use telegram::oauth_callback::{
        OAuthCallbackState, PendingAuthMap, run_bot_uds_server, run_pending_auth_cleanup,
    };

    let pending_auth: PendingAuthMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let oauth_status = telegram::oauth_status::OAuthFlowStatusStore::default();
    let progress_state = telegram::progress::ProgressState::default();
    let dashboard_foreground: telegram::StopTokens = Arc::new(DashMap::new());

    let progress_bot = telegram::bot::build_bot(token.clone());
    let agent_name = args.agent.clone();

    // Internal API client for bot→aggregator IPC (MCP add/remove/set-token)
    let internal_socket = home.join("run/internal.sock");
    let internal_client = Arc::new(right_mcp::internal_client::InternalClient::new(
        internal_socket,
    ));

    let oauth_state = OAuthCallbackState {
        pending_auth: Arc::clone(&pending_auth),
        oauth_status: oauth_status.clone(),
        agent_name: agent_name.clone(),
        bot: progress_bot,
        internal_client: Arc::clone(&internal_client),
    };
    // Spawn cleanup task
    tokio::spawn(run_pending_auth_cleanup(
        Arc::clone(&pending_auth),
        oauth_status.clone(),
    ));

    // Spawn axum bot UDS server and wait for it to bind before starting teloxide
    let socket_path = agent_dir.join("bot.sock");
    let started_at = std::time::Instant::now();

    // Build webhook URL from global tunnel hostname.
    //
    // No trailing slash: axum's `nest("/tg/<agent>", router)` matches
    // `/tg/<agent>` exactly (inner sees `/`) but does NOT rewrite
    // `/tg/<agent>/` to `/`, so a trailing slash here would yield 404.
    // The cloudflared ingress rule is anchored to match this exact path.
    let global_cfg = right_config::read_global_config(&home)?;
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
    let webhook_secret = right_mcp::derive_token(&agent_secret, "tg-webhook")?;

    // Build the webhook listener + router. The listener is consumed by
    // run_telegram → dispatcher.dispatch_with_listener; the router is mounted
    // on the bot.sock UDS axum app so cloudflared can POST updates.
    let (update_listener, _webhook_stop, webhook_router) =
        telegram::webhook::build_webhook_router(webhook_secret.clone(), webhook_url.clone());

    // Resolve sandbox name once — used throughout the bot lifetime.
    // None when running without sandbox (mode: none).
    let resolved_sandbox: Option<String> = if is_sandboxed {
        let explicit_sandbox_name = config.sandbox.as_ref().and_then(|s| s.name.as_deref());
        Some(right_openshell::openshell::resolve_sandbox_name(
            &args.agent,
            explicit_sandbox_name,
        ))
    } else {
        None
    };

    // Drain providers-reconcile signals from the config watcher (no restart path).
    {
        // The watcher has already advanced past this change, so there is no
        // automatic retry elsewhere: bound a few in-task attempts to ride out a
        // transient gateway hiccup during profile ensure, attach/detach, or
        // provider-profile policy reload. The durable state remains in
        // agent.yaml and the gateway profiles, so persistent failure leaves the
        // live sandbox's attachment/composition state stale until the next
        // restart or another re-save of agent.yaml.
        const HOT_RECONCILE_BACKOFFS_MS: [u64; 2] = [500, 2000];
        let mut providers_rx = providers_rx;
        let agent = args.agent.clone();
        let agent_dir = agent_dir.clone();
        let resolved_sandbox = resolved_sandbox.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                // Don't start a fresh reconcile once shutdown begins: a queued
                // providers change is superseded by the restart's own bring_up,
                // which re-ensures profiles, reconciles attachments, and reloads
                // provider-profile composition.
                let new_cfg = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    msg = providers_rx.recv() => match msg {
                        Some(c) => c,
                        None => break, // watcher dropped the sender (restart/shutdown)
                    },
                };
                let Some(sandbox) = resolved_sandbox.as_deref() else {
                    continue; // mode: none — no sandbox to reconcile
                };
                tracing::info!("hot-reconciling providers from agent.yaml change");
                let mut attempt = 0usize;
                loop {
                    match sandbox_supervisor::hot_reconcile_providers(
                        &agent, &agent_dir, sandbox, &new_cfg,
                    )
                    .await
                    {
                        Ok(()) => break,
                        Err(e) if attempt >= HOT_RECONCILE_BACKOFFS_MS.len() => {
                            tracing::warn!(error = %format!("{e:#}"),
                                "providers hot-reconcile failed after retries; live sandbox provider attachments/composition may stay stale until next bot restart — re-edit sandbox.providers or restart to retry");
                            break;
                        }
                        Err(e) => {
                            let backoff = HOT_RECONCILE_BACKOFFS_MS[attempt];
                            tracing::warn!(error = %format!("{e:#}"),
                                "providers hot-reconcile attempt {} failed; retrying in {}ms",
                                attempt + 1, backoff);
                            // Abort the backoff promptly if shutdown begins.
                            tokio::select! {
                                biased;
                                _ = shutdown.cancelled() => return,
                                _ = tokio::time::sleep(std::time::Duration::from_millis(backoff)) => {}
                            }
                            attempt += 1;
                        }
                    }
                }
            }
        });
    }

    // Shared flag for healthz "webhook_set"; flipped by Task 10's register loop.
    let webhook_set_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let menu_bot = telegram::bot::build_bot(token.clone());
    let menu_hostname = global_cfg.tunnel.hostname.clone();
    let menu_agent = args.agent.clone();
    tokio::spawn(async move {
        use teloxide::payloads::SetChatMenuButtonSetters as _;
        use teloxide::prelude::Requester as _;
        use teloxide::types::{MenuButton, WebAppInfo};

        match telegram::dashboard::dashboard_url(&menu_hostname, &menu_agent) {
            Ok(url) => {
                if let Err(e) = menu_bot
                    .set_chat_menu_button()
                    .menu_button(MenuButton::WebApp {
                        text: "Dashboard".to_string(),
                        web_app: WebAppInfo { url },
                    })
                    .await
                {
                    tracing::warn!("set_chat_menu_button failed: {e:#}");
                }
            }
            Err(e) => tracing::warn!("dashboard menu URL invalid: {e:#}"),
        }
    });

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
    migrate_oauth_state_to_db(&agent_dir).await;

    // --- OpenShell sandbox lifecycle (when sandbox mode is active) ---
    //
    // Bring-up runs once here via `sandbox_supervisor::bring_up_sandbox`. On a
    // backend-availability diagnosis the bot starts DEGRADED (logged at ERROR)
    // rather than crashing — the supervisor then auto-recovers. Hard config
    // errors still propagate via `?` and crash startup (they can't self-heal).
    //
    // The supervisor (monitor + recovery) owns the long-lived sync task from
    // here on; on the happy path it is seeded with the startup sync task.
    //
    // `sandbox_runtime` is always present (sandboxed or not). For non-sandboxed
    // agents the gate treats health as irrelevant (always Proceed), so Ready is
    // the correct initial value.
    let sandbox_runtime: Arc<sandbox_runtime::SandboxRuntimeHandle>;
    let (ssh_config_path, health_sandbox_exec, supervisor_handle) = if is_sandboxed {
        // SAFETY: resolved_sandbox is always Some when is_sandboxed is true.
        let resolved = resolved_sandbox.clone().unwrap();

        // Stable, deterministic ssh-config path. Valid whether or not bring-up
        // succeeds: bring-up regenerates the file at exactly this path (same
        // formula), so threaded snapshots keep working across degrade/recovery.
        // Nothing invokes CC while the backend is Unavailable (Task 9 health
        // gate + the structural host-exec backstop), so holding the path while
        // degraded is safe.
        let ssh_config_dir = home.join("run").join("ssh");
        let stable_ssh_config = ssh_config_dir.join(format!("{resolved}.ssh-config"));

        let bring_up_ctx = sandbox_supervisor::BringUpCtx {
            agent: &args.agent,
            home: &home,
            agent_dir: &agent_dir,
            resolved_sandbox: &resolved,
            config: &config,
        };
        let (initial_health, sbox_opt) =
            match sandbox_supervisor::bring_up_sandbox(&bring_up_ctx).await? {
                Ok(sandbox_supervisor::SandboxBringUp {
                    sandbox: sbox,
                    ssh_config_path: generated_ssh_config,
                }) => {
                    // The path bring-up generated MUST equal the stable path
                    // (same formula). We thread `stable_ssh_config` below so
                    // degrade/recovery share one snapshot; assert they agree.
                    debug_assert_eq!(
                        generated_ssh_config, stable_ssh_config,
                        "bring-up ssh-config path diverged from the stable path formula"
                    );
                    (sandbox_runtime::SandboxHealth::Ready, Some(sbox))
                }
                Err(diag) => {
                    tracing::error!(
                        agent = %args.agent,
                        cause = ?diag.cause,
                        fixes = ?diag.fixes,
                        "sandbox backend unavailable — starting DEGRADED (auto-recovers): {}",
                        diag.summary
                    );
                    (
                        sandbox_runtime::SandboxHealth::Unavailable {
                            diagnosis: std::sync::Arc::new(diag),
                        },
                        None,
                    )
                }
            };

        let (handle, failure_rx) = sandbox_runtime::SandboxRuntimeHandle::new(initial_health);
        if let Some(ref sbox) = sbox_opt {
            // Populate handle.current_sandbox() on the Ready path. (new() seeds
            // health from initial_health but leaves the sandbox slot empty.)
            handle.set_ready(sbox.clone());
        }
        let sync_seed = sbox_opt.as_ref().map(|sbox| {
            tokio::spawn(sync::run_sync_task(
                agent_dir.clone(),
                sbox.clone(),
                Some(std::sync::Arc::clone(&handle)),
                shutdown.clone(),
            ))
        });
        sandbox_runtime = handle;

        // Spawn the supervisor (monitor + recovery), seeded with the startup
        // sync task it now owns.
        let deps = sandbox_supervisor::SupervisorDeps::new(
            args.agent.clone(),
            home.clone(),
            agent_dir.clone(),
            resolved.clone(),
            config.clone(),
            shutdown.clone(),
        );
        let supervisor_bot = telegram::bot::build_bot(token.clone());
        let sup = sandbox_supervisor::spawn_supervisor(
            std::sync::Arc::clone(&sandbox_runtime),
            failure_rx,
            supervisor_bot,
            deps,
            sync_seed,
        );

        (Some(stable_ssh_config), sbox_opt, Some(sup))
    } else {
        let (handle, _failure_rx) =
            sandbox_runtime::SandboxRuntimeHandle::new(sandbox_runtime::SandboxHealth::Ready);
        sandbox_runtime = handle;
        (None, None, None)
    };

    // Snapshot for shutdown teardown — the originals are moved into run_telegram below.
    let shutdown_ssh_config = ssh_config_path.clone();
    let shutdown_sandbox = resolved_sandbox.clone();

    // Build the dashboard router AFTER health_sandbox_exec exists so the
    // dashboard can reuse the long-lived SandboxExec instead of opening a
    // fresh gRPC channel per request. Spawn axum here and wait for its UDS
    // bind before continuing with the rest of startup.
    let dashboard_sandbox_exec = health_sandbox_exec.clone();
    let dashboard_router =
        telegram::dashboard::build_dashboard_router(telegram::dashboard::DashboardState {
            agent_name: args.agent.clone(),
            bot_token: token.clone(),
            focus_notifier: telegram::dashboard::FocusNotifier::telegram(telegram::bot::build_bot(
                token.clone(),
            )),
            home: home.clone(),
            agent_dir: agent_dir.clone(),
            resolved_sandbox: resolved_sandbox.clone(),
            sandbox_exec: dashboard_sandbox_exec,
            sandbox_runtime: std::sync::Arc::clone(&sandbox_runtime),
            allowlist: allowlist.clone(),
            foreground: Arc::clone(&dashboard_foreground),
            internal_client: Arc::clone(&internal_client),
            pending_auth: Arc::clone(&pending_auth),
            oauth_status: oauth_status.clone(),
            #[cfg(test)]
            mcp_oauth_allow_private_urls: false,
            #[cfg(test)]
            doctor_checks: None,
        });

    let (axum_ready_tx, axum_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let axum_socket = socket_path.clone();
    let agent_name_for_uds = args.agent.clone();
    let webhook_set_for_axum = webhook_set_flag.clone();
    let progress_state_for_uds = progress_state.clone();
    let dashboard_router_for_uds = dashboard_router;
    let axum_handle = tokio::spawn(async move {
        run_bot_uds_server(
            axum_socket,
            oauth_state,
            progress_state_for_uds,
            dashboard_router_for_uds,
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

    // Per-main-session mutex map and per-(chat,thread) bg-request flags.
    // Shared across worker, delivery, and callback handlers.
    let session_locks: crate::telegram::SessionLocks = Arc::new(dashmap::DashMap::new());

    // CRON-01: spawn cron task alongside Telegram dispatcher.
    // Cron results are persisted to DB; Telegram delivery is handled separately.
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_model = Arc::clone(&model_arc);
    let cron_ssh_config = ssh_config_path.clone();
    let cron_internal_client = Arc::clone(&internal_client);
    let cron_shutdown = shutdown.clone();
    let cron_sandbox = resolved_sandbox.clone();
    let cron_upgrade_lock = Arc::clone(&upgrade_lock);
    let cron_debug = Arc::clone(&debug_flag);
    let cron_learning = config.learning.clone();
    let cron_session_locks = Arc::clone(&session_locks);
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
            cron_debug,
            cron_learning,
            cron_session_locks,
        )
        .await;
    });

    // Shared idle timestamp: tracks last handler/worker interaction for async delivery gating.
    use crate::telegram::handler::IdleTimestamp;
    let idle_timestamp = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
        chrono::Utc::now().timestamp(),
    ))));

    let compact_timers: crate::telegram::CompactTimers = Arc::new(dashmap::DashMap::new());
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

    // Async delivery loop: delivers pending async results through main CC session when idle
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
    let delivery_debug = Arc::clone(&debug_flag);
    let delivery_model = Arc::clone(&model_arc);
    let delivery_flush_args = (
        delivery_agent_dir.clone(),
        delivery_agent_name.clone(),
        Arc::clone(&delivery_model),
        delivery_bot.clone(),
        delivery_allowlist.clone(),
        Arc::clone(&delivery_idle_ts),
        delivery_ssh_config.clone(),
        Arc::clone(&delivery_internal_client),
        delivery_sandbox.clone(),
        Arc::clone(&delivery_upgrade_lock),
        Arc::clone(&delivery_session_locks),
        Arc::clone(&delivery_debug),
    );
    let delivery_handle = tokio::spawn(async move {
        async_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_model,
            delivery_bot,
            delivery_allowlist,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_internal_client,
            delivery_shutdown,
            delivery_sandbox,
            delivery_upgrade_lock,
            delivery_session_locks,
            delivery_debug,
        )
        .await;
    });

    // Curator ticker: periodically apply skill lifecycle transitions and run
    // the LLM consolidation pass (per-agent).
    {
        let curator_agent_dir = agent_dir.clone();
        let curator_agent_db_dir = agent_dir.clone();
        let curator_agent_name = args.agent.clone();
        let curator_ssh_config = ssh_config_path.clone();
        let curator_resolved = resolved_sandbox.clone();
        let curator_learning = config.learning.clone();
        let curator_debug = Arc::clone(&debug_flag);
        let curator_session_locks = Arc::clone(&session_locks);
        let curator_model = Arc::clone(&model_arc);
        let curator_shutdown = shutdown.clone();
        let curator_internal_client = Arc::clone(&internal_client);
        let curator_idle_ts = std::sync::Arc::clone(&idle_timestamp);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = curator_shutdown.cancelled() => return,
                }
                let curator_model_str = curator_learning
                    .curator_model
                    .clone()
                    .or_else(|| (**curator_model.load()).clone())
                    .unwrap_or_default();
                if curator_model_str.is_empty() {
                    continue;
                }
                let ctx = crate::learning_curator::CuratorContext {
                    agent_dir: curator_agent_dir.clone(),
                    agent_db_dir: curator_agent_db_dir.clone(),
                    agent_name: curator_agent_name.clone(),
                    ssh_config_path: curator_ssh_config.clone(),
                    resolved_sandbox: curator_resolved.clone(),
                    internal_client: Arc::clone(&curator_internal_client),
                    model: curator_model_str,
                    debug_flag: Arc::clone(&curator_debug),
                    session_locks: Arc::clone(&curator_session_locks),
                    config: crate::learning_curator::CuratorConfig {
                        enabled: curator_learning.curator_enabled,
                        paused: curator_learning.curator_paused,
                        interval_hours: curator_learning.curator_interval_hours,
                        min_idle_hours: curator_learning.curator_min_idle_hours,
                        min_cooldown_hours: curator_learning.curator_min_cooldown_hours,
                        stale_after_days: curator_learning.curator_stale_after_days,
                        archive_after_days: curator_learning.curator_archive_after_days,
                        cost_spike_k: curator_learning.curator_cost_spike_k,
                        cost_spike_baseline_days: curator_learning.curator_cost_spike_baseline_days,
                        cost_spike_min_floor_usd: curator_learning.curator_cost_spike_min_floor_usd,
                        skill_change_threshold: curator_learning.curator_skill_change_threshold,
                    },
                };
                let latest_activity = crate::learning_curator::idle_secs_to_activity(
                    curator_idle_ts.0.load(std::sync::atomic::Ordering::Relaxed),
                );
                crate::learning_curator::run_if_due(ctx, latest_activity).await;
            }
        });
    }

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
    let claude_health = keepalive::ClaudeHealth::new(
        args.agent.clone(),
        agent_dir.clone(),
        ssh_config_path.clone(),
        resolved_sandbox.clone(),
        health_sandbox_exec,
        Some(std::sync::Arc::clone(&sandbox_runtime)),
    );
    let keepalive_handle = keepalive::spawn_keepalive(Arc::clone(&claude_health), shutdown.clone());

    // Build STT context once at startup — shared across all worker sessions via Arc.
    let stt: Option<Arc<crate::stt::SttContext>> = if config.stt.enabled {
        let model_path = right_stt::model_cache_path(&home, config.stt.model);
        let transcriber = crate::stt::Transcriber::new(model_path);
        let ffmpeg_available = right_stt::ffmpeg_available();
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
            config.allowed_chat_ids.clone(),
            agent_dir,
            Arc::clone(&debug_flag),
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
            Arc::clone(&claude_health),
            std::sync::Arc::clone(&sandbox_runtime),
            Arc::clone(&session_locks),
            Arc::clone(&bg_requests),
            Arc::clone(&dashboard_foreground),
            progress_state,
            Arc::clone(&compact_timers),
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
    tracing::info!("waiting for async delivery to finish");
    let mut delivery_handle = delivery_handle;
    let delivery_loop_finished = wait_for_delivery_loop_shutdown(
        &mut delivery_handle,
        async_delivery::ASYNC_DELIVERY_SHUTDOWN_TIMEOUT,
    )
    .await;
    if delivery_loop_finished {
        tracing::info!("flushing ready async deliveries for shutdown");
    } else {
        tracing::warn!("skipping shutdown async delivery flush because normal loop did not finish");
    }
    let (
        flush_agent_dir,
        flush_agent_name,
        flush_model,
        flush_bot,
        flush_allowlist,
        flush_idle_ts,
        flush_ssh_config,
        flush_internal_client,
        flush_resolved_sandbox,
        flush_upgrade_lock,
        flush_session_locks,
        flush_debug,
    ) = delivery_flush_args;
    if delivery_loop_finished {
        async_delivery::flush_ready_deliveries_for_shutdown(
            flush_agent_dir,
            flush_agent_name,
            flush_model,
            flush_bot,
            flush_allowlist,
            flush_idle_ts,
            flush_ssh_config,
            flush_internal_client,
            flush_resolved_sandbox,
            flush_upgrade_lock,
            flush_session_locks,
            flush_debug,
        )
        .await;
    }
    // Await the supervisor so its in-flight bring-up/monitor work and the sync
    // task it owns resolve before the runtime drops (same rationale as the
    // keepalive_handle await below).
    if let Some(handle) = supervisor_handle {
        tracing::info!("waiting for sandbox supervisor to finish");
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
            right_openshell::openshell::control_master_socket_path(&ssh_config_dir, &sandbox_name);
        let host = right_openshell::openshell::ssh_host_for_sandbox(&sandbox_name);
        right_openshell::openshell::tear_down_control_master(&cfg_path, &host, &socket).await;
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

async fn wait_for_delivery_loop_shutdown(
    delivery_handle: &mut tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) -> bool {
    match tokio::time::timeout(timeout, &mut *delivery_handle).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::warn!("async delivery task panicked: {e}");
            false
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs = timeout.as_secs(),
                "async delivery task did not finish within shutdown deadline; aborting delivery loop"
            );
            delivery_handle.abort();
            let abort_wait = timeout.min(std::time::Duration::from_secs(1));
            match tokio::time::timeout(abort_wait, &mut *delivery_handle).await {
                Err(_) => {
                    tracing::warn!(
                        timeout_secs = abort_wait.as_secs(),
                        "async delivery task did not observe abort within shutdown deadline"
                    );
                    false
                }
                Ok(Ok(())) => false,
                Ok(Err(e)) => {
                    if !e.is_cancelled() {
                        tracing::warn!("async delivery task panicked after abort: {e}");
                    }
                    false
                }
            }
        }
    }
}

/// Memory drain loop. Periodically flushes `pending_retains` to Hindsight,
/// skipping ticks when the resilient wrapper is in a non-Healthy state.
///
/// Holds `&right_db::Connection` across `.await`, so the returned future is
/// `!Send` and must be driven from a `LocalSet`. Honours `shutdown` so the
/// loop exits cleanly before the runtime starts tearing down its time driver.
async fn run_drain_loop(
    wrapper: std::sync::Arc<right_memory::ResilientHindsight>,
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
        if !matches!(wrapper.status(), right_memory::MemoryStatus::Healthy) {
            continue;
        }
        let conn = match right_db::open_connection(&agent_db, false).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("drain: open_connection failed: {e:#}");
                continue;
            }
        };
        let w_call = wrapper.clone();
        let report = right_memory::retain_queue::drain_tick(&conn, |items| {
            let w = w_call.clone();
            async move {
                let item = right_memory::hindsight::RetainItem {
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
        if report.deleted + report.dropped_age + report.dropped_client + report.bumped_attempts > 0
        {
            tracing::debug!(?report, "drain tick");
        }
    }
}

/// Migrate OAuth state from oauth-state.json to SQLite (one-time).
/// Non-fatal — logs warnings and continues on error.
async fn migrate_oauth_state_to_db(agent_dir: &std::path::Path) {
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

    let conn = match right_db::open_connection(agent_dir, false).await {
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
            let oauth_resource = entry
                .get("resource")
                .or_else(|| entry.get("server_url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if let Err(e) = right_mcp::credentials::db_set_oauth_state(
                &conn,
                name,
                "",
                refresh_token,
                token_endpoint,
                client_id,
                client_secret,
                expires_at,
                oauth_resource,
            )
            .await
            {
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

    use super::{RESOLVE_HOST_IPS_BACKOFFS_MS, run_with_backoff};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Helper retries on transient failures and returns the first success.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_succeeds_after_transient_failures() {
        let attempts = AtomicUsize::new(0);
        let result: miette::Result<u32> =
            run_with_backoff("test_op", "test-agent", &[10, 20, 40], async || {
                let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(miette::miette!("transient {n}"))
                } else {
                    Ok(42u32)
                }
            })
            .await;
        assert_eq!(result.expect("must succeed"), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    /// Helper exhausts retry budget and propagates the last error.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_propagates_last_error_after_exhaustion() {
        let attempts = AtomicUsize::new(0);
        let result: miette::Result<u32> =
            run_with_backoff("test_op", "test-agent", &[10, 20, 40], async || {
                let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                Err(miette::miette!("attempt {n} failed"))
            })
            .await;
        let err = result.expect_err("must fail after exhausting retries");
        // Three backoffs == four attempts total.
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        assert!(
            format!("{err:#}").contains("attempt 4 failed"),
            "expected last error message preserved, got: {err:#}",
        );
    }

    /// Production constants stay sane (3 retries, in ms, ascending).
    #[tokio::test]
    async fn resolve_host_ips_backoff_constants() {
        assert_eq!(RESOLVE_HOST_IPS_BACKOFFS_MS, &[200u64, 500, 1000]);
    }

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

    #[tokio::test(flavor = "current_thread")]
    async fn delivery_loop_shutdown_wait_returns_after_timeout() {
        let mut handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let finished =
            super::wait_for_delivery_loop_shutdown(&mut handle, Duration::from_millis(10)).await;

        assert!(!finished);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delivery_loop_shutdown_wait_does_not_await_unresponsive_abort() {
        let mut handle = tokio::spawn(async {
            std::thread::sleep(Duration::from_millis(250));
        });
        let started = std::time::Instant::now();

        let finished =
            super::wait_for_delivery_loop_shutdown(&mut handle, Duration::from_millis(10)).await;

        assert!(!finished);
        assert!(started.elapsed() < Duration::from_millis(150));
    }
}
