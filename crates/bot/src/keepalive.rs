//! Claude health keepalive: periodic stream-json probe to verify agent-facing MCP status.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and strict MCP config,
//! then inspects the `system/init` event for Right MCP connectivity.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);

const HEALTH_PROMPT: &str = "Reply exactly OK. Do not use tools.";

#[allow(dead_code)]
const REPAIR_NOTICE: &str = "Right MCP stale needs-auth cache was repaired. Use current MCP tool availability, not previous disconnected status.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum HealthProbeOutcome {
    Healthy,
    NeedsRepair { status: Option<String> },
    NoInit,
}

fn classify_init_status(status: crate::cc::stream::RightMcpInitStatus) -> HealthProbeOutcome {
    match status {
        crate::cc::stream::RightMcpInitStatus::Connected => HealthProbeOutcome::Healthy,
        crate::cc::stream::RightMcpInitStatus::Unhealthy { status } => {
            HealthProbeOutcome::NeedsRepair { status }
        }
    }
}

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

pub(crate) struct ClaudeHealth {
    agent_name: String,
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    resolved_sandbox: Option<String>,
    #[allow(dead_code)]
    sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    // Used to serialize repair attempts in the next wiring task.
    #[allow(dead_code)]
    repair_lock: tokio::sync::Mutex<()>,
    repair_notice_pending: AtomicBool,
}

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

    // Used to inject a one-shot repair notice into the next agent turn.
    #[allow(dead_code)]
    pub(crate) fn consume_repair_notice(&self) -> Option<&'static str> {
        if self.repair_notice_pending.swap(false, Ordering::AcqRel) {
            Some(REPAIR_NOTICE)
        } else {
            None
        }
    }

    // Used after successful stale needs-auth repair.
    #[allow(dead_code)]
    fn mark_repaired_for_next_turn(&self) {
        self.repair_notice_pending.store(true, Ordering::Release);
    }

    pub(crate) async fn trigger_repair(self: &Arc<Self>, reason: &'static str) {
        tracing::warn!(
            agent = %self.agent_name,
            reason,
            "claude_health: repair deferred to Task 5"
        );
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
    let agent_name = agent_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_owned();
    let health = ClaudeHealth::new(
        agent_name,
        agent_dir,
        ssh_config_path,
        resolved_sandbox,
        None,
    );
    tokio::spawn(async move {
        run_keepalive_loop(health, shutdown).await;
    })
}

async fn run_keepalive_loop(health: Arc<ClaudeHealth>, shutdown: CancellationToken) {
    run_one_health_cycle(Arc::clone(&health), "startup").await;

    let mut interval = tokio::time::interval(DEFAULT_INTERVAL);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => run_one_health_cycle(Arc::clone(&health), "periodic").await,
            _ = shutdown.cancelled() => {
                tracing::debug!("claude_health: shutdown");
                return;
            }
        }
    }
}

async fn run_one_health_cycle(health: Arc<ClaudeHealth>, reason: &'static str) {
    tracing::info!(agent = %health.agent_name, reason, "claude_health: probing");
    match run_health_probe(&health).await {
        Ok(HealthProbeOutcome::Healthy) => {
            tracing::info!(agent = %health.agent_name, reason, "claude_health: ok");
        }
        Ok(HealthProbeOutcome::NeedsRepair { status }) => {
            tracing::warn!(
                agent = %health.agent_name,
                reason,
                right_status = status.as_deref().unwrap_or("missing"),
                "claude_health: right MCP unhealthy; scheduling repair"
            );
            health.trigger_repair(reason).await;
        }
        Ok(HealthProbeOutcome::NoInit) => {
            tracing::warn!(agent = %health.agent_name, reason, "claude_health: no system/init");
        }
        Err(e) => tracing::warn!(agent = %health.agent_name, reason, "claude_health: failed: {e}"),
    }
}

async fn run_health_probe(health: &ClaudeHealth) -> Result<HealthProbeOutcome, String> {
    let mcp_path = crate::cc::invocation::mcp_config_path(
        health.ssh_config_path.as_deref(),
        &health.agent_dir,
    );
    let args = health_probe_invocation(&mcp_path).into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &health.agent_dir,
        health.ssh_config_path.as_deref(),
        health.resolved_sandbox.as_deref(),
    );

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child =
        right_process::ProcessGroupChild::spawn(cmd).map_err(|e| format!("spawn failed: {e:#}"))?;
    let stdout = child
        .stdout()
        .ok_or_else(|| "health probe missing stdout".to_string())?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut init_outcome = HealthProbeOutcome::NoInit;
    let mut killed_for_repair = false;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("stdout read failed: {e:#}"))?
    {
        if let Some(status) = crate::cc::stream::parse_right_mcp_init_status(&line) {
            init_outcome = classify_init_status(status);
            if matches!(init_outcome, HealthProbeOutcome::NeedsRepair { .. }) {
                child.kill().await.ok();
                killed_for_repair = true;
            }
            break;
        }
    }

    if killed_for_repair {
        let _ = child.wait().await;
    } else {
        let status = child
            .wait()
            .await
            .map_err(|e| format!("wait failed: {e:#}"))?;
        if !status.success() {
            return Err(format!("exit code: {}", status.code().unwrap_or(-1)));
        }
    }

    Ok(init_outcome)
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
    fn init_status_decision_maps_only_connected_to_health_probe_outcome() {
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Connected),
            HealthProbeOutcome::Healthy
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy {
                status: Some("needs-auth".to_owned())
            }),
            HealthProbeOutcome::NeedsRepair {
                status: Some("needs-auth".to_owned())
            }
        );
        assert_eq!(
            classify_init_status(crate::cc::stream::RightMcpInitStatus::Unhealthy { status: None }),
            HealthProbeOutcome::NeedsRepair { status: None }
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
