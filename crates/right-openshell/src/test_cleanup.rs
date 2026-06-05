//! Test-only live OpenShell cleanup registry + panic hook.
//!
//! The workspace builds with `panic = "abort"` (see top-level Cargo.toml),
//! meaning stack unwinding is skipped on panic — `Drop` handlers do not run.
//! To still clean up OpenShell resources created by tests that panic, we:
//!
//! 1. Register each created sandbox name in a global `Mutex<Vec<String>>`.
//! 2. Register any live provider/profile names that may hold credentials.
//! 3. On first registration, install a `std::panic::set_hook` that drains
//!    the registries, deletes provider/profile resources via gRPC helpers,
//!    and issues `openshell sandbox delete` for each sandbox entry before
//!    calling the default panic hook (which then aborts).
//! 4. Happy-path `Drop for TestSandbox` calls `unregister_test_sandbox` +
//!    `delete_sandbox_sync`, which removes the entry and issues the same
//!    delete synchronously.
//!
//! Narrow `pkill_test_orphans(name)` is a separate safety net that kills
//! orphan openshell/ssh-proxy processes associated with a specific test
//! sandbox name, run at create-time to clean up leftovers from prior
//! SIGKILLed or externally-terminated runs.

use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
struct ProviderResource {
    provider_name: String,
    profile_id: Option<String>,
    sandbox_name: Option<String>,
}

static LIVE_TEST_SANDBOXES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LIVE_TEST_PROVIDER_RESOURCES: Mutex<Vec<ProviderResource>> = Mutex::new(Vec::new());
static HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

fn ensure_panic_hook_installed() {
    HOOK_INSTALLED.get_or_init(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            cleanup_all_registered();
            default(info);
        }));
    });
}

/// Register a test sandbox. Installs the panic hook on first call.
pub fn register_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .push(name.to_owned());

    ensure_panic_hook_installed();
}

/// Register a live provider/profile resource for abort-mode panic cleanup.
///
/// Provider operations still use the gRPC helper modules; this registry only
/// makes them callable synchronously from the panic hook.
pub fn register_test_provider(provider_name: &str, profile_id: Option<&str>) {
    let mut resources = LIVE_TEST_PROVIDER_RESOURCES
        .lock()
        .expect("registry lock poisoned");
    if let Some(existing) = resources
        .iter_mut()
        .find(|resource| resource.provider_name == provider_name)
    {
        existing.profile_id = profile_id.map(str::to_owned);
    } else {
        resources.push(ProviderResource {
            provider_name: provider_name.to_owned(),
            profile_id: profile_id.map(str::to_owned),
            sandbox_name: None,
        });
    }

    ensure_panic_hook_installed();
}

/// Record that a registered provider is attached to a sandbox, so panic cleanup
/// can detach before deleting the provider.
pub fn register_test_provider_attachment(provider_name: &str, sandbox_name: &str) {
    let mut resources = LIVE_TEST_PROVIDER_RESOURCES
        .lock()
        .expect("registry lock poisoned");
    if let Some(existing) = resources
        .iter_mut()
        .find(|resource| resource.provider_name == provider_name)
    {
        existing.sandbox_name = Some(sandbox_name.to_owned());
    } else {
        resources.push(ProviderResource {
            provider_name: provider_name.to_owned(),
            profile_id: None,
            sandbox_name: Some(sandbox_name.to_owned()),
        });
    }

    ensure_panic_hook_installed();
}

/// Unregister a sandbox (use from Drop — the caller should then invoke
/// `delete_sandbox_sync` to actually remove it).
pub fn unregister_test_sandbox(name: &str) {
    LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .retain(|n| n != name);
}

/// Unregister a provider/profile resource after successful explicit cleanup.
pub fn unregister_test_provider(provider_name: &str) {
    LIVE_TEST_PROVIDER_RESOURCES
        .lock()
        .expect("registry lock poisoned")
        .retain(|resource| resource.provider_name != provider_name);
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
    let provider_resources: Vec<ProviderResource> = LIVE_TEST_PROVIDER_RESOURCES
        .lock()
        .expect("registry lock poisoned")
        .drain(..)
        .collect();
    let names: Vec<String> = LIVE_TEST_SANDBOXES
        .lock()
        .expect("registry lock poisoned")
        .drain(..)
        .collect();

    cleanup_provider_resources_sync(provider_resources);

    for name in names {
        pkill_test_orphans(&name);
        delete_sandbox_sync(&name);
    }
}

fn cleanup_provider_resources_sync(resources: Vec<ProviderResource>) {
    if resources.is_empty() {
        return;
    }
    let _ = std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            let Ok(mut client) =
                crate::openshell::connect_grpc(&crate::openshell::default_mtls_dir()).await
            else {
                return;
            };
            for resource in resources {
                if let Some(sandbox_name) = resource.sandbox_name.as_deref() {
                    let _ = crate::providers::detach_from_sandbox(
                        &mut client,
                        sandbox_name,
                        &resource.provider_name,
                    )
                    .await;
                }
                let _ =
                    crate::providers::delete_provider(&mut client, &resource.provider_name).await;
                if let Some(profile_id) = resource.profile_id.as_deref() {
                    let _ = crate::managed_profiles::delete_profile(&mut client, profile_id).await;
                }
            }
        });
    })
    .join();
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
