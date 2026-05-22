//! Post-turn fork-probe: classify whether the just-finished foreground turn
//! contains a learnable signal that the agent failed to emit in its
//! structured reply.
//!
//! Spec: docs/superpowers/specs/2026-05-21-learning-fork-probe-design.md

use right_agent::learned_skills::{NudgeSignalKind, NudgeSignalSource};

/// Decision returned by [`should_run_probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeDecision {
    Run,
    SkipReplyHasSignal,
    SkipDisabled,
    SkipBudgetExceeded,
    SkipNonForeground,
}

/// Inputs to the probe gate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProbeGateInput {
    pub fork_probe_enabled: bool,
    pub is_foreground: bool,
    pub reply_has_signal: bool,
    pub today_spend_usd: f64,
    pub daily_budget_usd: f64,
}

/// Pure decision function. No I/O.
pub(crate) fn should_run_probe(input: ProbeGateInput) -> ProbeDecision {
    if !input.fork_probe_enabled {
        return ProbeDecision::SkipDisabled;
    }
    if !input.is_foreground {
        return ProbeDecision::SkipNonForeground;
    }
    if input.reply_has_signal {
        return ProbeDecision::SkipReplyHasSignal;
    }
    if input.today_spend_usd >= input.daily_budget_usd {
        return ProbeDecision::SkipBudgetExceeded;
    }
    ProbeDecision::Run
}

/// Parsed fork-probe stdout JSON.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedProbe {
    pub workflow_complete: bool,
    pub learning_signal: Option<serde_json::Value>,
    pub skill_issue_signal: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProbeParseError {
    #[error("probe stdout envelope unwrap failed: {0}")]
    Envelope(String),
    #[error("probe JSON missing required field `workflow_complete`")]
    MissingWorkflowComplete,
    #[error("probe JSON `workflow_complete` must be a boolean")]
    WorkflowCompleteNotBool,
}

/// Parse the JSON document returned by `--output-format json`.
///
/// CC emits the production envelope as either
/// `{"type":"result","structured_output":{...},...}` (newer) or
/// `{"type":"result","result":"<JSON-encoded string>",...}` (older). We
/// delegate envelope unwrapping to `learning_review::unwrap_structured_output_payload`
/// so the probe and the Stage 2 review path stay in lock-step. The flat
/// top-level shape (unit-test fixtures) also parses because the helper falls
/// back to the root document when neither wrapper field is present.
pub(crate) fn parse_probe_output(stdout: &str) -> Result<ParsedProbe, ProbeParseError> {
    let body = crate::learning_review::unwrap_structured_output_payload(stdout, "probe")
        .map_err(ProbeParseError::Envelope)?;
    let workflow_complete = body
        .get("workflow_complete")
        .ok_or(ProbeParseError::MissingWorkflowComplete)?
        .as_bool()
        .ok_or(ProbeParseError::WorkflowCompleteNotBool)?;
    let learning_signal = body
        .get("learning_signal")
        .filter(|v| !v.is_null())
        .cloned();
    let skill_issue_signal = body
        .get("skill_issue_signal")
        .filter(|v| !v.is_null())
        .cloned();
    Ok(ParsedProbe {
        workflow_complete,
        learning_signal,
        skill_issue_signal,
    })
}

/// Choose which (kind, payload) to persist when probe returned both.
/// Prefers `learning_signal` over `skill_issue_signal`.
pub(crate) fn select_probe_signal(
    parsed: &ParsedProbe,
) -> Option<(NudgeSignalKind, serde_json::Value)> {
    if let Some(payload) = parsed.learning_signal.clone() {
        return Some((NudgeSignalKind::Learning, payload));
    }
    if let Some(payload) = parsed.skill_issue_signal.clone() {
        return Some((NudgeSignalKind::SkillIssue, payload));
    }
    None
}

/// The source value to attach to a fork-probe-derived nudge signal.
pub(crate) fn probe_signal_source() -> NudgeSignalSource {
    NudgeSignalSource::ForkProbe
}

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Bundle of inputs needed to spawn one fork-probe.
#[derive(Debug, Clone)]
pub(crate) struct ProbeContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub main_session_id: String,
    pub chat_id: i64,
    pub thread_id: i64,
    pub probe_model: Option<String>,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub debug_flag: Arc<std::sync::atomic::AtomicBool>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Build the `ClaudeInvocation` for the fork-probe.
///
/// Session-bearing (preserves the contract invariant). Passes `--tools ""`
/// to block all tool use; MCP config is loaded but unused for tools.
pub(crate) fn build_probe_invocation(
    ctx: &ProbeContext,
    probe_session_id: &str,
) -> crate::cc::invocation::ClaudeInvocation {
    crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            ctx.ssh_config_path.as_deref(),
            &ctx.agent_dir,
        )),
        // STUB: deprecated learning fields, will be rewritten in Task 16/18/25.
        json_schema: Some(r#"{}"#.into()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model: ctx.probe_model.clone(),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: Some(ctx.main_session_id.clone()),
        new_session_id: Some(probe_session_id.to_owned()),
        fork_session: true,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        // STUB: deprecated learning fields, will be rewritten in Task 16/18/25.
        prompt: Some("".into()),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}

/// Fire-and-forget the probe. Spawned via `tokio::spawn` by the caller.
pub(crate) async fn run_probe(ctx: ProbeContext) {
    let probe_session_id = uuid::Uuid::new_v4().to_string();
    let invocation = build_probe_invocation(&ctx, &probe_session_id);
    let args = invocation.into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                "fork-probe spawn failed: {e:#}"
            );
            return;
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                "fork-probe timed out after {}s",
                PROBE_TIMEOUT.as_secs()
            );
            return;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            agent = %ctx.agent_name,
            main_session = %ctx.main_session_id,
            status = ?output.status,
            stderr = %stderr,
            "fork-probe exited non-zero"
        );
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    record_probe_result(&ctx, &probe_session_id, &stdout);
}

fn record_probe_result(ctx: &ProbeContext, probe_session_id: &str, stdout: &str) {
    // Open the DB and write the usage row BEFORE attempting to parse the schema
    // payload. Zero-signal probes still cost money and must be visible in
    // `usage_events`; otherwise parse failures would hide every dollar spent.
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false) {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "fork-probe db open failed: {e:#}"
            );
            return;
        }
    };

    if let Some(breakdown) = crate::cc::stream::parse_usage_full(stdout)
        && let Err(e) = right_agent::usage::insert::insert_learning_fork_probe(
            &conn,
            &breakdown,
            ctx.chat_id,
            ctx.thread_id,
        )
    {
        tracing::warn!(
            agent = %ctx.agent_name,
            "fork-probe usage insert failed: {e:#}"
        );
    }

    let parsed = match parse_probe_output(stdout) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                main_session = %ctx.main_session_id,
                error = %e,
                stdout_excerpt = %stdout.chars().take(256).collect::<String>(),
                "fork-probe stdout parse failed"
            );
            return;
        }
    };

    let Some((signal_kind, payload_json)) = select_probe_signal(&parsed) else {
        return;
    };

    let record = right_agent::learned_skills::NudgeSignalRecord {
        invocation_id: probe_session_id.to_owned(),
        agent_name: ctx.agent_name.clone(),
        root_session_id: Some(ctx.main_session_id.clone()),
        chat_id: Some(ctx.chat_id),
        thread_id: Some(ctx.thread_id),
        signal_kind,
        payload_json,
        source: probe_signal_source(),
    };
    if let Err(e) = right_agent::learned_skills::record_nudge_signal(&conn, &record) {
        tracing::warn!(
            agent = %ctx.agent_name,
            "fork-probe signal record failed: {e:#}"
        );
    }
}

/// Read today's spend across `LEARNING_SOURCES` to feed the budget gate.
pub(crate) fn today_spend_usd(
    conn: &rusqlite::Connection,
    now_utc: &str,
) -> Result<f64, rusqlite::Error> {
    let date_part = now_utc.split_once('T').map(|(d, _)| d).unwrap_or(now_utc);
    let today_start = format!("{date_part}T00:00:00Z");
    let placeholders = std::iter::repeat_n("?", right_agent::usage::LEARNING_SOURCES.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE ts >= ?1 AND source IN ({placeholders})"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&today_start];
    for s in right_agent::usage::LEARNING_SOURCES {
        params.push(s);
    }
    conn.query_row(&sql, params.as_slice(), |r| r.get::<_, f64>(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(enabled: bool, fg: bool, has: bool, spend: f64, budget: f64) -> ProbeGateInput {
        ProbeGateInput {
            fork_probe_enabled: enabled,
            is_foreground: fg,
            reply_has_signal: has,
            today_spend_usd: spend,
            daily_budget_usd: budget,
        }
    }

    #[test]
    fn gate_returns_run_when_all_conditions_met() {
        assert_eq!(
            should_run_probe(gate(true, true, false, 0.0, 1.0)),
            ProbeDecision::Run
        );
    }

    #[test]
    fn gate_skips_when_disabled() {
        assert_eq!(
            should_run_probe(gate(false, true, false, 0.0, 1.0)),
            ProbeDecision::SkipDisabled
        );
    }

    #[test]
    fn gate_skips_when_not_foreground() {
        assert_eq!(
            should_run_probe(gate(true, false, false, 0.0, 1.0)),
            ProbeDecision::SkipNonForeground
        );
    }

    #[test]
    fn gate_skips_when_reply_already_has_signal() {
        assert_eq!(
            should_run_probe(gate(true, true, true, 0.0, 1.0)),
            ProbeDecision::SkipReplyHasSignal
        );
    }

    #[test]
    fn gate_skips_when_budget_met_or_exceeded() {
        assert_eq!(
            should_run_probe(gate(true, true, false, 1.0, 1.0)),
            ProbeDecision::SkipBudgetExceeded
        );
        assert_eq!(
            should_run_probe(gate(true, true, false, 1.50, 1.0)),
            ProbeDecision::SkipBudgetExceeded
        );
    }

    #[test]
    fn parse_probe_output_accepts_null_signals() {
        let stdout =
            r#"{"workflow_complete":true,"learning_signal":null,"skill_issue_signal":null}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.workflow_complete);
        assert!(parsed.learning_signal.is_none());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_accepts_learning_signal() {
        let stdout = r#"{"workflow_complete":true,"learning_signal":{"kind":"create_candidate","package_name_hint":"x","trigger":"explicit_user_request","reason_not_written":"needs_full_context_review","event_refs":["e1"],"summary":"s"}}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.learning_signal.is_some());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_unwraps_structured_output_envelope() {
        let stdout = r#"{"type":"result","structured_output":{"workflow_complete":true,"learning_signal":null,"skill_issue_signal":null}}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.workflow_complete);
        assert!(parsed.learning_signal.is_none());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_unwraps_result_string_envelope() {
        let stdout = r#"{"type":"result","result":"{\"workflow_complete\":true,\"learning_signal\":null,\"skill_issue_signal\":null}"}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(parsed.workflow_complete);
        assert!(parsed.learning_signal.is_none());
        assert!(parsed.skill_issue_signal.is_none());
    }

    #[test]
    fn parse_probe_output_rejects_missing_required_field() {
        let stdout = r#"{}"#;
        let err = parse_probe_output(stdout).unwrap_err();
        assert!(matches!(err, ProbeParseError::MissingWorkflowComplete));
    }

    #[test]
    fn parse_probe_output_rejects_malformed_json() {
        let err = parse_probe_output("not json").unwrap_err();
        assert!(matches!(err, ProbeParseError::Envelope(_)));
    }

    #[test]
    fn select_probe_signal_prefers_learning_over_skill_issue() {
        let parsed = ParsedProbe {
            workflow_complete: true,
            learning_signal: Some(serde_json::json!({"kind":"create_candidate"})),
            skill_issue_signal: Some(serde_json::json!({"kind":"update_candidate"})),
        };
        let (kind, _) = select_probe_signal(&parsed).unwrap();
        assert_eq!(kind, NudgeSignalKind::Learning);
    }

    #[test]
    fn select_probe_signal_returns_none_when_both_null() {
        let parsed = ParsedProbe {
            workflow_complete: true,
            learning_signal: None,
            skill_issue_signal: None,
        };
        assert!(select_probe_signal(&parsed).is_none());
    }

    #[test]
    fn probe_signal_source_is_fork_probe() {
        assert_eq!(probe_signal_source(), NudgeSignalSource::ForkProbe);
    }

    #[test]
    fn build_probe_invocation_emits_fork_and_disables_tools() {
        use std::sync::atomic::AtomicBool;
        let ctx = ProbeContext {
            agent_dir: PathBuf::from("/tmp/agent"),
            agent_db_dir: PathBuf::from("/tmp/agent"),
            agent_name: "right".into(),
            main_session_id: "main-uuid".into(),
            chat_id: 100,
            thread_id: 0,
            probe_model: Some("claude-opus-4-7".into()),
            ssh_config_path: None,
            resolved_sandbox: None,
            debug_flag: Arc::new(AtomicBool::new(false)),
        };
        let inv = build_probe_invocation(&ctx, "probe-uuid");
        let args = inv.into_args();
        assert!(args.iter().any(|a| a == "--fork-session"));
        let resume_pos = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[resume_pos + 1], "main-uuid");
        let sid_pos = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(args[sid_pos + 1], "probe-uuid");
        let tools_pos = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[tools_pos + 1], "");
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[model_pos + 1], "claude-opus-4-7");
        let max_turns_pos = args.iter().position(|a| a == "--max-turns").unwrap();
        assert_eq!(args[max_turns_pos + 1], "1");
    }
}
