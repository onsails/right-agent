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
fn review_prompt_bounds_signal_lists_and_skills() {
    let long_signal = format!(
        "{{\"summary\":\"signal-head{}SIGNAL_TAIL_MARKER\"}}",
        "s".repeat(9000)
    );
    let mut event_timeline = vec![format!(
        "event-head {} EVENT_ITEM_TAIL_MARKER",
        "e".repeat(9000)
    )];
    event_timeline.extend((0..160).map(|i| format!("event-{i}")));
    event_timeline.push("EVENT_COUNT_TAIL_MARKER".to_owned());

    let mut learning_events = vec![format!(
        "learning-head {} LEARNING_ITEM_TAIL_MARKER",
        "l".repeat(9000)
    )];
    learning_events.extend((0..160).map(|i| format!("learning-{i}")));
    learning_events.push("LEARNING_COUNT_TAIL_MARKER".to_owned());

    let mut learned_skills = vec![LearnedSkillSummary {
        name: format!("rightx-head\n{}SKILL_NAME_TAIL_MARKER", "n".repeat(9000)),
        excerpt: format!(
            "description: skill-head {} SKILL_EXCERPT_TAIL_MARKER",
            "x".repeat(9000)
        ),
    }];
    learned_skills.extend((0..120).map(|i| LearnedSkillSummary {
        name: format!("rightx-skill-{i}"),
        excerpt: "description: bounded".to_owned(),
    }));
    learned_skills.push(LearnedSkillSummary {
        name: "rightx-skill-count-tail".to_owned(),
        excerpt: "SKILL_COUNT_TAIL_MARKER".to_owned(),
    });

    let bundle = ReviewBundle {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        trigger_kind: "learning_signal".to_owned(),
        accepted_signal_json: Some(long_signal),
        tool_iters_since_review: 15,
        turns_since_review: 3,
        skill_issue_hints_since_review: 2,
        event_timeline,
        learning_events,
        learned_skills,
    };

    let prompt = build_review_prompt(&bundle);

    assert!(prompt.contains("signal-head"));
    assert!(!prompt.contains("SIGNAL_TAIL_MARKER"));
    assert!(prompt.contains("event-head"));
    assert!(!prompt.contains("EVENT_ITEM_TAIL_MARKER"));
    assert!(!prompt.contains("EVENT_COUNT_TAIL_MARKER"));
    assert!(prompt.contains("learning-head"));
    assert!(!prompt.contains("LEARNING_ITEM_TAIL_MARKER"));
    assert!(!prompt.contains("LEARNING_COUNT_TAIL_MARKER"));
    assert!(prompt.contains("rightx-head"));
    assert!(!prompt.contains("rightx-head\n"));
    assert!(!prompt.contains("SKILL_NAME_TAIL_MARKER"));
    assert!(prompt.contains("description: skill-head"));
    assert!(!prompt.contains("SKILL_EXCERPT_TAIL_MARKER"));
    assert!(!prompt.contains("SKILL_COUNT_TAIL_MARKER"));
    assert!(prompt.len() < 25_000, "prompt must stay bounded");
}

#[test]
fn collect_host_rightx_skills_includes_only_learned_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".claude/skills");
    std::fs::create_dir_all(skills_dir.join("rightx-zeta")).unwrap();
    std::fs::write(
        skills_dir.join("rightx-zeta/SKILL.md"),
        "---\nname: rightx-zeta\ndescription: Zeta learned skill\n---\n# Zeta\nbody\n",
    )
    .unwrap();
    std::fs::create_dir_all(skills_dir.join("rightx-alpha")).unwrap();
    std::fs::write(
        skills_dir.join("rightx-alpha/SKILL.md"),
        "---\nname: rightx-alpha\ndescription: Alpha learned skill\n---\n# Alpha\nbody\n",
    )
    .unwrap();
    std::fs::create_dir_all(skills_dir.join("rightx-missing")).unwrap();
    std::fs::create_dir_all(skills_dir.join("custom-skill")).unwrap();
    std::fs::write(skills_dir.join("custom-skill/SKILL.md"), "# Custom\n").unwrap();

    let skills = collect_host_rightx_skill_index(dir.path()).unwrap();

    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0].name, "rightx-alpha");
    assert!(skills[0].excerpt.contains("Alpha learned skill"));
    assert_eq!(skills[1].name, "rightx-zeta");
    assert!(skills[1].excerpt.contains("Zeta learned skill"));
}

#[test]
fn parse_sandbox_skill_index_stdout_splits_records() {
    let stdout = "\
---RIGHT-SKILL---
/sandbox/.claude/skills/rightx-two/SKILL.md
description: Second skill
---RIGHT-SKILL---
/sandbox/.claude/skills/custom-skill/SKILL.md
description: Custom skill
---RIGHT-SKILL---
/sandbox/.claude/skills/rightx-one/SKILL.md
description: First skill
";

    let skills = parse_sandbox_skill_index_stdout(stdout);

    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0].name, "rightx-one");
    assert_eq!(skills[0].excerpt, "description: First skill");
    assert_eq!(skills[1].name, "rightx-two");
    assert_eq!(skills[1].excerpt, "description: Second skill");
}

#[test]
fn rightx_skill_index_excerpts_are_bounded_for_host_and_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(".claude/skills/rightx-long");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: rightx-long\ndescription: Long learned skill\n---\n{}\nHOST_TAIL_MARKER\n",
            "host-body\n".repeat(600)
        ),
    )
    .unwrap();

    let host_skills = collect_host_rightx_skill_index(dir.path()).unwrap();
    assert_eq!(host_skills.len(), 1);
    assert!(host_skills[0].excerpt.contains("Long learned skill"));
    assert!(!host_skills[0].excerpt.contains("HOST_TAIL_MARKER"));

    let sandbox_stdout = format!(
        "---RIGHT-SKILL---\n/sandbox/.claude/skills/rightx-long/SKILL.md\n---\nname: rightx-long\ndescription: Long learned skill\n---\n{}\nSANDBOX_TAIL_MARKER\n",
        "sandbox-body\n".repeat(600)
    );
    let sandbox_skills = parse_sandbox_skill_index_stdout(&sandbox_stdout);
    assert_eq!(sandbox_skills.len(), 1);
    assert!(sandbox_skills[0].excerpt.contains("Long learned skill"));
    assert!(!sandbox_skills[0].excerpt.contains("SANDBOX_TAIL_MARKER"));
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
