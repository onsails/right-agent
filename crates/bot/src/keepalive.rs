//! Claude health keepalive: periodic stream-json probe to verify agent-facing MCP status.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and strict MCP config,
//! then inspects the `system/init` event for Right MCP connectivity.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const REPAIR_OPERATION_TIMEOUT: Duration = Duration::from_secs(180);

const HEALTH_PROMPT: &str = "Reply exactly OK. Do not use tools.";

const INIT_AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(60);

const COMPETING_AUTH_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_ROLE_ARN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_PROJECT",
    "GCLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
];

/// Execution details for the init-time Claude API validation. The command uses
/// the persisted per-agent token and the same host/SSH transport as bot turns.
pub struct InitAuthProbe {
    agent_dir: PathBuf,
    ssh_config_path: Option<PathBuf>,
    resolved_sandbox: Option<String>,
    model: Option<String>,
    candidate_token: Option<String>,
}

impl InitAuthProbe {
    pub fn new(
        agent_dir: PathBuf,
        ssh_config_path: Option<PathBuf>,
        resolved_sandbox: Option<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            agent_dir,
            ssh_config_path,
            resolved_sandbox,
            model,
            candidate_token: None,
        }
    }
    /// Use an in-memory setup-token candidate for this validation only.
    ///
    /// The candidate is never persisted. Host probes pass it through the
    /// process environment and SSH probes pass it through stdin.
    pub fn with_candidate_token(mut self, token: String) -> Self {
        self.candidate_token = Some(token);
        self
    }
}

fn init_auth_probe_invocation(model: Option<String>) -> crate::cc::invocation::ClaudeInvocation {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: None,
        json_schema: None,
        output_format: crate::cc::invocation::OutputFormat::StreamJson,
        model,
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: {
            let mut args = crate::cc::invocation::disable_all_tools_args();
            args.push("--no-session-persistence".to_owned());
            args
        },
        prompt: Some(HEALTH_PROMPT.to_owned()),
        debug_flag: None,
    }
}

fn init_auth_probe_succeeded(stdout: &[u8]) -> bool {
    let mut saw_setup_token_auth = false;
    let mut saw_authenticated_rate_limit_rejection = false;
    let mut final_event = None;

    for line in stdout.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) else {
            return false;
        };
        if event.get("type").and_then(serde_json::Value::as_str) == Some("system")
            && event.get("subtype").and_then(serde_json::Value::as_str) == Some("init")
        {
            if event
                .get("apiKeySource")
                .and_then(serde_json::Value::as_str)
                != Some("none")
            {
                return false;
            }
            saw_setup_token_auth = true;
        }
        if saw_setup_token_auth
            && event.get("type").and_then(serde_json::Value::as_str) == Some("rate_limit_event")
            && event
                .get("rate_limit_info")
                .and_then(|info| info.get("status"))
                .and_then(serde_json::Value::as_str)
                == Some("rejected")
        {
            saw_authenticated_rate_limit_rejection = true;
        }
        final_event = Some(event);
    }

    let Some(final_event) = final_event else {
        return false;
    };
    saw_setup_token_auth
        && (saw_authenticated_rate_limit_rejection
            || (final_event.get("type").and_then(serde_json::Value::as_str) == Some("result")
                && final_event
                    .get("subtype")
                    .and_then(serde_json::Value::as_str)
                    == Some("success")
                && final_event
                    .get("is_error")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && final_event
                    .get("result")
                    .and_then(serde_json::Value::as_str)
                    == Some("OK")))
}

fn clear_competing_auth(command: &mut tokio::process::Command) {
    for name in COMPETING_AUTH_ENV {
        command.env_remove(name);
    }
}

fn ssh_init_auth_wrapper(args: &[String]) -> String {
    let mut script = String::from("unset CLAUDE_CODE_OAUTH_TOKEN");
    for name in COMPETING_AUTH_ENV {
        script.push(' ');
        script.push_str(name);
    }
    script.push_str("\nIFS= read -r CLAUDE_CODE_OAUTH_TOKEN || exit 1\n");
    script.push_str("[ -n \"$CLAUDE_CODE_OAUTH_TOKEN\" ] || exit 1\n");
    script.push_str("export CLAUDE_CODE_OAUTH_TOKEN\n");
    script.push_str("if command -v claude >/dev/null 2>&1; then CLAUDE_BIN=claude; ");
    script.push_str("elif command -v claude-bun >/dev/null 2>&1; then CLAUDE_BIN=claude-bun; ");
    script.push_str("else exit 127; fi\nexec \"$CLAUDE_BIN\"");
    if args.len() > 1 {
        script.push(' ');
        script.push_str(
            &right_openshell::openshell::quote_ssh_remote_args(
                args[1..].iter().map(String::as_str),
            )
            .expect("claude args should not contain nul bytes"),
        );
    }
    script
}

async fn resolve_host_claude_executable() -> anyhow::Result<PathBuf> {
    for name in ["claude", "claude-bun"] {
        let Ok(path) = which::which(name) else {
            continue;
        };
        let mut command = tokio::process::Command::new(&path);
        command.arg("--version");
        command.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        clear_competing_auth(&mut command);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        let Ok(status) = command.status().await else {
            continue;
        };
        if status.success() {
            return Ok(path);
        }
    }
    anyhow::bail!("Claude CLI is unavailable (tried `claude` and `claude-bun`)")
}

fn build_init_auth_command(
    args: &[String],
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
    token: &str,
) -> Result<tokio::process::Command, crate::cc::invocation::SandboxedHostExecRefused> {
    crate::cc::invocation::guard_no_sandboxed_host_exec(resolved_sandbox, ssh_config_path)?;
    if let Some(ssh_config) = ssh_config_path {
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(
            resolved_sandbox.expect("SSH probe must have a resolved sandbox"),
        );
        let mut command = tokio::process::Command::new("ssh");
        command
            .arg("-F")
            .arg(ssh_config)
            .arg(ssh_host)
            .arg("--")
            .arg(ssh_init_auth_wrapper(args));
        command.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        clear_competing_auth(&mut command);
        Ok(command)
    } else {
        let mut command = tokio::process::Command::new(&args[0]);
        command.args(&args[1..]);
        command.env("HOME", agent_dir);
        command.env("USE_BUILTIN_RIPGREP", "0");
        command.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        clear_competing_auth(&mut command);
        command.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        command.current_dir(agent_dir);
        Ok(command)
    }
}

async fn send_init_auth_token(
    stdin: &mut tokio::process::ChildStdin,
    token: &str,
) -> anyhow::Result<()> {
    stdin
        .write_all(token.as_bytes())
        .await
        .context("failed to send Claude authentication credential")?;
    stdin
        .write_all(b"\n")
        .await
        .context("failed to finish Claude authentication credential")?;
    stdin
        .shutdown()
        .await
        .context("failed to close Claude authentication credential input")?;
    Ok(())
}

async fn terminate_init_auth_process(
    mut child: right_process::ProcessGroupChild,
) -> anyhow::Result<()> {
    let kill_result = child.kill().await;
    let wait_result = child.wait().await;
    drop(child);

    kill_result.context("failed to kill Claude authentication validation process group")?;
    wait_result.context("failed to reap Claude authentication validation process group")?;
    Ok(())
}

async fn run_init_auth_command_with_timeout(
    mut command: tokio::process::Command,
    ssh_token: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    command.stdin(if ssh_token.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());

    let mut child = right_process::ProcessGroupChild::spawn(command)
        .context("failed to start Claude authentication validation")?;
    if let Some(token) = ssh_token {
        let Some(mut stdin) = child.stdin() else {
            terminate_init_auth_process(child).await?;
            anyhow::bail!("Claude authentication validation has no stdin");
        };
        if let Err(error) = send_init_auth_token(&mut stdin, token).await {
            drop(stdin);
            terminate_init_auth_process(child).await?;
            return Err(error);
        }
    }

    let wait_result = tokio::time::timeout(timeout, child.wait_with_output()).await;
    let output = match wait_result {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            terminate_init_auth_process(child).await?;
            return Err(error).context("Claude authentication validation process failed");
        }
        Err(_) => {
            terminate_init_auth_process(child).await?;
            anyhow::bail!("Claude authentication validation timed out");
        }
    };
    if !init_auth_probe_succeeded(&output.stdout) {
        anyhow::bail!(
            "Claude authentication validation failed; rerun init with a fresh token from `claude setup-token`"
        );
    }
    Ok(output.stdout)
}

async fn run_init_auth_command(
    command: tokio::process::Command,
    ssh_token: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    run_init_auth_command_with_timeout(command, ssh_token, INIT_AUTH_PROBE_TIMEOUT).await
}

/// Validate a setup token with a real one-turn Claude API call.
/// MCP, tools, and session persistence are disabled because init runs before
/// the aggregator exists. A candidate supplied through
/// [`InitAuthProbe::with_candidate_token`] remains in memory and is never
/// persisted; otherwise the existing credential is loaded through a read-only
/// database connection. Diagnostics intentionally discard command output so a
/// CLI or upstream error can never echo the token.
pub async fn validate_init_auth(probe: InitAuthProbe) -> anyhow::Result<()> {
    let token = if let Some(candidate_token) = probe.candidate_token {
        candidate_token
    } else {
        let connection = right_db::open_connection_readonly(&probe.agent_dir)
            .await
            .context("open agent database for Claude authentication validation")?;
        right_mcp::credentials::get_auth_token(&connection)
            .await
            .context("load Claude authentication credential")?
            .context("Claude authentication credential is missing")?
    };
    if token.contains(['\r', '\n']) {
        anyhow::bail!("Claude authentication credential has an invalid format");
    }

    let mut args = init_auth_probe_invocation(probe.model).into_args();
    if probe.ssh_config_path.is_none() {
        args[0] = resolve_host_claude_executable()
            .await?
            .to_string_lossy()
            .into_owned();
    }
    let command = build_init_auth_command(
        &args,
        &probe.agent_dir,
        probe.ssh_config_path.as_deref(),
        probe.resolved_sandbox.as_deref(),
        &token,
    )
    .map_err(anyhow::Error::from)?;
    let ssh_token = probe.ssh_config_path.as_ref().map(|_| token.as_str());
    run_init_auth_command(command, ssh_token).await?;
    Ok(())
}

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

    #[test]
    fn init_auth_probe_disables_mcp_tools_sessions_and_executable_override() {
        let args = init_auth_probe_invocation(Some("configured-model".to_owned())).into_args();

        assert_eq!(args[0], "claude");
        assert!(!args.contains(&"--mcp-config".to_owned()));
        assert!(!args.contains(&"--strict-mcp-config".to_owned()));
        assert!(!args.contains(&"--resume".to_owned()));
        assert!(!args.contains(&"--session-id".to_owned()));
        assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(args.contains(&"--no-session-persistence".to_owned()));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "configured-model"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--output-format", "stream-json"])
        );
    }

    #[test]
    fn init_auth_probe_requires_setup_token_init_and_exact_final_success() {
        const INIT: &str = r#"{"type":"system","subtype":"init","apiKeySource":"none"}"#;
        const OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"OK"}"#;

        assert!(init_auth_probe_succeeded(
            format!("{INIT}\n{OK}\n").as_bytes()
        ));
        assert!(!init_auth_probe_succeeded(OK.as_bytes()));
        assert!(!init_auth_probe_succeeded(
            format!("{INIT}\n{{not-json}}\n{OK}\n").as_bytes()
        ));
        assert!(!init_auth_probe_succeeded(
            format!("{INIT}\n{OK}\n{{\"type\":\"assistant\"}}\n").as_bytes()
        ));
        let non_exact = r#"{"type":"result","subtype":"success","is_error":false,"result":"OK\n"}"#;
        assert!(!init_auth_probe_succeeded(
            format!("{INIT}\n{non_exact}\n").as_bytes()
        ));
        let api_key_init = r#"{"type":"system","subtype":"init","apiKeySource":"api-key"}"#;
        assert!(!init_auth_probe_succeeded(
            format!("{api_key_init}\n{OK}\n").as_bytes()
        ));
    }

    #[test]
    fn init_auth_probe_accepts_authenticated_rate_limit_rejection() {
        const INIT: &str = r#"{"type":"system","subtype":"init","apiKeySource":"none"}"#;
        const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#;
        const ERROR: &str =
            r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;

        assert!(init_auth_probe_succeeded(
            format!("{INIT}\n{RATE_LIMIT}\n{ERROR}\n").as_bytes()
        ));
        assert!(!init_auth_probe_succeeded(
            format!("{RATE_LIMIT}\n{ERROR}\n").as_bytes()
        ));
    }

    #[test]
    fn init_auth_host_command_isolates_competing_auth() {
        let args = init_auth_probe_invocation(None).into_args();
        let command =
            build_init_auth_command(&args, Path::new("/agent"), None, None, "setup-secret")
                .unwrap();
        let std_command = command.as_std();
        let env: std::collections::HashMap<_, _> = std_command.get_envs().collect();

        assert_eq!(std_command.get_program(), "claude");
        assert_eq!(
            env.get(std::ffi::OsStr::new("CLAUDE_CODE_OAUTH_TOKEN")),
            Some(&Some(std::ffi::OsStr::new("setup-secret")))
        );
        for name in COMPETING_AUTH_ENV {
            assert_eq!(env.get(std::ffi::OsStr::new(name)), Some(&None));
        }
    }

    #[test]
    fn init_auth_ssh_argv_and_environment_contain_no_token() {
        let token = "setup-secret-never-in-argv";
        let args = init_auth_probe_invocation(None).into_args();
        let command = build_init_auth_command(
            &args,
            Path::new("/agent"),
            Some(Path::new("ssh-config")),
            Some("agent"),
            token,
        )
        .unwrap();
        let std_command = command.as_std();
        let argv = std_command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        assert_eq!(std_command.get_program(), "ssh");
        assert!(!argv.contains(token));
        assert_eq!(
            std_command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new("CLAUDE_CODE_OAUTH_TOKEN")),
            Some((std::ffi::OsStr::new("CLAUDE_CODE_OAUTH_TOKEN"), None))
        );
        let remote_script = std_command.get_args().last().unwrap().to_string_lossy();
        assert!(remote_script.contains("IFS= read -r CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(remote_script.contains("unset CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_API_KEY"));
        for name in COMPETING_AUTH_ENV {
            assert!(remote_script.contains(name));
        }
    }

    #[tokio::test]
    async fn init_auth_ssh_transport_sends_token_only_on_stdin() {
        let token = "setup-secret-stdin-only";
        let mut command = tokio::process::Command::new("sh");
        command.env("EXPECTED_PROBE_TOKEN", token).arg("-c").arg(
            "IFS= read -r received; [ \"$received\" = \"$EXPECTED_PROBE_TOKEN\" ] || exit 2; printf '%s\\n%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"apiKeySource\":\"none\"}' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\"}'",
        );
        let argv = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!argv.contains(token));

        let stdout = run_init_auth_command(command, Some(token)).await.unwrap();
        assert!(init_auth_probe_succeeded(&stdout));
        assert!(!String::from_utf8_lossy(&stdout).contains(token));
    }

    #[tokio::test]
    async fn init_auth_runner_accepts_authenticated_rate_limit_rejection_exit_one() {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(
            "printf '%s\\n%s\\n%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"apiKeySource\":\"none\"}' '{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"rejected\",\"rateLimitType\":\"seven_day\"}}' '{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true}'; exit 1",
        );

        let stdout = run_init_auth_command(command, None)
            .await
            .expect("authenticated rate-limit rejection must validate the credential");
        assert!(init_auth_probe_succeeded(&stdout));
    }

    #[tokio::test]
    async fn init_auth_failure_diagnostics_redact_output_and_token() {
        let token = "setup-secret-never-diagnose";
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("cat >/dev/null; printf 'upstream raw body setup-secret-never-diagnose'; printf 'stderr setup-secret-never-diagnose' >&2; exit 1");

        let error = run_init_auth_command(command, Some(token))
            .await
            .expect_err("failed probe must return an error");
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains(token));
        assert!(!diagnostic.contains("upstream raw body"));
        assert!(!diagnostic.contains("stderr"));
        assert!(diagnostic.contains("Claude authentication validation failed"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn init_auth_timeout_terminates_spawned_descendant() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let pid_file = temp_dir.path().join("descendant.pid");
        let mut command = tokio::process::Command::new("sh");
        command.env("DESCENDANT_PID_FILE", &pid_file).arg("-c").arg(
            "sleep 600 & descendant=$!; printf '%s' \"$descendant\" >\"$DESCENDANT_PID_FILE\"; wait",
        );

        let error = run_init_auth_command_with_timeout(command, None, Duration::from_secs(1))
            .await
            .expect_err("probe must time out");
        assert!(format!("{error:#}").contains("timed out"));

        let descendant_pid = std::fs::read_to_string(&pid_file)
            .expect("read descendant pid")
            .parse::<u32>()
            .expect("parse descendant pid");
        let status_path = PathBuf::from(format!("/proc/{descendant_pid}/status"));
        for _ in 0..100 {
            let running = std::fs::read_to_string(&status_path)
                .is_ok_and(|status| !status.lines().any(|line| line.starts_with("State:\tZ")));
            if !running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("spawned descendant {descendant_pid} survived probe timeout");
    }
    #[tokio::test]
    async fn init_auth_missing_database_does_not_create_files_or_sidecars() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let probe = InitAuthProbe::new(temp_dir.path().to_path_buf(), None, None, None);

        let error = validate_init_auth(probe)
            .await
            .expect_err("missing database must fail validation");

        assert!(format!("{error:#}").contains("open agent database"));
        for name in ["data.db", "data.db-wal", "data.db-shm", "data.db-tshm"] {
            assert!(
                !temp_dir.path().join(name).exists(),
                "readonly validation must not create {name}"
            );
        }
    }

    #[tokio::test]
    async fn init_auth_candidate_does_not_open_database_or_mutate_sidecars() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let sidecars = [
            ("data.db-wal", b"wal-sentinel".as_slice()),
            ("data.db-shm", b"shm-sentinel".as_slice()),
            ("data.db-tshm", b"tshm-sentinel".as_slice()),
        ];
        for (name, contents) in sidecars {
            std::fs::write(temp_dir.path().join(name), contents).expect("write sidecar sentinel");
        }
        let probe = InitAuthProbe::new(temp_dir.path().to_path_buf(), None, None, None)
            .with_candidate_token("invalid\ncredential".to_owned());

        let error = validate_init_auth(probe)
            .await
            .expect_err("invalid candidate must fail before process launch");

        assert!(format!("{error:#}").contains("invalid format"));
        assert!(!temp_dir.path().join("data.db").exists());
        for (name, contents) in sidecars {
            assert_eq!(
                std::fs::read(temp_dir.path().join(name)).expect("read sidecar sentinel"),
                contents
            );
        }
    }

    #[tokio::test]
    async fn failed_candidate_validation_preserves_stored_token() {
        const STORED_TOKEN: &str = "persisted-token-sentinel";
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let connection = right_db::open_connection(temp_dir.path(), true)
            .await
            .expect("create migrated database");
        right_mcp::credentials::save_auth_token(&connection, STORED_TOKEN)
            .await
            .expect("save stored token");
        drop(connection);

        let probe = InitAuthProbe::new(temp_dir.path().to_path_buf(), None, None, None)
            .with_candidate_token("invalid\ncredential".to_owned());
        validate_init_auth(probe)
            .await
            .expect_err("invalid candidate must fail validation");

        let connection = right_db::open_connection_readonly(temp_dir.path())
            .await
            .expect("reopen database read-only");
        assert_eq!(
            right_mcp::credentials::get_auth_token(&connection)
                .await
                .expect("load stored token"),
            Some(STORED_TOKEN.to_owned())
        );
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
            "agent-b".to_owned(),
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
            "agent-b".to_owned(),
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
            "agent-b".to_owned(),
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
            "agent-b".to_owned(),
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
