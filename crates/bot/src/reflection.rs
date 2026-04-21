//! Error reflection — on a failed CC invocation, run a short `--resume`-d pass
//! so the agent itself produces a user-friendly summary.
//!
//! Callers: `telegram::worker` (interactive) and `cron` (scheduled).
//! See: docs/superpowers/specs/2026-04-21-error-reflection-design.md

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use crate::telegram::stream::StreamEvent;

/// Maximum character length for one ring-buffer activity line's text snippet
/// or tool-argument summary in the reflection prompt. Kept short so the prompt
/// stays under a few hundred tokens.
const ACTIVITY_SNIPPET_LEN: usize = 80;

/// Classifies the failure we are reflecting on. Drives the human-readable
/// reason text inserted into the SYSTEM_NOTICE prompt.
#[derive(Debug, Clone)]
pub enum FailureKind {
    /// Process was killed by the 600-second safety net in worker.
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
pub enum ParentSource {
    Worker { chat_id: i64, thread_id: i64 },
    Cron { job_name: String },
}

/// Resource caps for a single reflection invocation.
#[derive(Debug, Clone, Copy)]
pub struct ReflectionLimits {
    pub max_turns: u32,
    pub max_budget_usd: f64,
    pub process_timeout: Duration,
}

impl ReflectionLimits {
    pub const WORKER: Self = Self {
        max_turns: 3,
        max_budget_usd: 0.20,
        process_timeout: Duration::from_secs(90),
    };
    pub const CRON: Self = Self {
        max_turns: 5,
        max_budget_usd: 0.40,
        process_timeout: Duration::from_secs(180),
    };
}

/// All inputs required to run one reflection pass.
#[derive(Debug, Clone)]
pub struct ReflectionContext {
    pub session_uuid: String,
    pub failure: FailureKind,
    pub ring_buffer_tail: VecDeque<StreamEvent>,
    pub limits: ReflectionLimits,
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub db_path: PathBuf,
    pub parent_source: ParentSource,
    pub model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReflectionError {
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
        FailureKind::SafetyTimeout { limit_secs } =>
            format!("hit the {limit_secs}-second safety limit before producing a reply"),
        FailureKind::BudgetExceeded { limit_usd } =>
            format!("exceeded the budget of ${limit_usd:.2}"),
        FailureKind::MaxTurns { limit } =>
            format!("reached the maximum turn count ({limit})"),
        FailureKind::NonZeroExit { code } =>
            format!("Claude process exited with code {code}"),
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
        StreamEvent::ToolUse { tool, input_summary } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_text_per_kind() {
        assert!(failure_reason_text(&FailureKind::SafetyTimeout { limit_secs: 600 })
            .contains("600-second safety limit"));
        assert!(failure_reason_text(&FailureKind::BudgetExceeded { limit_usd: 2.0 })
            .contains("$2.00"));
        assert!(failure_reason_text(&FailureKind::MaxTurns { limit: 30 })
            .contains("30"));
        assert!(failure_reason_text(&FailureKind::NonZeroExit { code: 137 })
            .contains("137"));
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
        let p = build_reflection_prompt(
            &FailureKind::SafetyTimeout { limit_secs: 600 },
            &tail,
            3,
        );
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
        let p = build_reflection_prompt(
            &FailureKind::NonZeroExit { code: 1 },
            &tail,
            3,
        );
        assert!(p.contains("(no tool activity recorded)"));
    }
}
