//! Claude health keepalive: periodic stream-json probe to verify agent-facing MCP status.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and strict MCP config,
//! then inspects the `system/init` event for Right MCP connectivity.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use std::future::Future;
use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const REPAIR_OPERATION_TIMEOUT: Duration = Duration::from_secs(180);

const HEALTH_PROMPT: &str = "Reply exactly OK. Do not use tools.";

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
        crate::cc::stream::RightMcpInitStatus::Unhealthy {
            status: Some(status),
        } if status == "pending" => HealthProbeOutcome::Healthy,
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
    sandbox_exec: Option<right_openshell::sandbox_exec::SandboxExec>,
    sandbox_runtime: Option<Arc<crate::sandbox_runtime::SandboxRuntimeHandle>>,
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
        sandbox_runtime: Option<Arc<crate::sandbox_runtime::SandboxRuntimeHandle>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_name,
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
            sandbox_exec,
            sandbox_runtime,
            repair_lock: tokio::sync::Mutex::new(()),
            repair_notice_pending: AtomicBool::new(false),
        })
    }

    fn report_backend_failure(&self) {
        // Sandboxed agents only: the supervisor verifies with a real gateway
        // probe before degrading, so reporting liberally is safe.
        if self.resolved_sandbox.is_some()
            && let Some(rt) = &self.sandbox_runtime
        {
            rt.report_suspected_failure();
        }
    }

    // Used to inject a one-shot repair notice into the next agent turn.
    pub(crate) fn consume_repair_notice(&self) -> Option<&'static str> {
        if self.repair_notice_pending.swap(false, Ordering::AcqRel) {
            Some(REPAIR_NOTICE)
        } else {
            None
        }
    }

    // Used after successful stale needs-auth repair.
    fn mark_repaired_for_next_turn(&self) {
        self.repair_notice_pending.store(true, Ordering::Release);
    }

    pub(crate) async fn trigger_repair(self: &Arc<Self>, reason: &'static str) {
        self.trigger_repair_with_timeout(reason, REPAIR_OPERATION_TIMEOUT, |health| async move {
            health.run_repair_body(reason).await;
        })
        .await;
    }

    async fn trigger_repair_with_timeout<MakeRepair, Repair>(
        self: &Arc<Self>,
        reason: &'static str,
        timeout: Duration,
        make_repair: MakeRepair,
    ) where
        MakeRepair: FnOnce(Arc<Self>) -> Repair,
        Repair: Future<Output = ()>,
    {
        let Ok(_guard) = self.repair_lock.try_lock() else {
            tracing::debug!(
                agent = %self.agent_name,
                reason,
                "claude_health: repair already running"
            );
            return;
        };

        tracing::warn!(agent = %self.agent_name, reason, "claude_health: repairing MCP cache");

        if tokio::time::timeout(timeout, make_repair(Arc::clone(self)))
            .await
            .is_err()
        {
            tracing::error!(
                agent = %self.agent_name,
                reason,
                timeout_secs = timeout.as_secs(),
                "claude_health: repair timed out"
            );
        }
    }

    async fn run_repair_body(self: &Arc<Self>, reason: &'static str) {
        if let Err(e) = remove_needs_auth_cache(self).await {
            tracing::warn!(agent = %self.agent_name, reason, "claude_health: {e}");
        }

        if let Err(e) = sync_after_cache_cleanup(self).await {
            tracing::error!(agent = %self.agent_name, reason, "claude_health: {e}");
            return;
        }

        match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, run_health_probe(self)).await {
            Ok(Ok(HealthProbeOutcome::Healthy)) => {
                self.mark_repaired_for_next_turn();
                tracing::info!(agent = %self.agent_name, reason, "claude_health: repair succeeded");
            }
            Ok(Ok(HealthProbeOutcome::NeedsRepair { status })) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    right_status = status.as_deref().unwrap_or("missing"),
                    "claude_health: repair probe still unhealthy"
                );
            }
            Ok(Ok(HealthProbeOutcome::NoInit)) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    "claude_health: repair probe had no init"
                );
            }
            Ok(Err(e)) => {
                tracing::error!(agent = %self.agent_name, reason, "claude_health: repair probe failed: {e}");
            }
            Err(_) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    timeout_secs = HEALTH_PROBE_TIMEOUT.as_secs(),
                    "claude_health: repair probe timed out"
                );
            }
        }
    }

    #[cfg(test)]
    fn try_begin_repair_for_test(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.repair_lock.try_lock().ok()
    }
}

async fn remove_needs_auth_cache(health: &ClaudeHealth) -> Result<(), String> {
    if let Some(sbox) = health.sandbox_exec.as_ref() {
        let (output, code) = sbox
            .exec(&["rm", "-f", "/sandbox/.claude/mcp-needs-auth-cache.json"])
            .await
            .map_err(|e| format!("sandbox cache cleanup exec failed: {e:#}"))?;
        if code != 0 {
            return Err(format!(
                "sandbox cache cleanup exited {code}: {}",
                output.trim()
            ));
        }
        return Ok(());
    }

    let path = health
        .agent_dir
        .join(".claude")
        .join("mcp-needs-auth-cache.json");
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "local cache cleanup failed at {}: {e:#}",
            path.display()
        )),
    }
}

async fn sync_after_cache_cleanup(health: &ClaudeHealth) -> Result<(), String> {
    if let Some(sbox) = health.sandbox_exec.as_ref() {
        crate::sync::sync_cycle(&health.agent_dir, sbox)
            .await
            .map_err(|e| format!("platform sync failed: {e:#}"))?;
    }
    Ok(())
}

/// Spawn the keepalive loop as a background task.
///
/// Returns the `JoinHandle` so the caller can await it during shutdown,
/// preventing a tokio runtime panic from in-flight `Interval::tick()` futures.
pub(crate) fn spawn_keepalive(
    health: Arc<ClaudeHealth>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_keepalive_loop(health, shutdown).await;
    })
}

async fn run_keepalive_loop(health: Arc<ClaudeHealth>, shutdown: CancellationToken) {
    run_one_health_cycle(Arc::clone(&health), "startup", &shutdown).await;
    if shutdown.is_cancelled() {
        return;
    }

    let mut interval = tokio::time::interval(DEFAULT_INTERVAL);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                run_one_health_cycle(Arc::clone(&health), "periodic", &shutdown).await;
                if shutdown.is_cancelled() {
                    return;
                }
            }
            _ = shutdown.cancelled() => {
                tracing::debug!("claude_health: shutdown");
                return;
            }
        }
    }
}

async fn run_one_health_cycle(
    health: Arc<ClaudeHealth>,
    reason: &'static str,
    shutdown: &CancellationToken,
) {
    tracing::info!(agent = %health.agent_name, reason, "claude_health: probing");
    let probe = tokio::time::timeout(HEALTH_PROBE_TIMEOUT, run_health_probe(&health));
    let result = tokio::select! {
        _ = shutdown.cancelled() => {
            tracing::debug!(agent = %health.agent_name, reason, "claude_health: shutdown");
            return;
        }
        result = probe => result,
    };

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::warn!(
                agent = %health.agent_name,
                reason,
                timeout_secs = HEALTH_PROBE_TIMEOUT.as_secs(),
                "claude_health: probe timed out"
            );
            health.report_backend_failure();
            return;
        }
    };

    match outcome {
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
        Err(e) => {
            tracing::warn!(agent = %health.agent_name, reason, "claude_health: failed: {e}");
            health.report_backend_failure();
        }
    }
}

async fn run_health_probe(health: &ClaudeHealth) -> Result<HealthProbeOutcome, String> {
    if health.ssh_config_path.is_some() && health.resolved_sandbox.is_none() {
        return Err("sandbox mode but no resolved sandbox name".to_owned());
    }

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
    )
    .await
    .map_err(|e| format!("{e:#}"))?;

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
                if let Err(e) = child.kill().await {
                    tracing::warn!(
                        agent = %health.agent_name,
                        "claude_health: failed to kill unhealthy probe: {e:#}"
                    );
                }
                killed_for_repair = true;
            }
            break;
        }
    }

    if killed_for_repair {
        if let Err(e) = child.wait().await {
            tracing::warn!(
                agent = %health.agent_name,
                "claude_health: failed to wait unhealthy probe: {e:#}"
            );
        }
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

    #[tokio::test]
    async fn default_interval_is_one_hour() {
        assert_eq!(DEFAULT_INTERVAL, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn health_probe_invocation_uses_haiku_stream_json_and_strict_mcp() {
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

    #[tokio::test]
    async fn pending_right_mcp_status_is_deferred_not_repairable() {
        let status = crate::cc::stream::RightMcpInitStatus::Unhealthy {
            status: Some("pending".to_owned()),
        };

        assert_eq!(classify_init_status(status), HealthProbeOutcome::Healthy);
    }

    #[tokio::test]
    async fn init_status_decision_repairs_terminal_unhealthy_states() {
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

    #[tokio::test]
    async fn health_probe_errors_when_sandbox_name_missing() {
        let health = ClaudeHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            Some(PathBuf::from("/tmp/ssh_config")),
            None,
            None,
            None,
        );

        assert_eq!(
            run_health_probe(&health).await,
            Err("sandbox mode but no resolved sandbox name".to_owned())
        );
    }

    #[tokio::test]
    async fn repair_notice_is_one_shot() {
        let health = ClaudeHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            None,
            None,
            None,
            None,
        );

        assert_eq!(health.consume_repair_notice(), None);
        health.mark_repaired_for_next_turn();
        assert_eq!(health.consume_repair_notice(), Some(REPAIR_NOTICE));
        assert_eq!(health.consume_repair_notice(), None);
    }

    #[tokio::test]
    async fn repair_lock_rejects_concurrent_second_holder() {
        let health = ClaudeHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            None,
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

    #[tokio::test]
    async fn repair_timeout_releases_repair_lock() {
        let health = ClaudeHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            None,
            None,
            None,
            None,
        );

        health
            .trigger_repair_with_timeout("test", Duration::from_millis(1), |_health| async {
                std::future::pending::<()>().await;
            })
            .await;

        assert!(health.try_begin_repair_for_test().is_some());
    }
}
