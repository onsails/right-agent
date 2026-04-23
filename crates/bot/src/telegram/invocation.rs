use std::path::Path;

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
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) disallowed_tools: Vec<String>,
    pub(crate) extra_args: Vec<String>,
    pub(crate) prompt: Option<String>,
}

impl ClaudeInvocation {
    /// Consume self and produce the full argument list for spawning `claude`.
    pub(crate) fn into_args(self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

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
        if let Some(id) = self.resume_session_id {
            args.push("--resume".into());
            args.push(id);
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
        rightclaw::openshell::SANDBOX_MCP_JSON_PATH.to_string()
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
        let ssh_host = rightclaw::openshell::ssh_host_for_sandbox(resolved_sandbox.unwrap());
        let mut script = String::new();
        if let Some(token) = crate::login::load_auth_token(agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            script.push_str(&format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n"));
        }
        // shlex::try_join fails only on nul bytes — safe for CLI args.
        let quoted = shlex::try_join(args.iter().map(|s| s.as_str()))
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
    use std::path::PathBuf;

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
            allowed_tools: vec![],
            disallowed_tools: vec![],
            extra_args: vec![],
            prompt: Some("hello".into()),
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
        assert_eq!(path, rightclaw::openshell::SANDBOX_MCP_JSON_PATH);
    }

    #[test]
    fn mcp_config_path_no_sandbox() {
        let agent_dir = PathBuf::from("/home/user/agents/foo");
        let path = mcp_config_path(None, &agent_dir);
        assert_eq!(path, "/home/user/agents/foo/mcp.json");
    }
}
