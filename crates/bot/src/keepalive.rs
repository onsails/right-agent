//! Token keepalive: periodic minimal `claude -p "hi"` to prevent OAuth token expiration.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and no system prompt,
//! MCP, or structured output — just enough to trigger CC's internal token refresh.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);

const HEALTH_PROMPT: &str = "Reply exactly OK. Do not use tools.";

// Pure probe helpers are wired into keepalive runtime flow in a later task.
#[allow(dead_code)]
const REPAIR_NOTICE: &str = "Right MCP stale needs-auth cache was repaired. Use current MCP tool availability, not previous disconnected status.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum ProbeInitDecision {
    Healthy,
    Repair,
}

#[cfg_attr(not(test), allow(dead_code))]
fn classify_init_status(status: crate::cc::stream::RightMcpInitStatus) -> ProbeInitDecision {
    match status {
        crate::cc::stream::RightMcpInitStatus::Connected => ProbeInitDecision::Healthy,
        crate::cc::stream::RightMcpInitStatus::Unhealthy { .. } => ProbeInitDecision::Repair,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn health_probe_invocation(mcp_config_path: &str) -> crate::cc::invocation::ClaudeInvocation {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path.to_owned()),
        json_schema: None,
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model: Some("haiku".to_owned()),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: vec!["--no-session-persistence".to_owned()],
        prompt: Some(HEALTH_PROMPT.to_owned()),
        debug_flag: None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ClaudeHealth {
    // Stored for the later repair subprocess wiring task.
    #[allow(dead_code)]
    agent_name: String,
    #[allow(dead_code)]
    agent_dir: PathBuf,
    #[allow(dead_code)]
    ssh_config_path: Option<PathBuf>,
    #[allow(dead_code)]
    resolved_sandbox: Option<String>,
    #[allow(dead_code)]
    sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    repair_lock: tokio::sync::Mutex<()>,
    repair_notice_pending: AtomicBool,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ClaudeHealth {
    pub(crate) fn new(
        agent_name: String,
        agent_dir: PathBuf,
        ssh_config_path: Option<PathBuf>,
        resolved_sandbox: Option<String>,
        sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_name,
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
            sandbox_exec,
            repair_lock: tokio::sync::Mutex::new(()),
            repair_notice_pending: AtomicBool::new(false),
        })
    }

    pub(crate) fn consume_repair_notice(&self) -> Option<&'static str> {
        if self.repair_notice_pending.swap(false, Ordering::AcqRel) {
            Some(REPAIR_NOTICE)
        } else {
            None
        }
    }

    fn mark_repaired_for_next_turn(&self) {
        self.repair_notice_pending.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn try_begin_repair_for_test(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.repair_lock.try_lock().ok()
    }
}

/// Spawn the keepalive loop as a background task.
///
/// Returns the `JoinHandle` so the caller can await it during shutdown,
/// preventing a tokio runtime panic from in-flight `Interval::tick()` futures.
pub(crate) fn spawn_keepalive(
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    resolved_sandbox: Option<String>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_keepalive_loop(
            &agent_dir,
            ssh_config_path.as_deref(),
            resolved_sandbox.as_deref(),
            shutdown,
        )
        .await;
    })
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
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(sandbox_name);

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

    let mut child =
        right_process::ProcessGroupChild::spawn(cmd).map_err(|e| format!("spawn failed: {e:#}"))?;
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

    #[test]
    fn health_probe_invocation_uses_haiku_stream_json_and_strict_mcp() {
        let args = health_probe_invocation("/sandbox/mcp.json").into_args();

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"haiku".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/sandbox/mcp.json".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn init_status_decision_maps_only_connected_to_healthy() {
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Connected),
            ProbeInitDecision::Healthy
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy {
                status: Some("needs-auth".to_owned())
            }),
            ProbeInitDecision::Repair
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy { status: None }),
            ProbeInitDecision::Repair
        );
    }

    #[test]
    fn repair_notice_is_one_shot() {
        let health = ClaudeHealth::new(
            "agent-b".to_owned(),
            PathBuf::from("/tmp/agent"),
            None,
            None,
            None,
        );

        assert_eq!(health.consume_repair_notice(), None);
        health.mark_repaired_for_next_turn();
        assert_eq!(health.consume_repair_notice(), Some(REPAIR_NOTICE));
        assert_eq!(health.consume_repair_notice(), None);
    }

    #[test]
    fn repair_lock_rejects_concurrent_second_holder() {
        let health = ClaudeHealth::new(
            "agent-b".to_owned(),
            PathBuf::from("/tmp/agent"),
            None,
            None,
            None,
        );

        let first = health.try_begin_repair_for_test();
        assert!(first.is_some());
        assert!(health.try_begin_repair_for_test().is_none());
        drop(first);
        assert!(health.try_begin_repair_for_test().is_some());
    }
}
