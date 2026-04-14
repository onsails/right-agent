# Upgrade Exclusivity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-enable the `claude upgrade` background task with RwLock-based exclusivity guarantees — upgrade blocks CC sessions, CC sessions block upgrade.

**Architecture:** A single `tokio::sync::RwLock<()>` shared across upgrade (write), worker (read), cron (read), and cron delivery (read). Startup upgrade runs as a blocking call before concurrent tasks exist.

**Tech Stack:** tokio::sync::RwLock, existing bot plumbing (Arc, CancellationToken, dptree DI)

---

### Task 1: Define UpgradeLock type and newtype wrapper

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs:79` (after `IdleTimestamp`)
- Modify: `crates/bot/src/telegram/worker.rs:55-87` (WorkerContext struct)

- [ ] **Step 1: Add UpgradeLock newtype in handler.rs**

In `crates/bot/src/telegram/handler.rs`, after the `IdleTimestamp` newtype (line 79), add:

```rust
/// RwLock gate for claude upgrade exclusivity.
/// Upgrade takes write (exclusive), CC invocations take read (shared).
#[derive(Clone)]
pub struct UpgradeLock(pub Arc<tokio::sync::RwLock<()>>);
```

Add `use std::sync::Arc;` if not already imported (it is — used by `IdleTimestamp`).

- [ ] **Step 2: Add upgrade_lock field to WorkerContext**

In `crates/bot/src/telegram/worker.rs`, add to `WorkerContext` struct after the `internal_client` field (line 86):

```rust
    /// RwLock gate — worker acquires read lock before invoke_cc to block during upgrades.
    pub upgrade_lock: Arc<tokio::sync::RwLock<()>>,
```

Add `use tokio::sync::RwLock;` to the imports at the top of worker.rs (or use the full path).

- [ ] **Step 3: Verify it compiles**

Run: `devenv shell -- cargo check --workspace 2>&1 | head -30`

Expected: Compilation errors in handler.rs where `WorkerContext` is constructed (missing field). This is expected — we fix it in Task 3.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
git commit -m "feat: define UpgradeLock type and add to WorkerContext"
```

---

### Task 2: Re-enable upgrade loop with write lock

**Files:**
- Modify: `crates/bot/src/upgrade.rs`

- [ ] **Step 1: Write test for upgrade lock exclusivity**

In `crates/bot/src/upgrade.rs`, add a test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Verify try_write fails when a read lock is held (simulates active CC session).
    #[tokio::test]
    async fn upgrade_skips_when_sessions_active() {
        let lock = Arc::new(RwLock::new(()));
        let _read_guard = lock.read().await;
        // try_write must fail — a read lock is held
        assert!(lock.try_write().is_err());
    }

    /// Verify try_write succeeds when no locks are held.
    #[tokio::test]
    async fn upgrade_runs_when_idle() {
        let lock = Arc::new(RwLock::new(()));
        assert!(lock.try_write().is_ok());
    }

    /// Verify read().await blocks while write lock is held, then proceeds.
    #[tokio::test]
    async fn sessions_block_during_upgrade() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let lock = Arc::new(RwLock::new(()));
        let write_guard = lock.write().await;
        let blocked = Arc::new(AtomicBool::new(true));
        let blocked_clone = Arc::clone(&blocked);
        let lock_clone = Arc::clone(&lock);

        let handle = tokio::spawn(async move {
            let _read = lock_clone.read().await;
            blocked_clone.store(false, Ordering::SeqCst);
        });

        // Give the spawned task a chance to reach read().await
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(blocked.load(Ordering::SeqCst), "reader should be blocked");

        drop(write_guard);
        handle.await.unwrap();
        assert!(!blocked.load(Ordering::SeqCst), "reader should have proceeded");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw-bot upgrade:: -- --nocapture 2>&1 | tail -20`

Expected: 3 tests pass.

- [ ] **Step 3: Re-enable run_upgrade_loop with try_write**

Replace the disabled `run_upgrade_loop` in `crates/bot/src/upgrade.rs` with:

```rust
/// Spawn a background task that periodically runs `claude upgrade` in the sandbox.
///
/// First tick fires after UPGRADE_INTERVAL (not immediately — startup upgrade
/// already ran as a blocking call). Errors are logged but never propagated.
pub fn spawn_upgrade_task(
    ssh_config_path: PathBuf,
    agent_name: String,
    shutdown: CancellationToken,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) {
    tokio::spawn(async move {
        run_upgrade_loop(&ssh_config_path, &agent_name, shutdown, &upgrade_lock).await;
    });
}

async fn run_upgrade_loop(
    ssh_config_path: &Path,
    agent_name: &str,
    shutdown: CancellationToken,
    upgrade_lock: &tokio::sync::RwLock<()>,
) {
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    let mut interval = tokio::time::interval(UPGRADE_INTERVAL);
    // First tick fires immediately — consume it since startup upgrade already ran.
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.cancelled() => {
                tracing::info!(agent = %agent_name, "upgrade task shutting down");
                return;
            }
        }

        // try_write: skip if any CC session holds a read lock.
        let Ok(_guard) = upgrade_lock.try_write() else {
            tracing::info!(agent = %agent_name, "skipping upgrade — active sessions");
            continue;
        };

        run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
        // _guard dropped here — CC sessions unblock
    }
}
```

Remove `#[allow(dead_code)]` from `UPGRADE_INTERVAL`, `UPGRADE_TIMEOUT_SECS`, `run_upgrade()`, and the old `spawn_upgrade_task` signature.

Add `use std::sync::Arc;` to imports.

- [ ] **Step 4: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | head -20`

Expected: Error in `lib.rs` — `spawn_upgrade_task` now takes 4 args. Fixed in Task 4.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/upgrade.rs
git commit -m "feat: re-enable upgrade loop with RwLock try_write exclusivity"
```

---

### Task 3: Thread UpgradeLock through telegram dispatch

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:61-180`
- Modify: `crates/bot/src/telegram/handler.rs:123-230`

- [ ] **Step 1: Add UpgradeLock parameter to run_telegram**

In `crates/bot/src/telegram/dispatch.rs`, add parameter after `internal_client`:

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
    max_turns: u32,
    max_budget_usd: f64,
    show_thinking: bool,
    model: Option<String>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) -> miette::Result<()> {
```

- [ ] **Step 2: Wrap in newtype and inject into dptree deps**

In the same function, after `let stop_tokens` (line 112), add:

```rust
    let upgrade_lock_arc: Arc<super::handler::UpgradeLock> = Arc::new(super::handler::UpgradeLock(upgrade_lock));
```

Add to `dptree::deps!` (after `Arc::clone(&idle_ts)`):

```rust
            Arc::clone(&upgrade_lock_arc)
```

- [ ] **Step 3: Add UpgradeLock to handle_message parameters**

In `crates/bot/src/telegram/handler.rs`, add parameter to `handle_message` (after `internal_api: Arc<InternalApi>`):

```rust
    upgrade_lock: Arc<UpgradeLock>,
```

- [ ] **Step 4: Pass upgrade_lock into WorkerContext construction**

In `handle_message`, in the `WorkerContext` struct literal (~line 210-228), add after `internal_client`:

```rust
                    upgrade_lock: Arc::clone(&upgrade_lock.0),
```

- [ ] **Step 5: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | head -20`

Expected: Error in `lib.rs` where `run_telegram` is called (missing arg). Fixed in Task 4.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs
git commit -m "feat: thread UpgradeLock through telegram dispatch and handler"
```

---

### Task 4: Wire UpgradeLock in lib.rs — startup upgrade + task spawning

**Files:**
- Modify: `crates/bot/src/lib.rs:374-420`

- [ ] **Step 1: Create UpgradeLock and run blocking startup upgrade**

In `crates/bot/src/lib.rs`, after the sync handle block (~line 358) and the attachment cleanup block (~line 372), before the cron spawn (~line 374), add:

```rust
    // Upgrade lock: upgrade (write) vs CC sessions (read).
    let upgrade_lock = Arc::new(tokio::sync::RwLock::new(()));

    // Startup upgrade: runs before cron/telegram — no lock contention.
    if let Some(ref cfg_path) = ssh_config_path {
        upgrade::run_startup_upgrade(cfg_path, &args.agent).await;
    }
```

- [ ] **Step 2: Add run_startup_upgrade to upgrade.rs**

In `crates/bot/src/upgrade.rs`, add a public function:

```rust
/// Run a single upgrade attempt at startup (blocking).
/// Called before cron/telegram tasks exist — no lock needed.
pub async fn run_startup_upgrade(ssh_config_path: &Path, agent_name: &str) {
    let ssh_host = rightclaw::openshell::ssh_host(agent_name);
    run_upgrade(ssh_config_path, &ssh_host, agent_name).await;
}
```

- [ ] **Step 3: Pass upgrade_lock to cron task**

Change the cron spawn block (~line 381):

```rust
    let cron_upgrade_lock = Arc::clone(&upgrade_lock);
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_model, cron_ssh_config, cron_shutdown, cron_upgrade_lock).await;
    });
```

- [ ] **Step 4: Pass upgrade_lock to cron delivery**

Change the delivery spawn block (~line 400):

```rust
    let delivery_upgrade_lock = Arc::clone(&upgrade_lock);
    let delivery_handle = tokio::spawn(async move {
        cron_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_model,
            delivery_bot,
            delivery_chat_ids,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_shutdown,
            delivery_upgrade_lock,
        ).await;
    });
```

- [ ] **Step 5: Pass upgrade_lock to spawn_upgrade_task**

Change the upgrade spawn block (~line 414):

```rust
    if let Some(ref cfg_path) = ssh_config_path {
        upgrade::spawn_upgrade_task(
            cfg_path.clone(),
            args.agent.clone(),
            shutdown.clone(),
            Arc::clone(&upgrade_lock),
        );
    }
```

- [ ] **Step 6: Pass upgrade_lock to run_telegram**

In the `tokio::select!` block (~line 422), add `upgrade_lock` as the last argument:

```rust
        result = telegram::run_telegram(
            token,
            config.allowed_chat_ids,
            agent_dir,
            args.debug,
            Arc::clone(&pending_auth),
            home.clone(),
            ssh_config_path,
            refresh_tx_for_handler,
            config.max_turns,
            config.max_budget_usd,
            config.show_thinking,
            config.model.clone(),
            shutdown.clone(),
            Arc::clone(&idle_timestamp),
            Arc::clone(&internal_client),
            upgrade_lock,
        ) => result,
```

- [ ] **Step 7: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | head -20`

Expected: Errors in `cron.rs` and `cron_delivery.rs` — new parameter not yet accepted. Fixed in Tasks 5-6.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/upgrade.rs
git commit -m "feat: wire UpgradeLock in lib.rs — startup upgrade + task spawning"
```

---

### Task 5: Add read lock to cron execute_job

**Files:**
- Modify: `crates/bot/src/cron.rs:105-155` (execute_job), `427-460` (run_cron_task), `617-668` (run_job_loop)

- [ ] **Step 1: Add upgrade_lock parameter to run_cron_task**

In `crates/bot/src/cron.rs`, change the signature of `run_cron_task` (~line 427):

```rust
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    shutdown: CancellationToken,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
) {
```

- [ ] **Step 2: Thread upgrade_lock through reconcile_jobs → run_job_loop → execute_job**

Add `upgrade_lock` parameter to `reconcile_jobs`, `run_job_loop`, and `execute_job`. Pass it through the call chain:

In `run_cron_task`, pass to `reconcile_jobs`:

```rust
    reconcile_jobs(&mut handles, &mut triggered_handles, &conn, &agent_dir, &agent_name, &model, &ssh_config_path, &execute_handles, &upgrade_lock);
```

In `reconcile_jobs`, add `upgrade_lock: &std::sync::Arc<tokio::sync::RwLock<()>>` as last parameter. Pass to `run_job_loop` and `execute_job` spawns:

For `run_job_loop` spawn, clone the Arc:
```rust
    let ul = Arc::clone(upgrade_lock);
    // ... in tokio::spawn:
    run_job_loop(name, spec, agent_dir, agent_name, model, ssh_config_path, execute_handles, ul).await;
```

For triggered `execute_job` spawn, clone the Arc:
```rust
    let ul = Arc::clone(upgrade_lock);
    // ... in tokio::spawn:
    execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref(), &ul).await;
```

In `run_job_loop`, add `upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>` parameter. Pass to spawned `execute_job`:

```rust
    let ul = Arc::clone(&upgrade_lock);
    let handle = tokio::spawn(async move {
        execute_job(&jn, &sp, &ad, &an, md.as_deref(), sc.as_deref(), &ul).await;
    });
```

- [ ] **Step 3: Acquire read lock in execute_job**

In `execute_job`, add `upgrade_lock: &tokio::sync::RwLock<()>` as the last parameter. After the lock-check block (~line 120, after `return` on stale lock) and before the lock-file write (~line 122), add:

```rust
    // Block while upgrade is running (upgrade holds write lock).
    let _upgrade_guard = upgrade_lock.read().await;
```

- [ ] **Step 4: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | head -20`

Expected: Error in `cron_delivery.rs` — missing parameter. Fixed in Task 6.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat: add upgrade read lock to cron execute_job"
```

---

### Task 6: Add read lock to cron delivery and worker

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:160-170` (run_delivery_loop), `282-291` (deliver_through_session)
- Modify: `crates/bot/src/telegram/worker.rs:362-365` (before invoke_cc)

- [ ] **Step 1: Add upgrade_lock parameter to run_delivery_loop**

In `crates/bot/src/cron_delivery.rs`, add parameter after `shutdown`:

```rust
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    model: Option<String>,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    shutdown: tokio_util::sync::CancellationToken,
    upgrade_lock: std::sync::Arc<tokio::sync::RwLock<()>>,
) {
```

Thread it to `deliver_through_session` — add `upgrade_lock: &tokio::sync::RwLock<()>` as the last parameter of `deliver_through_session`.

- [ ] **Step 2: Acquire read lock in deliver_through_session**

In `deliver_through_session`, at the top of the function body (after the `notify_chat_ids.is_empty()` check, ~line 296):

```rust
    // Block while upgrade is running.
    let _upgrade_guard = upgrade_lock.read().await;
```

- [ ] **Step 3: Acquire read lock in worker before invoke_cc**

In `crates/bot/src/telegram/worker.rs`, before `invoke_cc` (~line 365):

```rust
            // Block while upgrade is running (upgrade holds write lock).
            let _upgrade_guard = ctx.upgrade_lock.read().await;
```

The guard will be held through `invoke_cc` and dropped when the variable goes out of scope at the end of the loop iteration (after reply processing). This is the desired behavior — it covers the CC subprocess and bootstrap reverse sync.

- [ ] **Step 4: Verify full workspace compiles**

Run: `devenv shell -- cargo check --workspace 2>&1 | tail -10`

Expected: Clean compilation, no errors.

- [ ] **Step 5: Run all existing tests**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -20`

Expected: All tests pass including the 3 new upgrade tests from Task 2.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron_delivery.rs crates/bot/src/telegram/worker.rs
git commit -m "feat: add upgrade read lock to cron delivery and worker"
```

---

### Task 7: Final verification and cleanup

**Files:**
- Review: all modified files

- [ ] **Step 1: Remove dead_code allows from upgrade.rs**

Verify that `#[allow(dead_code)]` on `UPGRADE_INTERVAL`, `UPGRADE_TIMEOUT_SECS`, and `run_upgrade` have been removed (should have been done in Task 2 Step 3). If any remain, remove them.

- [ ] **Step 2: Run clippy**

Run: `devenv shell -- cargo clippy --workspace 2>&1 | tail -20`

Expected: No new warnings related to upgrade changes.

- [ ] **Step 3: Run full test suite**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -30`

Expected: All tests pass.

- [ ] **Step 4: Commit cleanup if needed**

```bash
git add -A
git commit -m "chore: cleanup dead_code allows in upgrade.rs"
```
