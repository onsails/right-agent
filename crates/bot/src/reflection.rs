//! Error reflection — on a failed CC invocation, run a short `--resume`-d pass
//! so the agent itself produces a user-friendly summary.
//!
//! Callers: `telegram::worker` (interactive) and `cron` (scheduled).
//! See: docs/superpowers/specs/2026-04-21-error-reflection-design.md

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use right_agent::usage::insert::{insert_reflection_cron, insert_reflection_worker};

use crate::telegram::invocation::{ClaudeInvocation, OutputFormat};
use crate::telegram::stream::{StreamEvent, parse_stream_event};

/// Maximum character length for one ring-buffer activity line's text snippet
/// or tool-argument summary in the reflection prompt. Kept short so the prompt
/// stays under a few hundred tokens.
const ACTIVITY_SNIPPET_LEN: usize = 80;

/// Classifies the failure we are reflecting on. Drives the human-readable
/// reason text inserted into the SYSTEM_NOTICE prompt.
#[derive(Debug, Clone)]
pub(crate) enum FailureKind {
    /// Process was killed by a safety-timeout net. Worker no longer emits this
    /// (timeouts now go through the Backgrounded path); retained for cron-side
    /// classification if a cron job's CC subprocess hits its own safety net.
    #[allow(dead_code)]
    SafetyTimeout { limit_secs: u64 },
    /// CC reported `--max-budget-usd` exhaustion.
    BudgetExceeded { limit_usd: f64 },
    /// CC reported `--max-turns` exhaustion.
    MaxTurns { limit: u32 },
    /// Non-zero exit code with no auth-error classification.
    NonZeroExit { code: i32 },
}

/// Discriminator for where the reflection originated — decides how the usage
/// row is written and helps /usage render a breakdown.
#[derive(Debug, Clone)]
pub(crate) enum ParentSource {
    Worker { chat_id: i64, thread_id: i64 },
    Cron { job_name: String },
}

/// Resource caps for a single reflection invocation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReflectionLimits {
    pub(crate) max_turns: u32,
    pub(crate) max_budget_usd: f64,
    pub(crate) process_timeout: Duration,
}

impl ReflectionLimits {
    pub(crate) const WORKER: Self = Self {
        max_turns: 3,
        max_budget_usd: 0.20,
        process_timeout: Duration::from_secs(90),
    };
    pub(crate) const CRON: Self = Self {
        max_turns: 5,
        max_budget_usd: 0.40,
        process_timeout: Duration::from_secs(180),
    };
}

/// All inputs required to run one reflection pass.
#[derive(Debug, Clone)]
pub(crate) struct ReflectionContext {
    pub(crate) session_uuid: String,
    pub(crate) failure: FailureKind,
    pub(crate) ring_buffer_tail: VecDeque<StreamEvent>,
    pub(crate) limits: ReflectionLimits,
    pub(crate) agent_name: String,
    pub(crate) agent_dir: PathBuf,
    pub(crate) ssh_config_path: Option<PathBuf>,
    pub(crate) resolved_sandbox: Option<String>,
    pub(crate) parent_source: ParentSource,
    pub(crate) model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReflectionError {
    #[error("reflection spawn failed: {0}")]
    Spawn(String),
    #[error("reflection timed out after {0:?}")]
    Timeout(Duration),
    #[error("reflection CC exited with code {code}: {detail}")]
    NonZeroExit { code: i32, detail: String },
    #[error("reflection output parse failed: {0}")]
    Parse(String),
    #[error("reflection I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Render a human-readable reason text for the SYSTEM_NOTICE header.
pub(crate) fn failure_reason_text(kind: &FailureKind) -> String {
    match kind {
        FailureKind::SafetyTimeout { limit_secs } => {
            format!("hit the {limit_secs}-second safety limit before producing a reply")
        }
        FailureKind::BudgetExceeded { limit_usd } => {
            format!("exceeded the budget of ${limit_usd:.2}")
        }
        FailureKind::MaxTurns { limit } => format!("reached the maximum turn count ({limit})"),
        FailureKind::NonZeroExit { code } => format!("Claude process exited with code {code}"),
    }
}

/// Render a short, inlinable description of one ring-buffer event for the
/// "Your most recent activity" list.
// Truncation is silent (no "…" suffix) because the output is consumed by the
// LLM inside a SYSTEM_NOTICE prompt where an ellipsis would read as content.
pub(crate) fn format_ring_event(event: &StreamEvent) -> Option<String> {
    match event {
        StreamEvent::Text(t) => {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                return None;
            }
            let snippet: String = trimmed.chars().take(ACTIVITY_SNIPPET_LEN).collect();
            Some(format!("- said: {snippet}"))
        }
        StreamEvent::Thinking => Some("- was thinking".to_string()),
        StreamEvent::ToolUse {
            tool,
            input_summary,
        } => {
            let args: String = input_summary.chars().take(ACTIVITY_SNIPPET_LEN).collect();
            Some(format!("- called {tool}({args})"))
        }
        StreamEvent::Result(_) | StreamEvent::Other => None,
    }
}

/// Build the full stdin prompt for a reflection `claude -p --resume` call.
pub(crate) fn build_reflection_prompt(
    kind: &FailureKind,
    ring_buffer_tail: &VecDeque<StreamEvent>,
    max_turns: u32,
) -> String {
    let reason = failure_reason_text(kind);
    let mut activity = String::new();
    for e in ring_buffer_tail {
        if let Some(line) = format_ring_event(e) {
            activity.push_str(&line);
            activity.push('\n');
        }
    }
    let activity_block = if activity.is_empty() {
        "- (no tool activity recorded)\n".to_string()
    } else {
        activity
    };
    format!(
        "⟨⟨SYSTEM_NOTICE⟩⟩\n\
         \n\
         Your previous turn did not complete successfully.\n\
         \n\
         Reason: {reason}.\n\
         \n\
         Your most recent activity:\n\
         {activity_block}\
         \n\
         Please write a short reply for the user that:\n\
         1. Acknowledges the interruption honestly (1 sentence).\n\
         2. Summarizes what you were doing and any findings worth sharing.\n\
         3. Suggests a concrete next step (narrower scope, different approach,\n\
            or ask for clarification).\n\
         \n\
         Do NOT continue the original investigation — stay within {max_turns} turns.\n\
         Do NOT call Agent or other long-running tools.\n\
         ⟨⟨/SYSTEM_NOTICE⟩⟩\n"
    )
}

/// Run one reflection pass for a failed CC invocation.
///
/// Resumes the failed session, pipes a SYSTEM_NOTICE-wrapped prompt via stdin,
/// parses the final `result` stream event, accounts the usage row, and returns
/// the agent's reply text. Any failure of the reflection itself returns `Err`
/// — the caller is responsible for a raw-error fallback.
pub(crate) async fn reflect_on_failure(ctx: ReflectionContext) -> Result<String, ReflectionError> {
    let span = tracing::info_span!(
        "reflection",
        session_uuid = %ctx.session_uuid,
        parent = ?ctx.parent_source,
        failure = ?ctx.failure,
    );
    let _enter = span.enter();

    tracing::info!("reflection starting");

    // 1. Build stdin prompt from pure helpers.
    let input = build_reflection_prompt(&ctx.failure, &ctx.ring_buffer_tail, ctx.limits.max_turns);

    // 2. Reply schema (reuse worker's — sandbox vs no-sandbox both read same file).
    let schema_path = ctx.agent_dir.join(".claude").join("reply-schema.json");
    let reply_schema = std::fs::read_to_string(&schema_path)?;

    // 3. MCP config path (reuse worker's helper).
    let mcp_path = crate::telegram::invocation::mcp_config_path(
        ctx.ssh_config_path.as_deref(),
        &ctx.agent_dir,
    );

    // 4. ClaudeInvocation — resume, stream-json, tight caps, no Agent tool.
    let invocation = ClaudeInvocation {
        mcp_config_path: Some(mcp_path),
        json_schema: Some(reply_schema),
        output_format: OutputFormat::StreamJson,
        model: ctx.model.clone(),
        max_budget_usd: Some(ctx.limits.max_budget_usd),
        max_turns: Some(ctx.limits.max_turns),
        resume_session_id: Some(ctx.session_uuid.clone()),
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: {
            let mut d = crate::telegram::invocation::baseline_disallowed_tools();
            d.push("Agent".into());
            d
        },
        extra_args: vec![],
        prompt: None,
    };
    let claude_args = invocation.into_args();

    // 5. System-prompt assembly (match worker's pattern; no MCP refresh, no memory).
    let (sandbox_mode, home_dir) = if ctx.ssh_config_path.is_some() {
        (
            right_agent::agent::types::SandboxMode::Openshell,
            "/sandbox".to_owned(),
        )
    } else {
        (
            right_agent::agent::types::SandboxMode::None,
            ctx.agent_dir.to_string_lossy().into_owned(),
        )
    };
    let base_prompt =
        right_agent::codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);

    let mut cmd = if let Some(ref ssh_config) = ctx.ssh_config_path {
        let sandbox_name = ctx.resolved_sandbox.as_deref().ok_or_else(|| {
            ReflectionError::Spawn("ssh_config_path set but resolved_sandbox is None".into())
        })?;
        let ssh_host = right_agent::openshell::ssh_host_for_sandbox(sandbox_name);
        let prompt_path = format!("/tmp/right-reflection-prompt-{}.md", ctx.session_uuid);
        let mut assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false, // not bootstrap mode
            "/sandbox",
            &prompt_path,
            "/sandbox",
            &claude_args,
            None, // no MCP instructions refresh
            None, // no memory section
        );
        if let Some(token) = crate::login::load_auth_token(&ctx.agent_dir) {
            let escaped = token.replace('\'', "'\\''");
            assembly_script =
                format!("export CLAUDE_CODE_OAUTH_TOKEN='{escaped}'\n{assembly_script}");
        }
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-F").arg(ssh_config);
        c.arg(&ssh_host);
        c.arg("--");
        c.arg(assembly_script);
        c
    } else {
        let agent_dir_str = ctx.agent_dir.to_string_lossy();
        let prompt_path = ctx.agent_dir.join(".claude").join(format!(
            "composite-reflection-prompt-{}.md",
            ctx.session_uuid
        ));
        let prompt_path_str = prompt_path.to_string_lossy();
        let assembly_script = crate::telegram::prompt::build_prompt_assembly_script(
            &base_prompt,
            false,
            &agent_dir_str,
            &prompt_path_str,
            &agent_dir_str,
            &claude_args,
            None,
            None,
        );
        let mut c = tokio::process::Command::new("bash");
        c.arg("-c");
        c.arg(&assembly_script);
        c.env("HOME", &ctx.agent_dir);
        c.env("USE_BUILTIN_RIPGREP", "0");
        if let Some(token) = crate::login::load_auth_token(&ctx.agent_dir) {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", &token);
        }
        c.current_dir(&ctx.agent_dir);
        c
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());

    let mut child = right_agent::process_group::ProcessGroupChild::spawn(cmd)
        .map_err(|e| ReflectionError::Spawn(format!("{:#}", e)))?;

    // Pipe the prompt, then drop stdin to signal EOF.
    if let Some(mut stdin) = child.stdin() {
        stdin.write_all(input.as_bytes()).await?;
        // drop at end of scope
    }

    // Read stdout streaming; capture the last `result` event's raw JSON.
    let stdout = child
        .stdout()
        .ok_or_else(|| ReflectionError::Spawn("no stdout handle".into()))?;
    let mut lines = BufReader::new(stdout).lines();

    let mut last_result_line: Option<String> = None;
    let read_fut = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let StreamEvent::Result(raw) = parse_stream_event(&line) {
                last_result_line = Some(raw);
            }
        }
    };

    let timed_out = tokio::time::timeout(ctx.limits.process_timeout, read_fut)
        .await
        .is_err();

    // Kill the child regardless of outcome; collect exit code best-effort.
    let _ = child.kill().await;
    let exit = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);

    if timed_out {
        tracing::warn!(
            duration_ms = ctx.limits.process_timeout.as_millis() as u64,
            "reflection timed out"
        );
        return Err(ReflectionError::Timeout(ctx.limits.process_timeout));
    }

    if exit != 0 {
        let detail = match &last_result_line {
            Some(line) => line.chars().take(400).collect::<String>(),
            None => "<no stream-json output before exit>".to_string(),
        };
        return Err(ReflectionError::NonZeroExit { code: exit, detail });
    }

    let result_line = last_result_line.ok_or_else(|| {
        ReflectionError::Parse("no `result` stream event on successful exit".into())
    })?;

    // Parse reply via the shared helper (handles content: Option<String>, nested result, etc.).
    let (reply_output, _session_id) = crate::telegram::worker::parse_reply_output(&result_line)
        .map_err(ReflectionError::Parse)?;
    let content = reply_output
        .content
        .ok_or_else(|| ReflectionError::Parse("reply content was null".into()))?;

    // Account usage (best-effort — log but don't fail reflection on usage insert error).
    if let Some(breakdown) = crate::telegram::stream::parse_usage_full(&result_line) {
        match right_agent::memory::open_connection(&ctx.agent_dir, false) {
            Ok(conn) => {
                let res = match &ctx.parent_source {
                    ParentSource::Worker { chat_id, thread_id } => {
                        insert_reflection_worker(&conn, &breakdown, *chat_id, *thread_id)
                    }
                    ParentSource::Cron { job_name } => {
                        insert_reflection_cron(&conn, &breakdown, job_name)
                    }
                };
                if let Err(e) = res {
                    tracing::warn!("reflection usage insert failed: {:#}", e);
                }
            }
            Err(e) => {
                tracing::warn!("reflection usage DB open failed: {:#}", e);
            }
        }
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&result_line).unwrap_or(serde_json::Value::Null);
    tracing::info!(
        cost_usd = parsed
            .get("total_cost_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        turns = parsed
            .get("num_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        "reflection completed"
    );

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_text_per_kind() {
        assert!(
            failure_reason_text(&FailureKind::SafetyTimeout { limit_secs: 600 })
                .contains("600-second safety limit")
        );
        assert!(
            failure_reason_text(&FailureKind::BudgetExceeded { limit_usd: 2.0 }).contains("$2.00")
        );
        assert!(failure_reason_text(&FailureKind::MaxTurns { limit: 30 }).contains("30"));
        assert!(failure_reason_text(&FailureKind::NonZeroExit { code: 137 }).contains("137"));
    }

    #[test]
    fn format_ring_event_truncates_text() {
        let ev = StreamEvent::Text("x".repeat(200));
        let out = format_ring_event(&ev).unwrap();
        assert!(out.starts_with("- said: "));
        assert!(out.len() < 120);
    }

    #[test]
    fn format_ring_event_tool_use() {
        let ev = StreamEvent::ToolUse {
            tool: "Read".into(),
            input_summary: r#"{"path":"/x"}"#.into(),
        };
        let out = format_ring_event(&ev).unwrap();
        assert!(out.contains("called Read"));
        assert!(out.contains("/x"));
    }

    #[test]
    fn format_ring_event_tool_use_truncates_long_input_summary() {
        let ev = StreamEvent::ToolUse {
            tool: "Bash".into(),
            input_summary: "a".repeat(200),
        };
        let out = format_ring_event(&ev).unwrap();
        // prefix "- called Bash(" (14) + up to ACTIVITY_SNIPPET_LEN + ")" (1)
        // A char-count upper bound is tighter than byte length.
        assert!(out.chars().count() <= 14 + ACTIVITY_SNIPPET_LEN + 1);
        assert!(out.starts_with("- called Bash("));
    }

    #[test]
    fn format_ring_event_skips_empty_text_and_other() {
        assert!(format_ring_event(&StreamEvent::Text("   ".into())).is_none());
        assert!(format_ring_event(&StreamEvent::Other).is_none());
        assert!(format_ring_event(&StreamEvent::Result("{}".into())).is_none());
    }

    #[test]
    fn prompt_contains_markers_and_reason() {
        let tail = VecDeque::from([
            StreamEvent::ToolUse {
                tool: "Read".into(),
                input_summary: "{}".into(),
            },
            StreamEvent::Text("partial finding".into()),
        ]);
        let p = build_reflection_prompt(&FailureKind::SafetyTimeout { limit_secs: 600 }, &tail, 3);
        assert!(p.starts_with("⟨⟨SYSTEM_NOTICE⟩⟩"));
        assert!(p.contains("⟨⟨/SYSTEM_NOTICE⟩⟩"));
        assert!(p.contains("600-second safety limit"));
        assert!(p.contains("called Read"));
        assert!(p.contains("partial finding"));
        assert!(p.contains("stay within 3 turns"));
    }

    #[test]
    fn prompt_handles_empty_ring_buffer() {
        let tail: VecDeque<StreamEvent> = VecDeque::new();
        let p = build_reflection_prompt(&FailureKind::NonZeroExit { code: 1 }, &tail, 3);
        assert!(p.contains("(no tool activity recorded)"));
    }
}
