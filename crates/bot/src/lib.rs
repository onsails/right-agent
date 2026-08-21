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
pub(crate) mod sandbox;
pub(crate) mod sandbox_copy;
pub mod sandbox_runtime;
pub(crate) mod sandbox_supervisor;
mod stt;
pub(crate) mod sync;
pub mod telegram;
mod upgrade;
pub use keepalive::{InitAuthProbe, validate_init_auth};
pub use sandbox::Sandbox;
pub use sandbox_supervisor::agent_sandbox_spec_for;
pub use telegram::tg_bot::validate_telegram_token_live;

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

/// Max time to wait for the bot UDS server (webhook/dashboard/oauth) to drain
/// in-flight requests on shutdown before proceeding with teardown.
const UDS_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Register the Telegram webhook with retry-and-backoff.
///
/// Calls `setWebhook` with the derived URL, secret, and allowed updates.
/// Retries with capped exponential backoff (2s → 60s, jittered) on transient
/// errors. Exits with code 2 on an invalid-token API response (401/404).
/// Cancels on shutdown.
async fn webhook_register_loop(
    bot: telegram::BotType,
    url: url::Url,
    secret: String,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    use std::sync::atomic::Ordering;
    use tokio::time::Duration;

    let allowed = telegram::webhook::webhook_allowed_updates();
    let mut delay = Duration::from_secs(2);

    loop {
        if shutdown.is_cancelled() {
            return;
        }

        match bot
            .set_webhook(url.as_str(), &secret, allowed.clone(), 40)
            .await
        {
            Ok(()) => {
                webhook_set.store(true, Ordering::Relaxed);
                tracing::info!(target: "bot::webhook", url = %url, "webhook registered");
                return;
            }
            Err(e) => {
                if e.is_invalid_token() {
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
/// the webhook handler with graceful shutdown wiring.
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

    let bootstrap_pending = agent_dir.join("BOOTSTRAP.md").exists();
    tracing::info!(
        agent = %args.agent,
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
    match telegram::tg_bot::RightBot::connect(token.clone()).await {
        Ok(probe) => {
            let me = probe.me();
            tracing::info!(
                agent = %args.agent,
                bot_id = me.id,
                bot_username = %me.username.as_deref().unwrap_or(""),
                "bot identity confirmed"
            );
        }
        Err(e) => tracing::warn!(
            agent = %args.agent,
            "getMe failed (non-fatal, bot identity unknown): {e:#}"
        ),
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
    // handler share the same Arc. The watcher writes; the handler reads.
    let model_arc: Arc<arc_swap::ArcSwap<Option<String>>> =
        Arc::new(arc_swap::ArcSwap::from_pointee(config.model.clone()));
    // Provider-only reloads publish accepted YAML here. The supervisor reads a
    // fresh snapshot for every recovery rather than retaining startup config.
    let provider_config: Arc<arc_swap::ArcSwap<right_agent::agent::types::AgentConfig>> =
        Arc::new(arc_swap::ArcSwap::from_pointee(config.clone()));
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

    // Spawn axum bot UDS server and wait for it to bind before registering the webhook
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

    // The webhook router (an `axum::Router`) is produced by `setup_telegram`
    // below, once all handler dependencies exist, then nested onto the bot.sock
    // UDS axum app so cloudflared can POST updates.

    // Deterministic sandbox name, resolved once and used for the bot's life.
    // An explicit `sandbox.name` is fitted into the SDK's name space rather
    // than rejected, so an over-long legacy name still resolves.
    let sandbox_name = match config.sandbox.as_ref().and_then(|s| s.name.as_deref()) {
        Some(explicit) => right_sandbox::fit_sandbox_name(explicit),
        None => right_sandbox::sandbox_name(&args.agent),
    };

    // Provider credentials reach the sandbox as source-ref secrets; the store
    // is the only reader of a stored credential.
    let providers = std::sync::Arc::new(
        right_providers::ProviderStore::open(&home)
            .await
            .map_err(|e| miette::miette!("failed to open the provider store: {e:#}"))?,
    );
    // Serializes config-watcher and dashboard provider operations that mutate
    // provider state or address the sandbox.
    let provider_mutation = Arc::new(tokio::sync::Mutex::new(()));

    // Shared flag for healthz "webhook_set"; flipped by Task 10's register loop.
    let webhook_set_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let menu_bot = telegram::bot::build_bot(token.clone());
    let menu_hostname = global_cfg.tunnel.hostname.clone();
    let menu_agent = args.agent.clone();
    tokio::spawn(async move {
        match telegram::dashboard::dashboard_url(&menu_hostname, &menu_agent) {
            Ok(url) => {
                if let Err(e) = menu_bot
                    .set_chat_menu_button_webapp("Dashboard", url.to_string())
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

    // --- Agent Sandbox lifecycle ---
    //
    // Bring-up runs once here via `sandbox_supervisor::bring_up_sandbox`. On an
    // availability diagnosis the bot starts DEGRADED (logged at ERROR) rather
    // than crashing — the supervisor then auto-recovers. Hard config errors
    // still propagate via `?` and crash startup (they can't self-heal).
    //
    // The supervisor (monitor + recovery) owns the long-lived sync task from
    // here on; startup takes the same authoritative per-agent provider lock
    // used by every later sandbox mutation.
    let _startup_provider_guard = providers
        .agent_lock(&args.agent)
        .await
        .map_err(|error| miette::miette!("failed to lock startup provider reconcile: {error:#}"))?;
    let bring_up_ctx = sandbox_supervisor::BringUpCtx {
        agent: &args.agent,
        agent_dir: &agent_dir,
        sandbox_name: &sandbox_name,
        config: &config,
        providers: &providers,
    };
    let startup_bring_up = sandbox_supervisor::bring_up_sandbox(&bring_up_ctx).await;
    drop(_startup_provider_guard);
    let initial_sandbox = match startup_bring_up? {
        Ok(sandbox_supervisor::SandboxBringUp { sandbox }) => Ok(sandbox),
        Err(diagnosis) => {
            tracing::error!(
                agent = %args.agent,
                cause = ?diagnosis.cause,
                fixes = ?diagnosis.fixes,
                "sandbox unavailable — starting DEGRADED (auto-recovers): {}",
                diagnosis.summary
            );
            Err(std::sync::Arc::new(diagnosis))
        }
    };

    // The runtime handle is the single source of the live sandbox from here
    // on: it is seeded with bring-up's outcome and the supervisor republishes
    // a new handle on every recovery. Startup one-shots below read it once;
    // long-lived tasks re-read it per unit of work.
    let (sandbox_runtime, failure_rx) = sandbox_runtime::SandboxRuntimeHandle::new(initial_sandbox);
    let sandbox = sandbox_runtime.current_sandbox();
    let sync_seed = sandbox.as_ref().map(|sbox| {
        tokio::spawn(sync::run_sync_task(
            agent_dir.clone(),
            std::sync::Arc::clone(sbox),
            Some(std::sync::Arc::clone(&sandbox_runtime)),
            shutdown.clone(),
        ))
    });

    // Spawn the supervisor (monitor + recovery), seeded with the startup sync
    // task it now owns.
    let supervisor_handle = sandbox_supervisor::spawn_supervisor(
        std::sync::Arc::clone(&sandbox_runtime),
        failure_rx,
        telegram::bot::build_bot(token.clone()),
        sandbox_supervisor::SupervisorDeps::new(
            args.agent.clone(),
            agent_dir.clone(),
            sandbox_name.clone(),
            Arc::clone(&provider_config),
            std::sync::Arc::clone(&providers),
            Arc::clone(&provider_mutation),
            shutdown.clone(),
        ),
        sync_seed,
    );

    // Complete or reject any crash-interrupted bootstrap finalization before
    // Telegram dispatch is constructed. Recovery verifies authoritative
    // identity state and fails startup rather than exposing false Normal mode.
    // A degraded backend has no sandbox to probe, so recovery is deferred to
    // the next start rather than guessed at.
    telegram::worker::recover_bootstrap_finalization(&agent_dir, sandbox.as_ref())
        .await
        .map_err(|error| miette::miette!("failed to recover bootstrap finalization: {error:#}"))?;

    // Drain providers-reconcile signals from the config watcher. Existing
    // bindings rotate live; a newly usable binding is added through the SDK's
    // restart-backed modify path without deleting the sandbox.
    {
        // The watcher has already advanced past this change, so there is no
        // automatic retry elsewhere: bound a few in-task attempts to ride out a
        // transient failure. The durable state remains in the provider store,
        // so persistent failure leaves the live sandbox's credentials stale
        // until the next bot restart or another re-save of agent.yaml.
        const HOT_RECONCILE_BACKOFFS_MS: [u64; 2] = [500, 2000];
        let mut providers_rx = providers_rx;
        let agent = args.agent.clone();
        let store = std::sync::Arc::clone(&providers);
        let runtime = std::sync::Arc::clone(&sandbox_runtime);
        let shutdown = shutdown.clone();
        let provider_mutation = Arc::clone(&provider_mutation);
        let provider_config = Arc::clone(&provider_config);
        tokio::spawn(async move {
            loop {
                // Don't start a fresh reconcile once shutdown begins: a queued
                // providers change is superseded by the restart's own bring-up.
                let new_cfg = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    msg = providers_rx.recv() => match msg {
                        Some(c) => c,
                        None => break, // watcher dropped the sender (restart/shutdown)
                    },
                };
                let _mutation = provider_mutation.lock().await;
                let _agent_guard = match store.agent_lock(&agent).await {
                    Ok(guard) => guard,
                    Err(error) => {
                        tracing::error!(error = %format!("{error:#}"),
                            "providers hot-reconcile could not acquire the agent mutation lock");
                        continue;
                    }
                };
                // Capture the previously accepted provider declarations before
                // publishing the new durable truth. Reconcile needs that union
                // to identify bindings removed by this edit without ever
                // considering unrelated sandbox secrets.
                let previous_cfg = provider_config.load_full();
                provider_config.store(Arc::new((*new_cfg).clone()));
                let Some(sandbox) = runtime.current_sandbox() else {
                    tracing::warn!(
                        "providers changed while the sandbox is unavailable; \
                         the next bring-up applies them"
                    );
                    continue;
                };
                tracing::info!("hot-reconciling providers from agent.yaml change");
                let mut attempt = 0usize;
                loop {
                    match sandbox_supervisor::hot_reconcile_providers(
                        &agent,
                        std::slice::from_ref(previous_cfg.as_ref()),
                        &new_cfg,
                        &store,
                        &sandbox,
                    )
                    .await
                    {
                        Ok(()) => break,
                        Err(e) if attempt >= HOT_RECONCILE_BACKOFFS_MS.len() => {
                            tracing::warn!(error = %format!("{e:#}"),
                                "providers hot-reconcile failed after retries; live sandbox credentials may stay stale until next bot restart — obsolete credentials remain a security failure, re-edit sandbox.providers or restart to retry");
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

    // Build the dashboard router AFTER the runtime handle exists so the
    // dashboard resolves the live sandbox per request instead of attaching
    // per request or holding a startup snapshot. Spawn axum here and wait for
    // its UDS bind before continuing with the rest of startup.
    let dashboard_router =
        telegram::dashboard::build_dashboard_router(telegram::dashboard::DashboardState {
            agent_name: args.agent.clone(),
            bot_token: token.clone(),
            focus_notifier: telegram::dashboard::FocusNotifier::telegram(telegram::bot::build_bot(
                token.clone(),
            )),
            home: home.clone(),
            agent_dir: agent_dir.clone(),
            sandbox_name: sandbox_name.clone(),
            sandbox_runtime: std::sync::Arc::clone(&sandbox_runtime),
            allowlist: allowlist.clone(),
            foreground: Arc::clone(&dashboard_foreground),
            internal_client: Arc::clone(&internal_client),
            providers: Some(Arc::clone(&providers)),
            provider_mutation: Arc::clone(&provider_mutation),
            provider_config: Arc::clone(&provider_config),
            pending_auth: Arc::clone(&pending_auth),
            oauth_status: oauth_status.clone(),
            #[cfg(test)]
            mcp_oauth_allow_private_urls: false,
            #[cfg(test)]
            doctor_checks: None,
        });

    // --- Telegram handler dependencies (hoisted above the UDS spawn) ---
    //
    // The webhook router needs a fully-built `HandlerCtx`, which requires the
    // per-session control maps, idle timestamp, keepalive health, and STT
    // context. These are constructed here (once) and the resulting `Arc`s are
    // shared with the cron/delivery/keepalive tasks spawned later.

    let session_locks: crate::telegram::SessionLocks = Arc::new(dashmap::DashMap::new());

    // Shared idle timestamp: tracks last handler/worker interaction for async delivery gating.
    use crate::telegram::handler::IdleTimestamp;
    let idle_timestamp = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
        chrono::Utc::now().timestamp(),
    ))));

    let compact_timers: crate::telegram::CompactTimers = Arc::new(dashmap::DashMap::new());
    let bg_requests: crate::telegram::BgRequests = Arc::new(dashmap::DashMap::new());

    // Upgrade lock: upgrade (write) vs CC sessions (read).
    let upgrade_lock = Arc::new(tokio::sync::RwLock::new(()));

    // Token keepalive health (shared with the keepalive task spawned later).
    let claude_health = keepalive::ClaudeHealth::new(
        args.agent.clone(),
        agent_dir.clone(),
        std::sync::Arc::clone(&sandbox_runtime),
    );

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

    let stop_tokens_for_tg = Arc::clone(&dashboard_foreground);
    let (webhook_router, telegram_lifecycle) = telegram::setup_telegram(
        token.clone(),
        allowlist.clone(),
        config.allowed_chat_ids.clone(),
        agent_dir.clone(),
        Arc::clone(&debug_flag),
        home.clone(),
        config.show_thinking,
        Arc::clone(&model_arc),
        shutdown.clone(),
        Arc::clone(&idle_timestamp),
        Arc::clone(&internal_client),
        hindsight_wrapper.clone(),
        prefetch_cache.clone(),
        Arc::clone(&upgrade_lock),
        stt.clone(),
        Arc::clone(&claude_health),
        std::sync::Arc::clone(&sandbox_runtime),
        Arc::clone(&session_locks),
        Arc::clone(&bg_requests),
        stop_tokens_for_tg,
        progress_state.clone(),
        Arc::clone(&compact_timers),
        webhook_secret.clone(),
    )
    .await?;

    let (axum_ready_tx, axum_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let axum_socket = socket_path.clone();
    let agent_name_for_uds = args.agent.clone();
    let webhook_set_for_axum = webhook_set_flag.clone();
    let progress_state_for_uds = progress_state.clone();
    let dashboard_router_for_uds = dashboard_router;
    // Dedicated drain signal for the UDS server (webhook + dashboard + oauth):
    // fired at shutdown so in-flight requests drain gracefully. Kept separate
    // from the `shutdown` token so cron/sync teardown ordering is unchanged.
    let uds_drain = tokio_util::sync::CancellationToken::new();
    let uds_drain_for_server = uds_drain.clone();
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
            uds_drain_for_server,
            Some(axum_ready_tx),
        )
        .await
    });
    // Wait for axum to bind (ensures callback socket is ready) before registering the webhook
    let _ = axum_ready_rx.await;

    // Periodic attachment cleanup: resolves the live sandbox per sweep.
    telegram::attachments::spawn_cleanup_task(
        agent_dir.clone(),
        std::sync::Arc::clone(&sandbox_runtime),
        config.attachments.retention_days,
    );

    // Startup upgrade: runs before cron/telegram — no lock contention.
    // (`upgrade_lock` is built above, before `setup_telegram`.)
    if let Some(sandbox) = sandbox.as_ref() {
        upgrade::run_startup_upgrade(sandbox, &args.agent).await;
    }

    // CRON-01: spawn cron task alongside the Telegram webhook handler.
    // Cron results are persisted to DB; Telegram delivery is handled separately.
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_model = Arc::clone(&model_arc);
    let cron_internal_client = Arc::clone(&internal_client);
    let cron_shutdown = shutdown.clone();
    let cron_upgrade_lock = Arc::clone(&upgrade_lock);
    let cron_debug = Arc::clone(&debug_flag);
    let cron_learning = config.learning.clone();
    let cron_session_locks = Arc::clone(&session_locks);
    let cron_progress_state = progress_state.clone();
    let cron_sandbox_runtime = std::sync::Arc::clone(&sandbox_runtime);

    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(
            cron_agent_dir,
            cron_agent_name,
            cron_model,
            cron_sandbox_runtime,
            cron_internal_client,
            cron_shutdown,
            cron_upgrade_lock,
            cron_debug,
            cron_learning,
            cron_session_locks,
            cron_progress_state,
        )
        .await;
    });

    // (`idle_timestamp`, `compact_timers`, `bg_requests` are built above, before
    // `setup_telegram`.)

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
    let delivery_sandbox_runtime = std::sync::Arc::clone(&sandbox_runtime);
    let delivery_internal_client = Arc::clone(&internal_client);
    let delivery_shutdown = shutdown.clone();
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
        std::sync::Arc::clone(&delivery_sandbox_runtime),
        Arc::clone(&delivery_internal_client),
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
            delivery_sandbox_runtime,
            delivery_internal_client,
            delivery_shutdown,
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
        let curator_sandbox_runtime = std::sync::Arc::clone(&sandbox_runtime);
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
                    // Resolved per tick: the curator ticker outlives any one
                    // sandbox handle.
                    sandbox: curator_sandbox_runtime.current_sandbox(),
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
                        circuit_failure_threshold: curator_learning
                            .curator_circuit_failure_threshold,
                        circuit_cooldown_hours: curator_learning.curator_circuit_cooldown_hours,
                        mode: curator_learning.curator_mode,
                    },
                };
                let latest_activity = crate::learning_curator::idle_secs_to_activity(
                    curator_idle_ts.0.load(std::sync::atomic::Ordering::Relaxed),
                );
                crate::learning_curator::run_if_due(ctx, latest_activity).await;
            }
        });
    }

    // Spawn periodic claude upgrade task. It resolves the live sandbox per
    // upgrade attempt, so it keeps working across a recovery.
    let upgrade_handle = upgrade::spawn_upgrade_task(
        std::sync::Arc::clone(&sandbox_runtime),
        args.agent.clone(),
        shutdown.clone(),
        Arc::clone(&upgrade_lock),
    );

    // Token keepalive: periodic `claude -p "hi"` to prevent OAuth token
    // expiration. `claude_health` was built above (before `setup_telegram`).
    let keepalive_handle = keepalive::spawn_keepalive(Arc::clone(&claude_health), shutdown.clone());

    // The telegram lifecycle future (built by `setup_telegram`) resolves when
    // the bot shuts down (SIGTERM/SIGINT or config change) after draining
    // foreground background-handoff gates. The webhook router is nested on the
    // UDS app served by `axum_handle`; updates arrive over HTTP.
    //
    // Teardown order matters: cron jobs post through the bot-local UDS
    // (progress/message/channel endpoints), so on normal shutdown we stop the
    // cron reconciler and await its in-flight jobs BEFORE draining the UDS
    // server; only then do we signal `uds_drain` so an in-flight webhook
    // update finishes routing instead of being dropped mid-request.
    let mut axum_handle = axum_handle;
    let mut axum_result: Option<miette::Result<()>> = None;
    tokio::select! {
        () = telegram_lifecycle => {}
        result = &mut axum_handle => {
            axum_result = Some(result.map_err(|e| miette::miette!("axum task panicked: {e:#}"))?);
        }
    }

    // Signal cron/sync tasks to stop and wait for cron BEFORE draining the bot
    // UDS so in-flight cron jobs keep their channel-post endpoint while they
    // finish.
    shutdown.cancel();

    tracing::info!("waiting for cron to finish");
    let _ = cron_handle.await;

    let result: miette::Result<()> = match axum_result {
        Some(result) => result,
        None => {
            uds_drain.cancel();
            match tokio::time::timeout(UDS_DRAIN_TIMEOUT, &mut axum_handle).await {
                Ok(joined) => joined.map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
                Err(_) => {
                    tracing::warn!(
                        "bot UDS server did not drain within {UDS_DRAIN_TIMEOUT:?}; proceeding with shutdown"
                    );
                    Ok(())
                }
            }
        }
    };

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
        flush_sandbox,
        flush_internal_client,
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
            flush_sandbox,
            flush_internal_client,
            flush_upgrade_lock,
            flush_session_locks,
            flush_debug,
        )
        .await;
    }
    // Await the supervisor so its in-flight bring-up/monitor work and the sync
    // task it owns resolve before the runtime drops (same rationale as the
    // keepalive_handle await below).
    tracing::info!("waiting for sandbox supervisor to finish");
    let _ = supervisor_handle.await;
    // Await keepalive/upgrade so their in-flight Interval::tick() futures
    // resolve before the tokio runtime is dropped. Without this, the runtime
    // drop panics: "A Tokio 1.x context was found, but it is being shutdown."
    let _ = keepalive_handle.await;
    let _ = upgrade_handle.await;

    tracing::info!("graceful shutdown complete");

    // Propagate any telegram/axum error first, then signal config restart.
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
