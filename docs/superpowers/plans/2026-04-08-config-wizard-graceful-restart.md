# Config Wizard + Graceful Restart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix config UX so `rightclaw config` launches an interactive wizard (global + agent settings), restructure get/set as separate subcommands, and implement graceful restart when agent.yaml changes — ensuring in-flight message pipelines always complete.

**Architecture:** The bot watches `agent.yaml` with the `notify` crate (debounced 2s). On change, it signals all subsystems via `CancellationToken`: teloxide stops polling, cron stops spawning (running jobs finish), sync stops, workers finish their current pipeline (download → claude -p → reply). When all drain, the process exits 0 and process-compose restarts it with fresh config.

**Tech Stack:** notify 8.0 (file watcher), tokio-util CancellationToken (already in deps), teloxide ShutdownToken, clap (CLI restructure)

---

### Task 1: Restructure `rightclaw config` CLI commands

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:24-37` (ConfigCommands enum)
- Modify: `crates/rightclaw-cli/src/main.rs:170-174` (Commands::Config)
- Modify: `crates/rightclaw-cli/src/main.rs:321-355` (config dispatch match arm)

- [ ] **Step 1: Update ConfigCommands enum**

Replace the current `ConfigCommands` enum with separate Get/Set subcommands, and make `Config` accept an optional subcommand so bare `rightclaw config` works:

```rust
/// Subcommands for `rightclaw config`.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
    /// Read a config value (e.g. tunnel.hostname, agent.right.allowed-chat-ids)
    Get {
        /// Config key
        key: String,
    },
    /// Set a config value
    Set {
        /// Config key
        key: String,
        /// New value
        value: String,
    },
}
```

Update `Commands::Config` to make the subcommand optional:

```rust
    /// Manage RightClaw configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
```

- [ ] **Step 2: Update config dispatch match arm**

Replace the current `Commands::Config` match arm (lines 321-355):

```rust
Commands::Config { command } => match command {
    None => {
        // Bare `rightclaw config` → combined wizard (global + agents)
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
            "Direct set not yet implemented. Use `rightclaw config` for interactive mode."
        ))
    }
},
```

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "refactor: restructure config CLI — bare config launches wizard, separate get/set"
```

---

### Task 2: Add combined wizard (global + agent settings)

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs:386-442` (add combined_setting_menu, update global_setting_menu)

- [ ] **Step 1: Add `combined_setting_menu` function**

Add this function above `global_setting_menu`:

```rust
// ---------------------------------------------------------------------------
// Combined settings menu (public)
// ---------------------------------------------------------------------------

/// Interactive menu that shows both global and per-agent settings.
/// Launched by bare `rightclaw config`.
pub fn combined_setting_menu(home: &Path) -> miette::Result<()> {
    let agents_dir = home.join("agents");
    let agents = rightclaw::agent::discover_agents(&agents_dir).unwrap_or_default();

    loop {
        let config = read_global_config(home)?;

        let tunnel_label = match &config.tunnel {
            Some(t) => format!(
                "Tunnel: {} ({})",
                t.hostname,
                &t.tunnel_uuid[..8.min(t.tunnel_uuid.len())]
            ),
            None => "Tunnel: (not configured)".to_string(),
        };

        let mut options = vec![tunnel_label.clone()];
        let agent_names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        for agent in &agents {
            let label = format!("Agent: {}", agent.name);
            options.push(label);
        }
        options.push("Done".to_string());

        let selection = inquire::Select::new("Settings:", options)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if selection == "Done" {
            break;
        }

        if selection == tunnel_label {
            let tunnel_name = inquire::Text::new("Tunnel name:")
                .with_default("rightclaw")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let result = tunnel_setup(tunnel_name.trim(), None, true)?;
            let new_config = rightclaw::config::GlobalConfig { tunnel: result };
            write_global_config(home, &new_config)?;
            println!("Global config saved.");
            continue;
        }

        // Must be an agent selection
        for name in &agent_names {
            if selection == format!("Agent: {name}") {
                agent_setting_menu(home, Some(name))?;
                break;
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 3: Manual test**

Run: `cargo run --release --bin rightclaw config`
Expected: Shows interactive menu with "Tunnel: ...", "Agent: right", "Done".

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/wizard.rs
git commit -m "feat: combined settings wizard for bare rightclaw config"
```

---

### Task 3: Add `notify` dependency

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/bot/Cargo.toml` (bot deps)

- [ ] **Step 1: Add notify to workspace dependencies**

In root `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
notify = "8.0"
notify-debouncer-mini = "0.6"
```

- [ ] **Step 2: Add notify to bot crate**

In `crates/bot/Cargo.toml`, add to `[dependencies]`:

```toml
notify = { workspace = true }
notify-debouncer-mini = { workspace = true }
```

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles, `notify` crate downloaded and built.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/bot/Cargo.toml
git commit -m "deps: add notify + notify-debouncer-mini for config file watching"
```

---

### Task 4: Add config watcher module

**Files:**
- Create: `crates/bot/src/config_watcher.rs`
- Modify: `crates/bot/src/lib.rs:1` (add `mod config_watcher;`)

- [ ] **Step 1: Create `config_watcher.rs`**

```rust
//! Watch agent.yaml for changes and trigger graceful restart.
//!
//! Uses `notify` with debouncing (2s) to avoid reacting to partial writes.
//! On change detection, cancels the provided `CancellationToken`.

use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Spawn a blocking thread that watches `agent.yaml` for modifications.
///
/// When a change is detected (debounced 2s), the `token` is cancelled,
/// signalling all subsystems to begin graceful shutdown.
pub fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
) -> miette::Result<()> {
    use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
    use std::sync::mpsc;
    use std::time::Duration;

    let watch_dir = agent_yaml
        .parent()
        .ok_or_else(|| miette::miette!("agent.yaml has no parent directory"))?
        .to_path_buf();
    let yaml_filename = agent_yaml
        .file_name()
        .ok_or_else(|| miette::miette!("agent.yaml has no filename"))?
        .to_os_string();

    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_secs(2), tx)
        .map_err(|e| miette::miette!("failed to create file watcher: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| miette::miette!("failed to watch {}: {e:#}", watch_dir.display()))?;

    std::thread::spawn(move || {
        // Move debouncer into thread to keep it alive
        let _debouncer = debouncer;

        for result in rx {
            match result {
                Ok(events) => {
                    let relevant = events.iter().any(|e| {
                        e.kind == DebouncedEventKind::Any
                            && e.path.file_name() == Some(&yaml_filename)
                    });
                    if relevant {
                        tracing::info!("agent.yaml changed — initiating graceful restart");
                        token.cancel();
                        return; // watcher thread exits
                    }
                }
                Err(errs) => {
                    for e in errs {
                        tracing::warn!("file watcher error: {e:#}");
                    }
                }
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 2: Add mod declaration**

In `crates/bot/src/lib.rs`, add near the top with other mod declarations:

```rust
mod config_watcher;
```

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/config_watcher.rs crates/bot/src/lib.rs
git commit -m "feat: add config_watcher module — watches agent.yaml for changes"
```

---

### Task 5: Thread CancellationToken through cron

**Files:**
- Modify: `crates/bot/src/cron.rs:357-375` (run_cron_task signature + loop)
- Modify: `crates/bot/src/lib.rs:147-155` (cron spawn site)

The cron reconciler loop must stop scheduling new jobs when cancelled, but wait for currently executing jobs to finish.

- [ ] **Step 1: Update `run_cron_task` to accept CancellationToken**

Modify the function signature and loop in `crates/bot/src/cron.rs`:

```rust
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    bot: BotType,
    notify_chat_ids: Vec<i64>,
    shutdown: CancellationToken,
) {
    tracing::info!(agent = %agent_name, "cron task started");
    let mut handles: HashMap<String, (CronSpec, JoinHandle<()>)> = HashMap::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.tick().await; // consume immediate first tick

    // Run immediately on startup too
    reconcile_jobs(&mut handles, &agent_dir, &agent_name, &bot, &notify_chat_ids).await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_jobs(&mut handles, &agent_dir, &agent_name, &bot, &notify_chat_ids).await;
            }
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "cron shutdown: stopping reconciler, waiting for running jobs");
                break;
            }
        }
    }

    // Wait for all running job handles to finish (don't abort them).
    for (name, (_, handle)) in handles {
        tracing::info!(job = %name, "cron shutdown: waiting for job to finish");
        let _ = handle.await;
    }
    tracing::info!(agent = %agent_name, "cron shutdown complete — all jobs finished");
}
```

Add the import at the top of `cron.rs`:

```rust
use tokio_util::sync::CancellationToken;
```

- [ ] **Step 2: Update cron spawn site in lib.rs**

In `crates/bot/src/lib.rs`, update the cron spawn (around line 147-155). The `shutdown` token will be created in Task 7, but for now just update the call signature to accept it. Add a placeholder token:

```rust
    // CRON-01: spawn cron task alongside Telegram dispatcher.
    let cron_bot = telegram::bot::build_bot(token.clone());
    let cron_agent_dir = agent_dir.clone();
    let cron_agent_name = args.agent.clone();
    let cron_chat_ids = config.allowed_chat_ids.clone();
    let cron_shutdown = shutdown.clone();
    tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_bot, cron_chat_ids, cron_shutdown).await;
    });
```

Note: `shutdown` will be defined in Task 7 when we wire everything together. For now, to keep the code compiling, add a temporary `let shutdown = CancellationToken::new();` before the cron spawn. We'll remove it in Task 7.

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "feat: cron graceful shutdown — stop scheduling, wait for running jobs"
```

---

### Task 6: Thread CancellationToken through sync

**Files:**
- Modify: `crates/bot/src/sync.rs:17-29` (run_sync_task signature + loop)
- Modify: `crates/bot/src/lib.rs:326-328` (sync spawn site)

- [ ] **Step 1: Update `run_sync_task` to accept CancellationToken**

```rust
use tokio_util::sync::CancellationToken;

/// Run the periodic sync loop (spawned as background task after initial_sync).
pub async fn run_sync_task(agent_dir: PathBuf, sandbox_name: String, shutdown: CancellationToken) {
    let mut tick = interval(SYNC_INTERVAL);
    tick.tick().await; // consume immediate tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                tracing::debug!(sandbox = %sandbox_name, "sync: starting cycle");
                if let Err(e) = sync_cycle(&agent_dir, &sandbox_name).await {
                    tracing::error!(sandbox = %sandbox_name, "sync cycle failed: {e:#}");
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!(sandbox = %sandbox_name, "sync task shutting down");
                break;
            }
        }
    }
}
```

- [ ] **Step 2: Update sync spawn site in lib.rs**

Update the sync spawn (around line 326-328):

```rust
        let sync_shutdown = shutdown.clone();
        tokio::spawn(sync::run_sync_task(sync_agent_dir, sync_sandbox_bg, sync_shutdown));
```

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/sync.rs crates/bot/src/lib.rs
git commit -m "feat: sync task graceful shutdown via CancellationToken"
```

---

### Task 7: Thread CancellationToken through dispatch and wire everything in lib.rs

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:54-175` (accept token, trigger shutdown from both signal and config watcher)
- Modify: `crates/bot/src/lib.rs:137-361` (create token, wire watcher, await all tasks)

This is the main integration task. The `CancellationToken` created in `lib.rs::run()` flows to every subsystem.

- [ ] **Step 1: Update `run_telegram` to accept CancellationToken**

In `crates/bot/src/telegram/dispatch.rs`, update the function signature to add the shutdown token:

```rust
pub async fn run_telegram(
    token: String,
    allowed_chat_ids: Vec<i64>,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    refresh_tx: tokio::sync::mpsc::Sender<rightclaw::mcp::refresh::RefreshMessage>,
    shutdown: CancellationToken,
) -> miette::Result<()> {
```

Add the import at the top:

```rust
use tokio_util::sync::CancellationToken;
```

Replace the signal handler task (lines 128-157) to also listen for the CancellationToken:

```rust
    let shutdown_token = dispatcher.shutdown_token();

    // Shutdown handler: triggered by SIGTERM, SIGINT, or config watcher CancellationToken.
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received -- initiating graceful shutdown");
            }
            result = tokio::signal::ctrl_c() => {
                if result.is_ok() {
                    tracing::info!("SIGINT received -- initiating graceful shutdown");
                }
            }
            _ = shutdown.cancelled() => {
                tracing::info!("config change detected -- initiating graceful shutdown");
            }
        }

        // Shutdown dispatcher: stops accepting new updates.
        // Workers finish their current pipeline (download → claude -p → reply) via drain.
        match shutdown_token.shutdown() {
            Ok(fut) => {
                fut.await;
                tracing::info!("dispatcher stopped");
            }
            Err(_idle) => {
                tracing::debug!("dispatcher was idle at shutdown -- already stopped");
            }
        }
    });
```

- [ ] **Step 2: Wire everything together in lib.rs**

In `crates/bot/src/lib.rs`, this is the main orchestration change. Add the import:

```rust
use tokio_util::sync::CancellationToken;
```

After the `allowed_chat_ids` warning (around line 145), create the shutdown token and config watcher:

```rust
    // Create shared shutdown token for graceful restart on config change.
    let shutdown = CancellationToken::new();

    // Watch agent.yaml — cancel token on change, triggering graceful restart.
    let agent_yaml_path = agent_dir.join("agent.yaml");
    config_watcher::spawn_config_watcher(&agent_yaml_path, shutdown.clone())?;
```

Remove the temporary `let shutdown = CancellationToken::new();` added in Task 5.

Update the cron spawn to pass `shutdown.clone()` (already done in Task 5).

Update the sync spawn to pass `shutdown.clone()` (already done in Task 6).

Store the cron task handle so we can await it during shutdown:

```rust
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_bot, cron_chat_ids, cron_shutdown).await;
    });
```

Store the sync task handle:

```rust
        let sync_handle = tokio::spawn(sync::run_sync_task(sync_agent_dir, sync_sandbox_bg, sync_shutdown));
```

(For no-sandbox mode, set `let sync_handle: Option<JoinHandle<()>> = None;` and wrap the spawn in `Some(...)`)

Update the `run_telegram` call to pass the shutdown token:

```rust
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
```

After the `tokio::select!`, await the background tasks:

```rust
    // Wait for cron to drain (running jobs finish, no new ones spawn).
    tracing::info!("waiting for cron to finish");
    let _ = cron_handle.await;

    // Wait for sync to finish current cycle.
    if let Some(handle) = sync_handle {
        tracing::info!("waiting for sync to finish");
        let _ = handle.await;
    }

    tracing::info!("graceful shutdown complete");
    result
```

- [ ] **Step 3: Build and verify**

Run: `cargo build 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs
git commit -m "feat: wire graceful restart — config watcher + CancellationToken through all subsystems"
```

---

### Task 8: Ensure worker pipeline completes during shutdown

**Files:**
- Review: `crates/bot/src/telegram/worker.rs:246-300` (worker loop)

This task verifies that the existing worker design already handles graceful drain correctly. No code changes should be needed — this is a verification task.

- [ ] **Step 1: Verify worker drain behavior**

The worker loop (worker.rs:259) does:
```rust
loop {
    let Some(first) = rx.recv().await else { break; };
    // ... debounce, download, claude -p, send reply ...
}
```

When teloxide's dispatcher shuts down:
1. Dispatcher stops accepting new updates → no new messages routed to `handle_message`
2. No new `DebounceMsg` sent to worker channels
3. Workers drain existing messages in their channel
4. When channel is empty and all senders are dropped, `rx.recv()` returns `None`
5. Worker breaks out of loop, exits

**Critical invariant:** Once a worker enters the pipeline (after `rx.recv()` returns `Some`), it runs to completion — download attachments, invoke claude -p, send reply. The `CancellationToken` does NOT interrupt this. Only the outer `rx.recv()` respects channel closure.

This is correct by design. No changes needed.

**However**, there is one concern: `kill_on_drop(true)` on the CC subprocess. When the tokio runtime shuts down and drops spawned tasks, in-flight CC processes would be killed. We must ensure `lib.rs` awaits the teloxide dispatcher (which awaits worker drain) before returning.

The current flow is:
1. `dispatcher.dispatch().await` — blocks until all workers drain
2. `run_telegram()` returns
3. `lib.rs::run()` returns

This is correct. The dispatcher doesn't return until all handlers complete. Workers that are mid-pipeline will finish.

- [ ] **Step 2: Verify with a log trace**

Run: `cargo run --release --bin rightclaw up --no-sandbox`
Then in another terminal, edit `~/.rightclaw/agents/right/agent.yaml` (add a comment).
Expected log output:
```
INFO  agent.yaml changed — initiating graceful restart
INFO  config change detected -- initiating graceful shutdown
INFO  dispatcher stopped
INFO  waiting for cron to finish
INFO  cron shutdown: stopping reconciler, waiting for running jobs
INFO  cron shutdown complete — all jobs finished
INFO  graceful shutdown complete
```
Then process-compose should restart the bot automatically.

- [ ] **Step 3: Commit (if any fixes needed)**

No commit expected unless issues found during verification.

---

### Task 9: Clean up unused code from old `config set` dispatch

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (remove dead match arms if any remain)

- [ ] **Step 1: Verify no dead code warnings**

Run: `cargo build 2>&1 | grep warning`
Expected: No warnings related to config dispatch.

- [ ] **Step 2: Commit if cleanup needed**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "refactor: remove dead config dispatch code"
```

---

### Task 10: Integration test — full graceful restart cycle

**Files:**
- No new files — manual integration test

- [ ] **Step 1: Start the bot**

```bash
cargo run --release --bin rightclaw up --no-sandbox
```

- [ ] **Step 2: Send a message to the bot via Telegram**

Verify the message is processed normally (or dropped with allow-list warning if chat ID not set).

- [ ] **Step 3: While a message is being processed, edit agent.yaml**

Add `allowed_chat_ids` with your chat ID (or change model). Verify:
- The in-flight message completes (reply sent)
- Bot logs "agent.yaml changed — initiating graceful restart"
- Bot exits cleanly (exit code 0)
- process-compose restarts the bot
- New config is loaded (check logs for updated allowed_chat_ids)

- [ ] **Step 4: Verify cron drain**

If a cron job is running when config changes, verify it completes before exit.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "feat: config wizard UX + graceful restart on agent.yaml change"
```
