//! Haiku classifier deciding whether to spawn the probe-writer.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use crate::telegram::worker::ProbeAnchor;

/// Decision returned by the prefilter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefilterDecision {
    Probe,
    Skip,
}

pub(crate) const PREFILTER_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "should_probe": { "type": "boolean" },
    "reason": { "type": "string" }
  },
  "required": ["should_probe", "reason"]
}"#;

/// Sum today's spend across learning sources from `usage_events`. Used by the
/// worker to gate the prefilter+probe-writer pipeline against the daily budget.
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

/// Compose the prompt that goes to Haiku.
pub(crate) fn build_prompt(anchor: &ProbeAnchor) -> String {
    let user: String = anchor.user_msg_text.chars().take(2000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(4000).collect();
    format!(
        "Decide whether the just-finished turn produced a reusable workflow \
worth examining for skill creation/update. Reply JSON per schema.

USER: {user}
ASSISTANT: {assistant}

Set should_probe=true if any of:
- workflow involved multi-step coordination across tools/files;
- user explicitly asked to remember/save/fix;
- user corrected a previous approach;
- a non-obvious gotcha was discovered.

Otherwise should_probe=false (chat, trivial command, conversational reply)."
    )
}

/// Parse Haiku's JSON output into a decision. Returns Skip on any parse error.
pub(crate) fn parse_output(stdout: &str) -> PrefilterDecision {
    // CC --output-format json wraps assistant text. Strip the envelope first.
    let inner = match crate::learning_review::unwrap_structured_output_payload(stdout, "prefilter")
    {
        Ok(v) => v,
        Err(_) => return PrefilterDecision::Skip,
    };
    inner
        .get("should_probe")
        .and_then(|v| v.as_bool())
        .map(|b| {
            if b {
                PrefilterDecision::Probe
            } else {
                PrefilterDecision::Skip
            }
        })
        .unwrap_or(PrefilterDecision::Skip)
}

use std::path::PathBuf;
use std::time::Duration;

const PREFILTER_TIMEOUT: Duration = Duration::from_secs(30);

/// Bundle of inputs needed to run one prefilter invocation.
#[derive(Debug, Clone)]
pub(crate) struct PrefilterContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub chat_id: i64,
    pub thread_id: i64,
}

/// Run the Haiku prefilter on an anchor. Logs warns on any failure, returns Skip.
pub(crate) async fn run(ctx: PrefilterContext, anchor: ProbeAnchor) -> PrefilterDecision {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat, build_claude_command};

    let prompt = build_prompt(&anchor);
    let invocation = ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(PREFILTER_SCHEMA_JSON.into()),
        output_format: OutputFormat::Json,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(1),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        extra_args: crate::cc::invocation::disable_all_tools_args(),
        prompt: Some(prompt),
        debug_flag: None,
    };
    let args = invocation.into_args();
    let mut cmd = build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(PREFILTER_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "prefilter spawn failed: {e:#}"
            );
            return PrefilterDecision::Skip;
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "prefilter timed out after {}s",
                PREFILTER_TIMEOUT.as_secs()
            );
            return PrefilterDecision::Skip;
        }
    };

    if !output.status.success() {
        tracing::warn!(
            agent = %ctx.agent_name,
            status = ?output.status,
            stderr = %String::from_utf8_lossy(&output.stderr),
            "prefilter non-zero exit"
        );
        return PrefilterDecision::Skip;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    // Record usage event. Zero-signal results still cost money and must be visible.
    if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
        && let Ok(conn) = right_db::open_connection(&ctx.agent_db_dir, false)
        && let Err(e) = right_agent::usage::insert::insert_learning_prefilter(
            &conn,
            &b,
            ctx.chat_id,
            ctx.thread_id,
        )
    {
        tracing::warn!(agent = %ctx.agent_name, "prefilter usage insert failed: {e:#}");
    }

    parse_output(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(user: &str, assistant: &str) -> ProbeAnchor {
        ProbeAnchor {
            user_msg_text: user.to_owned(),
            assistant_reply_text: assistant.to_owned(),
            main_session_uuid: "uuid-main".to_owned(),
            captured_at: chrono::Utc::now(),
            chat_id: 1,
            thread_id: 0,
            num_turns: 1,
            total_cost_usd: 0.0,
            wall_elapsed_ms: 0,
            used_skill_receipts: Vec::new(),
        }
    }

    #[test]
    fn build_prompt_embeds_anchor_texts() {
        let p = build_prompt(&anchor("hello world", "hi back"));
        assert!(p.contains("hello world"));
        assert!(p.contains("hi back"));
        assert!(p.contains("should_probe"));
    }

    #[test]
    fn parse_output_should_probe_true_returns_probe() {
        let stdout =
            r#"{"type":"result","structured_output":{"should_probe":true,"reason":"multi-step"}}"#;
        assert_eq!(parse_output(stdout), PrefilterDecision::Probe);
    }

    #[test]
    fn parse_output_should_probe_false_returns_skip() {
        let stdout =
            r#"{"type":"result","structured_output":{"should_probe":false,"reason":"chat"}}"#;
        assert_eq!(parse_output(stdout), PrefilterDecision::Skip);
    }

    #[test]
    fn parse_output_invalid_json_returns_skip() {
        assert_eq!(parse_output("not json"), PrefilterDecision::Skip);
    }

    #[test]
    fn parse_output_missing_field_returns_skip() {
        let stdout = r#"{"type":"result","structured_output":{}}"#;
        assert_eq!(parse_output(stdout), PrefilterDecision::Skip);
    }

    #[test]
    fn build_prompt_truncates_long_inputs() {
        // Use non-ASCII markers absent from the template prose.
        let long_user = "ы".repeat(10_000);
        let long_asst = "ё".repeat(10_000);
        let p = build_prompt(&anchor(&long_user, &long_asst));
        // User truncated to 2000 chars, assistant to 4000 chars.
        assert_eq!(p.matches('ы').count(), 2000);
        assert_eq!(p.matches('ё').count(), 4000);
    }
}
