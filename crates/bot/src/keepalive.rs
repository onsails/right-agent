//! Claude health keepalive: periodic stream-json probe to verify agent-facing MCP status.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and strict MCP config,
//! then inspects the `system/init` event for Right MCP connectivity.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::AsyncBufReadExt;
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

/// Execution details for the init-time Claude API validation.
///
/// The probe runs inside the agent's sandbox, exactly like a bot turn: there
/// is no host transport, so the sandbox handle is required rather than
/// optional. The caller (`right agent init`) attaches to the sandbox and
/// fails fast if it cannot.
pub struct InitAuthProbe {
    agent_dir: PathBuf,
    sandbox: crate::sandbox::Sandbox,
    model: Option<String>,
    candidate_token: Option<String>,
}

impl InitAuthProbe {
    pub fn new(
        agent_dir: PathBuf,
        sandbox: crate::sandbox::Sandbox,
        model: Option<String>,
    ) -> Self {
        Self {
            agent_dir,
            sandbox,
            model,
            candidate_token: None,
        }
    }
    /// Use an in-memory setup-token candidate for this validation only.
    ///
    /// The candidate is never persisted; it reaches the guest as a per-exec
    /// environment variable, never through argv or the script text.
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
        final_event = Some(event);
    }

    let Some(final_event) = final_event else {
        return false;
    };
    saw_setup_token_auth
        && final_event.get("type").and_then(serde_json::Value::as_str) == Some("result")
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
            == Some("OK")
}

/// Guest script for the init-time auth probe.
///
/// Competing provider credentials are unset in the guest shell so the probe
/// exercises the setup token and nothing else. The token itself arrives as a
/// per-exec environment variable — never in the script text and never in the
/// argv, both of which are readable by anything that can list guest processes
/// — so the script only has to confirm it is present.
fn init_auth_probe_script(args: &[String]) -> String {
    let mut script = String::from("unset");
    for name in COMPETING_AUTH_ENV {
        script.push(' ');
        script.push_str(name);
    }
    script.push_str("\n[ -n \"$CLAUDE_CODE_OAUTH_TOKEN\" ] || exit 1\n");
    script.push_str("if command -v claude >/dev/null 2>&1; then CLAUDE_BIN=claude; ");
    script.push_str("elif command -v claude-bun >/dev/null 2>&1; then CLAUDE_BIN=claude-bun; ");
    script.push_str("else exit 127; fi\nexec \"$CLAUDE_BIN\"");
    if args.len() > 1 {
        script.push(' ');
        script.push_str(
            &crate::cc::invocation::quote_guest_args(args[1..].iter().map(String::as_str))
                .expect("claude args contain no NUL byte"),
        );
    }
    script
}

fn build_init_auth_command(
    args: &[String],
    sandbox: &crate::sandbox::Sandbox,
    token: &str,
) -> crate::cc::sandbox_process::SandboxCommand {
    crate::cc::sandbox_process::SandboxCommand::shell(sandbox, init_auth_probe_script(args))
        .env("CLAUDE_CODE_OAUTH_TOKEN", token)
        .stdout(crate::cc::sandbox_process::Capture::Pipe)
        .stderr(crate::cc::sandbox_process::Capture::Null)
}

/// Decide the probe verdict from the finished guest process.
///
/// Split out from the run so the redaction property — a failure diagnostic
/// never echoes the token or any byte the CLI or upstream produced — is
/// testable without a live sandbox.
fn init_auth_verdict(
    output: crate::cc::sandbox_process::SandboxOutput,
) -> anyhow::Result<Vec<u8>> {
    if !output.success() || !init_auth_probe_succeeded(&output.stdout) {
        anyhow::bail!(
            "Claude authentication validation failed; rerun init with a fresh token from `claude setup-token`"
        );
    }
    Ok(output.stdout)
}

async fn run_init_auth_command(
    command: crate::cc::sandbox_process::SandboxCommand,
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let child = command
        .spawn()
        .await
        .context("failed to start Claude authentication validation")?;
    match crate::cc::invocation::wait_with_output_or_kill(child, timeout)
        .await
        .context("Claude authentication validation process failed")?
    {
        crate::cc::invocation::ChildOutput::Completed(output) => init_auth_verdict(output),
        crate::cc::invocation::ChildOutput::TimedOut => {
            anyhow::bail!("Claude authentication validation timed out")
        }
    }
}

/// Resolve the credential this probe should validate.
///
/// A candidate supplied through [`InitAuthProbe::with_candidate_token`] stays
/// in memory and is never persisted; otherwise the stored credential is read
/// through a read-only connection, so a failed validation cannot create or
/// mutate the database or its sidecars.
async fn resolve_init_auth_token(
    agent_dir: &Path,
    candidate: Option<String>,
) -> anyhow::Result<String> {
    let token = match candidate {
        Some(candidate_token) => candidate_token,
        None => {
            let connection = right_db::open_connection_readonly(agent_dir)
                .await
                .context("open agent database for Claude authentication validation")?;
            right_mcp::credentials::get_auth_token(&connection)
                .await
                .context("load Claude authentication credential")?
                .context("Claude authentication credential is missing")?
        }
    };
    if token.contains(['\r', '\n']) {
        anyhow::bail!("Claude authentication credential has an invalid format");
    }
    Ok(token)
}

/// Validate a setup token with a real one-turn Claude API call, run in the
/// agent's sandbox.
///
/// MCP, tools, and session persistence are disabled because init runs before
/// the aggregator exists. Diagnostics intentionally discard command output so
/// a CLI or upstream error can never echo the token.
pub async fn validate_init_auth(probe: InitAuthProbe) -> anyhow::Result<()> {
    let token = resolve_init_auth_token(&probe.agent_dir, probe.candidate_token).await?;
    let args = init_auth_probe_invocation(probe.model).into_args();
    let command = build_init_auth_command(&args, &probe.sandbox, &token);
    run_init_auth_command(command, INIT_AUTH_PROBE_TIMEOUT).await?;
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
    /// `None` once the sandbox backend has degraded. Nothing runs without it —
    /// see [`crate::cc::invocation::guard_no_sandboxed_host_exec`].
    sandbox: Option<crate::sandbox::Sandbox>,
    sandbox_runtime: Option<Arc<crate::sandbox_runtime::SandboxRuntimeHandle>>,
    repair_lock: tokio::sync::Mutex<()>,
    repair_notice_pending: AtomicBool,
}

impl ClaudeHealth {
    pub(crate) fn new(
        agent_name: String,
        agent_dir: PathBuf,
        sandbox: Option<crate::sandbox::Sandbox>,
        sandbox_runtime: Option<Arc<crate::sandbox_runtime::SandboxRuntimeHandle>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_name,
            agent_dir,
            sandbox,
            sandbox_runtime,
            repair_lock: tokio::sync::Mutex::new(()),
            repair_notice_pending: AtomicBool::new(false),
        })
    }

    /// Guarded access to the sandbox, fail-closed: a degraded backend has no
    /// host fallback, so every probe and repair step simply cannot run.
    fn sandbox(&self) -> Result<&crate::sandbox::Sandbox, String> {
        crate::cc::invocation::guard_no_sandboxed_host_exec(
            &self.agent_name,
            self.sandbox.as_ref(),
        )
        .map_err(|e| format!("{e:#}"))
    }

    fn report_backend_failure(&self) {
        // The supervisor verifies with a real gateway probe before degrading,
        // so reporting liberally is safe.
        if let Some(rt) = &self.sandbox_runtime {
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

/// Guest path of the MCP `needs-auth` cache the repair path clears.
const NEEDS_AUTH_CACHE_PATH: &str = "/sandbox/.claude/mcp-needs-auth-cache.json";

async fn remove_needs_auth_cache(health: &ClaudeHealth) -> Result<(), String> {
    let sandbox = health.sandbox()?;
    let (output, code) = crate::sandbox::exec_argv(sandbox, &["rm", "-f", NEEDS_AUTH_CACHE_PATH])
        .await
        .map_err(|e| format!("sandbox cache cleanup exec failed: {e:#}"))?;
    if code != 0 {
        return Err(format!(
            "sandbox cache cleanup exited {code}: {}",
            output.trim()
        ));
    }
    Ok(())
}

async fn sync_after_cache_cleanup(health: &ClaudeHealth) -> Result<(), String> {
    let sandbox = health.sandbox()?;
    crate::sync::sync_cycle(&health.agent_dir, sandbox)
        .await
        .map_err(|e| format!("platform sync failed: {e:#}"))
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
    let sandbox = health.sandbox()?;
    let args = health_probe_invocation(crate::sandbox::SANDBOX_MCP_JSON_PATH).into_args();
    let mut child =
        crate::cc::invocation::build_claude_command(&args, &health.agent_dir, sandbox)
            .await
            .stdout(crate::cc::sandbox_process::Capture::Pipe)
            .stderr(crate::cc::sandbox_process::Capture::Null)
            .spawn()
            .await
            .map_err(|e| format!("spawn failed: {e:#}"))?;
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
                child.kill().await;
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
        let code = child
            .wait()
            .await
            .map_err(|e| format!("wait failed: {e:#}"))?;
        if code != 0 {
            return Err(format!("exit code: {code}"));
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
    fn init_auth_guest_script_isolates_competing_auth_and_hides_the_token() {
        let args = init_auth_probe_invocation(None).into_args();
        let script = init_auth_probe_script(&args);

        // The token is not an input to the script: it can only reach the guest
        // through `SandboxCommand::env`, which is not readable from the guest
        // process table the way argv and the script text are.
        assert!(script.starts_with("unset ANTHROPIC_API_KEY"));
        for name in COMPETING_AUTH_ENV {
            assert!(script.contains(name), "script must unset {name}");
        }
        // CLAUDE_CODE_OAUTH_TOKEN is the one auth variable NOT unset — it is
        // what the probe is validating.
        assert!(!script.contains("unset CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(script.contains("[ -n \"$CLAUDE_CODE_OAUTH_TOKEN\" ] || exit 1"));
        // The stdin handoff the SSH transport needed is gone.
        assert!(!script.contains("read -r"));
        assert!(script.contains("exec \"$CLAUDE_BIN\""));
        assert!(script.contains("--output-format stream-json"));
    }

    #[test]
    fn init_auth_failure_diagnostics_redact_output_and_token() {
        let token = "setup-secret-never-diagnose";
        let output = crate::cc::sandbox_process::SandboxOutput {
            code: 1,
            stdout: format!("upstream raw body {token}").into_bytes(),
            stderr: format!("stderr {token}").into_bytes(),
        };

        let error = init_auth_verdict(output).expect_err("failed probe must return an error");
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains(token));
        assert!(!diagnostic.contains("upstream raw body"));
        assert!(!diagnostic.contains("stderr"));
        assert!(diagnostic.contains("Claude authentication validation failed"));
    }

    #[test]
    fn init_auth_verdict_rejects_zero_exit_with_unauthenticated_stream() {
        let output = crate::cc::sandbox_process::SandboxOutput {
            code: 0,
            stdout: br#"{"type":"result","subtype":"success","is_error":false,"result":"OK"}"#
                .to_vec(),
            stderr: Vec::new(),
        };

        // No `system/init` with `apiKeySource: none` means the token was not
        // what authenticated the call.
        assert!(init_auth_verdict(output).is_err());
    }

    #[tokio::test]
    async fn init_auth_missing_database_does_not_create_files_or_sidecars() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");

        let error = resolve_init_auth_token(temp_dir.path(), None)
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

        let error =
            resolve_init_auth_token(temp_dir.path(), Some("invalid\ncredential".to_owned()))
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

        resolve_init_auth_token(temp_dir.path(), Some("invalid\ncredential".to_owned()))
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
    async fn every_repair_step_refuses_to_run_without_a_sandbox() {
        let health = ClaudeHealth::new("agent-b".to_owned(), PathBuf::from("/tmp/agent"), None, None);
        let refusal = "refusing to run agent 'agent-b' on the host: its sandbox is unavailable";

        // Fail-closed: no host fallback exists for any of these.
        assert_eq!(run_health_probe(&health).await, Err(refusal.to_owned()));
        assert_eq!(
            remove_needs_auth_cache(&health).await,
            Err(refusal.to_owned())
        );
        assert_eq!(
            sync_after_cache_cleanup(&health).await,
            Err(refusal.to_owned())
        );
    }

    #[tokio::test]
    async fn repair_notice_is_one_shot() {
        let health = ClaudeHealth::new("agent-b".to_owned(), PathBuf::from("/tmp/agent"), None, None);

        assert_eq!(health.consume_repair_notice(), None);
        health.mark_repaired_for_next_turn();
        assert_eq!(health.consume_repair_notice(), Some(REPAIR_NOTICE));
        assert_eq!(health.consume_repair_notice(), None);
    }

    #[tokio::test]
    async fn repair_lock_rejects_concurrent_second_holder() {
        let health = ClaudeHealth::new("agent-b".to_owned(), PathBuf::from("/tmp/agent"), None, None);

        let first = health.try_begin_repair_for_test();
        assert!(first.is_some());
        assert!(health.try_begin_repair_for_test().is_none());
        drop(first);
        assert!(health.try_begin_repair_for_test().is_some());
    }

    #[tokio::test]
    async fn repair_timeout_releases_repair_lock() {
        let health = ClaudeHealth::new("agent-b".to_owned(), PathBuf::from("/tmp/agent"), None, None);

        health
            .trigger_repair_with_timeout("test", Duration::from_millis(1), |_health| async {
                std::future::pending::<()>().await;
            })
            .await;

        assert!(health.try_begin_repair_for_test().is_some());
    }
}
