//! Sandbox command execution via gRPC.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, OnceCell};
use tonic::transport::Channel;

use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// Handle for executing commands inside a sandbox via gRPC.
/// Clonable — can be shared across sync tasks.
#[derive(Clone)]
pub struct SandboxExec {
    mtls_dir: PathBuf,
    sandbox_name: String,
    sandbox_id: String,
    /// Lazily-initialized gRPC client, shared across clones via `Arc`.
    /// One mTLS handshake per `SandboxExec` family — subsequent execs reuse
    /// the underlying `Channel` (tonic's `Channel` is internally `Arc`-shared
    /// and handles reconnects transparently). The `Mutex` serializes the
    /// `&mut OpenShellClient` requirement of [`crate::openshell::exec_in_sandbox`].
    client: Arc<OnceCell<Mutex<OpenShellClient<Channel>>>>,
}

impl SandboxExec {
    pub fn new(mtls_dir: PathBuf, sandbox_name: String, sandbox_id: String) -> Self {
        Self {
            mtls_dir,
            sandbox_name,
            sandbox_id,
            client: Arc::new(OnceCell::new()),
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
        let mutex = self
            .client
            .get_or_try_init(|| async {
                let client = crate::openshell::connect_grpc(&self.mtls_dir).await?;
                Ok::<_, miette::Report>(Mutex::new(client))
            })
            .await?;
        let mut guard = mutex.lock().await;
        crate::openshell::exec_in_sandbox(&mut *guard, &self.sandbox_id, cmd, timeout_seconds).await
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
