use std::path::{Path, PathBuf};

/// Built-in CC harness tools blocked for every agent-driven `claude -p` call.
///
/// `Cron*` / memory / etc. are reserved for our MCP equivalents; the rest are
/// harness-only tools (multi-agent UI, dynamic /loop wakeup, plan mode,
/// worktree juggling, push notifications, in-process Monitor) that don't
/// belong in a headless Telegram-driven agent.
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
    "TeamCreate",
    "TeamDelete",
    "AskUserQuestion",
];

pub(crate) fn baseline_disallowed_tools() -> Vec<String> {
    BASELINE_DISALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

pub(crate) use right_mcp::internal_client::PROGRESS_MCP_TOOL as SEND_PROGRESS_MCP_TOOL;

pub(crate) fn disallow_send_progress(mut tools: Vec<String>) -> Vec<String> {
    if !tools.iter().any(|tool| tool == SEND_PROGRESS_MCP_TOOL) {
        tools.push(SEND_PROGRESS_MCP_TOOL.to_owned());
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
    ] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}

pub(crate) fn disallow_foreground_only_tools(tools: Vec<String>) -> Vec<String> {
    disallow_conversation_search(disallow_learning_tools(disallow_send_progress(tools)))
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

/// Resolve MCP config path: sandbox path when SSH is configured, local path otherwise.
pub(crate) fn mcp_config_path(ssh_config_path: Option<&Path>, agent_dir: &Path) -> String {
    if ssh_config_path.is_some() {
        right_openshell::openshell::SANDBOX_MCP_JSON_PATH.to_string()
    } else {
        agent_dir.join("mcp.json").to_string_lossy().into_owned()
    }
}

/// Build a `tokio::process::Command` from `ClaudeInvocation` args, with auth
/// token injected, either inside an OpenShell sandbox (via SSH) or locally.
///
/// **SSH path**: shell-quotes args via `shlex`, prepends
/// `export CLAUDE_CODE_OAUTH_TOKEN=...`, passes as single SSH remote command.
///
/// **Local path**: uses `Command::args()` directly (no shell), injects token
/// via env var.
///
/// Stdio is NOT configured — caller must set stdin/stdout/stderr after.
pub(crate) fn build_claude_command(
    args: &[String],
    agent_dir: &Path,
    ssh_config_path: Option<&Path>,
    resolved_sandbox: Option<&str>,
) -> tokio::process::Command {
    if let Some(ssh_config) = ssh_config_path {
        let ssh_host = right_openshell::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mut script = String::new();
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
        }
        let quoted =
            right_openshell::openshell::quote_ssh_remote_args(args.iter().map(String::as_str))
                .expect("claude args should not contain nul bytes");
        script.push_str(&quoted);
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(script);
        c
    } else {
        let mut c = tokio::process::Command::new(&args[0]);
        c.args(&args[1..]);
        c.env("HOME", agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(agent_dir);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

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

    #[test]
    fn minimal_invocation_has_invariants() {
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

    #[test]
    fn prompt_comes_after_double_dash() {
        let args = minimal().into_args();
        let dash_pos = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[dash_pos + 1], "hello");
    }

    #[test]
    fn no_prompt_no_double_dash() {
        let mut inv = minimal();
        inv.prompt = None;
        let args = inv.into_args();
        assert!(!args.contains(&"--".to_string()));
    }

    #[test]
    fn ssh_invocation_quotes_claude_argv_as_one_remote_script() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec![
            "claude".to_string(),
            "-p".to_string(),
            "--".to_string(),
            "alpha beta; $(nope) quote'arg".to_string(),
        ];

        let cmd = build_claude_command(
            &args,
            temp.path(),
            Some(Path::new("config")),
            Some("example"),
        );
        let std_cmd = cmd.as_std();
        let ssh_args: Vec<String> = std_cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(std_cmd.get_program(), "ssh");
        assert_eq!(ssh_args[0], "-F");
        assert_eq!(ssh_args[1], "config");
        assert_eq!(ssh_args[2], "openshell-example");
        assert_eq!(ssh_args[3], "--");
        assert_eq!(
            ssh_args[4..].len(),
            1,
            "claude ssh invocation must pass exactly one remote script argument"
        );

        let probe = format!(
            "claude() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {}",
            ssh_args[4]
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

    #[test]
    fn optional_model() {
        let mut inv = minimal();
        inv.model = Some("claude-haiku-4-5-20251001".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[pos + 1], "claude-haiku-4-5-20251001");
    }

    #[test]
    fn optional_budget() {
        let mut inv = minimal();
        inv.max_budget_usd = Some(1.5);
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--max-budget-usd").unwrap();
        assert_eq!(args[pos + 1], "1.50");
    }

    #[test]
    fn optional_max_turns() {
        let mut inv = minimal();
        inv.max_turns = Some(10);
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--max-turns").unwrap();
        assert_eq!(args[pos + 1], "10");
    }

    #[test]
    fn disallowed_tools_expanded() {
        let mut inv = minimal();
        inv.disallowed_tools = vec!["CronCreate".into(), "CronList".into()];
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--disallowedTools").unwrap();
        assert_eq!(args[pos + 1], "CronCreate");
        assert_eq!(args[pos + 2], "CronList");
    }

    #[test]
    fn disable_all_tools_args_emit_empty_tools_flag() {
        let mut inv = minimal();
        inv.extra_args = disable_all_tools_args();

        let args = inv.into_args();

        let pos = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[pos + 1], "");
    }

    #[test]
    fn baseline_disallowed_tools_blocks_harness_self_loops() {
        let baseline = baseline_disallowed_tools();
        for required in [
            "ScheduleWakeup",
            "EnterWorktree",
            "ExitWorktree",
            "Monitor",
            "PushNotification",
            "TeamCreate",
            "TeamDelete",
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
        // Tools we deliberately keep available.
        for kept in ["SendMessage", "LSP", "WebFetch", "WebSearch", "Agent"] {
            assert!(
                !baseline.iter().any(|s| s == kept),
                "baseline must NOT block {kept}"
            );
        }
    }

    #[test]
    fn invocation_mcp_config_adds_progress_header_and_preserves_authorization() {
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

    #[test]
    fn disallow_progress_adds_full_mcp_tool_name() {
        let tools = disallow_send_progress(vec!["Agent".to_owned()]);

        assert!(tools.iter().any(|tool| tool == SEND_PROGRESS_MCP_TOOL));
        assert!(tools.iter().any(|tool| tool == "Agent"));
    }

    #[test]
    fn disallow_send_progress_is_idempotent() {
        let tools = disallow_send_progress(disallow_send_progress(Vec::new()));
        let count = tools
            .iter()
            .filter(|tool| tool.as_str() == SEND_PROGRESS_MCP_TOOL)
            .count();

        assert_eq!(count, 1);
    }

    #[test]
    fn disallow_learning_tools_adds_full_mcp_tool_names() {
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

    #[test]
    fn disallow_foreground_only_tools_is_idempotent() {
        let tools = disallow_foreground_only_tools(disallow_foreground_only_tools(Vec::new()));

        for tool_name in [
            SEND_PROGRESS_MCP_TOOL,
            right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
            right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
            right_mcp::internal_client::THREAD_SEARCH_MCP_TOOL,
            right_mcp::internal_client::CHAT_SEARCH_MCP_TOOL,
        ] {
            let count = tools
                .iter()
                .filter(|tool| tool.as_str() == tool_name)
                .count();
            assert_eq!(count, 1, "{tool_name} should be present once");
        }
    }

    #[test]
    fn write_invocation_mcp_config_writes_agent_scoped_file() {
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

    #[test]
    fn resume_session() {
        let mut inv = minimal();
        inv.resume_session_id = Some("abc-123".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[pos + 1], "abc-123");
    }

    #[test]
    fn new_session() {
        let mut inv = minimal();
        inv.new_session_id = Some("def-456".into());
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(args[pos + 1], "def-456");
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn json_output_format() {
        let mut inv = minimal();
        inv.output_format = OutputFormat::Json;
        let args = inv.into_args();
        assert!(args.contains(&"json".to_string()));
        assert!(!args.contains(&"stream-json".to_string()));
        assert!(!args.contains(&"--verbose".to_string()));
    }

    #[test]
    fn allowed_tools_joined() {
        let mut inv = minimal();
        inv.allowed_tools = vec!["WebSearch".into(), "WebFetch".into()];
        let args = inv.into_args();
        let pos = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[pos + 1], "WebSearch,WebFetch");
    }

    #[test]
    fn no_mcp_no_schema() {
        let mut inv = minimal();
        inv.mcp_config_path = None;
        inv.json_schema = None;
        let args = inv.into_args();
        assert!(!args.contains(&"--mcp-config".to_string()));
        assert!(!args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args.contains(&"--json-schema".to_string()));
    }

    #[test]
    fn mcp_config_path_sandbox() {
        let path = mcp_config_path(
            Some(Path::new("/tmp/ssh.config")),
            Path::new("/home/user/agents/foo"),
        );
        assert_eq!(path, right_openshell::openshell::SANDBOX_MCP_JSON_PATH);
    }

    #[test]
    fn mcp_config_path_no_sandbox() {
        let agent_dir = PathBuf::from("/home/user/agents/foo");
        let path = mcp_config_path(None, &agent_dir);
        assert_eq!(path, "/home/user/agents/foo/mcp.json");
    }

    #[test]
    fn fork_session_emits_resume_fork_and_session_id() {
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

    #[test]
    fn fork_session_without_resume_does_not_emit_flag() {
        let mut inv = minimal();
        inv.new_session_id = Some("only-new".into());
        inv.fork_session = true;
        let args = inv.into_args();
        assert!(!args.contains(&"--fork-session".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn debug_flag_true_appends_debug_and_debug_file() {
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

    #[test]
    fn debug_flag_false_omits_debug_and_debug_file() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--debug-file=")));
    }

    #[test]
    fn debug_flag_absent_omits_debug() {
        // No debug_flag set at all (None) — should behave like false.
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
    }

    #[test]
    fn debug_flag_uses_resume_session_id_when_no_fork() {
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

    #[test]
    fn debug_flag_uses_new_session_id_when_forking() {
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

    #[test]
    fn debug_flag_runtime_toggle_picked_up_at_build_time() {
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

    #[test]
    fn debug_flag_no_session_id_omits_debug_file_but_still_emits_debug() {
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
}
