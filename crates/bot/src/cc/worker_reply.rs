use std::path::Path;

use crate::cc::attachments_dto::OutboundAttachment;

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct UsedSkillReceipt {
    pub package_name: String,
    pub message: String,
}

/// Parsed output from CC structured JSON response (`result` field per D-03).
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<right_rich_content::RichContent>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<OutboundAttachment>>,
    pub used_skill_receipts: Option<Vec<UsedSkillReceipt>>,
    /// Bootstrap mode: the stage this structured response claims to handle.
    pub bootstrap_stage: Option<String>,
    /// Bootstrap mode: `true` signals agent claims onboarding is complete.
    pub bootstrap_complete: Option<bool>,
}

/// Decide whether a null structured reply is a suspected delivery-channel
/// mistake worth one repair resume. All conditions must hold:
/// - `content` is null (nothing was delivered);
/// - no attachments (media-only replies legitimately use `content: null`);
/// - no `mcp__right__send_message` call this turn (terminal null is
///   sanctioned after send_message);
/// - the turn's last assistant text block is non-empty (the agent tried to
///   say something; intentional silence produces no text block).
pub(crate) fn null_reply_needs_repair(
    output: &ReplyOutput,
    send_message_used: bool,
    last_assistant_text: Option<&str>,
) -> bool {
    output.content.is_none()
        && output.attachments.as_ref().is_none_or(Vec::is_empty)
        && !send_message_used
        && last_assistant_text.is_some_and(|t| !t.trim().is_empty())
}

/// Host-mode bootstrap completion check.
///
/// Sandboxed bootstrap is verified in `telegram::worker` by reconciling the
/// authoritative `/sandbox` identity files into the required host mirror.
pub(crate) fn should_accept_bootstrap(agent_dir: &Path) -> bool {
    right_agent::identity_mirror::host_identity_mirror_complete(agent_dir)
}

/// Parse CC structured JSON output (D-03, D-04).
///
/// Returns `Ok((ReplyOutput, Option<session_id>))` on success.
/// Returns `Err(String)` if JSON is malformed or the `result` field is missing.
/// Returns `Ok((ReplyOutput { content: None, .. }, _))` if content=null (silent response per D-04).
pub fn parse_reply_output(raw_json: &str) -> Result<(ReplyOutput, Option<String>), String> {
    tracing::debug!("CC raw JSON output: {}", raw_json);

    let parsed: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| format!("JSON parse error: {e}"))?;

    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let result_val = parsed
        .get("structured_output")
        .filter(|v| !v.is_null())
        .or_else(|| parsed.get("result"))
        .ok_or_else(|| {
            "CC response missing both 'structured_output' and 'result' fields".to_string()
        })?;

    // CC sometimes returns a plain string after tool use. Treat it as literal
    // content for delivery compatibility; schema-compliant outputs are objects.
    let output: ReplyOutput = if let Some(text) = result_val.as_str() {
        ReplyOutput {
            content: if text.trim().is_empty() {
                None
            } else {
                Some(
                    right_rich_content::RichContent::platform_text(text.to_owned())
                        .map_err(|e| e.to_string())?,
                )
            },
            reply_to_message_id: None,
            attachments: None,
            used_skill_receipts: None,
            bootstrap_complete: None,
            bootstrap_stage: None,
        }
    } else {
        serde_json::from_value(result_val.clone())
            .map_err(|e| format!("failed to deserialize result: {e}"))?
    };

    Ok((output, session_id))
}

/// Returns `true` when `name` is a `rightx-` skill package name (prefix "rightx-").
///
/// Used to filter skill receipts for probe-anchor collection and lifecycle
/// bump-use; the "rightx-" prefix is the convention for user-local skills
/// created or patched by the skill-learning pipeline.
pub(crate) const fn is_rightx_skill(name: &str) -> bool {
    let bytes = name.as_bytes();
    matches!(bytes, [b'r', b'i', b'g', b'h', b't', b'x', b'-', ..])
}

pub(crate) fn append_used_skill_receipts(
    content: Option<right_rich_content::RichContent>,
    receipts: Option<&[UsedSkillReceipt]>,
) -> Option<right_rich_content::RichContent> {
    let Some(receipts) = receipts else {
        return content;
    };
    if receipts.is_empty() {
        return content;
    }

    let lines: Vec<String> = receipts
        .iter()
        .filter(|r| is_rightx_skill(&r.package_name))
        .filter(|r| !r.message.trim().is_empty())
        .map(|r| format!("💡 {} (`{}`)", r.message.trim(), r.package_name.trim()))
        .collect();
    if lines.is_empty() {
        return content;
    }
    let joined = lines.join("\n");
    match content {
        Some(mut content) => {
            content.append_platform_paragraph(joined);
            Some(content)
        }
        None => right_rich_content::RichContent::paragraph(joined).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(content: &Option<right_rich_content::RichContent>) -> Option<String> {
        content
            .as_ref()
            .map(right_rich_content::RichContent::normalized_text)
    }

    #[test]
    fn parses_object_content_and_plain_result_compatibility() {
        let (output, session) =
            parse_reply_output(r#"{"session_id":"abc","result":{"content":{"text":"hello"}}}"#)
                .unwrap();
        assert_eq!(text(&output.content).as_deref(), Some("hello"));
        assert_eq!(session.as_deref(), Some("abc"));
        let (fallback, _) = parse_reply_output(r#"{"result":"literal *text*"}"#).unwrap();
        assert_eq!(text(&fallback.content).as_deref(), Some("literal *text*"));
    }

    #[test]
    fn oversized_plain_string_result_is_delivered_without_loss() {
        // CC plain-string results are platform-owned: 40,000 chars over the
        // 32,768 UTF-16 cap must parse into content whose delivery parts cover
        // the full text instead of failing the whole reply.
        let body = "result-body-".repeat(4_000);
        let raw = serde_json::json!({ "result": body }).to_string();
        let (output, _) = parse_reply_output(&raw).unwrap();
        let content = output.content.expect("oversized result must parse");
        content.validate().unwrap();
        let parts = content.delivery_parts();
        assert!(parts.len() > 1, "must fan out: {}", parts.len());
        let non_whitespace = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        for part in &parts {
            part.validate().unwrap();
        }
        assert_eq!(
            non_whitespace(
                &parts
                    .iter()
                    .map(right_rich_content::RichContent::normalized_text)
                    .collect::<String>()
            ),
            non_whitespace(&body)
        );
    }

    #[test]
    fn rejects_string_content_in_structured_output() {
        let error = parse_reply_output(r#"{"result":{"content":"markdown"}}"#).unwrap_err();
        assert!(error.contains("deserialize"));
    }

    #[test]
    fn receipts_append_platform_owned_paragraph() {
        let receipts = [UsedSkillReceipt {
            package_name: "rightx-workflow".into(),
            message: "Applied workflow".into(),
        }];
        let content = append_used_skill_receipts(
            Some(right_rich_content::RichContent::literal("Done").unwrap()),
            Some(&receipts),
        )
        .unwrap();
        assert_eq!(
            content.normalized_text(),
            "Done\n\n💡 Applied workflow (`rightx-workflow`)"
        );
    }

    #[test]
    fn null_reply_repair_contract() {
        let output = ReplyOutput {
            content: None,
            reply_to_message_id: None,
            attachments: None,
            used_skill_receipts: None,
            bootstrap_stage: None,
            bootstrap_complete: None,
        };
        assert!(null_reply_needs_repair(&output, false, Some("undelivered")));
        assert!(!null_reply_needs_repair(&output, true, Some("undelivered")));
    }

    #[test]
    fn used_skill_receipts_filter_only_rightx_names() {
        assert!(is_rightx_skill("rightx-foo"));
        assert!(!is_rightx_skill("foo"));
    }
}
