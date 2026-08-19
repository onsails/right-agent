//! Shared helpers for the live-microVM assumption probes (`ci_msb_*`).
//!
//! Every helper here is used by at least one probe binary; `dead_code` is
//! allowed because each binary links only the subset it needs.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use microsandbox::sandbox::SandboxStatus;
use microsandbox::{MicrosandboxError, Sandbox, setup};
use tokio::sync::OnceCell;

/// Small arm64-native guest image used by most probes.
pub const ALPINE_IMAGE: &str = "alpine:3";

/// Node image used by the probe that runs a real Claude Code turn.
pub const NODE_IMAGE: &str = "node:22-slim";

/// vCPUs given to a probe VM that is not testing the designed default.
pub const PROBE_CPUS: u8 = 2;

/// Memory (MiB) given to a probe VM that is not testing the designed default.
pub const PROBE_MEMORY_MIB: u32 = 2048;

/// Grace period given to a sandbox to stop before it is killed during cleanup.
pub const CLEANUP_KILL_TIMEOUT: Duration = Duration::from_secs(15);

static RUNTIME_INSTALL: OnceCell<()> = OnceCell::const_new();
static NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Install the SDK's pinned `msb`/`libkrunfw` runtime once per test process.
///
/// `setup::is_installed()` is a file-presence check, so the fast path costs two
/// `stat` calls. The download itself is guarded by a cross-process advisory
/// lock: `cargo nextest` runs each test in its own process (and a second agent
/// may be running its own probes from another worktree), so two concurrent
/// first-run installs into `~/.microsandbox` are otherwise possible.
pub async fn ensure_runtime_installed() -> Result<()> {
    RUNTIME_INSTALL
        .get_or_try_init(|| async {
            if setup::is_installed() {
                return anyhow::Ok(());
            }
            let _lock = acquire_file_lock("rt-msb-runtime-install");
            if setup::is_installed() {
                return anyhow::Ok(());
            }
            setup::install()
                .await
                .context("install pinned microsandbox runtime into ~/.microsandbox")?;
            anyhow::ensure!(
                setup::is_installed(),
                "setup::install() returned Ok but setup::is_installed() is still false"
            );
            anyhow::Ok(())
        })
        .await?;
    Ok(())
}

/// A sandbox name unique across processes, worktrees, and runs.
///
/// `scope` is the owning probe suite (`exec`, `net`), which keeps the two
/// concurrent agents' sandboxes trivially distinguishable. The result matches
/// the upstream charset (`[A-Za-z0-9._-]`, alphanumeric first byte) and stays
/// far below the 128-byte limit.
pub fn unique_sandbox_name(scope: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_millis();
    let seq = NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "rt-msb-{scope}-{pid}-{millis}-{seq}",
        pid = std::process::id()
    )
}

/// An advisory cross-process lock held for as long as the value lives.
pub struct FileLock {
    _file: std::fs::File,
}

/// Take an exclusive advisory lock on `$TMPDIR/<key>.lock`, blocking until free.
///
/// Mirrors `right_openshell::openshell::acquire_test_name_lock`: the lock is
/// held across worktrees and test binaries, and the kernel releases it if the
/// holder dies.
pub fn acquire_file_lock(key: &str) -> FileLock {
    let path = std::env::temp_dir().join(format!("{key}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap_or_else(|err| panic!("open lock file {}: {err:#}", path.display()));
    loop {
        match file.try_lock() {
            Ok(()) => return FileLock { _file: file },
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(std::fs::TryLockError::Error(err)) => {
                panic!("lock {}: {err:#}", path.display())
            }
        }
    }
}

/// Default number of live probe microVMs allowed on one host at a time.
///
/// Two agents run probes concurrently on a single Mac and a probe VM is 2 GiB,
/// so the host-global cap is deliberately small.
pub const DEFAULT_MAX_CONCURRENT_VMS: u8 = 2;

const VM_SLOT_LIMIT_ENV: &str = "RT_MSB_MAX_CONCURRENT_VMS";

/// Host-global cap on concurrently booted probe microVMs.
pub fn max_concurrent_vms() -> u8 {
    std::env::var(VM_SLOT_LIMIT_ENV)
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_VMS)
}

/// Hold one of [`max_concurrent_vms`] host-global microVM slots.
///
/// Blocks until a slot frees. Drop releases it. Acquire this *before* booting a
/// sandbox so no probe (in any worktree) over-subscribes the host's memory.
pub fn acquire_vm_slot() -> FileLock {
    loop {
        for slot in 1..=max_concurrent_vms() {
            let path = std::env::temp_dir().join(format!("rt-msb-vm-slot-{slot}.lock"));
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .unwrap_or_else(|err| panic!("open vm-slot lock {}: {err:#}", path.display()));
            match file.try_lock() {
                Ok(()) => return FileLock { _file: file },
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(err)) => {
                    panic!("vm-slot lock {}: {err:#}", path.display())
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stop (killing if needed) and delete the sandbox named `name`.
///
/// Succeeds when the sandbox does not exist, so it is safe to call twice.
pub async fn destroy_sandbox(name: &str) -> Result<()> {
    let handle = match Sandbox::get(name).await {
        Ok(handle) => handle,
        Err(MicrosandboxError::SandboxNotFound(_)) => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("look up sandbox {name}")),
    };

    if matches!(
        handle.status_snapshot(),
        SandboxStatus::Created
            | SandboxStatus::Starting
            | SandboxStatus::Running
            | SandboxStatus::Draining
            | SandboxStatus::Paused
    ) {
        handle
            .kill_with_timeout(CLEANUP_KILL_TIMEOUT)
            .await
            .with_context(|| format!("kill sandbox {name}"))?;
    }

    match Sandbox::remove(name).await {
        Ok(()) => Ok(()),
        Err(MicrosandboxError::SandboxNotFound(_)) => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove sandbox {name}")),
    }
}

/// Owns a probe sandbox name and deletes the sandbox when dropped.
///
/// Cleanup runs on a dedicated thread with its own current-thread runtime, so
/// it works from inside an async test and from a panicking unwind alike (Cargo
/// forces `panic=unwind` for test targets even though this workspace sets
/// `panic=abort` for `[profile.dev]`).
pub struct SandboxGuard {
    name: String,
    armed: bool,
}

impl SandboxGuard {
    /// Reserve a fresh unique name in `scope` and arm cleanup for it.
    pub fn new(scope: &str) -> Self {
        Self {
            name: unique_sandbox_name(scope),
            armed: true,
        }
    }

    /// Arm cleanup for an already-chosen sandbox name.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            armed: true,
        }
    }

    /// The reserved sandbox name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Delete the sandbox now and disarm the drop-time cleanup.
    pub async fn destroy(mut self) -> Result<()> {
        self.armed = false;
        destroy_sandbox(&self.name).await
    }
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let name = self.name.clone();
        let outcome = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("build cleanup runtime")?;
            runtime.block_on(destroy_sandbox(&name))
        })
        .join();

        let error = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(err)) => format!("{err:#}"),
            Err(_) => "cleanup thread panicked".to_string(),
        };
        // Never mask the original failure by panicking during an unwind, but
        // never let a leaked microVM pass silently either.
        eprintln!(
            "SandboxGuard: failed to remove sandbox {}: {error}",
            self.name
        );
        assert!(
            std::thread::panicking(),
            "SandboxGuard: failed to remove sandbox {}: {error}",
            self.name
        );
    }
}
