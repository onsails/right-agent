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

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(user: &str, assistant: &str) -> ProbeAnchor {
        ProbeAnchor {
            user_msg_text: user.to_owned(),
            assistant_reply_text: assistant.to_owned(),
            main_session_uuid: "main".to_owned(),
            captured_at: chrono::Utc::now(),
            chat_id: 1,
            thread_id: 0,
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
