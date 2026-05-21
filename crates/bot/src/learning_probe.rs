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
    #[error("probe stdout is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("probe JSON missing required field `workflow_complete`")]
    MissingWorkflowComplete,
    #[error("probe JSON `workflow_complete` must be a boolean")]
    WorkflowCompleteNotBool,
}

/// Parse the JSON document returned by `--output-format json`.
///
/// CC wraps the assistant reply in `{"result": {...}}` for non-stream output;
/// we tolerate both shapes (unwrapped object or `result`-wrapped).
pub(crate) fn parse_probe_output(stdout: &str) -> Result<ParsedProbe, ProbeParseError> {
    let value: serde_json::Value = serde_json::from_str(stdout)?;
    let body = match value.get("result") {
        Some(serde_json::Value::Object(_)) => &value["result"],
        _ => &value,
    };
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
    fn parse_probe_output_unwraps_result_envelope() {
        let stdout = r#"{"result":{"workflow_complete":false,"learning_signal":null,"skill_issue_signal":null}}"#;
        let parsed = parse_probe_output(stdout).unwrap();
        assert!(!parsed.workflow_complete);
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
        assert!(matches!(err, ProbeParseError::Json(_)));
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
}
