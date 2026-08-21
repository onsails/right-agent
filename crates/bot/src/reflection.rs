//! Error reflection — on a failed CC invocation, run a short `--resume`-d pass
//! so the agent itself produces a user-friendly summary.
//!
//! Callers: `telegram::worker` (interactive) and `cron` (scheduled).
//! See: docs/superpowers/specs/2026-04-21-error-reflection-design.md

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use right_agent::usage::insert::{insert_reflection_cron, insert_reflection_worker};

use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
use crate::cc::stream::{StreamEvent, parse_stream_event};

/// Maximum character length for one ring-buffer activity line's text snippet
/// or tool-argument summary in the reflection prompt. Kept short so the prompt
/// stays under a few hundred tokens.
const ACTIVITY_SNIPPET_LEN: usize = 80;

/// Bound on transport cleanup after a terminal result or forced termination.
const NOTICE_RESUME_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Classifies the failure we are reflecting on. Drives the human-readable
/// reason text inserted into the SYSTEM_NOTICE prompt.
#[derive(Debug, Clone)]
pub(crate) enum FailureKind {
    /// CC reported `--max-budget-usd` exhaustion.
    BudgetExceeded { limit_usd: f64 },
    /// CC reported `--max-turns` exhaustion.
    MaxTurns { limit: u32 },
    /// Non-zero exit code with no auth-error classification.
    NonZeroExit { code: i32 },
    /// Aborted after repeated structured-output schema rejections.
    StructuredOutputLoop { rejections: u32 },
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
    /// Null-reply repair runs worker-side only, with the same risk profile
    /// as a worker reflection: short resume, tight caps.
    pub(crate) const NULL_REPAIR: Self = Self::WORKER;
}

/// All inputs required to run one notice-resume pass (failure reflection or
/// null-reply repair).
#[derive(Debug, Clone)]
pub(crate) struct ReflectionContext {
    pub(crate) session_uuid: String,
    pub(crate) limits: ReflectionLimits,
    pub(crate) agent_name: String,
    pub(crate) agent_dir: PathBuf,
    /// `None` when the sandbox backend is degraded; the reflection then
    /// refuses rather than resuming the session on the host.
    pub(crate) sandbox: Option<crate::sandbox::Sandbox>,
    pub(crate) parent_source: ParentSource,
    pub(crate) model: Option<String>,
    /// Hot-reloadable debug flag. Forwarded to ClaudeInvocation.debug_flag.
    pub(crate) debug: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReflectionError {
    #[error("reflection spawn failed: {0}")]
    Spawn(String),
    #[error("reflection guest process failed: {0}")]
    Process(#[from] crate::cc::sandbox_process::SandboxProcessError),
    #[error("reflection timed out after {0:?}")]
    Timeout(Duration),
    #[error("reflection CC exited with code {code}: {detail}")]
    NonZeroExit { code: i32, detail: String },
    #[error("reflection output parse failed: {0}")]
    Parse(String),
    #[error("reflection I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Derive the semantic process exit from CC's terminal result.
///
/// A well-formed `is_error` is authoritative because the transport may not
/// report an exit after the terminal result. Malformed or missing result data
/// falls back to the transport exit, if one was observed.
fn effective_notice_exit(result_line: Option<&str>, transport_exit: Option<i32>) -> i32 {
    let result_is_error = result_line.and_then(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()?
            .get("is_error")?
            .as_bool()
    });
    match result_is_error {
        Some(false) => 0,
        Some(true) => 1,
        None => transport_exit.unwrap_or(-1),
    }
}

async fn kill_and_bounded_wait(
    child: &mut crate::cc::sandbox_process::SandboxChild,
) -> Option<i32> {
    child.kill().await;
    match tokio::time::timeout(NOTICE_RESUME_CLEANUP_TIMEOUT, child.wait()).await {
        Ok(Ok(code)) => Some(code),
        Ok(Err(error)) => {
            tracing::debug!("notice-resume cleanup wait failed: {error:#}");
            None
        }
        Err(_) => {
            tracing::debug!("notice-resume cleanup wait timed out");
            None
        }
    }
}

/// Render a human-readable reason text for the SYSTEM_NOTICE header.
pub(crate) fn failure_reason_text(kind: &FailureKind) -> String {
    match kind {
        FailureKind::BudgetExceeded { limit_usd } => {
            format!("exceeded the budget of ${limit_usd:.2}")
        }
        FailureKind::MaxTurns { limit } => format!("reached the maximum turn count ({limit})"),
        FailureKind::NonZeroExit { code } => format!("Claude process exited with code {code}"),
        FailureKind::StructuredOutputLoop { rejections } => format!(
            "could not produce a valid structured reply after {rejections} attempts (its output kept failing schema validation)"
        ),
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
    token: &str,
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
    let body = format!(
        "\n\
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
         Do NOT call Agent or other long-running tools.\n"
    );
    crate::cc::system_notice::wrap_system_notice(token, &body)
}

/// Run one reflection pass for a failed CC invocation.
///
/// Resumes the failed session, pipes a SYSTEM_NOTICE-wrapped prompt via stdin,
/// parses the final `result` stream event, accounts the usage row, and returns
/// the agent's reply text. Any failure of the reflection itself returns `Err`
/// — the caller is responsible for a raw-error fallback.
pub(crate) async fn reflect_on_failure(
    ctx: ReflectionContext,
    failure: FailureKind,
    ring_buffer_tail: VecDeque<StreamEvent>,
) -> Result<String, ReflectionError> {
    let span = tracing::info_span!(
        "reflection",
        session_uuid = %ctx.session_uuid,
        parent = ?ctx.parent_source,
        failure = ?failure,
    );
    let _enter = span.enter();

    tracing::info!("reflection starting");

    let notice_token = fetch_notice_token(&ctx.agent_dir).await?;
    let input = build_reflection_prompt(
        &failure,
        &ring_buffer_tail,
        ctx.limits.max_turns,
        &notice_token,
    );
    let (reply_output, _raw) =
        run_notice_resume(&ctx, input, &notice_token, NoticeResumeMode::Normal).await?;
    reply_output
        .content
        .ok_or_else(|| ReflectionError::Parse("reply content was null".into()))
}

/// Maximum chars of the discarded text block embedded in the repair prompt.
const NULL_REPAIR_SNIPPET_LEN: usize = 2000;

/// Build the stdin prompt for a null-reply repair `claude -p --resume` call.
pub(crate) fn build_null_repair_prompt(last_text: &str, max_turns: u32, token: &str) -> String {
    let snippet: String = last_text
        .trim()
        .chars()
        .take(NULL_REPAIR_SNIPPET_LEN)
        .collect();
    let body = format!(
        "\n\
         Your previous reply was NOT delivered to the user.\n\
         \n\
         Reason: your structured output had `content: null`. Only the structured\n\
         `content` field reaches Telegram — assistant text blocks are discarded.\n\
         \n\
         The discarded text block was:\n\
         \"\"\"\n\
         {snippet}\n\
         \"\"\"\n\
         \n\
         If you intended to reply: rewrite it now and return it in the structured\n\
         output `content` field.\n\
         If you intentionally stay silent: return `content: null` again — nothing\n\
         will be sent and no further repair will be attempted.\n\
         \n\
         Stay within {max_turns} turns. Do NOT call Agent or other long-running tools.\n"
    );
    crate::cc::system_notice::wrap_system_notice(token, &body)
}

/// Run one repair pass for a suspicious null reply: resume the session and ask
/// the agent to re-emit its reply via structured `content` (or confirm
/// intentional silence with another null). Returns the parsed reply as-is —
/// a second null is a valid outcome the caller must respect.
pub(crate) async fn repair_null_reply(
    ctx: ReflectionContext,
    last_assistant_text: &str,
) -> Result<crate::cc::worker_reply::ReplyOutput, ReflectionError> {
    let span = tracing::info_span!(
        "null_reply_repair",
        session_uuid = %ctx.session_uuid,
        parent = ?ctx.parent_source,
    );
    let _enter = span.enter();

    tracing::info!("null-reply repair starting");

    let notice_token = fetch_notice_token(&ctx.agent_dir).await?;
    let input = build_null_repair_prompt(last_assistant_text, ctx.limits.max_turns, &notice_token);
    let (reply_output, _raw) =
        run_notice_resume(&ctx, input, &notice_token, NoticeResumeMode::Normal).await?;
    Ok(reply_output)
}

/// Fetch the per-agent notice token used to wrap trusted SYSTEM_NOTICE prompts,
/// so the agent can verify SYSTEM_NOTICE markers.
async fn fetch_notice_token(agent_dir: &std::path::Path) -> Result<String, ReflectionError> {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .map_err(|e| ReflectionError::Spawn(format!("{e:#}")))?;
    right_mcp::credentials::get_or_create_notice_token(&conn)
        .await
        .map_err(|e| ReflectionError::Spawn(format!("{e:#}")))
}

#[derive(Debug, Clone)]
enum NoticeResumeMode {
    Normal,
}

impl NoticeResumeMode {
    const fn schema_filename(&self) -> &'static str {
        "reply-schema.json"
    }

    const fn prompt_mode(&self) -> crate::cc::prompt::PromptMode {
        crate::cc::prompt::PromptMode::Normal
    }
}

/// Shared `claude -p --resume` plumbing for reflection and repair passes:
/// pipes a SYSTEM_NOTICE prompt via stdin, parses the final `result` stream
/// event, and accounts the usage row. Null-tolerant: returns the parsed reply
/// as-is together with the raw result line.
async fn run_notice_resume(
    ctx: &ReflectionContext,
    input: String,
    notice_token: &str,
    mode: NoticeResumeMode,
) -> Result<(crate::cc::worker_reply::ReplyOutput, String), ReflectionError> {
    // 1. Use the output schema matching the resumed prompt mode.
    let schema_path = ctx.agent_dir.join(".claude").join(mode.schema_filename());
    let reply_schema = std::fs::read_to_string(&schema_path)?;

    // 2. Aggregator config — one fixed guest path.
    let mcp_path = crate::sandbox::SANDBOX_MCP_JSON_PATH.to_owned();

    // 3. ClaudeInvocation — resume, stream-json, tight caps, no Agent tool.
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
            let mut d = crate::cc::invocation::baseline_disallowed_tools();
            d.push("Agent".into());
            crate::cc::invocation::disallow_channel_post(
                crate::cc::invocation::disallow_foreground_only_tools(d),
            )
        },
        extra_args: vec![],
        prompt: None,
        debug_flag: ctx.debug.clone(),
    };
    let claude_args = invocation.into_args();

    // 4. System-prompt assembly (match worker's pattern; no MCP refresh, no memory).
    let base_prompt = right_codegen::generate_system_prompt(&ctx.agent_name, "/sandbox");

    let sandbox =
        crate::cc::invocation::guard_no_sandboxed_host_exec(&ctx.agent_name, ctx.sandbox.as_ref())
            .map_err(|e| ReflectionError::Spawn(format!("{e:#}")))?;

    let prompt_path = crate::cc::prompt::sandbox_prompt_file_path("reflection-prompt");
    let assembly_script = crate::cc::prompt::build_prompt_assembly_script(
        &base_prompt,
        mode.prompt_mode(),
        "/sandbox",
        &prompt_path,
        "/sandbox",
        &claude_args,
        None, // no MCP instructions refresh
        None, // no memory section
        None,
        None,
        Some(notice_token),
    );

    let mut child = crate::cc::invocation::build_claude_script_command(
        assembly_script,
        &ctx.agent_dir,
        sandbox,
    )
    .await
    .stdin_piped()
    .stdout(crate::cc::sandbox_process::Capture::Pipe)
    .stderr(crate::cc::sandbox_process::Capture::Null)
    .spawn()
    .await
    .map_err(|e| ReflectionError::Spawn(format!("{e:#}")))?;

    // Pipe the prompt and guest EOF as one delivery operation. The SDK EOF
    // acknowledgement is part of delivery and must not outlive the reflection
    // process timeout.
    if let Some(mut stdin) = child.stdin() {
        let delivery = async move {
            stdin.write_all(input.as_bytes()).await?;
            stdin.close().await
        };
        match tokio::time::timeout(ctx.limits.process_timeout, delivery).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                kill_and_bounded_wait(&mut child).await;
                return Err(error.into());
            }
            Err(_) => {
                kill_and_bounded_wait(&mut child).await;
                tracing::warn!(
                    duration_ms = ctx.limits.process_timeout.as_millis() as u64,
                    "notice-resume stdin delivery timed out"
                );
                return Err(ReflectionError::Timeout(ctx.limits.process_timeout));
            }
        }
    } else {
        kill_and_bounded_wait(&mut child).await;
        return Err(ReflectionError::Spawn("no stdin handle".into()));
    }

    // Read stdout only until the first terminal result. The SDK can deliver
    // that record without ever closing stdout or reporting process exit.
    let stdout = match child.stdout() {
        Some(stdout) => stdout,
        None => {
            kill_and_bounded_wait(&mut child).await;
            return Err(ReflectionError::Spawn("no stdout handle".into()));
        }
    };
    let mut lines = BufReader::new(stdout).lines();
    let read_result = tokio::time::timeout(ctx.limits.process_timeout, async {
        loop {
            match lines.next_line().await? {
                Some(line) => {
                    if let StreamEvent::Result(raw) = parse_stream_event(&line) {
                        return Ok::<Option<String>, std::io::Error>(Some(raw));
                    }
                }
                None => return Ok(None),
            }
        }
    })
    .await;

    let last_result_line = match read_result {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            kill_and_bounded_wait(&mut child).await;
            return Err(error.into());
        }
        Err(_) => {
            kill_and_bounded_wait(&mut child).await;
            tracing::warn!(
                duration_ms = ctx.limits.process_timeout.as_millis() as u64,
                "notice-resume timed out"
            );
            return Err(ReflectionError::Timeout(ctx.limits.process_timeout));
        }
    };

    // A result is terminal regardless of whether the SDK later reports EOF.
    // Kill is cleanup, while `is_error` below remains the semantic exit.
    let transport_exit = if last_result_line.is_some() {
        kill_and_bounded_wait(&mut child).await
    } else {
        match tokio::time::timeout(NOTICE_RESUME_CLEANUP_TIMEOUT, child.wait()).await {
            Ok(Ok(code)) => Some(code),
            Ok(Err(error)) => {
                tracing::debug!("notice-resume exit wait failed: {error:#}");
                None
            }
            Err(_) => kill_and_bounded_wait(&mut child).await,
        }
    };
    let exit = effective_notice_exit(last_result_line.as_deref(), transport_exit);

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
    let (reply_output, _session_id) = crate::cc::worker_reply::parse_reply_output(&result_line)
        .map_err(ReflectionError::Parse)?;

    // Account usage (best-effort — log but don't fail reflection on usage insert error).
    if let Some(breakdown) = crate::cc::stream::parse_usage_full(&result_line) {
        match right_db::open_connection(&ctx.agent_dir, false).await {
            Ok(conn) => {
                let res = match &ctx.parent_source {
                    ParentSource::Worker { chat_id, thread_id } => {
                        insert_reflection_worker(&conn, &breakdown, *chat_id, *thread_id).await
                    }
                    ParentSource::Cron { job_name } => {
                        insert_reflection_cron(&conn, &breakdown, job_name).await
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
        "notice-resume completed"
    );

    Ok((reply_output, result_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reason_text_per_kind() {
        assert!(
            failure_reason_text(&FailureKind::BudgetExceeded { limit_usd: 2.0 }).contains("$2.00")
        );
        assert!(failure_reason_text(&FailureKind::MaxTurns { limit: 30 }).contains("30"));
        assert!(failure_reason_text(&FailureKind::NonZeroExit { code: 137 }).contains("137"));
        assert!(failure_reason_text(&FailureKind::NonZeroExit { code: -1 }).contains("-1"));
    }

    #[test]
    fn failure_reason_text_for_structured_output_loop() {
        let s = failure_reason_text(&FailureKind::StructuredOutputLoop { rejections: 3 });
        assert!(s.contains("structured"), "{s}");
        assert!(s.contains('3'), "{s}");
    }

    #[tokio::test]
    async fn format_ring_event_truncates_text() {
        let ev = StreamEvent::Text("x".repeat(200));
        let out = format_ring_event(&ev).unwrap();
        assert!(out.starts_with("- said: "));
        assert!(out.len() < 120);
    }

    #[tokio::test]
    async fn format_ring_event_tool_use() {
        let ev = StreamEvent::ToolUse {
            tool: "Read".into(),
            input_summary: r#"{"path":"/x"}"#.into(),
        };
        let out = format_ring_event(&ev).unwrap();
        assert!(out.contains("called Read"));
        assert!(out.contains("/x"));
    }

    #[tokio::test]
    async fn format_ring_event_tool_use_truncates_long_input_summary() {
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

    #[tokio::test]
    async fn format_ring_event_skips_empty_text_and_other() {
        assert!(format_ring_event(&StreamEvent::Text("   ".into())).is_none());
        assert!(format_ring_event(&StreamEvent::Other).is_none());
        assert!(format_ring_event(&StreamEvent::Result("{}".into())).is_none());
    }

    #[test]
    fn terminal_result_semantics_override_unreliable_transport_exit() {
        let success = r#"{"type":"result","is_error":false,"result":"ok"}"#;
        let failure = r#"{"type":"result","is_error":true,"result":"failed"}"#;

        assert_eq!(effective_notice_exit(Some(success), None), 0);
        assert_eq!(effective_notice_exit(Some(success), Some(137)), 0);
        assert_eq!(effective_notice_exit(Some(failure), Some(0)), 1);
    }

    #[test]
    fn malformed_or_missing_terminal_semantics_fall_back_to_transport() {
        assert_eq!(effective_notice_exit(Some("not json"), Some(7)), 7);
        assert_eq!(
            effective_notice_exit(Some(r#"{"type":"result"}"#), None),
            -1
        );
        assert_eq!(effective_notice_exit(None, Some(9)), 9);
    }

    #[tokio::test]
    async fn prompt_contains_markers_and_reason() {
        let tail = VecDeque::from([
            StreamEvent::ToolUse {
                tool: "Read".into(),
                input_summary: "{}".into(),
            },
            StreamEvent::Text("partial finding".into()),
        ]);
        let p =
            build_reflection_prompt(&FailureKind::NonZeroExit { code: -1 }, &tail, 3, "deadbeef");
        assert!(p.starts_with("\u{27e8}\u{27e8}SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(p.contains("\u{27e8}\u{27e8}/SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(p.contains("exited with code -1"));
        assert!(p.contains("called Read"));
        assert!(p.contains("partial finding"));
        assert!(p.contains("stay within 3 turns"));
    }

    #[tokio::test]
    async fn null_repair_prompt_has_markers_snippet_and_escape_hatch() {
        let p = build_null_repair_prompt("Done. Rescheduled news-live.", 3, "deadbeef");
        assert!(p.starts_with("\u{27e8}\u{27e8}SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(p.contains("\u{27e8}\u{27e8}/SYSTEM_NOTICE:deadbeef\u{27e9}\u{27e9}"));
        assert!(p.contains("content: null"), "{p}");
        assert!(p.contains("Done. Rescheduled news-live."), "{p}");
        // Explicit intentional-silence escape hatch keeps lurk mode safe.
        assert!(p.contains("intentionally stay silent"), "{p}");
        assert!(p.contains("Stay within 3 turns"), "{p}");
    }

    #[tokio::test]
    async fn null_repair_prompt_truncates_long_text() {
        let long = "x".repeat(NULL_REPAIR_SNIPPET_LEN + 500);
        let p = build_null_repair_prompt(&long, 3, "deadbeef");
        assert!(
            p.chars().count() < NULL_REPAIR_SNIPPET_LEN + 1500,
            "prompt must stay bounded, got {} chars",
            p.chars().count()
        );
    }

    #[tokio::test]
    async fn prompt_handles_empty_ring_buffer() {
        let tail: VecDeque<StreamEvent> = VecDeque::new();
        let p =
            build_reflection_prompt(&FailureKind::NonZeroExit { code: 1 }, &tail, 3, "deadbeef");
        assert!(p.contains("(no tool activity recorded)"));
    }
}
