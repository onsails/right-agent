//! Test-only helpers for consumers that need a live OpenShell sandbox.
//!
//! Gated behind `cfg(all(unix, any(test, feature = "test-support")))`.
//! Consumers outside `rightclaw`'s own test binary depend on the
//! `test-support` feature.

use std::path::PathBuf;

use crate::openshell;
use crate::test_cleanup;

/// Ephemeral test sandbox. Created per test, destroyed on `Drop`. Panic-hook
/// cleanup in `test_cleanup` handles `panic = "abort"` cases.
pub struct TestSandbox {
    name: String,
    mtls_dir: PathBuf,
    _tmp: tempfile::TempDir, // keeps policy file alive
}

impl TestSandbox {
    /// Create an ephemeral sandbox for testing. Cleans up any leftover from
    /// previous runs. The sandbox name is `right-test-<test_name>`.
    pub async fn create(test_name: &str) -> Self {
        let name = format!("right-test-{test_name}");

        // Belt-and-suspenders cleanup of any orphan processes from a
        // previous SIGKILLed test run that Drop/hook could not handle.
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

        // Minimal policy — fast startup, permissive network (wildcard 443).
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let policy = "\
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
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
";
        std::fs::write(&policy_path, policy).unwrap();

        let mut child =
            openshell::spawn_sandbox(&name, &policy_path, None).expect("failed to spawn sandbox");
        openshell::wait_for_ready(&mut client, &name, 120, 2)
            .await
            .expect("sandbox did not become READY");

        // Kill the create process — it doesn't exit on its own after READY.
        let _ = child.kill().await;

        Self {
            name,
            mtls_dir,
            _tmp: tmp,
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
        let mut client = openshell::connect_grpc(&self.mtls_dir).await.unwrap();
        let id = openshell::resolve_sandbox_id(&mut client, &self.name)
            .await
            .unwrap();
        openshell::exec_in_sandbox(&mut client, &id, cmd, timeout_seconds)
            .await
            .unwrap()
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        test_cleanup::unregister_test_sandbox(&self.name);
        test_cleanup::delete_sandbox_sync(&self.name);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sandbox_uses_right_test_prefix() {
        // We don't actually call OpenShell — just inspect the format string.
        let computed = format!("right-test-{}", "lifecycle");
        assert_eq!(computed, "right-test-lifecycle");

        // Smoke check: confirm test_support.rs source file was updated.
        // The expected format string in the `create` function body:
        let src = include_str!("test_support.rs");
        let expected_fmt = concat!("right-test-{test", "_name}");
        let old_fmt = concat!("rightclaw-test-{test", "_name}");
        assert!(
            src.contains(expected_fmt),
            "test_support.rs must use 'right-test-' prefix in sandbox name format"
        );
        assert!(
            !src.contains(old_fmt),
            "test_support.rs must not still contain old 'rightclaw-test-' format"
        );
    }
}
