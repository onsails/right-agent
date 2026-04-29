//! Register a newly-created agent with a running process-compose.
//!
//! Mirrors [`crate::agent::destroy`]: probes PC via
//! [`crate::runtime::PcClient::from_home`] (which enforces `--home` isolation),
//! regenerates cross-agent codegen, and reloads PC's configuration so the new
//! agent's bot process appears live.

use std::path::Path;

/// Inputs for [`register_with_running_pc`].
pub struct RegisterOptions {
    pub agent_name: String,
    /// True when init wiped a pre-existing agent dir (`--force-recreate` on an
    /// existing agent). Drives the post-reload `restart_process` call.
    pub recreated: bool,
}

/// Outcome of [`register_with_running_pc`].
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterResult {
    /// True if PC was alive and the reload succeeded.
    /// False if PC was not running (no `state.json`, stale port, health-check fail).
    pub pc_running: bool,
}

/// Register a newly-init'd agent with a running PC instance.
///
/// Returns `Ok(RegisterResult { pc_running: false })` if PC isn't running —
/// caller should print `next: right up`. Returns `Err` only if PC was alive but
/// the config reload failed; caller renders a warn row.
pub async fn register_with_running_pc(
    home: &Path,
    options: RegisterOptions,
) -> miette::Result<RegisterResult> {
    // `from_home` enforces --home isolation by reading
    // `<home>/run/state.json` for the PC port + token. Absent or stale state
    // ⇒ no PC ⇒ skip everything. See ARCHITECTURE.md
    // "Runtime isolation — mandatory".
    let Some(client) = crate::runtime::PcClient::from_home(home)? else {
        tracing::debug!(
            home = %home.display(),
            agent = %options.agent_name,
            "no runtime state — PC not running, skipping reload"
        );
        return Ok(RegisterResult { pc_running: false });
    };

    if client.health_check().await.is_err() {
        tracing::debug!(
            agent = %options.agent_name,
            "state.json present but PC health-check failed — treating as not running"
        );
        return Ok(RegisterResult { pc_running: false });
    }

    // PC is alive. Implementation continues in subsequent tasks.
    miette::bail!("PC-alive path not yet implemented")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_pc_running_false_when_state_json_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = register_with_running_pc(
            dir.path(),
            RegisterOptions {
                agent_name: "test".to_string(),
                recreated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, RegisterResult { pc_running: false });
    }
}
