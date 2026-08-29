use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

/// Built-in CC harness tools blocked for every agent-driven `claude -p` call.
///
/// `Cron*` / memory / etc. are reserved for our MCP equivalents; the rest are
/// harness-only tools (dynamic /loop wakeup, plan mode, worktree juggling,
/// push notifications, in-process Monitor) that don't belong in a headless
/// Telegram-driven agent. Only list tool names the running `claude` still
/// registers — denying a removed/renamed tool just emits a startup
/// "matches no known tool" warning (see the `TeamCreate`/`TeamDelete` note
/// below).
///
/// `Agent` is NOT in this list — foreground workers use subagents legitimately.
/// Callsites layer on additional denies when needed.
pub(crate) const BASELINE_DISALLOWED_TOOLS: &[&str] = &[
    // Right Agent provides MCP equivalents — block harness versions.
    "CronCreate",
    "CronList",
    "CronDelete",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
    "TaskOutput",
    "TaskStop",
    // Harness-only tools that don't fit a headless Telegram agent.
    "EnterPlanMode",
    "ExitPlanMode",
    "RemoteTrigger",
    "ScheduleWakeup",
    "EnterWorktree",
    "ExitWorktree",
    "Monitor",
    "PushNotification",
    // NOTE: `TeamCreate`/`TeamDelete` were removed from headless Claude Code
    // upstream (present in 2.1.143/2.1.173, gone by 2.1.177). Denying a tool
    // name CC no longer registers only emits a startup
    // "matches no known tool — check for typos." warning, so they are
    // intentionally not listed. Harness tool names drift across releases.
    "AskUserQuestion",
];

pub(crate) fn baseline_disallowed_tools() -> Vec<String> {
    BASELINE_DISALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

pub(crate) use right_mcp::internal_client::PROGRESS_MCP_TOOL as SEND_PROGRESS_MCP_TOOL;
pub(crate) use right_mcp::internal_client::SEND_MESSAGE_MCP_TOOL;

pub(crate) fn disallow_send_progress(mut tools: Vec<String>) -> Vec<String> {
    if !tools.iter().any(|tool| tool == SEND_PROGRESS_MCP_TOOL) {
        tools.push(SEND_PROGRESS_MCP_TOOL.to_owned());
    }
    tools
}

pub(crate) fn disallow_send_message(mut tools: Vec<String>) -> Vec<String> {
    if !tools.iter().any(|tool| tool == SEND_MESSAGE_MCP_TOOL) {
        tools.push(SEND_MESSAGE_MCP_TOOL.to_owned());
    }
    tools
}

/// `channel_post` is foreground+cron only: hide it from background-continuation,
/// delivery, and reflection invocations. It intentionally does not belong in
/// the shared foreground-only chains because cron uses those chains.
pub(crate) fn disallow_channel_post(mut tools: Vec<String>) -> Vec<String> {
    const TOOL: &str = right_mcp::internal_client::CHANNEL_POST_MCP_TOOL;
    if !tools.iter().any(|tool| tool == TOOL) {
        tools.push(TOOL.to_owned());
    }
    tools
}

pub(crate) fn disallow_learning_tools(mut tools: Vec<String>) -> Vec<String> {
    for tool_name in [
        right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}

pub(crate) fn disallow_conversation_search(mut tools: Vec<String>) -> Vec<String> {
    for tool_name in [
        right_mcp::internal_client::THREAD_SEARCH_MCP_TOOL,
        right_mcp::internal_client::CHAT_SEARCH_MCP_TOOL,
        right_mcp::internal_client::GET_MESSAGES_BY_ID_MCP_TOOL,
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}

pub(crate) fn disallow_thread_focus_set(mut tools: Vec<String>) -> Vec<String> {
    if !tools
        .iter()
        .any(|tool| tool == right_mcp::internal_client::THREAD_FOCUS_SET_MCP_TOOL)
    {
        tools.push(right_mcp::internal_client::THREAD_FOCUS_SET_MCP_TOOL.to_owned());
    }
    tools
}

pub(crate) fn disallow_forum_topic_tools(mut tools: Vec<String>) -> Vec<String> {
    for tool_name in [
        right_mcp::internal_client::FORUM_TOPIC_CREATE_MCP_TOOL,
        right_mcp::internal_client::FORUM_TOPIC_EDIT_MCP_TOOL,
        right_mcp::internal_client::FORUM_TOPIC_CLOSE_MCP_TOOL,
        right_mcp::internal_client::FORUM_TOPIC_REOPEN_MCP_TOOL,
        right_mcp::internal_client::FORUM_TOPIC_LIST_MCP_TOOL,
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}

/// Foreground-only tool restrictions EXCEPT learning tools. Used by cron turns
/// that may author skills inline.
pub(crate) fn disallow_foreground_only_tools_keep_learning(tools: Vec<String>) -> Vec<String> {
    disallow_thread_focus_set(disallow_forum_topic_tools(disallow_conversation_search(
        disallow_send_message(disallow_send_progress(tools)),
    )))
}

pub(crate) fn disallow_foreground_only_tools(tools: Vec<String>) -> Vec<String> {
    disallow_learning_tools(disallow_foreground_only_tools_keep_learning(tools))
}

pub(crate) fn disable_all_tools_args() -> Vec<String> {
    vec!["--tools".to_owned(), String::new()]
}

pub(crate) fn with_progress_invocation_header(
    mut config: serde_json::Value,
    invocation_id: &str,
) -> anyhow::Result<serde_json::Value> {
    let headers = config
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|servers| servers.get_mut("right"))
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|right| right.get_mut("headers"))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("mcp config missing mcpServers.right.headers object"))?;

    headers.insert(
        right_mcp::internal_client::PROGRESS_INVOCATION_HEADER.to_owned(),
        serde_json::Value::String(invocation_id.to_owned()),
    );
    Ok(config)
}

pub(crate) fn write_invocation_mcp_config(
    agent_dir: &Path,
    invocation_id: &str,
) -> anyhow::Result<PathBuf> {
    let base_path = agent_dir.join("mcp.json");
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&base_path)
            .map_err(|e| anyhow::anyhow!("read {}: {e:#}", base_path.display()))?,
    )?;
    let config = with_progress_invocation_header(config, invocation_id)?;

    let claude_dir = agent_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    let output_path = claude_dir.join(format!("mcp-{invocation_id}.json"));
    std::fs::write(&output_path, serde_json::to_string(&config)?)?;
    Ok(output_path)
}

#[derive(Clone)]
pub(crate) struct NonForegroundInvocationRegistration {
    pub(crate) agent_name: String,
    pub(crate) agent_dir: PathBuf,
    pub(crate) sandbox: crate::sandbox::Sandbox,
    pub(crate) internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub(crate) kind: right_mcp::internal_client::ProgressInvocationKindDto,
    pub(crate) chat_id: Option<i64>,
    pub(crate) thread_id: Option<i64>,
    /// Bot-local UDS progress registry for invocations that need Telegram
    /// delivery. Background learning callers without a bot-local route leave
    /// this absent.
    pub(crate) progress_state: Option<crate::telegram::progress::ProgressState>,
}

#[must_use = "registered invocations must be cleaned up with cleanup().await"]
pub(crate) struct RegisteredNonForegroundInvocation {
    invocation_id: String,
    agent_name: String,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
    local_mcp_config_path: PathBuf,
    claude_mcp_config_path: String,
    sandbox: crate::sandbox::Sandbox,
    sandbox_mcp_config_path: Option<String>,
    progress_state: Option<crate::telegram::progress::ProgressState>,

    cleaned: bool,
}

impl RegisteredNonForegroundInvocation {
    pub(crate) fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    pub(crate) fn mcp_config_path(&self) -> &str {
        &self.claude_mcp_config_path
    }

    pub(crate) async fn cleanup(mut self) {
        if self.cleaned {
            return;
        }
        unregister_invocation(&self.internal_client, &self.agent_name, &self.invocation_id).await;
        self.cleanup_local_and_sandbox();
    }

    /// Drop the local MCP config file and schedule sandbox-side cleanup.
    /// Sync — safe for `Drop`. Does NOT unregister the invocation (only the
    /// async `cleanup()` path does that).
    fn cleanup_local_and_sandbox(&mut self) {
        if let Some(progress_state) = &self.progress_state {
            progress_state.unregister(&self.invocation_id);
        }

        remove_invocation_mcp_config_file(&self.local_mcp_config_path);
        if let Some(sandbox_path) = self.sandbox_mcp_config_path.take() {
            spawn_sandbox_invocation_mcp_cleanup(
                self.invocation_id.clone(),
                Arc::clone(&self.sandbox),
                sandbox_path,
            );
        }
        self.cleaned = true;
    }
}

impl Drop for RegisteredNonForegroundInvocation {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleanup_local_and_sandbox();
    }
}

/// Everything registration does on the host: announce the invocation to the
/// aggregator and write its per-invocation MCP config, unwinding the
/// announcement when the write fails.
///
/// Split from the guest upload because the two have different failure domains
/// — and because this half is the one that has no sandbox dependency.
async fn register_invocation_on_host(
    internal_client: &right_mcp::internal_client::InternalClient,
    agent_dir: &Path,
    register_req: &right_mcp::internal_client::ProgressRegisterRequest,
) -> anyhow::Result<PathBuf> {
    internal_client
        .progress_register(register_req)
        .await
        .with_context(|| format!("register invocation {}", register_req.invocation_id))?;

    match write_invocation_mcp_config(agent_dir, &register_req.invocation_id) {
        Ok(path) => Ok(path),
        Err(e) => {
            unregister_invocation(
                internal_client,
                &register_req.agent,
                &register_req.invocation_id,
            )
            .await;
            Err(e).context("write invocation MCP config")
        }
    }
}

pub(crate) async fn register_non_foreground_invocation(
    registration: NonForegroundInvocationRegistration,
) -> anyhow::Result<RegisteredNonForegroundInvocation> {
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let register_req = right_mcp::internal_client::ProgressRegisterRequest {
        agent: registration.agent_name.clone(),
        invocation_id: invocation_id.clone(),
        kind: registration.kind,
        bot_send_token: right_runtime_state::generate_pc_api_token(),
        chat_id: registration.chat_id,
        thread_id: registration.thread_id,
    };

    let local_mcp_config_path = register_invocation_on_host(
        &registration.internal_client,
        &registration.agent_dir,
        &register_req,
    )
    .await?;

    let sandbox_mcp_config_path = invocation_sandbox_mcp_path(&invocation_id);
    if let Err(e) = crate::sandbox::upload_into_dir(
        &registration.sandbox,
        &local_mcp_config_path,
        "/sandbox/.claude",
    )
    .await
    {
        unregister_invocation(
            &registration.internal_client,
            &registration.agent_name,
            &invocation_id,
        )
        .await;
        remove_invocation_mcp_config_file(&local_mcp_config_path);
        anyhow::bail!("upload invocation MCP config: {e:#}");
    }

    if let Some(progress_state) = registration.progress_state.as_ref() {
        progress_state.register(crate::telegram::progress::ProgressTarget {
            invocation_id: invocation_id.clone(),
            token: register_req.bot_send_token.clone(),
            chat_id: registration.chat_id.unwrap_or_default(),
            thread_id: registration.thread_id.unwrap_or_default(),
            agent_dir: registration.agent_dir.clone(),
            sandbox: Some(Arc::clone(&registration.sandbox)),
            channel_post_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
    }

    Ok(RegisteredNonForegroundInvocation {
        invocation_id,
        agent_name: registration.agent_name,
        internal_client: registration.internal_client,
        local_mcp_config_path,
        claude_mcp_config_path: sandbox_mcp_config_path.clone(),
        sandbox: registration.sandbox,
        sandbox_mcp_config_path: Some(sandbox_mcp_config_path),
        progress_state: registration.progress_state,
        cleaned: false,
    })
}

fn invocation_sandbox_mcp_path(invocation_id: &str) -> String {
    format!("/sandbox/.claude/mcp-{invocation_id}.json")
}

async fn unregister_invocation(
    internal_client: &right_mcp::internal_client::InternalClient,
    agent_name: &str,
    invocation_id: &str,
) {
    let unregister_req = right_mcp::internal_client::ProgressUnregisterRequest {
        agent: agent_name.to_owned(),
        invocation_id: invocation_id.to_owned(),
    };
    if let Err(e) = internal_client.progress_unregister(&unregister_req).await {
        tracing::warn!(invocation_id, "invocation unregister failed: {e:#}");
    }
}

fn remove_invocation_mcp_config_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "invocation MCP config cleanup failed: {e:#}"
            );
        }
    }
}

fn spawn_sandbox_invocation_mcp_cleanup(
    invocation_id: String,
    sandbox: crate::sandbox::Sandbox,
    sandbox_path: String,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!(
            invocation_id,
            sandbox_path,
            "sandbox invocation MCP config cleanup skipped: no Tokio runtime"
        );
        return;
    };
    std::mem::drop(handle.spawn(async move {
        if let Err(e) = sandbox.fs_remove(&sandbox_path).await {
            tracing::warn!(
                invocation_id,
                sandbox_path,
                "sandbox invocation MCP config cleanup failed: {e:#}"
            );
        }
    }));
}

#[derive(Debug)]
pub(crate) enum ChildOutput {
    Completed(crate::cc::sandbox_process::SandboxOutput),
    TimedOut,
}

pub(crate) async fn wait_with_output_or_kill(
    mut child: crate::cc::sandbox_process::SandboxChild,
    timeout: Duration,
) -> anyhow::Result<ChildOutput> {
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result
            .map(ChildOutput::Completed)
            .context("wait for guest process"),
        Err(_) => {
            child.kill().await;
            drop(child);
            Ok(ChildOutput::TimedOut)
        }
    }
}

/// CC output format flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    StreamJson,
    Json,
}

/// Builder-style struct for assembling `claude -p` CLI arguments.
#[derive(Debug, Clone)]
pub(crate) struct ClaudeInvocation {
    pub(crate) mcp_config_path: Option<String>,
    pub(crate) json_schema: Option<String>,
    pub(crate) output_format: OutputFormat,
    pub(crate) model: Option<String>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) max_turns: Option<u32>,
    pub(crate) resume_session_id: Option<String>,
    pub(crate) new_session_id: Option<String>,
    pub(crate) fork_session: bool,
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) disallowed_tools: Vec<String>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) prompt: Option<String>,
    /// Hot-reloadable debug toggle. None = off (treated as false).
    /// When true at `into_args()` time, appends `--debug --debug-file=<path>`.
    pub(crate) debug_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl ClaudeInvocation {
    /// Consume self and produce the full argument list for spawning `claude`.
    pub(crate) fn into_args(self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // Pre-compute debug state before session fields are moved below.
        // `debug_on`: whether to emit --debug at all.
        // `debug_file_arg`: the --debug-file=<path> value, if we know the session UUID.
        // Priority matches `effective_session_id`: fork wins, then resume, then new.
        let debug_on = self
            .debug_flag
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
        let debug_file_arg: Option<String> = if debug_on {
            let sid: Option<&str> = if self.fork_session {
                self.new_session_id.as_deref()
            } else if self.resume_session_id.is_some() {
                self.resume_session_id.as_deref()
            } else {
                self.new_session_id.as_deref()
            };
            sid.map(|s| format!("--debug-file=/sandbox/.claude/logs/{s}.log"))
        } else {
            None
        };

        // 1. Base command
        args.extend(["claude", "-p", "--dangerously-skip-permissions"].map(Into::into));

        // 2. MCP config
        if let Some(mcp_path) = self.mcp_config_path {
            args.push("--mcp-config".into());
            args.push(mcp_path);
            args.push("--strict-mcp-config".into());
        }

        // 3. Allowed / disallowed tools
        if !self.allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.push(self.allowed_tools.join(","));
        }
        if !self.disallowed_tools.is_empty() {
            args.push("--disallowedTools".into());
            args.extend(self.disallowed_tools);
        }

        // 4. Session
        if let Some(resume_id) = self.resume_session_id {
            args.push("--resume".into());
            args.push(resume_id);
            if self.fork_session {
                args.push("--fork-session".into());
                if let Some(new_id) = self.new_session_id {
                    args.push("--session-id".into());
                    args.push(new_id);
                }
            }
        } else if let Some(id) = self.new_session_id {
            args.push("--session-id".into());
            args.push(id);
        }

        // 5. Model
        if let Some(model) = self.model {
            args.push("--model".into());
            args.push(model);
        }

        // 6. Budget
        if let Some(budget) = self.max_budget_usd {
            args.push("--max-budget-usd".into());
            args.push(format!("{budget:.2}"));
        }

        // 7. Max turns
        if let Some(turns) = self.max_turns {
            args.push("--max-turns".into());
            args.push(turns.to_string());
        }

        // 8. Extra args
        args.extend(self.extra_args);

        // 8.5: Debug flag (hot-reloadable, read at build time).
        // `debug_on` / `debug_file_arg` were computed at the top before session fields moved.
        if debug_on {
            args.push("--debug".into());
            if let Some(file_arg) = debug_file_arg {
                args.push(file_arg);
            }
        }

        // 9. Output format (--verbose only for stream-json)
        match self.output_format {
            OutputFormat::StreamJson => {
                args.push("--verbose".into());
                args.push("--output-format".into());
                args.push("stream-json".into());
            }
            OutputFormat::Json => {
                args.push("--output-format".into());
                args.push("json".into());
            }
        }

        // 10. JSON schema
        if let Some(schema) = self.json_schema {
            args.push("--json-schema".into());
            args.push(schema);
        }

        // 11. Prompt
        if let Some(prompt) = self.prompt {
            args.push("--".into());
            args.push(prompt);
        }

        args
    }
}

/// Quote a `claude` argv into a single guest shell command line.
///
/// The guest command is always `sh -c <script>` because every call site
/// prepends a system-prompt assembly script, so the argv must be quoted the
/// same way the SSH remote command quoted it.
pub(crate) fn quote_guest_args<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> Result<String, shlex::QuoteError> {
    shlex::try_join(args)
}

/// Error returned when a turn would run outside the Agent Sandbox.
///
/// Fail-closed: an agent whose sandbox backend is unavailable MUST NOT fall
/// back to host execution — that would run `--dangerously-skip-permissions`
/// outside the sandbox (sandbox escape). Since every agent is sandboxed, this
/// is simply "no sandbox, no turn".
#[derive(Debug, thiserror::Error)]
#[error("refusing to run agent '{agent}' on the host: its sandbox is unavailable")]
pub(crate) struct SandboxedHostExecRefused {
    pub agent: String,
}

/// Fail-closed guard. Call BEFORE constructing any Claude command.
///
/// Returns the live sandbox handle, so a caller cannot reach a Claude command
/// without passing the guard: there is no host branch to fall through to.
pub(crate) fn guard_no_sandboxed_host_exec<'a>(
    agent: &str,
    sandbox: Option<&'a crate::sandbox::Sandbox>,
) -> Result<&'a crate::sandbox::Sandbox, SandboxedHostExecRefused> {
    sandbox.ok_or_else(|| SandboxedHostExecRefused {
        agent: agent.to_owned(),
    })
}

/// Build the guest command for a `ClaudeInvocation` argv, with the auth token
/// injected as a per-command environment variable.
///
/// The token never enters the script text or the argv: both are visible to
/// anything that can list guest processes, while per-exec env is not.
///
/// Stdio is NOT configured — the caller sets stdin/stdout/stderr after.
pub(crate) async fn build_claude_command(
    args: &[String],
    agent_dir: &Path,
    sandbox: &crate::sandbox::Sandbox,
) -> anyhow::Result<crate::cc::sandbox_process::SandboxCommand> {
    let script =
        quote_guest_args(args.iter().map(String::as_str)).expect("claude args contain no NUL byte");
    build_claude_script_command(script, agent_dir, sandbox).await
}

/// Build the guest command for an already-assembled shell script (the
/// system-prompt assembly script plus the quoted `claude` argv).
pub(crate) async fn build_claude_script_command(
    script: String,
    agent_dir: &Path,
    sandbox: &crate::sandbox::Sandbox,
) -> anyhow::Result<crate::cc::sandbox_process::SandboxCommand> {
    // `claude` installs into /sandbox/.local/bin, which reaches PATH through
    // /sandbox/.right/env.sh. Under the old SSH transport a login shell
    // sourced that via .bashrc; a direct guest exec gets no login shell, so
    // the script has to source it itself or `claude` is simply not found.
    let script = format!(
        "if [ -r {env} ]; then . {env}; fi\n{script}",
        env = crate::sandbox::GUEST_ENV_SCRIPT,
    );
    build_claude_script_command_with_token(
        crate::login::load_auth_token(agent_dir).await,
        |token| {
            let command = crate::cc::sandbox_process::SandboxCommand::shell(sandbox, script);
            match token {
                Some(token) => command.env("CLAUDE_CODE_OAUTH_TOKEN", token),
                None => command,
            }
        },
    )
}

fn build_claude_script_command_with_token<T>(
    token_result: anyhow::Result<Option<String>>,
    build: impl FnOnce(Option<String>) -> T,
) -> anyhow::Result<T> {
    let token = token_result.context("load Claude authentication token")?;
    Ok(build(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn guard_refuses_when_the_sandbox_is_gone() {
        // The fail-closed case, and the only case reachable without a live
        // sandbox: no handle → no turn. There is no host branch to allow,
        // because the guard hands back the handle every caller needs.
        let refused = guard_no_sandboxed_host_exec("agent-x", None)
            .expect_err("a missing sandbox must refuse");
        assert_eq!(refused.agent, "agent-x");
        assert!(format!("{refused}").contains("refusing to run agent 'agent-x' on the host"));
    }

    #[test]
    fn command_build_preserves_auth_load_error_chain() {
        let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "database denied");
        let error = build_claude_script_command_with_token(
            Err(anyhow::Error::new(source).context("open auth token database")),
            |_| (),
        )
        .expect_err("auth DB failure must prevent command construction");

        assert_eq!(
            format!("{error:#}"),
            "load Claude authentication token: open auth token database: database denied"
        );
    }

    #[test]
    fn command_build_allows_an_absent_auth_token() {
        let built = build_claude_script_command_with_token(Ok(None), |token| {
            assert!(token.is_none());
            "built"
        })
        .expect("an absent token row is not an error");
        assert_eq!(built, "built");
    }

    fn minimal() -> ClaudeInvocation {
        ClaudeInvocation {
            mcp_config_path: Some("/sandbox/mcp.json".into()),
            json_schema: Some(r#"{"type":"object"}"#.into()),
            output_format: OutputFormat::StreamJson,
            model: None,
            max_budget_usd: None,
            max_turns: None,
            resume_session_id: None,
            new_session_id: None,
            fork_session: false,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            extra_args: vec![],
            prompt: Some("hello".into()),
            debug_flag: None,
        }
    }

    struct CapturedProgressRequest {
        path: String,
        body: serde_json::Value,
    }

    fn write_base_mcp_config(agent_dir: &Path) {
        std::fs::write(
            agent_dir.join("mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "right": {
                        "headers": {
                            "Authorization": "Bearer existing-token"
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn spawn_progress_api(
        socket_path: PathBuf,
        request_count: usize,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<CapturedProgressRequest>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::UnixListener::bind(socket_path).unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().await.unwrap();
                let captured = read_progress_request(&mut stream).await;
                tx.send(captured).unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                    )
                    .await
                    .unwrap();
            }
        });
        (rx, handle)
    }

    async fn read_progress_request(stream: &mut tokio::net::UnixStream) -> CapturedProgressRequest {
        let mut buf = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "client closed before HTTP request completed");
            buf.extend_from_slice(&chunk[..n]);

            let Some(header_end) = header_end(&buf) else {
                continue;
            };
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = content_length(&headers);
            let body_start = header_end + 4;
            if buf.len() >= body_start + content_length {
                let request_line = headers.lines().next().unwrap();
                let path = request_line.split_whitespace().nth(1).unwrap().to_owned();
                let body =
                    serde_json::from_slice(&buf[body_start..body_start + content_length]).unwrap();
                return CapturedProgressRequest { path, body };
            }
        }
    }

    fn header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|window| window == b"\r\n\r\n")
    }

    fn content_length(headers: &str) -> usize {
        headers
            .lines()
            .find_map(|line| {
                let mut parts = line.splitn(2, ':');
                let name = parts.next()?.trim();
                let value = parts.next()?.trim();
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.parse::<usize>().unwrap())
            })
            .unwrap()
    }

    #[tokio::test]
    async fn minimal_invocation_has_invariants() {
        let args = minimal().into_args();
        assert_eq!(args[0], "claude");
        assert_eq!(args[1], "-p");
        assert_eq!(args[2], "--dangerously-skip-permissions");
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/sandbox/mcp.json".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.contains(&"--verbose".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--json-schema".to_string()));
    }

    #[tokio::test]
    async fn prompt_comes_after_double_dash() {
        let args = minimal().into_args();
        let dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[dash_pos + 1], "hello");
    }

    #[tokio::test]
    async fn no_prompt_no_double_dash() {
        let mut inv = minimal();
        inv.prompt = None;
        let args = inv.into_args();
        assert!(!args.contains(&"--".to_string()));
    }

    #[test]
    fn guest_script_quotes_claude_argv_as_one_shell_command() {
        let args = vec![
            "claude".to_string(),
            "-p".to_string(),
            "--".to_string(),
            "alpha beta; $(nope) quote'arg".to_string(),
        ];

        let script = quote_guest_args(args.iter().map(String::as_str)).expect("quotable argv");

        // The guest runs `sh -c <script>`, so the argv must survive one round
        // of shell word-splitting with no expansion of the payload.
        let probe = format!(
            "claude() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {script}"
        );
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(probe)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "quoted claude args should parse under sh; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "<-p>\n<-->\n<alpha beta; $(nope) quote'arg>\n"
        );
    }

    #[tokio::test]
    async fn optional_model() {
        let mut inv = minimal();
        inv.model = Some("claude-haiku-4-5-20251001".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[pos + 1], "claude-haiku-4-5-20251001");
    }

    #[tokio::test]
    async fn optional_budget() {
        let mut inv = minimal();
        inv.max_budget_usd = Some(1.5);
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--max-budget-usd").unwrap();
        assert_eq!(args[pos + 1], "1.50");
    }

    #[tokio::test]
    async fn optional_max_turns() {
        let mut inv = minimal();
        inv.max_turns = Some(10);
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--max-turns").unwrap();
        assert_eq!(args[pos + 1], "10");
    }

    #[tokio::test]
    async fn disallowed_tools_expanded() {
        let mut inv = minimal();
        inv.disallowed_tools = vec!["CronCreate".into(), "CronList".into()];
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--disallowedTools").unwrap();
        assert_eq!(args[pos + 1], "CronCreate");
        assert_eq!(args[pos + 2], "CronList");
    }

    #[tokio::test]
    async fn disable_all_tools_args_emit_empty_tools_flag() {
        let mut inv = minimal();
        inv.extra_args = disable_all_tools_args();

        let args = inv.into_args();

        let pos = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[pos + 1], "");
    }

    #[tokio::test]
    async fn baseline_disallowed_tools_blocks_harness_self_loops() {
        let baseline = baseline_disallowed_tools();
        for required in [
            "ScheduleWakeup",
            "EnterWorktree",
            "ExitWorktree",
            "Monitor",
            "PushNotification",
            "AskUserQuestion",
            "EnterPlanMode",
            "RemoteTrigger",
            "CronCreate",
            "TaskCreate",
        ] {
            assert!(
                baseline.iter().any(|s| s == required),
                "baseline must block {required}"
            );
        }
        // `TeamCreate`/`TeamDelete` were removed from headless Claude Code
        // upstream (gone by 2.1.177); denying a non-existent tool only emits a
        // startup "matches no known tool" warning, so baseline must NOT carry
        // them.
        for gone in ["TeamCreate", "TeamDelete"] {
            assert!(
                !baseline.iter().any(|s| s == gone),
                "baseline must NOT block removed-upstream tool {gone}"
            );
        }
        // Tools we deliberately keep available.
        for kept in ["SendMessage", "LSP", "WebFetch", "WebSearch", "Agent"] {
            assert!(
                !baseline.iter().any(|s| s == kept),
                "baseline must NOT block {kept}"
            );
        }
    }

    #[tokio::test]
    async fn invocation_mcp_config_adds_progress_header_and_preserves_authorization() {
        let config = serde_json::json!({
            "mcpServers": {
                "right": {
                    "command": "right-mcp",
                    "headers": {
                        "Authorization": "Bearer existing-token"
                    }
                }
            },
            "other": true
        });

        let updated = with_progress_invocation_header(config, "inv-1").unwrap();

        let headers = &updated["mcpServers"]["right"]["headers"];
        assert_eq!(headers["Authorization"], "Bearer existing-token");
        assert_eq!(headers["X-Right-Invocation"], "inv-1");
        assert_eq!(updated["other"], true);
    }

    #[tokio::test]
    async fn disallow_progress_adds_full_mcp_tool_name() {
        let tools = disallow_send_progress(vec!["Agent".to_owned()]);

        assert!(tools.iter().any(|tool| tool == SEND_PROGRESS_MCP_TOOL));
        assert!(tools.iter().any(|tool| tool == "Agent"));
    }

    #[tokio::test]
    async fn disallow_send_progress_is_idempotent() {
        let tools = disallow_send_progress(disallow_send_progress(Vec::new()));
        let count = tools
            .iter()
            .filter(|tool| tool.as_str() == SEND_PROGRESS_MCP_TOOL)
            .count();

        assert_eq!(count, 1);
    }

    #[test]
    fn disallow_channel_post_adds_the_tool_once() {
        let tools = disallow_channel_post(disallow_channel_post(Vec::new()));
        let count = tools
            .iter()
            .filter(|tool| tool.as_str() == right_mcp::internal_client::CHANNEL_POST_MCP_TOOL)
            .count();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn disallow_learning_tools_adds_full_mcp_tool_names() {
        let tools = disallow_learning_tools(vec!["Agent".to_owned()]);

        assert!(
            tools
                .iter()
                .any(|tool| { tool == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL })
        );
        assert!(
            tools
                .iter()
                .any(|tool| { tool == right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL })
        );
        assert!(tools.iter().any(|tool| tool == "Agent"));
    }

    #[tokio::test]
    async fn disallow_foreground_only_tools_is_idempotent() {
        let tools = disallow_foreground_only_tools(disallow_foreground_only_tools(Vec::new()));

        for tool_name in [
            SEND_PROGRESS_MCP_TOOL,
            SEND_MESSAGE_MCP_TOOL,
            right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
            right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
            right_mcp::internal_client::THREAD_SEARCH_MCP_TOOL,
            right_mcp::internal_client::CHAT_SEARCH_MCP_TOOL,
            right_mcp::internal_client::GET_MESSAGES_BY_ID_MCP_TOOL,
            right_mcp::internal_client::THREAD_FOCUS_SET_MCP_TOOL,
            right_mcp::internal_client::FORUM_TOPIC_CREATE_MCP_TOOL,
            right_mcp::internal_client::FORUM_TOPIC_EDIT_MCP_TOOL,
            right_mcp::internal_client::FORUM_TOPIC_CLOSE_MCP_TOOL,
            right_mcp::internal_client::FORUM_TOPIC_REOPEN_MCP_TOOL,
            right_mcp::internal_client::FORUM_TOPIC_LIST_MCP_TOOL,
        ] {
            let count = tools
                .iter()
                .filter(|tool| tool.as_str() == tool_name)
                .count();
            assert_eq!(count, 1, "{tool_name} should be present once");
        }
    }

    #[tokio::test]
    async fn write_invocation_mcp_config_writes_agent_scoped_file() {
        let temp = tempfile::tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            temp.path().join("mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "right": {
                        "headers": {
                            "Authorization": "Bearer existing-token"
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let path = write_invocation_mcp_config(temp.path(), "inv-1").unwrap();

        assert_eq!(path, claude_dir.join("mcp-inv-1.json"));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            written["mcpServers"]["right"]["headers"]["Authorization"],
            "Bearer existing-token"
        );
        assert_eq!(
            written["mcpServers"]["right"]["headers"]["X-Right-Invocation"],
            "inv-1"
        );
    }

    /// Build the aggregator announcement the way
    /// `register_non_foreground_invocation` does, so the tests below assert
    /// the wire shape it actually sends.
    fn register_request(
        invocation_id: &str,
        kind: right_mcp::internal_client::ProgressInvocationKindDto,
        chat_id: Option<i64>,
        thread_id: Option<i64>,
    ) -> right_mcp::internal_client::ProgressRegisterRequest {
        right_mcp::internal_client::ProgressRegisterRequest {
            agent: "agent-1".to_owned(),
            invocation_id: invocation_id.to_owned(),
            kind,
            bot_send_token: right_runtime_state::generate_pc_api_token(),
            chat_id,
            thread_id,
        }
    }

    #[tokio::test]
    async fn probe_writer_registration_announces_kind_and_writes_its_mcp_config() {
        let agent_dir = tempfile::tempdir().unwrap();
        write_base_mcp_config(agent_dir.path());
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("internal.sock");
        let (mut requests, server) = spawn_progress_api(socket_path.clone(), 2);
        let client = right_mcp::internal_client::InternalClient::new(&socket_path);

        let req = register_request(
            "inv-probe",
            right_mcp::internal_client::ProgressInvocationKindDto::ProbeWriter,
            Some(42),
            Some(7),
        );
        let local_mcp_path = register_invocation_on_host(&client, agent_dir.path(), &req)
            .await
            .expect("host-side registration");

        let register = requests.recv().await.unwrap();
        assert_eq!(register.path, "/progress/register");
        assert_eq!(register.body["kind"], "probe_writer");
        assert_ne!(register.body["kind"], "background_review");
        assert_eq!(register.body["chat_id"], 42);
        assert_eq!(register.body["thread_id"], 7);
        assert_eq!(register.body["invocation_id"], "inv-probe");

        assert_eq!(
            local_mcp_path,
            agent_dir.path().join(".claude").join("mcp-inv-probe.json")
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&local_mcp_path).unwrap()).unwrap();
        assert_eq!(
            written["mcpServers"]["right"]["headers"]["X-Right-Invocation"],
            "inv-probe"
        );

        // The cleanup half: unregister, then drop the per-invocation config.
        unregister_invocation(&client, "agent-1", "inv-probe").await;
        remove_invocation_mcp_config_file(&local_mcp_path);

        let unregister = requests.recv().await.unwrap();
        assert_eq!(unregister.path, "/progress/unregister");
        assert_eq!(unregister.body["invocation_id"], "inv-probe");
        assert!(
            !local_mcp_path.exists(),
            "cleanup should remove per-invocation MCP config"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn guest_mcp_config_path_is_per_invocation() {
        assert_eq!(
            invocation_sandbox_mcp_path("inv-9"),
            "/sandbox/.claude/mcp-inv-9.json"
        );
    }

    #[tokio::test]
    async fn cron_registration_is_visible_to_the_bot_uds_router() {
        use tower::ServiceExt as _;

        let agent_dir = tempfile::tempdir().expect("agent dir");
        write_base_mcp_config(agent_dir.path());
        let socket_dir = tempfile::tempdir().expect("socket dir");
        let socket_path = socket_dir.path().join("internal.sock");
        let (mut requests, server) = spawn_progress_api(socket_path.clone(), 1);
        let client = right_mcp::internal_client::InternalClient::new(&socket_path);
        let progress_state = crate::telegram::progress::ProgressState::default();

        let req = register_request(
            "inv-cron",
            right_mcp::internal_client::ProgressInvocationKindDto::Cron,
            None,
            None,
        );
        register_invocation_on_host(&client, agent_dir.path(), &req)
            .await
            .expect("register cron invocation");
        progress_state.register(crate::telegram::progress::ProgressTarget {
            invocation_id: req.invocation_id.clone(),
            token: req.bot_send_token.clone(),
            chat_id: 0,
            thread_id: 0,
            agent_dir: agent_dir.path().to_path_buf(),
            sandbox: None,
            channel_post_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });

        let register = requests
            .recv()
            .await
            .expect("progress registration request");
        assert_eq!(register.path, "/progress/register");
        assert_eq!(register.body["kind"], "cron");

        let app = crate::telegram::progress::build_progress_router(
            crate::telegram::progress::ProgressEndpointState {
                bot: crate::telegram::bot::build_bot("123:test".to_owned()),
                progress: progress_state.clone(),
            },
        );
        let body = serde_json::to_vec(&right_mcp::internal_client::ChannelPostRequest {
            invocation_id: req.invocation_id.clone(),
            token: "wrong-token".to_owned(),
            chat_id: -100,
            content: right_rich_content::RichContent::literal("test").unwrap(),
        })
        .expect("serialize channel post");
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/channel/post")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(body))
                    .expect("build channel post request"),
            )
            .await
            .expect("router request");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::FORBIDDEN,
            "the registered cron invocation must reach the channel-post token gate"
        );

        progress_state.unregister(&req.invocation_id);
        assert!(
            progress_state.get(&req.invocation_id).is_none(),
            "cleanup must remove the bot-local UDS target"
        );
        server.await.expect("progress API server");
    }

    #[tokio::test]
    async fn curator_registration_omits_chat_and_thread() {
        let agent_dir = tempfile::tempdir().unwrap();
        write_base_mcp_config(agent_dir.path());
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("internal.sock");
        let (mut requests, server) = spawn_progress_api(socket_path.clone(), 1);
        let client = right_mcp::internal_client::InternalClient::new(&socket_path);

        let req = register_request(
            "inv-curator",
            right_mcp::internal_client::ProgressInvocationKindDto::Curator,
            None,
            None,
        );
        register_invocation_on_host(&client, agent_dir.path(), &req)
            .await
            .expect("host-side registration");

        let register = requests.recv().await.unwrap();
        assert_eq!(register.path, "/progress/register");
        assert_eq!(register.body["kind"], "curator");
        assert_ne!(register.body["kind"], "background_review");
        assert!(register.body.get("chat_id").is_none());
        assert!(register.body.get("thread_id").is_none());
        assert_eq!(register.body["invocation_id"], "inv-curator");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn registration_unwinds_the_announcement_when_the_mcp_config_cannot_be_written() {
        // No base mcp.json in the agent dir: the config write must fail, and
        // the aggregator must not be left holding a registration for an
        // invocation that will never run.
        let agent_dir = tempfile::tempdir().unwrap();
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("internal.sock");
        let (mut requests, server) = spawn_progress_api(socket_path.clone(), 2);
        let client = right_mcp::internal_client::InternalClient::new(&socket_path);

        let req = register_request(
            "inv-doomed",
            right_mcp::internal_client::ProgressInvocationKindDto::ProbeWriter,
            None,
            None,
        );
        let error = register_invocation_on_host(&client, agent_dir.path(), &req)
            .await
            .expect_err("missing base MCP config must fail registration");
        assert!(format!("{error:#}").contains("write invocation MCP config"));

        assert_eq!(requests.recv().await.unwrap().path, "/progress/register");
        let unregister = requests.recv().await.unwrap();
        assert_eq!(unregister.path, "/progress/unregister");
        assert_eq!(unregister.body["invocation_id"], "inv-doomed");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn resume_session() {
        let mut inv = minimal();
        inv.resume_session_id = Some("abc-123".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[pos + 1], "abc-123");
    }

    #[tokio::test]
    async fn new_session() {
        let mut inv = minimal();
        inv.new_session_id = Some("def-456".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(args[pos + 1], "def-456");
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[tokio::test]
    async fn json_output_format() {
        let mut inv = minimal();
        inv.output_format = OutputFormat::Json;
        let args = inv.into_args();
        assert!(args.contains(&"json".to_string()));
        assert!(!args.contains(&"stream-json".to_string()));
        assert!(!args.contains(&"--verbose".to_string()));
    }

    #[tokio::test]
    async fn allowed_tools_joined() {
        let mut inv = minimal();
        inv.allowed_tools = vec!["WebSearch".into(), "WebFetch".into()];
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[pos + 1], "WebSearch,WebFetch");
    }

    #[tokio::test]
    async fn no_mcp_no_schema() {
        let mut inv = minimal();
        inv.mcp_config_path = None;
        inv.json_schema = None;
        let args = inv.into_args();
        assert!(!args.contains(&"--mcp-config".to_string()));
        assert!(!args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args.contains(&"--json-schema".to_string()));
    }

    #[test]
    fn mcp_config_path_is_the_fixed_guest_path() {
        // Every turn now runs in the guest, so there is one aggregator config
        // path — the host-vs-sandbox fork is gone.
        assert_eq!(crate::sandbox::SANDBOX_MCP_JSON_PATH, "/sandbox/mcp.json");
    }

    #[tokio::test]
    async fn fork_session_emits_resume_fork_and_session_id() {
        let mut inv = minimal();
        inv.resume_session_id = Some("main-uuid".into());
        inv.new_session_id = Some("fork-uuid".into());
        inv.fork_session = true;
        let args = inv.into_args();

        let resume_pos = args
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume missing");
        let fork_pos = args
            .iter()
            .position(|a| a == "--fork-session")
            .expect("--fork-session missing");
        let session_pos = args
            .iter()
            .position(|a| a == "--session-id")
            .expect("--session-id missing");

        assert!(
            resume_pos < fork_pos,
            "--resume must precede --fork-session"
        );
        assert!(
            fork_pos < session_pos,
            "--fork-session must precede --session-id"
        );
        assert_eq!(args[resume_pos + 1], "main-uuid");
        assert_eq!(args[session_pos + 1], "fork-uuid");
    }

    #[tokio::test]
    async fn fork_session_without_resume_does_not_emit_flag() {
        let mut inv = minimal();
        inv.new_session_id = Some("only-new".into());
        inv.fork_session = true;
        let args = inv.into_args();
        assert!(!args.contains(&"--fork-session".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[tokio::test]
    async fn debug_flag_true_appends_debug_and_debug_file() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(
            args.contains(&"--debug".to_string()),
            "expected --debug:\n{args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "--debug-file=/sandbox/.claude/logs/abc-123.log"),
            "expected --debug-file=/sandbox/.claude/logs/abc-123.log:\n{args:?}"
        );
    }

    #[tokio::test]
    async fn debug_flag_false_omits_debug_and_debug_file() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--debug-file=")));
    }

    #[tokio::test]
    async fn debug_flag_absent_omits_debug() {
        // No debug_flag set at all (None) — should behave like false.
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
    }

    #[tokio::test]
    async fn debug_flag_uses_resume_session_id_when_no_fork() {
        let mut inv = minimal();
        inv.resume_session_id = Some("resume-uuid".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(
            args.iter()
                .any(|a| a == "--debug-file=/sandbox/.claude/logs/resume-uuid.log"),
            "with --resume (no fork), debug-file should use resume-uuid:\n{args:?}"
        );
    }

    #[tokio::test]
    async fn debug_flag_uses_new_session_id_when_forking() {
        let mut inv = minimal();
        inv.resume_session_id = Some("old-uuid".into());
        inv.new_session_id = Some("new-uuid".into());
        inv.fork_session = true;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(
            args.iter()
                .any(|a| a == "--debug-file=/sandbox/.claude/logs/new-uuid.log"),
            "with --fork-session, debug-file should use new session id (CC writes JSONL by new id):\n{args:?}"
        );
    }

    #[tokio::test]
    async fn debug_flag_runtime_toggle_picked_up_at_build_time() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        // Flip after construction.
        flag.store(true, std::sync::atomic::Ordering::Release);
        let args = inv.into_args();
        assert!(
            args.contains(&"--debug".to_string()),
            "build-time read must observe the flip"
        );
    }

    #[tokio::test]
    async fn debug_flag_no_session_id_omits_debug_file_but_still_emits_debug() {
        let mut inv = minimal();
        // Neither resume nor new session id set.
        inv.resume_session_id = None;
        inv.new_session_id = None;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(args.contains(&"--debug".to_string()));
        // No --debug-file because we have no session UUID to put in the path.
        assert!(!args.iter().any(|a| a.starts_with("--debug-file=")));
    }

    #[test]
    fn keep_learning_variant_allows_learning_tools_but_disallows_others() {
        let kept = disallow_foreground_only_tools_keep_learning(baseline_disallowed_tools());
        assert!(
            !kept
                .iter()
                .any(|t| t == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL)
        );
        assert!(
            !kept
                .iter()
                .any(|t| t == right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL)
        );
        assert!(kept.iter().any(|t| t == SEND_PROGRESS_MCP_TOOL));
        assert!(kept.iter().any(|t| t == SEND_MESSAGE_MCP_TOOL));
        let full = disallow_foreground_only_tools(baseline_disallowed_tools());
        assert!(
            full.iter()
                .any(|t| t == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL)
        );
    }

    #[test]
    fn shared_foreground_only_disallow_chains_keep_channel_post_available() {
        let kept = disallow_foreground_only_tools_keep_learning(baseline_disallowed_tools());
        let full = disallow_foreground_only_tools(baseline_disallowed_tools());
        let channel_post = right_mcp::internal_client::CHANNEL_POST_MCP_TOOL;

        assert!(!kept.iter().any(|tool| tool == channel_post));
        assert!(!full.iter().any(|tool| tool == channel_post));
    }
}
