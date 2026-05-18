use super::*;

#[test]
fn parses_high_confidence_create_candidate() {
    let value = serde_json::json!({
        "status": "create_candidate",
        "confidence": "high",
        "candidate_skill_name": "rightx-oauth-debugging",
        "candidate_summary": "Capture verified OAuth callback setup.",
        "evidence_refs": ["event-1", "event-2"],
        "user_notice": "I found a reusable workflow candidate for OAuth MCP setup."
    });

    let output = ReviewOutput::parse(value).unwrap();
    assert_eq!(output.status, ReviewOutputStatus::CreateCandidate);
    assert_eq!(output.confidence, ReviewOutputConfidence::High);
    assert_eq!(
        output.status.as_domain(),
        right_agent::learned_skills::ReviewStatus::CreateCandidate
    );
    assert_eq!(
        output.confidence.as_domain(),
        right_agent::learned_skills::ReviewConfidence::High
    );
    assert!(output.should_notify_user());
}

#[test]
fn low_confidence_candidate_does_not_notify() {
    let value = serde_json::json!({
        "status": "update_candidate",
        "confidence": "medium",
        "candidate_skill_name": "rightx-oauth-debugging",
        "candidate_summary": "Maybe add token refresh note.",
        "evidence_refs": ["event-1"],
        "user_notice": "This should not be sent."
    });

    let output = ReviewOutput::parse(value).unwrap();
    assert!(!output.should_notify_user());
}

#[test]
fn rejects_non_rightx_candidate_name() {
    let value = serde_json::json!({
        "status": "create_candidate",
        "confidence": "high",
        "candidate_skill_name": "oauth-debugging",
        "candidate_summary": "Capture verified OAuth callback setup.",
        "evidence_refs": ["event-1"],
        "user_notice": "Candidate."
    });

    let err = ReviewOutput::parse(value).unwrap_err();
    assert!(err.contains(right_mcp::LEARNED_SKILL_PREFIX), "{err}");
}

#[test]
fn nothing_to_learn_accepts_empty_candidate_fields() {
    let value = serde_json::json!({
        "status": "nothing_to_learn",
        "confidence": "low",
        "candidate_skill_name": null,
        "candidate_summary": null,
        "evidence_refs": [],
        "user_notice": null
    });

    let output = ReviewOutput::parse(value).unwrap();
    assert_eq!(output.status, ReviewOutputStatus::NothingToLearn);
    assert!(!output.should_notify_user());
}

#[test]
fn review_prompt_says_report_only_and_nothing_to_learn_is_normal() {
    let bundle = ReviewBundle {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        trigger_kind: "effort_threshold".to_owned(),
        accepted_signal_json: None,
        tool_iters_since_review: 15,
        turns_since_review: 3,
        skill_issue_hints_since_review: 0,
        event_timeline: vec!["event-1 user asked for OAuth setup".to_owned()],
        learning_events: vec!["start create rightx-oauth-debugging".to_owned()],
        learned_skills: vec![LearnedSkillSummary {
            name: "rightx-oauth-debugging".to_owned(),
            excerpt: "description: Use for OAuth MCP setup".to_owned(),
        }],
    };

    let prompt = build_review_prompt(&bundle);

    assert!(prompt.contains("Report-only"));
    assert!(prompt.contains("Do not write files"));
    assert!(prompt.contains("Do not call learning tools"));
    assert!(prompt.contains("Do not ask the user questions"));
    assert!(prompt.contains("nothing_to_learn is normal"));
    assert!(prompt.contains("rightx-oauth-debugging"));
}

#[test]
fn stream_event_timeline_is_stable_and_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agents/right");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let log_path = review_stream_log_path(&agent_dir, "session-1");
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    std::fs::write(
        &log_path,
        concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"checked OAuth callback settings"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"right mcp list --agent right"}}]}}"#,
            "\n",
            r#"{"type":"result","num_turns":3,"total_cost_usd":0.01,"session_id":"session-1"}"#,
            "\n"
        ),
    )
    .unwrap();

    let timeline = collect_stream_event_timeline(&agent_dir, "session-1", 8).unwrap();
    assert_eq!(timeline.len(), 3);
    assert!(timeline[0].starts_with("event-1 assistant_text: checked OAuth"));
    assert!(timeline[1].contains("tool_use Bash"));
    assert!(timeline[2].contains("result"));
}
