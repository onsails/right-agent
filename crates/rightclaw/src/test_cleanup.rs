#![cfg(unix)]
//! Test-only sandbox cleanup registry + panic hook.
//!
//! The workspace builds with `panic = "abort"` (see top-level Cargo.toml),
//! meaning stack unwinding is skipped on panic — `Drop` handlers do not run.
//! To still clean up OpenShell sandboxes created by tests that panic, we:
//!
//! 1. Register each created sandbox name in a global `Mutex<Vec<String>>`.
//! 2. On first registration, install a `std::panic::set_hook` that drains
//!    the registry and issues `openshell sandbox delete` for each entry
//!    before calling the default panic hook (which then aborts).
//! 3. Happy-path `Drop for TestSandbox` calls `unregister_test_sandbox` +
//!    `delete_sandbox_sync`, which removes the entry and issues the same
//!    delete synchronously.
//!
//! Narrow `pkill_test_orphans(name)` is a separate safety net that kills
//! orphan openshell/ssh-proxy processes associated with a specific test
//! sandbox name, run at create-time to clean up leftovers from prior
//! SIGKILLed or externally-terminated runs.

use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

static LIVE_TEST_SANDBOXES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

/// Register a test sandbox. Installs the panic hook on first call.
pub fn register_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .push(name.to_owned());

    HOOK_INSTALLED.get_or_init(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            cleanup_all_registered();
            default(info);
        }));
    });
}

/// Unregister a sandbox (use from Drop — the caller should then invoke
/// `delete_sandbox_sync` to actually remove it).
pub fn unregister_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .retain(|n| n != name);
}

/// Synchronously delete a sandbox via `openshell sandbox delete`. Safe to
/// call from `Drop` and from a panic hook (no tokio/async required).
pub fn delete_sandbox_sync(name: &str) {
    let _ = std::process::Command::new("openshell")
        .args(["sandbox", "delete", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Called from the panic hook: drains the registry and synchronously kills
/// orphan processes + deletes each sandbox.
fn cleanup_all_registered() {
    let names: Vec<String> = LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .drain(..)
        .collect();

    for name in names {
        pkill_test_orphans(&name);
        delete_sandbox_sync(&name);
    }
}

/// Narrow `pkill -9 -f` for a specific test sandbox. Kills only processes
/// whose argv matches one of three OpenShell patterns scoped to this
/// sandbox name. Never matches broad patterns like bare "openshell".
pub fn pkill_test_orphans(sandbox_name: &str) {
    let patterns = [
        format!("openshell sandbox create --name {sandbox_name}"),
        format!("openshell sandbox upload {sandbox_name}"),
        format!("openshell ssh-proxy.*sandbox-id.*{sandbox_name}"),
    ];

    for pattern in &patterns {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
