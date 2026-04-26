//! Sandbox command execution via gRPC.

use std::path::PathBuf;

/// Handle for executing commands inside a sandbox via gRPC.
/// Clonable — can be shared across sync tasks.
#[derive(Clone)]
pub struct SandboxExec {
    mtls_dir: PathBuf,
    sandbox_name: String,
    sandbox_id: String,
}

impl SandboxExec {
    pub fn new(mtls_dir: PathBuf, sandbox_name: String, sandbox_id: String) -> Self {
        Self {
            mtls_dir,
            sandbox_name,
            sandbox_id,
        }
    }

    /// Execute a command inside the sandbox via gRPC with the default 10s timeout.
    /// Suitable for cheap shell probes (`test -f`, `mkdir`, etc.).
    /// For network-bound commands use [`Self::exec_with_timeout`].
    pub async fn exec(&self, cmd: &[&str]) -> miette::Result<(String, i32)> {
        self.exec_with_timeout(cmd, crate::openshell::DEFAULT_EXEC_TIMEOUT_SECS)
            .await
    }

    /// Execute a command inside the sandbox with an explicit server-side timeout
    /// (seconds). OpenShell kills the process and returns exit 124 once the
    /// timer expires.
    pub async fn exec_with_timeout(
        &self,
        cmd: &[&str],
        timeout_seconds: u32,
    ) -> miette::Result<(String, i32)> {
        let mut client = crate::openshell::connect_grpc(&self.mtls_dir).await?;
        crate::openshell::exec_in_sandbox(&mut client, &self.sandbox_id, cmd, timeout_seconds).await
    }

    /// Sandbox name for CLI operations (upload_file).
    pub fn sandbox_name(&self) -> &str {
        &self.sandbox_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_exec_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<SandboxExec>();
    }
}
