//! Token keepalive: periodic minimal `claude -p "hi"` to prevent OAuth token expiration.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and no system prompt,
//! MCP, or structured output — just enough to trigger CC's internal token refresh.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);

/// Spawn the keepalive loop as a background task.
pub fn spawn_keepalive(
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    resolved_sandbox: Option<String>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        run_keepalive_loop(
            &agent_dir,
            ssh_config_path.as_deref(),
            resolved_sandbox.as_deref(),
            shutdown,
        )
        .await;
    });
}

async fn run_keepalive_loop(
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(DEFAULT_INTERVAL);
    // Skip immediate first tick — token is fresh on startup.
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.cancelled() => {
                tracing::debug!("keepalive: shutdown");
                return;
            }
        }

        tracing::info!("keepalive: pinging claude to refresh token");
        match ping_claude(agent_dir, ssh_config_path, resolved_sandbox).await {
            Ok(()) => tracing::info!("keepalive: ok"),
            Err(e) => tracing::warn!("keepalive: failed: {e}"),
        }
    }
}

async fn ping_claude(
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> Result<(), String> {
    let claude_args = "claude -p --model haiku --max-turns 1 --output-format text -- hi";

    let mut cmd = if let Some(ssh_config) = ssh_config_path {
        let sandbox_name = resolved_sandbox
            .ok_or_else(|| "sandbox mode but no resolved sandbox name".to_string())?;
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(sandbox_name);

        let mut script = String::new();
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
        }
        script.push_str(claude_args);

        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(script);
        c
    } else {
        if which::which("claude").is_err() && which::which("claude-bun").is_err() {
            return Err("claude binary not found in PATH".into());
        }

        let mut script = String::new();
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
        }
        script.push_str(claude_args);

        let mut c = tokio::process::Command::new("bash");
        c.arg("-c").arg(script);
        c.current_dir(agent_dir);
        c
    };

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let mut child = rightclaw::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| format!("spawn failed: {e:#}"))?;
    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait failed: {e:#}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("exit code: {}", status.code().unwrap_or(-1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_is_one_hour() {
        assert_eq!(DEFAULT_INTERVAL, Duration::from_secs(3600));
    }
}
