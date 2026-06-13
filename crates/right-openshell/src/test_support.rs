//! Test-only helpers for consumers that need a live OpenShell sandbox.
//!
//! Gated behind `cfg(all(unix, any(test, feature = "test-support")))`.
//! Consumers outside `right-agent`'s own test binary depend on the
//! `test-support` feature.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::openshell;
use crate::test_cleanup;

/// Process-wide lock for tests that mutate the global environment (PATH,
/// arbitrary env vars). Hold this guard for the entire duration of the
/// mutation to serialize against any other test in the same binary that
/// touches the process environment.
pub static PROCESS_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// RAII guard that prepends a directory to `PATH` and restores the prior
/// value on drop. Tests using this MUST first acquire [`PROCESS_ENV_LOCK`]
/// so concurrent test threads don't race on the global PATH.
pub struct PathGuard(Option<OsString>);

impl PathGuard {
    pub fn prepend(path: &Path) -> Self {
        let old_path = std::env::var_os("PATH");
        let mut new_path = OsString::from(path.as_os_str());
        if let Some(old_path) = &old_path {
            new_path.push(":");
            new_path.push(old_path);
        }
        unsafe {
            std::env::set_var("PATH", new_path);
        }
        Self(old_path)
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

const SANDBOX_READY_TIMEOUT_ENV: &str = "RIGHT_TEST_SANDBOX_READY_TIMEOUT_SECS";
const SANDBOX_SSH_TIMEOUT_ENV: &str = "RIGHT_TEST_SANDBOX_SSH_TIMEOUT_SECS";

/// Minimal fast-startup policy: public `allowed_ips` endpoint on 443, all
/// binaries allowed. Shared by [`TestSandbox::create`] and [`shared_sandbox`].
pub(crate) const MINIMAL_POLICY: &str = "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  outbound:
    endpoints:
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
";

fn timeout_secs_from_env_value(value: Option<&str>, default_secs: u64) -> u64 {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs)
}

fn timeout_secs_from_env(env: &str, default_secs: u64) -> u64 {
    timeout_secs_from_env_value(std::env::var(env).ok().as_deref(), default_secs)
}

pub fn sandbox_ready_timeout_secs(default_secs: u64) -> u64 {
    timeout_secs_from_env(SANDBOX_READY_TIMEOUT_ENV, default_secs)
}

pub fn sandbox_ssh_timeout_secs(default_secs: u64) -> u64 {
    timeout_secs_from_env(SANDBOX_SSH_TIMEOUT_ENV, default_secs)
}

/// Ephemeral test sandbox. Created per test, destroyed on `Drop`. Panic-hook
/// cleanup in `test_cleanup` handles `panic = "abort"` cases.
pub struct TestSandbox {
    name: String,
    mtls_dir: PathBuf,
    _tmp: tempfile::TempDir, // keeps policy file alive
    _slot: openshell::SandboxTestSlot,
    _name_lock: openshell::TestNameLock,
}

impl TestSandbox {
    /// Create an ephemeral sandbox for testing. Cleans up any leftover from
    /// previous runs. The sandbox name is `right-test-<test_name>`. Uses a
    /// minimal fast-startup policy; use [`create_with_policy`] when a test
    /// needs the sandbox to boot with a specific policy (e.g. so a later
    /// `policy set` only changes the network section and OpenShell does not
    /// reject a landlock mismatch).
    ///
    /// [`create_with_policy`]: Self::create_with_policy
    pub async fn create(test_name: &str) -> Self {
        Self::create_with_policy(test_name, MINIMAL_POLICY).await
    }

    /// Like [`create`](Self::create) but boots the sandbox with the given
    /// policy YAML. Landlock/filesystem rules are applied at startup and
    /// cannot be changed on a live sandbox, so a test that hot-applies a
    /// `policy set` afterward must create with a policy whose
    /// filesystem/landlock matches what it will later apply.
    pub async fn create_with_policy(test_name: &str, policy: &str) -> Self {
        let name = format!("right-test-{test_name}");

        // Hold one global sandbox slot for the sandbox lifetime. CI can set the
        // slot limit to 1 to serialize only live sandbox tests, not the whole
        // workspace test runner.
        let slot = openshell::acquire_sandbox_slot();

        // Acquire the per-name lock. Blocks until any other process
        // (including a different worktree's test binary) holding the same
        // name has finished and released. Held for the lifetime of `Self`
        // — released only after Drop completes, by which point the sandbox
        // is already destroyed.
        let name_lock = openshell::acquire_test_name_lock(&name);

        // Belt-and-suspenders cleanup of any orphan processes from a
        // previous SIGKILLed test run that Drop/hook could not handle.
        // Safe under the name lock: we are the unique owner of this name.
        test_cleanup::pkill_test_orphans(&name);

        // Register in the panic-hook registry so abort-on-panic still
        // triggers sandbox cleanup.
        test_cleanup::register_test_sandbox(&name);

        let mtls_dir = match openshell::preflight_check() {
            openshell::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };

        // Clean up leftover from a previous failed run.
        let mut client = openshell::connect_grpc(&mtls_dir).await.unwrap();
        if openshell::sandbox_exists(&mut client, &name).await.unwrap() {
            openshell::delete_sandbox(&name).await;
            openshell::wait_for_deleted(&mut client, &name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }

        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        std::fs::write(&policy_path, policy).unwrap();

        let mut child = openshell::spawn_sandbox(&name, &policy_path, None, &[])
            .expect("failed to spawn sandbox");
        openshell::wait_for_ready(&mut client, &name, sandbox_ready_timeout_secs(120), 2)
            .await
            .expect("sandbox did not become READY");

        // gRPC READY doesn't guarantee SSH transport is accepting connections —
        // the first gRPC `ExecSandbox` after creation can return "Connection
        // reset by peer". `exec_in_sandbox`'s 5-retry loop covers most cases
        // but exhausts when SSH takes > ~7s to come up. Match what
        // `ensure_sandbox` does in production (openshell.rs:1038).
        let sandbox_id = openshell::resolve_sandbox_id(&mut client, &name)
            .await
            .expect("resolve sandbox id");
        openshell::wait_for_ssh(&mut client, &sandbox_id, sandbox_ssh_timeout_secs(60), 2)
            .await
            .expect("SSH transport did not become ready");

        // Kill the create process — it doesn't exit on its own after READY.
        let _ = child.kill().await;

        Self {
            name,
            mtls_dir,
            _tmp: tmp,
            _slot: slot,
            _name_lock: name_lock,
        }
    }

    /// Sandbox name (already prefixed with `right-test-`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Execute a command inside the sandbox via gRPC with the default 10s
    /// timeout. For commands that do network I/O (e.g. `claude upgrade`) use
    /// [`Self::exec_with_timeout`].
    pub async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        self.exec_with_timeout(cmd, openshell::DEFAULT_EXEC_TIMEOUT_SECS)
            .await
    }

    /// Execute a command inside the sandbox with an explicit server-side
    /// timeout (seconds). OpenShell returns exit 124 once the timer expires.
    pub async fn exec_with_timeout(&self, cmd: &[&str], timeout_seconds: u32) -> (String, i32) {
        exec_in_named_sandbox(&self.mtls_dir, &self.name, cmd, timeout_seconds).await
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        test_cleanup::unregister_test_sandbox(&self.name);
        test_cleanup::delete_sandbox_sync(&self.name);
    }
}

/// Per-run identifier shared by every test process of one runner invocation.
/// Under nextest each test is its own process, but they all share one parent
/// (the nextest runner), so `parent_id()` is identical across the run and
/// distinct across invocations. `RIGHT_TEST_RUN_ID` overrides it (CI pins it
/// to the GitHub run id).
fn test_run_id() -> String {
    std::env::var("RIGHT_TEST_RUN_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::os::unix::process::parent_id().to_string())
}

/// Non-owning handle to a long-lived, cross-process shared sandbox.
///
/// Unlike [`TestSandbox`] it does NOT delete the sandbox on drop and does NOT
/// hold the lifetime name lock — so many test processes can attach
/// concurrently (capped only by the nextest test-group). The sandbox persists
/// past the process; a later run with a different run id recreates a fresh one.
pub struct SharedSandboxRef {
    name: String,
    mtls_dir: PathBuf,
}

impl SharedSandboxRef {
    /// Sandbox name (already prefixed with `right-test-shared-`).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        self.exec_with_timeout(cmd, openshell::DEFAULT_EXEC_TIMEOUT_SECS)
            .await
    }

    pub async fn exec_with_timeout(&self, cmd: &[&str], timeout_seconds: u32) -> (String, i32) {
        exec_in_named_sandbox(&self.mtls_dir, &self.name, cmd, timeout_seconds).await
    }
}

/// Boot-once-per-run, reuse-across-processes shared sandbox for tests that need
/// a generic working sandbox and don't care about its initial state. Each
/// caller MUST use a distinct sandbox-side path to avoid stepping on peers.
///
/// Safety: the coordination lock is advisory and held ONLY across this
/// create-or-attach block (kernel releases it on process death). The sandbox
/// name is run-scoped (`right-test-shared-<label>-<runid>`), so concurrent
/// runs/worktrees never block on each other's lock or delete each other's live
/// sandbox. Attach is gated on liveness (`exists && ready`), never on mere
/// existence. The sandbox slot is held only during boot.
pub async fn shared_sandbox(label: &str) -> SharedSandboxRef {
    let runid = test_run_id();
    let name = format!("right-test-shared-{label}-{runid}");

    // Coordination lock — released when `_create_lock` drops on return.
    let _create_lock = openshell::acquire_test_name_lock(&format!("shared-create-{name}"));

    let mtls_dir = match openshell::preflight_check() {
        openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };
    let mut client = openshell::connect_grpc(&mtls_dir).await.unwrap();

    // Attach to a live, healthy shared sandbox booted earlier in this run.
    if openshell::sandbox_exists(&mut client, &name).await.unwrap()
        && openshell::is_sandbox_ready(&mut client, &name)
            .await
            .unwrap()
    {
        let id = openshell::resolve_sandbox_id(&mut client, &name)
            .await
            .unwrap();
        openshell::wait_for_ssh(&mut client, &id, sandbox_ssh_timeout_secs(60), 2)
            .await
            .expect("shared sandbox SSH not ready");
        return SharedSandboxRef { name, mtls_dir };
    }

    // Stale / half-booted leftover (crashed boot, or a prior run that reused
    // this run id): delete before recreating.
    if openshell::sandbox_exists(&mut client, &name).await.unwrap() {
        openshell::delete_sandbox(&name).await;
        openshell::wait_for_deleted(&mut client, &name, 60, 2)
            .await
            .expect("cleanup of stale shared sandbox failed");
    }

    // Boot once. Hold a sandbox slot ONLY during creation so the long-lived
    // idle shared sandbox doesn't permanently consume a concurrency slot.
    let slot = openshell::acquire_sandbox_slot();
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, MINIMAL_POLICY).unwrap();

    let mut child = openshell::spawn_sandbox(&name, &policy_path, None, &[])
        .expect("failed to spawn shared sandbox");
    openshell::wait_for_ready(&mut client, &name, sandbox_ready_timeout_secs(120), 2)
        .await
        .expect("shared sandbox did not become READY");
    let id = openshell::resolve_sandbox_id(&mut client, &name)
        .await
        .unwrap();
    openshell::wait_for_ssh(&mut client, &id, sandbox_ssh_timeout_secs(60), 2)
        .await
        .expect("shared sandbox SSH did not become ready");
    let _ = child.kill().await;
    drop(slot);

    SharedSandboxRef { name, mtls_dir }
}

/// Execute a command inside a named sandbox via gRPC. Shared by
/// [`TestSandbox`] and [`SharedSandboxRef`].
pub(crate) async fn exec_in_named_sandbox(
    mtls_dir: &Path,
    name: &str,
    cmd: &[&str],
    timeout_seconds: u32,
) -> (String, i32) {
    let mut client = openshell::connect_grpc(mtls_dir).await.unwrap();
    let id = openshell::resolve_sandbox_id(&mut client, name)
        .await
        .unwrap();
    openshell::exec_in_sandbox(&mut client, &id, cmd, timeout_seconds)
        .await
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::timeout_secs_from_env_value;

    #[test]
    fn timeout_secs_from_env_value_uses_positive_integer_override() {
        assert_eq!(timeout_secs_from_env_value(Some("360"), 120), 360);
    }

    #[test]
    fn timeout_secs_from_env_value_rejects_missing_invalid_and_zero_values() {
        assert_eq!(timeout_secs_from_env_value(None, 120), 120);
        assert_eq!(timeout_secs_from_env_value(Some(""), 120), 120);
        assert_eq!(timeout_secs_from_env_value(Some("abc"), 120), 120);
        assert_eq!(timeout_secs_from_env_value(Some("0"), 120), 120);
    }
}
