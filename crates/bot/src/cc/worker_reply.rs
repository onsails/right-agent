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
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<OutboundAttachment>>,
    pub used_skill_receipts: Option<Vec<UsedSkillReceipt>>,
    /// Bootstrap mode: `true` signals agent claims onboarding is complete.
    /// Server-side file check (`should_accept_bootstrap`) gates actual completion.
    pub bootstrap_complete: Option<bool>,
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

    // CC sometimes returns result as a plain string (e.g. after multi-turn MCP tool use)
    // instead of complying with --json-schema. Wrap it as ReplyOutput so the message is delivered.
    let mut output: ReplyOutput = if let Some(text) = result_val.as_str() {
        ReplyOutput {
            content: if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            },
            reply_to_message_id: None,
            attachments: None,
            used_skill_receipts: None,
            bootstrap_complete: None,
        }
    } else {
        serde_json::from_value(result_val.clone())
            .map_err(|e| format!("failed to deserialize result: {e}"))?
    };

    strip_caption_duplicating_content(&mut output);

    Ok((output, session_id))
}

/// Telegram delivers the top-level `content` string as its own message and an
/// attachment `caption` as part of the media message. When the model puts the
/// same text in both, the user receives it twice (observed: a news-post cron
/// authored the post into `content` *and* the chart photo's caption). Strip any
/// caption that duplicates `content` (trimmed compare) so the post is sent once;
/// the file itself is preserved. Keeping `content` (rather than the caption) is
/// the always-deliverable choice — `content` has no length cap, whereas Telegram
/// caps captions at 1024 chars.
fn strip_caption_duplicating_content(output: &mut ReplyOutput) {
    let content = match output.content.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c.to_owned(),
        _ => return,
    };
    let Some(attachments) = output.attachments.as_mut() else {
        return;
    };
    for att in attachments.iter_mut() {
        if att.caption.as_deref().map(str::trim) == Some(content.as_str()) {
            tracing::warn!(
                path = %att.path,
                "stripping attachment caption identical to reply content to avoid duplicate Telegram send"
            );
            att.caption = None;
        }
    }
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
    content: Option<String>,
    receipts: Option<&[UsedSkillReceipt]>,
) -> Option<String> {
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
        .map(|r| {
            format!(
                "💡 {} (<code>{}</code>)",
                r.message.trim(),
                r.package_name.trim()
            )
        })
        .collect();
    if lines.is_empty() {
        return content.filter(|c| !c.is_empty());
    }
    let joined = lines.join("\n");
    match content {
        Some(c) if !c.is_empty() => Some(format!("{c}\n\n{joined}")),
        _ => Some(joined),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // parse_reply_output tests (new structured output format per D-03)
    #[tokio::test]
    async fn parse_reply_output_content_string() {
        let json = r#"{"session_id":"abc","result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn parse_reply_output_content_null() {
        let json = r#"{"result":{"content":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[tokio::test]
    async fn parse_reply_output_missing_result_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing both"));
    }

    #[tokio::test]
    async fn parse_reply_output_reply_to_message_id() {
        let json = r#"{"result":{"content":"hi","reply_to_message_id":42,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.reply_to_message_id, Some(42));
    }

    #[tokio::test]
    async fn parse_reply_output_plain_string_result_wrapped_as_content() {
        // CC sometimes returns "result": "plain text" after MCP tool use instead of complying
        // with --json-schema. Must deliver the message rather than show an error.
        let json = r#"{"session_id":"abc","result":"hello from plain result"}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello from plain result"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn parse_reply_output_empty_string_result_is_silent() {
        let json = r#"{"result":""}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.content.is_none());
    }

    #[tokio::test]
    async fn parse_reply_output_array_result_returns_error() {
        // Array instead of object should fail deserialization
        let json = r#"{"result":[{"type":"text"}]}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn parse_reply_output_structured_output_field() {
        // When structured_output is present, it should be used instead of result
        let json = r#"{"session_id":"abc","result":"","structured_output":{"content":"Hello from structured!","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Hello from structured!"));
        assert_eq!(session_id.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn parse_reply_output_falls_back_to_result_when_no_structured_output() {
        // When structured_output is absent, fall back to result field
        let json = r#"{"session_id":"xyz","result":{"content":"Fallback result","reply_to_message_id":null,"attachments":null}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Fallback result"));
        assert_eq!(session_id.as_deref(), Some("xyz"));
    }

    #[tokio::test]
    async fn parse_reply_output_missing_result_and_structured_output_returns_error() {
        let json = r#"{"session_id":"x"}"#;
        let result = parse_reply_output(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("missing both"),
            "error should mention both fields: {err}"
        );
    }

    #[tokio::test]
    async fn parse_reply_output_with_attachments() {
        let json = r#"{"session_id":"abc","result":{"content":"Here you go","attachments":[{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Exported data"}]}}"#;
        let (output, session_id) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Here you go"));
        assert_eq!(session_id.as_deref(), Some("abc"));
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].path, "/sandbox/outbox/data.csv");
        assert_eq!(atts[0].filename.as_deref(), Some("results.csv"));
    }

    #[tokio::test]
    async fn parse_reply_output_strips_caption_identical_to_content() {
        // Telegram sends `content` as one message and an attachment `caption` as
        // part of the media message. Identical text in both double-posts. The
        // parser must strip the duplicate caption while keeping content + the file.
        let json = r#"{"result":{"content":"Big news: the thing happened.","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png","caption":"Big news: the thing happened."}]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(
            output.content.as_deref(),
            Some("Big news: the thing happened.")
        );
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 1, "attachment is preserved");
        assert_eq!(atts[0].path, "/sandbox/outbox/chart.png");
        assert!(
            atts[0].caption.is_none(),
            "caption duplicating content must be stripped, got {:?}",
            atts[0].caption
        );
    }

    #[tokio::test]
    async fn parse_reply_output_keeps_caption_differing_from_content() {
        // A genuinely different caption is intentional and must survive.
        let json = r#"{"result":{"content":"See the latest chart.","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png","caption":"BTC 4h, 2026-06-14"}]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        let atts = output.attachments.unwrap();
        assert_eq!(atts[0].caption.as_deref(), Some("BTC 4h, 2026-06-14"));
    }

    #[tokio::test]
    async fn parse_reply_output_strips_caption_differing_only_in_whitespace() {
        // Models routinely add a trailing newline to one side but not the other.
        // The trimmed compare must treat these as duplicates (exercises trim on
        // the caption side specifically).
        let json = r#"{"result":{"content":"hello","attachments":[{"type":"photo","path":"/sandbox/outbox/a.png","caption":"hello\n"}]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.attachments.unwrap()[0].caption.is_none());
    }

    #[tokio::test]
    async fn parse_reply_output_strips_duplicate_caption_in_media_group() {
        // The duplicate may sit on a non-first item of a media group. It must be
        // stripped here at parse time, BEFORE send-time caption folding
        // (`merge_group_captions`) would otherwise fold it into the visible group
        // caption. Locks the strip-before-fold ordering.
        let json = r#"{"result":{"content":"Post body.","attachments":[{"type":"photo","path":"/sandbox/outbox/a.png","media_group_id":"g","caption":"Chart A"},{"type":"photo","path":"/sandbox/outbox/b.png","media_group_id":"g","caption":"Post body."}]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        let atts = output.attachments.unwrap();
        assert_eq!(atts.len(), 2);
        assert_eq!(
            atts[0].caption.as_deref(),
            Some("Chart A"),
            "distinct caption survives"
        );
        assert!(
            atts[1].caption.is_none(),
            "content-duplicating caption stripped"
        );
    }

    #[tokio::test]
    async fn parse_reply_output_text_only() {
        let json =
            r#"{"result":{"content":"hello","reply_to_message_id":null,"attachments":null}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("hello"));
        assert!(output.attachments.is_none());
    }

    #[tokio::test]
    async fn parse_reply_output_plain_string_fallback() {
        let json = r#"{"result":"plain text fallback"}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("plain text fallback"));
        assert!(output.attachments.is_none());
    }

    #[tokio::test]
    async fn parse_reply_output_accepts_used_skill_receipts() {
        let json = r#"{"result":{"content":"done","used_skill_receipts":[{"package_name":"rightx-foo","message":"Used my workflow"}]}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        let receipts = output.used_skill_receipts.unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].package_name, "rightx-foo");
        assert_eq!(receipts[0].message, "Used my workflow");
    }

    #[tokio::test]
    async fn parse_reply_output_ignores_legacy_learning_signal_fields() {
        let json = r#"{"result":{"content":"done","learning_signal":{"kind":"create_candidate","package_name_hint":"right-demo","trigger":"explicit_user_request","reason_not_written":"needs_full_context_review","event_refs":["event-1"],"summary":"Capture this workflow."}}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn parse_reply_output_keeps_skill_fields_optional() {
        let json = r#"{"result":{"content":"hello"}}"#;
        let (output, _) = parse_reply_output(json).unwrap();
        assert!(output.used_skill_receipts.is_none());
    }

    #[tokio::test]
    async fn append_used_skill_receipts_renders_visual_marker_with_package_name() {
        let receipts = vec![UsedSkillReceipt {
            package_name: "rightx-foo".into(),
            message: "Used my workflow".into(),
        }];
        let content =
            append_used_skill_receipts(Some("Done".to_owned()), Some(receipts.as_slice())).unwrap();
        assert!(content.contains("💡"));
        assert!(content.contains("Used my workflow"));
        assert!(content.contains("<code>rightx-foo</code>"));
        assert!(content.starts_with("Done"));
    }

    #[tokio::test]
    async fn append_used_skill_receipts_filters_non_rightx_packages() {
        let receipts = vec![
            UsedSkillReceipt {
                package_name: "rightx-good".into(),
                message: "ok".into(),
            },
            UsedSkillReceipt {
                package_name: "built-in".into(),
                message: "leaked".into(),
            },
        ];
        let content =
            append_used_skill_receipts(Some("Done".to_owned()), Some(receipts.as_slice())).unwrap();
        assert!(content.contains("rightx-good"));
        assert!(!content.contains("leaked"));
        assert!(!content.contains("built-in"));
    }

    #[tokio::test]
    async fn append_used_skill_receipts_handles_multiple_receipts() {
        let receipts = vec![
            UsedSkillReceipt {
                package_name: "rightx-a".into(),
                message: "did a".into(),
            },
            UsedSkillReceipt {
                package_name: "rightx-b".into(),
                message: "did b".into(),
            },
        ];
        let content =
            append_used_skill_receipts(Some("Reply".to_owned()), Some(receipts.as_slice()))
                .unwrap();
        let lines: Vec<&str> = content.split('\n').collect();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("rightx-a") && l.contains("did a"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("rightx-b") && l.contains("did b"))
        );
    }

    #[tokio::test]
    async fn append_used_skill_receipts_filters_blank_messages() {
        let receipts = vec![
            UsedSkillReceipt {
                package_name: "rightx-a".into(),
                message: "   ".into(),
            },
            UsedSkillReceipt {
                package_name: "rightx-b".into(),
                message: "Real msg".into(),
            },
        ];
        let content =
            append_used_skill_receipts(Some("Done".to_owned()), Some(receipts.as_slice())).unwrap();
        assert!(content.contains("Real msg"));
        // Blank-only receipt should be filtered out, no trailing line for it
    }

    #[tokio::test]
    async fn append_used_skill_receipts_all_blank_returns_content_unchanged() {
        let receipts = vec![UsedSkillReceipt {
            package_name: "rightx-blank".into(),
            message: "  \n  ".into(),
        }];
        let content =
            append_used_skill_receipts(Some("Done".to_owned()), Some(receipts.as_slice()));
        assert_eq!(content.as_deref(), Some("Done"));
    }

    #[tokio::test]
    async fn append_used_skill_receipts_empty_receipts_leaves_content_unchanged() {
        let content = append_used_skill_receipts(Some("Done".to_owned()), Some(&[]));

        assert_eq!(content.as_deref(), Some("Done"));
    }

    // bootstrap mode tests
    #[tokio::test]
    async fn parse_reply_output_bootstrap_complete_true() {
        let json = r#"{"type":"result","result":{"content":"Done!","bootstrap_complete":true},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Done!"));
        assert_eq!(output.bootstrap_complete, Some(true));
    }

    #[tokio::test]
    async fn parse_reply_output_bootstrap_complete_false() {
        let json = r#"{"type":"result","result":{"content":"What's your name?","bootstrap_complete":false},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, Some(false));
    }

    #[tokio::test]
    async fn parse_reply_output_no_bootstrap_field() {
        let json = r#"{"type":"result","result":{"content":"Hello!"},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, None);
    }

    #[tokio::test]
    async fn should_accept_bootstrap_all_files_present() {
        let dir = tempfile::tempdir().unwrap();
        for f in right_agent::identity_mirror::IDENTITY_MIRROR_FILES {
            std::fs::write(dir.path().join(f), "# test").unwrap();
        }
        assert!(should_accept_bootstrap(dir.path()));
    }

    #[tokio::test]
    async fn should_accept_bootstrap_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // No identity files created
        assert!(!should_accept_bootstrap(dir.path()));
    }

    #[tokio::test]
    async fn should_accept_bootstrap_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        // Only IDENTITY.md exists
        std::fs::write(dir.path().join("IDENTITY.md"), "# test").unwrap();
        assert!(!should_accept_bootstrap(dir.path()));
    }

    #[tokio::test]
    async fn used_skill_receipts_filter_only_rightx_names() {
        assert!(is_rightx_skill("rightx-foo"));
        assert!(is_rightx_skill("rightx-x"));
        assert!(is_rightx_skill("rightx-some-skill-name"));
        assert!(!is_rightx_skill("foo"));
        assert!(!is_rightx_skill("rightx")); // no dash → not a rightx skill
        assert!(!is_rightx_skill(""));
        assert!(!is_rightx_skill("mcp__right__use_skill"));
    }
}
