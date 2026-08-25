//! Right MCP init keepalive: periodic stream-json probe that verifies the
//! `system/init` MCP handshake, not overall Claude Code health.
//!
//! Runs every hour (default). Uses haiku model with max-turns=1 and strict MCP
//! config, then inspects the `system/init` event for Right MCP connectivity.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

/// Default interval between keepalive pings.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const REPAIR_OPERATION_TIMEOUT: Duration = Duration::from_secs(180);
/// Char bound for the stderr excerpt included in probe failure diagnostics.
const STDERR_EXCERPT_CHARS: usize = 2 * 1024;
/// Byte bound for the raw stdout tail retained in probe failure diagnostics.
const STDOUT_TAIL_BYTES: usize = STDERR_EXCERPT_CHARS * 4;
const PROBE_STDERR_GRACE: Duration = Duration::from_millis(100);

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

/// Local status of the stored Claude setup token for a runtime turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeAuthStatus {
    /// A syntactically valid setup token is stored for the agent.
    Valid,
    /// No setup token is stored for the agent.
    Missing,
    /// A stored token is empty or contains a line break.
    Invalid,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitAuthProbeSuccess {
    ExactOk,
    AuthenticatedRateLimitRejection,
}

fn parse_init_auth_probe_success(stdout: &[u8]) -> Option<InitAuthProbeSuccess> {
    let mut saw_setup_token_auth = false;
    let mut saw_authenticated_rate_limit_rejection = false;
    let mut final_event = None;

    for line in stdout.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) else {
            return None;
        };
        if event.get("type").and_then(serde_json::Value::as_str) == Some("system")
            && event.get("subtype").and_then(serde_json::Value::as_str) == Some("init")
        {
            if event
                .get("apiKeySource")
                .and_then(serde_json::Value::as_str)
                != Some("none")
            {
                return None;
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

    if !saw_setup_token_auth {
        return None;
    }
    if saw_authenticated_rate_limit_rejection {
        return Some(InitAuthProbeSuccess::AuthenticatedRateLimitRejection);
    }

    let final_event = final_event?;
    (final_event.get("type").and_then(serde_json::Value::as_str) == Some("result")
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
            == Some("OK"))
    .then_some(InitAuthProbeSuccess::ExactOk)
}

/// Guest script for the init-time auth probe.
///
/// Competing provider credentials are unset in the guest shell so the probe
/// exercises the setup token and nothing else. The token itself arrives as a
/// per-exec environment variable — never in the script text and never in the
/// argv, both of which are readable by anything that can list guest processes
/// — so the script only has to confirm it is present.
fn init_auth_probe_script(args: &[String]) -> String {
    // Same reason as the turn path: a direct guest exec has no login shell, so
    // /sandbox/.local/bin is off PATH and `claude` resolves to nothing.
    let mut script = format!(
        "if [ -r {env} ]; then . {env}; fi\nunset",
        env = crate::sandbox::GUEST_ENV_SCRIPT,
    );
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
fn init_auth_verdict(output: crate::cc::sandbox_process::SandboxOutput) -> anyhow::Result<Vec<u8>> {
    let accepted = match parse_init_auth_probe_success(&output.stdout) {
        Some(InitAuthProbeSuccess::ExactOk) => output.success(),
        Some(InitAuthProbeSuccess::AuthenticatedRateLimitRejection) => true,
        None => false,
    };
    if !accepted {
        anyhow::bail!(
            "Claude authentication validation failed; rerun init with a fresh token from `claude setup-token`"
        );
    }
    Ok(output.stdout)
}

#[derive(Debug)]
enum AuthCommandOutcome {
    Completed(crate::cc::sandbox_process::SandboxOutput),
    TimedOut,
}

async fn run_auth_command(
    command: crate::cc::sandbox_process::SandboxCommand,
    timeout: Duration,
) -> anyhow::Result<AuthCommandOutcome> {
    let outcome = tokio::time::timeout(timeout, async {
        let child = command
            .spawn()
            .await
            .context("failed to start Claude authentication validation")?;
        crate::cc::invocation::wait_with_output_or_kill(child, timeout)
            .await
            .context("Claude authentication validation process failed")
    })
    .await;

    match outcome {
        Ok(Ok(crate::cc::invocation::ChildOutput::Completed(output))) => {
            Ok(AuthCommandOutcome::Completed(output))
        }
        Ok(Ok(crate::cc::invocation::ChildOutput::TimedOut)) | Err(_) => {
            Ok(AuthCommandOutcome::TimedOut)
        }
        Ok(Err(error)) => Err(error),
    }
}
fn init_auth_outcome(outcome: anyhow::Result<AuthCommandOutcome>) -> anyhow::Result<Vec<u8>> {
    match outcome? {
        AuthCommandOutcome::Completed(output) => init_auth_verdict(output),
        AuthCommandOutcome::TimedOut => {
            anyhow::bail!("Claude authentication validation timed out")
        }
    }
}

/// Resolve the credential this offline init probe should validate.
///
/// A candidate supplied through [`InitAuthProbe::with_candidate_token`] stays
/// in memory and is never persisted; otherwise the stored credential is read
/// through a read-only connection. This adapter is CLI-init-only and requires
/// runtime quiescence before touching `data.db`.
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
    require_init_runtime_quiesced(&probe.agent_dir)?;
    let token = resolve_init_auth_token(&probe.agent_dir, probe.candidate_token).await?;
    let args = init_auth_probe_invocation(probe.model).into_args();
    let command = build_init_auth_command(&args, &probe.sandbox, &token);
    let outcome = run_auth_command(command, INIT_AUTH_PROBE_TIMEOUT).await;
    init_auth_outcome(outcome)?;
    Ok(())
}

/// Inspect the stored runtime credential through the Aggregator-owned DB.
///
/// The foreground turn is the runtime API validator. This pre-session check is
/// limited to a typed secret read and local syntax checks; owner/transport
/// failures propagate and never fall back to opening `data.db`.
pub(crate) async fn runtime_auth_status(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
) -> anyhow::Result<RuntimeAuthStatus> {
    use secrecy::ExposeSecret as _;

    let response = client
        .auth_token_get(&right_mcp::internal_db::AuthTokenGetRequest {
            agent: agent.to_owned(),
        })
        .await
        .context("load runtime Claude authentication credential through database owner")?;
    let token = response.token.as_ref().map(|token| token.expose_secret());
    Ok(classify_runtime_auth_token(token))
}

fn classify_runtime_auth_token(token: Option<&str>) -> RuntimeAuthStatus {
    match token {
        None => RuntimeAuthStatus::Missing,
        Some(token) if token.is_empty() || token.contains(['\r', '\n']) => {
            RuntimeAuthStatus::Invalid
        }
        Some(_) => RuntimeAuthStatus::Valid,
    }
}

/// Fail closed before the CLI-init adapter reads `data.db`. Init is defined to
/// run before the Aggregator exists; a retained runtime state file means
/// quiescence has not been established by the caller.
fn require_init_runtime_quiesced(agent_dir: &Path) -> anyhow::Result<()> {
    let agents_dir = agent_dir
        .parent()
        .context("agent directory has no parent")?;
    let home = agents_dir
        .parent()
        .context("agents directory has no parent")?;
    let state = home.join("run/state.json");
    anyhow::ensure!(
        !state.exists(),
        "offline init auth validation requires the Right runtime to be quiesced; {} exists",
        state.display()
    );
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

pub(crate) struct McpInitHealth {
    agent_name: String,
    agent_dir: PathBuf,
    /// The live sandbox is resolved from here per probe/repair step: recovery
    /// publishes a new handle, and the keepalive task outlives many of them.
    sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    repair_lock: tokio::sync::Mutex<()>,
    repair_notice_pending: AtomicBool,
}

impl McpInitHealth {
    pub(crate) fn new(
        agent_name: String,
        agent_dir: PathBuf,
        sandbox_runtime: Arc<crate::sandbox_runtime::SandboxRuntimeHandle>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_name,
            agent_dir,
            sandbox_runtime,
            repair_lock: tokio::sync::Mutex::new(()),
            repair_notice_pending: AtomicBool::new(false),
        })
    }

    /// Guarded access to the currently published sandbox, fail-closed: a
    /// degraded backend has no host fallback, so every probe and repair step
    /// simply cannot run.
    fn sandbox(&self) -> Result<crate::sandbox::Sandbox, String> {
        let sandbox = self.sandbox_runtime.current_sandbox();
        crate::cc::invocation::guard_no_sandboxed_host_exec(&self.agent_name, sandbox.as_ref())
            .map(crate::sandbox::Sandbox::clone)
            .map_err(|e| format!("{e:#}"))
    }

    fn report_backend_failure(&self) {
        // The supervisor verifies with a real gateway probe before degrading,
        // so reporting liberally is safe.
        self.sandbox_runtime.report_suspected_failure();
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
                "right_mcp_init: repair already running"
            );
            return;
        };

        tracing::warn!(agent = %self.agent_name, reason, "right_mcp_init: repairing MCP cache");

        if tokio::time::timeout(timeout, make_repair(Arc::clone(self)))
            .await
            .is_err()
        {
            tracing::error!(
                agent = %self.agent_name,
                reason,
                timeout_secs = timeout.as_secs(),
                "right_mcp_init: repair timed out"
            );
        }
    }

    async fn run_repair_body(self: &Arc<Self>, reason: &'static str) {
        if let Err(e) = remove_needs_auth_cache(self).await {
            tracing::warn!(agent = %self.agent_name, reason, "right_mcp_init: {e}");
        }

        if let Err(e) = sync_after_cache_cleanup(self).await {
            tracing::error!(agent = %self.agent_name, reason, "right_mcp_init: {e}");
            return;
        }

        match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, run_health_probe(self)).await {
            Ok(Ok(HealthProbeOutcome::Healthy)) => {
                self.mark_repaired_for_next_turn();
                tracing::info!(agent = %self.agent_name, reason, "right_mcp_init: repair succeeded");
            }
            Ok(Ok(HealthProbeOutcome::NeedsRepair { status })) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    right_status = status.as_deref().unwrap_or("missing"),
                    "right_mcp_init: repair probe still unhealthy"
                );
            }
            Ok(Ok(HealthProbeOutcome::NoInit)) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    "right_mcp_init: repair probe had no init"
                );
            }
            Ok(Err(e)) => {
                tracing::error!(agent = %self.agent_name, reason, "right_mcp_init: repair probe failed: {e}");
            }
            Err(_) => {
                tracing::error!(
                    agent = %self.agent_name,
                    reason,
                    timeout_secs = HEALTH_PROBE_TIMEOUT.as_secs(),
                    "right_mcp_init: repair probe timed out"
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

async fn remove_needs_auth_cache(health: &McpInitHealth) -> Result<(), String> {
    let sandbox = health.sandbox()?;
    let (output, code) = crate::sandbox::exec_argv(&sandbox, &["rm", "-f", NEEDS_AUTH_CACHE_PATH])
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

async fn sync_after_cache_cleanup(health: &McpInitHealth) -> Result<(), String> {
    let sandbox = health.sandbox()?;
    crate::sync::sync_cycle(&health.agent_dir, &sandbox)
        .await
        .map_err(|e| format!("platform sync failed: {e:#}"))
}

/// Spawn the keepalive loop as a background task.
///
/// Returns the `JoinHandle` so the caller can await it during shutdown,
/// preventing a tokio runtime panic from in-flight `Interval::tick()` futures.
pub(crate) fn spawn_keepalive(
    health: Arc<McpInitHealth>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_keepalive_loop(health, shutdown).await;
    })
}

async fn run_keepalive_loop(health: Arc<McpInitHealth>, shutdown: CancellationToken) {
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
                tracing::debug!("right_mcp_init: shutdown");
                return;
            }
        }
    }
}

async fn run_one_health_cycle(
    health: Arc<McpInitHealth>,
    reason: &'static str,
    shutdown: &CancellationToken,
) {
    tracing::info!(agent = %health.agent_name, reason, "right_mcp_init: probing");
    let probe = tokio::time::timeout(HEALTH_PROBE_TIMEOUT, run_health_probe(&health));
    let result = tokio::select! {
        _ = shutdown.cancelled() => {
            tracing::debug!(agent = %health.agent_name, reason, "right_mcp_init: shutdown");
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
                "right_mcp_init: probe timed out"
            );
            health.report_backend_failure();
            return;
        }
    };

    match outcome {
        Ok(HealthProbeOutcome::Healthy) => {
            tracing::info!(agent = %health.agent_name, reason, "right_mcp_init: ok");
        }
        Ok(HealthProbeOutcome::NeedsRepair { status }) => {
            tracing::warn!(
                agent = %health.agent_name,
                reason,
                right_status = status.as_deref().unwrap_or("missing"),
                "right_mcp_init: right MCP unhealthy; scheduling repair"
            );
            health.trigger_repair(reason).await;
        }
        Ok(HealthProbeOutcome::NoInit) => {
            tracing::warn!(agent = %health.agent_name, reason, "right_mcp_init: no system/init");
        }
        Err(e) => {
            tracing::warn!(agent = %health.agent_name, reason, "right_mcp_init: failed: {e}");
            health.report_backend_failure();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthProbeStreamCompletion {
    TerminalResult,
    NeedsRepair,
    Eof,
}

#[derive(Debug)]
struct HealthProbeStdout {
    init_outcome: HealthProbeOutcome,
    stdout_tail: Vec<String>,
    authenticated_rate_limit: bool,
    completion: HealthProbeStreamCompletion,
}

#[derive(Debug, Default)]
struct ProbeStdoutTail {
    lines: VecDeque<String>,
    bytes: usize,
}

impl ProbeStdoutTail {
    fn push(&mut self, mut line: String) {
        if line.len() > STDOUT_TAIL_BYTES {
            let mut start = line.len() - STDOUT_TAIL_BYTES;
            while !line.is_char_boundary(start) {
                start += 1;
            }
            line = line[start..].to_owned();
            self.lines.clear();
            self.bytes = 0;
        }
        while self.bytes.saturating_add(line.len()) > STDOUT_TAIL_BYTES {
            let Some(removed) = self.lines.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.len());
        }
        self.bytes += line.len();
        self.lines.push_back(line);
    }

    fn into_lines(self) -> Vec<String> {
        self.lines.into_iter().collect()
    }
}

async fn read_probe_line<R>(reader: &mut R, line: &mut Vec<u8>) -> Result<usize, String>
where
    R: AsyncBufRead + Unpin,
{
    line.clear();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|e| format!("stdout read failed: {e:#}"))?;
        if available.is_empty() {
            return Ok(line.len());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(consumed) > STDOUT_TAIL_BYTES {
            return Err(format!(
                "health probe stdout line exceeds {STDOUT_TAIL_BYTES} bytes"
            ));
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(line.len());
        }
    }
}

async fn read_health_probe_stdout<R>(stdout: R) -> Result<HealthProbeStdout, String>
where
    R: AsyncRead + Unpin,
{
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut init_outcome = HealthProbeOutcome::NoInit;
    let mut stdout_tail = ProbeStdoutTail::default();
    let mut authenticated_rate_limit = false;

    let mut line_bytes = Vec::new();
    let completion = loop {
        if read_probe_line(&mut reader, &mut line_bytes).await? == 0 {
            break HealthProbeStreamCompletion::Eof;
        }
        if line_bytes.last() == Some(&b'\n') {
            line_bytes.pop();
        }
        if line_bytes.last() == Some(&b'\r') {
            line_bytes.pop();
        }
        let line = String::from_utf8(line_bytes.clone())
            .map_err(|_| "health probe stdout line is not valid UTF-8".to_owned())?;

        if let Some(status) = crate::cc::stream::parse_right_mcp_init_status(&line) {
            init_outcome = classify_init_status(status);
        }
        authenticated_rate_limit |= probe_line_is_authenticated_rate_limit(&line);
        let (terminal_result, terminal_result_error) =
            match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(event)
                    if event.get("type").and_then(serde_json::Value::as_str) == Some("result") =>
                {
                    (
                        true,
                        event.get("is_error").and_then(serde_json::Value::as_bool) == Some(true),
                    )
                }
                Ok(_) | Err(_) => (false, false),
            };

        stdout_tail.push(line);

        if matches!(init_outcome, HealthProbeOutcome::NeedsRepair { .. }) {
            break HealthProbeStreamCompletion::NeedsRepair;
        }
        if terminal_result_error && !authenticated_rate_limit {
            return Err("health probe terminal result reported an error".to_owned());
        }
        if terminal_result {
            tracing::debug!(
                event_type = "result",
                "right_mcp_init: terminal probe event"
            );
            break HealthProbeStreamCompletion::TerminalResult;
        }
    };

    Ok(HealthProbeStdout {
        init_outcome,
        stdout_tail: stdout_tail.into_lines(),
        authenticated_rate_limit,
        completion,
    })
}

async fn run_health_probe(health: &McpInitHealth) -> Result<HealthProbeOutcome, String> {
    let sandbox = health.sandbox()?;
    let args = health_probe_invocation(crate::sandbox::SANDBOX_MCP_JSON_PATH).into_args();
    let mut child = crate::cc::invocation::build_claude_command(&args, &health.agent_dir, &sandbox)
        .await
        .map_err(|e| format!("build health probe command: {e:#}"))?
        .stdout(crate::cc::sandbox_process::Capture::Pipe)
        .stderr(crate::cc::sandbox_process::Capture::Pipe)
        .spawn()
        .await
        .map_err(|e| format!("spawn failed: {e:#}"))?;

    // Take both pipes before reading either one. The sandbox event pump writes
    // them sequentially, so leaving stderr unread while waiting for stdout can
    // fill stderr's bounded pipe and prevent the pump from delivering stdout
    // or the process exit.
    let stderr = child.stderr();
    let stdout = child.stdout();
    let Some(stderr) = stderr else {
        drop(stdout);
        child.kill().await;
        child
            .wait()
            .await
            .map_err(|e| format!("wait after missing stderr failed: {e:#}"))?;
        return Err("health probe missing stderr".to_owned());
    };
    let stderr_drain = spawn_probe_stderr_drain(stderr);
    let Some(stdout) = stdout else {
        child.kill().await;
        child
            .wait()
            .await
            .map_err(|e| format!("wait after missing stdout failed: {e:#}"))?;
        abort_probe_stderr(stderr_drain).await?;
        return Err("health probe missing stdout".to_owned());
    };
    let stdout_result = read_health_probe_stdout(stdout).await;

    match stdout_result {
        Ok(observed)
            if matches!(
                observed.completion,
                HealthProbeStreamCompletion::TerminalResult
                    | HealthProbeStreamCompletion::NeedsRepair
            ) =>
        {
            child.kill().await;
            child
                .wait()
                .await
                .map_err(|e| format!("wait after deliberate probe termination failed: {e:#}"))?;
            abort_probe_stderr(stderr_drain).await?;
            Ok(observed.init_outcome)
        }
        Ok(observed) => {
            let code = child
                .wait()
                .await
                .map_err(|e| format!("wait failed: {e:#}"))?;
            let stderr_tail = await_probe_stderr(stderr_drain).await?;
            if code != 0 && !observed.authenticated_rate_limit {
                probe_exit_verdict_with_stderr(code, &observed.stdout_tail, &stderr_tail)?;
            }
            Ok(observed.init_outcome)
        }
        Err(stdout_error) => {
            child.kill().await;
            child
                .wait()
                .await
                .map_err(|e| format!("wait after stdout failure failed: {e:#}"))?;
            let stderr_tail = await_probe_stderr_bounded(stderr_drain, PROBE_STDERR_GRACE).await?;
            Err(format!("{stdout_error}; stderr tail: {stderr_tail}"))
        }
    }
}

/// True when the probe's post-init stdout carries an authenticated
/// rate-limit rejection: a `rate_limit_event` with status `rejected`, or a
/// result envelope with `api_error_status` 429. Mirrors the
/// `AuthenticatedRateLimitRejection` acceptance in
/// `parse_init_auth_probe_success` — a weekly-limit 429 proves the
/// credential authenticated and must not be reported as a probe failure.
fn probe_line_is_authenticated_rate_limit(line: &str) -> bool {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    let rate_limit_rejected = event.get("type").and_then(serde_json::Value::as_str)
        == Some("rate_limit_event")
        && event
            .get("rate_limit_info")
            .and_then(|info| info.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("rejected");
    let result_429 = event.get("type").and_then(serde_json::Value::as_str) == Some("result")
        && event
            .get("api_error_status")
            .and_then(serde_json::Value::as_i64)
            == Some(429);
    rate_limit_rejected || result_429
}

fn probe_tail_is_authenticated_rate_limit(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|line| probe_line_is_authenticated_rate_limit(line))
}

/// Exit-code verdict for a finished (not killed) health probe.
///
/// A non-zero exit with an authenticated rate-limit rejection in the stdout
/// tail is healthy — the probe answered its question (MCP connected or not)
/// before the weekly limit cut the turn. Any other non-zero exit stays an
/// error, carrying both the stderr and stdout tails so `Not logged in` and
/// rate-limit envelopes are distinguishable in the logs.
fn probe_exit_verdict(code: i32, stdout_lines: &[String]) -> Result<(), String> {
    probe_exit_verdict_with_stderr(code, stdout_lines, "")
}

fn probe_exit_verdict_with_stderr(
    code: i32,
    stdout_lines: &[String],
    stderr_tail: &str,
) -> Result<(), String> {
    if probe_tail_is_authenticated_rate_limit(stdout_lines) {
        return Ok(());
    }
    let stdout_tail = stdout_lines_excerpt(stdout_lines);
    Err(format!(
        "exit code: {code}; stderr tail: {stderr_tail}; stdout tail: {stdout_tail}"
    ))
}

/// Bounded tail excerpt of the probe's raw stdout lines for diagnostics.
fn stdout_lines_excerpt(lines: &[String]) -> String {
    let joined = lines.join("\n");
    stderr_excerpt(joined.as_bytes())
}

struct ProbeStderrDrain {
    task: Option<tokio::task::JoinHandle<Result<String, String>>>,
}

impl Drop for ProbeStderrDrain {
    fn drop(&mut self) {
        if let Some(task) = self.task.as_ref() {
            task.abort();
        }
    }
}

fn spawn_probe_stderr_drain<R>(stderr: R) -> ProbeStderrDrain
where
    R: AsyncRead + Send + Unpin + 'static,
{
    ProbeStderrDrain {
        task: Some(tokio::spawn(drain_probe_stderr(stderr))),
    }
}

async fn await_probe_stderr(mut drain: ProbeStderrDrain) -> Result<String, String> {
    let task = drain
        .task
        .take()
        .ok_or_else(|| "stderr drain task already consumed".to_owned())?;
    task.await
        .map_err(|e| format!("stderr drain task failed: {e:#}"))?
}
async fn abort_probe_stderr(mut drain: ProbeStderrDrain) -> Result<(), String> {
    let task = drain
        .task
        .take()
        .ok_or_else(|| "stderr drain task already consumed".to_owned())?;
    task.abort();
    match task.await {
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(format!(
            "stderr drain task failed while aborting: {error:#}"
        )),
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error),
    }
}

async fn await_probe_stderr_bounded(
    mut drain: ProbeStderrDrain,
    grace: Duration,
) -> Result<String, String> {
    let mut task = drain
        .task
        .take()
        .ok_or_else(|| "stderr drain task already consumed".to_owned())?;
    match tokio::time::timeout(grace, &mut task).await {
        Ok(joined) => joined.map_err(|e| format!("stderr drain task failed: {e:#}"))?,
        Err(_) => {
            task.abort();
            match task.await {
                Err(error) if error.is_cancelled() => Ok(String::new()),
                Err(error) => Err(format!(
                    "stderr drain task failed while aborting: {error:#}"
                )),
                Ok(result) => result,
            }
        }
    }
}

async fn drain_probe_stderr<R>(mut stderr: R) -> Result<String, String>
where
    R: AsyncRead + Unpin,
{
    // Four bytes per diagnostic character is enough to retain the complete
    // UTF-8 tail while keeping memory fixed even when the guest floods stderr.
    const TAIL_BYTES: usize = STDERR_EXCERPT_CHARS * 4;
    const READ_BYTES: usize = 8 * 1024;

    let mut tail = VecDeque::with_capacity(TAIL_BYTES);
    let mut chunk = [0_u8; READ_BYTES];
    loop {
        let read = stderr
            .read(&mut chunk)
            .await
            .map_err(|e| format!("stderr read failed: {e:#}"))?;
        if read == 0 {
            break;
        }
        if read >= TAIL_BYTES {
            tail.clear();
            tail.extend(&chunk[read - TAIL_BYTES..read]);
            continue;
        }
        let overflow = tail.len().saturating_add(read).saturating_sub(TAIL_BYTES);
        tail.drain(..overflow);
        tail.extend(&chunk[..read]);
    }
    let bytes: Vec<u8> = tail.into_iter().collect();
    Ok(stderr_excerpt(&bytes))
}

/// Tail excerpt of the probe's stderr, lossy-decoded and char-bounded.
fn stderr_excerpt(bytes: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    let len = lossy.chars().count();
    if len <= STDERR_EXCERPT_CHARS {
        return lossy.into_owned();
    }
    lossy.chars().skip(len - STDERR_EXCERPT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

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
    fn stderr_excerpt_keeps_tail_and_bounds_length() {
        let short = b"boom\n".to_vec();
        assert_eq!(stderr_excerpt(&short), "boom\n");

        let long: Vec<u8> = (b'a'..=b'z').cycle().take(26 * 500).collect();
        let excerpt = stderr_excerpt(&long);
        assert!(excerpt.chars().count() <= STDERR_EXCERPT_CHARS);
        // Tail, not head: the excerpt must end with the final byte written.
        assert!(excerpt.ends_with('z'));
    }

    #[test]
    fn stderr_excerpt_lossy_on_invalid_utf8() {
        let excerpt = stderr_excerpt(&[0xff, 0xfe, b'!']);
        assert!(excerpt.ends_with('!'));
    }

    #[tokio::test]
    async fn probe_stderr_drain_prevents_full_pipe_deadlock_and_keeps_bounded_tail() {
        const STDERR_BYTES: usize = 300 * 1024;
        const STDERR_SENTINEL: &[u8] = b"stderr-tail-sentinel";
        const INIT: &[u8] =
            b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"connected\"}]}\n";
        const RATE_LIMIT: &[u8] =
            b"{\"type\":\"result\",\"is_error\":true,\"api_error_status\":429}\n";

        const TEST_PIPE_BYTES: usize = 64 * 1024;
        let (stderr_writer, stderr_reader) = tokio::io::duplex(TEST_PIPE_BYTES);
        let mut stderr_writer = stderr_writer;
        let stderr_drain = spawn_probe_stderr_drain(stderr_reader);
        let (output_tx, output_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn(async move {
            let write_result = async {
                let chunk = [b'x'; 8 * 1024];
                for _ in 0..(STDERR_BYTES / chunk.len()) {
                    stderr_writer.write_all(&chunk).await?;
                }
                stderr_writer.write_all(STDERR_SENTINEL).await?;
                Ok::<_, std::io::Error>(())
            }
            .await;
            drop(stderr_writer);
            output_tx
                .send((write_result, INIT, RATE_LIMIT))
                .expect("output receiver must remain alive");
        });

        let completed = tokio::time::timeout(Duration::from_secs(2), async {
            let (write_result, init, rate_limit) = output_rx.await.expect("fixture output");
            write_result?;
            producer.await.expect("producer task");
            let stderr_tail = await_probe_stderr(stderr_drain)
                .await
                .expect("stderr drain");
            Ok::<_, std::io::Error>((
                String::from_utf8_lossy(init).trim().to_owned(),
                String::from_utf8_lossy(rate_limit).trim().to_owned(),
                stderr_tail,
            ))
        })
        .await
        .expect("concurrent drain must prevent the full stderr pipe from deadlocking")
        .expect("fixture I/O");

        assert!(crate::cc::stream::parse_right_mcp_init_status(&completed.0).is_some());
        assert!(probe_tail_is_authenticated_rate_limit(&[completed.1]));
        assert!(completed.2.ends_with("stderr-tail-sentinel"));
        assert!(completed.2.chars().count() <= STDERR_EXCERPT_CHARS);
    }

    struct FailingProbeReader;

    impl tokio::io::AsyncRead for FailingProbeReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("injected read failure")))
        }
    }

    #[tokio::test]
    async fn health_probe_stdout_completes_at_result_without_waiting_for_eof() {
        const INIT: &[u8] =
            b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"connected\"}]}\n";
        const ASSISTANT: &[u8] = b"{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n";
        const RESULT: &[u8] = b"{\"type\":\"result\",\"is_error\":false,\"result\":\"OK\"}\n";

        let (mut writer, reader) = tokio::io::duplex(4 * 1024);
        writer.write_all(INIT).await.expect("write init");
        writer.write_all(ASSISTANT).await.expect("write assistant");
        writer.write_all(RESULT).await.expect("write result");

        let observed =
            tokio::time::timeout(Duration::from_millis(100), read_health_probe_stdout(reader))
                .await
                .expect("terminal result must complete the probe while stdout remains open")
                .expect("read probe stdout");

        assert_eq!(observed.init_outcome, HealthProbeOutcome::Healthy);
        assert!(!observed.authenticated_rate_limit);
        assert_eq!(
            observed.completion,
            HealthProbeStreamCompletion::TerminalResult
        );
        drop(writer);
    }

    #[tokio::test]
    async fn probe_stderr_drain_aborts_when_guard_is_dropped() {
        let (_writer, reader) = tokio::io::duplex(64);
        let drain = spawn_probe_stderr_drain(reader);
        let abort = drain.task.as_ref().expect("stderr task").abort_handle();

        drop(drain);
        tokio::task::yield_now().await;

        assert!(abort.is_finished());
    }
    #[tokio::test]
    async fn terminal_probe_cleanup_aborts_stderr_that_remains_open() {
        let (_writer, reader) = tokio::io::duplex(64);
        let drain = spawn_probe_stderr_drain(reader);
        let abort = drain.task.as_ref().expect("stderr task").abort_handle();

        tokio::time::timeout(Duration::from_millis(100), abort_probe_stderr(drain))
            .await
            .expect("deliberate cleanup must not wait for stderr EOF")
            .expect("abort stderr drain");

        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn stdout_failure_stderr_grace_aborts_open_drain() {
        let (_writer, reader) = tokio::io::duplex(64);
        let drain = spawn_probe_stderr_drain(reader);
        let abort = drain.task.as_ref().expect("stderr task").abort_handle();

        let tail = tokio::time::timeout(
            Duration::from_millis(100),
            await_probe_stderr_bounded(drain, Duration::from_millis(10)),
        )
        .await
        .expect("bounded diagnostic grace must return")
        .expect("bounded stderr drain");

        assert!(tail.is_empty());
        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn probe_stderr_drain_propagates_read_failure() {
        let error = drain_probe_stderr(FailingProbeReader)
            .await
            .expect_err("stderr read errors must propagate");

        assert!(error.contains("stderr read failed"), "{error}");
        assert!(error.contains("injected read failure"), "{error}");
    }

    #[tokio::test]
    async fn health_probe_stdout_kills_for_unhealthy_init_without_waiting_for_result() {
        const UNHEALTHY_INIT: &[u8] = b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"needs-auth\"}]}\n";

        let (mut writer, reader) = tokio::io::duplex(4 * 1024);
        writer
            .write_all(UNHEALTHY_INIT)
            .await
            .expect("write unhealthy init");

        let observed =
            tokio::time::timeout(Duration::from_millis(100), read_health_probe_stdout(reader))
                .await
                .expect("unhealthy init must request immediate repair kill")
                .expect("read probe stdout");

        assert_eq!(
            observed.init_outcome,
            HealthProbeOutcome::NeedsRepair {
                status: Some("needs-auth".to_owned())
            }
        );
        assert_eq!(
            observed.completion,
            HealthProbeStreamCompletion::NeedsRepair
        );
        drop(writer);
    }

    #[tokio::test]
    async fn health_probe_stdout_preserves_rate_limit_evidence_at_terminal_result() {
        const INIT: &[u8] = b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"connected\"}]}\n";
        const RATE_LIMIT: &[u8] = b"{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"rejected\",\"rateLimitType\":\"seven_day\"}}\n";
        const RESULT: &[u8] = b"{\"type\":\"result\",\"is_error\":true,\"api_error_status\":429}\n";

        let (mut writer, reader) = tokio::io::duplex(4 * 1024);
        writer.write_all(INIT).await.expect("write init");
        writer
            .write_all(RATE_LIMIT)
            .await
            .expect("write rate limit");
        writer.write_all(RESULT).await.expect("write result");

        let observed =
            tokio::time::timeout(Duration::from_millis(100), read_health_probe_stdout(reader))
                .await
                .expect("rate-limited result must complete without EOF")
                .expect("read probe stdout");

        assert_eq!(observed.init_outcome, HealthProbeOutcome::Healthy);
        assert!(observed.authenticated_rate_limit);
        assert!(probe_tail_is_authenticated_rate_limit(
            &observed.stdout_tail
        ));
        drop(writer);
    }

    #[tokio::test]
    async fn health_probe_stdout_rejects_oversized_line_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(STDOUT_TAIL_BYTES * 2);
        writer
            .write_all(&vec![b'x'; STDOUT_TAIL_BYTES + 1])
            .await
            .expect("write oversized line");

        let error =
            tokio::time::timeout(Duration::from_millis(100), read_health_probe_stdout(reader))
                .await
                .expect("oversized line must fail before EOF")
                .expect_err("oversized line must fail");

        assert!(error.contains("stdout line exceeds"), "{error}");
        drop(writer);
    }

    #[tokio::test]
    async fn health_probe_stdout_rejects_non_rate_limit_error_result() {
        const INIT: &[u8] = b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"connected\"}]}\n";
        const RESULT: &[u8] =
            b"{\"type\":\"result\",\"is_error\":true,\"result\":\"Not logged in\"}\n";

        let (mut writer, reader) = tokio::io::duplex(4 * 1024);
        writer.write_all(INIT).await.expect("write init");
        writer.write_all(RESULT).await.expect("write result");

        let error = read_health_probe_stdout(reader)
            .await
            .expect_err("non-rate-limit result errors must fail the probe");

        assert!(
            error.contains("terminal result reported an error"),
            "{error}"
        );
        assert!(
            !error.contains("Not logged in"),
            "raw result must stay redacted"
        );
        drop(writer);
    }

    #[tokio::test]
    async fn health_probe_stdout_reaches_eof_without_terminal_result() {
        const INIT: &[u8] = b"{\"type\":\"system\",\"subtype\":\"init\",\"mcp_servers\":[{\"name\":\"right\",\"status\":\"connected\"}]}\n";
        const ASSISTANT: &[u8] = b"{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n";

        let (mut writer, reader) = tokio::io::duplex(4 * 1024);
        writer.write_all(INIT).await.expect("write init");
        writer.write_all(ASSISTANT).await.expect("write assistant");
        drop(writer);

        let observed = read_health_probe_stdout(reader)
            .await
            .expect("read probe stdout through EOF");

        assert_eq!(observed.init_outcome, HealthProbeOutcome::Healthy);
        assert_eq!(observed.completion, HealthProbeStreamCompletion::Eof);
        assert!(
            observed
                .stdout_tail
                .iter()
                .any(|line| line.contains("assistant"))
        );
    }

    #[test]
    fn health_probe_stdout_tail_is_bounded() {
        let mut tail = ProbeStdoutTail::default();
        tail.push("old".repeat(STDOUT_TAIL_BYTES));
        tail.push("new-tail-sentinel".to_owned());

        let lines = tail.into_lines();
        assert!(lines.iter().map(String::len).sum::<usize>() <= STDOUT_TAIL_BYTES);
        assert_eq!(lines.last().map(String::as_str), Some("new-tail-sentinel"));
    }

    #[test]
    fn init_auth_probe_disables_mcp_tools_sessions_and_executable_override() {
        let args = init_auth_probe_invocation(Some("configured-model".to_owned())).into_args();

        assert_eq!(args[0], "claude");
        assert!(!args.contains(&"--mcp-config".to_owned()));
        assert!(!args.contains(&"--strict-mcp-config".to_owned()));
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

        assert_eq!(
            parse_init_auth_probe_success(format!("{INIT}\n{OK}\n").as_bytes()),
            Some(InitAuthProbeSuccess::ExactOk)
        );
        assert_eq!(parse_init_auth_probe_success(OK.as_bytes()), None);
        assert_eq!(
            parse_init_auth_probe_success(format!("{INIT}\n{{not-json}}\n{OK}\n").as_bytes()),
            None
        );
        assert_eq!(
            parse_init_auth_probe_success(
                format!("{INIT}\n{OK}\n{{\"type\":\"assistant\"}}\n").as_bytes()
            ),
            None
        );
        let non_exact = r#"{"type":"result","subtype":"success","is_error":false,"result":"OK\n"}"#;
        assert_eq!(
            parse_init_auth_probe_success(format!("{INIT}\n{non_exact}\n").as_bytes()),
            None
        );
        let api_key_init = r#"{"type":"system","subtype":"init","apiKeySource":"api-key"}"#;
        assert_eq!(
            parse_init_auth_probe_success(format!("{api_key_init}\n{OK}\n").as_bytes()),
            None
        );
    }

    #[test]
    fn init_auth_probe_accepts_authenticated_rate_limit_rejection() {
        const INIT: &str = r#"{"type":"system","subtype":"init","apiKeySource":"none"}"#;
        const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#;
        const ERROR: &str =
            r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;

        assert_eq!(
            parse_init_auth_probe_success(format!("{INIT}\n{RATE_LIMIT}\n{ERROR}\n").as_bytes()),
            Some(InitAuthProbeSuccess::AuthenticatedRateLimitRejection)
        );
        assert_eq!(
            parse_init_auth_probe_success(format!("{RATE_LIMIT}\n{ERROR}\n").as_bytes()),
            None
        );
    }

    #[test]
    fn init_auth_verdict_accepts_authenticated_rate_limit_exit_one() {
        let output = crate::cc::sandbox_process::SandboxOutput {
            code: 1,
            stdout: br#"{"type":"system","subtype":"init","apiKeySource":"none"}
{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}
{"type":"result","subtype":"error_during_execution","is_error":true}"#
                .to_vec(),
            stderr: Vec::new(),
        };

        let stdout = init_auth_verdict(output)
            .expect("authenticated rate-limit rejection must validate the credential");
        assert_eq!(
            parse_init_auth_probe_success(&stdout),
            Some(InitAuthProbeSuccess::AuthenticatedRateLimitRejection)
        );
    }

    #[test]
    fn init_auth_verdict_rejects_exact_ok_exit_one() {
        let output = crate::cc::sandbox_process::SandboxOutput {
            code: 1,
            stdout: br#"{"type":"system","subtype":"init","apiKeySource":"none"}
{"type":"result","subtype":"success","is_error":false,"result":"OK"}"#
                .to_vec(),
            stderr: Vec::new(),
        };

        assert!(init_auth_verdict(output).is_err());
    }

    #[test]
    fn probe_tail_detects_authenticated_rate_limit_rejection() {
        const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#;
        const RESULT_429: &str = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"You've hit your weekly limit"}"#;
        const NOT_LOGGED_IN: &str = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        const ECHO: &str = r#"{"type":"assistant","message":{"content":[]}}"#;

        assert!(probe_tail_is_authenticated_rate_limit(&[
            RATE_LIMIT.to_owned(),
            RESULT_429.to_owned(),
        ]));
        assert!(probe_tail_is_authenticated_rate_limit(&[
            RESULT_429.to_owned()
        ]));
        assert!(probe_tail_is_authenticated_rate_limit(&[
            ECHO.to_owned(),
            RATE_LIMIT.to_owned(),
        ]));
        assert!(!probe_tail_is_authenticated_rate_limit(&[
            NOT_LOGGED_IN.to_owned()
        ]));
        assert!(!probe_tail_is_authenticated_rate_limit(&[]));
        assert!(!probe_tail_is_authenticated_rate_limit(&[
            "not-json".to_owned()
        ]));
    }

    #[test]
    fn probe_exit_verdict_accepts_authenticated_rate_limit_exit_one() {
        const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#;
        const RESULT_429: &str = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"You've hit your weekly limit"}"#;

        assert!(probe_exit_verdict(1, &[RATE_LIMIT.to_owned(), RESULT_429.to_owned()]).is_ok());
    }

    #[test]
    fn probe_exit_verdict_keeps_nonzero_exit_failure_with_stdout_tail() {
        const NOT_LOGGED_IN: &str = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;

        let err = probe_exit_verdict(1, &[NOT_LOGGED_IN.to_owned()])
            .expect_err("non-429 nonzero exit must stay a failure");
        assert!(err.contains("exit code: 1"), "{err}");
        assert!(err.contains("Not logged in"), "{err}");
    }

    #[test]
    fn init_auth_guest_script_isolates_competing_auth_and_hides_the_token() {
        let args = init_auth_probe_invocation(None).into_args();
        let script = init_auth_probe_script(&args);

        // The token is not an input to the script: it can only reach the guest
        // through `SandboxCommand::env`, which is not readable from the guest
        // process table the way argv and the script text are.
        // The env script is sourced first so `claude` is on PATH at all; the
        // competing-credential unset still happens before anything runs.
        assert!(script.contains(crate::sandbox::GUEST_ENV_SCRIPT));
        assert!(script.contains("unset ANTHROPIC_API_KEY"));
        assert!(
            script.find(crate::sandbox::GUEST_ENV_SCRIPT) < script.find("exec \"$CLAUDE_BIN\""),
            "PATH must be set up before the binary is resolved: {script}"
        );
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

    #[test]
    fn init_auth_timeout_remains_an_error() {
        let error = init_auth_outcome(Ok(AuthCommandOutcome::TimedOut))
            .expect_err("init validation must fail fast on timeout");

        assert_eq!(
            error.to_string(),
            "Claude authentication validation timed out"
        );
    }

    #[test]
    fn runtime_auth_status_is_local_presence_and_syntax_check() {
        for (token, expected) in [
            (None, RuntimeAuthStatus::Missing),
            (Some(""), RuntimeAuthStatus::Invalid),
            (Some("bad\rcredential"), RuntimeAuthStatus::Invalid),
            (Some("bad\ncredential"), RuntimeAuthStatus::Invalid),
            (Some("stored-token"), RuntimeAuthStatus::Valid),
        ] {
            assert_eq!(classify_runtime_auth_token(token), expected);
        }
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

    /// A runtime handle with no sandbox published — the state the bot is in
    /// while bring-up has failed or a recovery is in flight.
    fn unavailable_runtime() -> Arc<crate::sandbox_runtime::SandboxRuntimeHandle> {
        // The supervisor's failure receiver is irrelevant here: reports are
        // coalesced with `try_send` and dropped when nobody listens.
        let (handle, _rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(Err(Arc::new(
            right_sandbox::SandboxCause::HypervisorUnavailable.diagnose(),
        )));
        handle
    }

    #[tokio::test]
    async fn every_repair_step_refuses_to_run_without_a_sandbox() {
        let health = McpInitHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            unavailable_runtime(),
        );
        let refusal = "refusing to run agent 'him' on the host: its sandbox is unavailable";

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

    /// Regression for the startup-snapshot bug: the keepalive task runs for the
    /// bot's whole life, across sandbox recoveries that each publish a NEW
    /// handle. It must therefore hold the runtime handle, not a sandbox: it
    /// resolves nothing when built, and resolves once per probe/repair step.
    #[tokio::test]
    async fn every_step_resolves_the_sandbox_at_use_time() {
        let runtime = unavailable_runtime();
        let health = McpInitHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            Arc::clone(&runtime),
        );
        assert_eq!(
            runtime.sandbox_reads(),
            0,
            "building McpInitHealth must not snapshot the sandbox"
        );

        let _ = run_health_probe(&health).await;
        let _ = remove_needs_auth_cache(&health).await;
        let _ = sync_after_cache_cleanup(&health).await;

        assert_eq!(
            runtime.sandbox_reads(),
            3,
            "each step reads the handle published at that moment"
        );
    }

    #[tokio::test]
    async fn repair_notice_is_one_shot() {
        let health = McpInitHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            unavailable_runtime(),
        );

        assert_eq!(health.consume_repair_notice(), None);
        health.mark_repaired_for_next_turn();
        assert_eq!(health.consume_repair_notice(), Some(REPAIR_NOTICE));
        assert_eq!(health.consume_repair_notice(), None);
    }

    #[tokio::test]
    async fn repair_lock_rejects_concurrent_second_holder() {
        let health = McpInitHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            unavailable_runtime(),
        );

        let first = health.try_begin_repair_for_test();
        assert!(first.is_some());
        assert!(health.try_begin_repair_for_test().is_none());
        drop(first);
        assert!(health.try_begin_repair_for_test().is_some());
    }

    #[tokio::test]
    async fn repair_timeout_releases_repair_lock() {
        let health = McpInitHealth::new(
            "him".to_owned(),
            PathBuf::from("/tmp/agent"),
            unavailable_runtime(),
        );

        health
            .trigger_repair_with_timeout("test", Duration::from_millis(1), |_health| async {
                std::future::pending::<()>().await;
            })
            .await;

        assert!(health.try_begin_repair_for_test().is_some());
    }
}
