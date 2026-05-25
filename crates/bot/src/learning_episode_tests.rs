use super::*;
use right_agent::learning_episodes::{
    EpisodeSeedTriggerKind, ExecutionEventKind, LearningEpisodeKind,
};
use right_agent::learning_episodes::{NewExecutionEvent, SelectedEpisodeUpdate, TrustLabel};
use right_db::conversation::{ConversationMessage, ConversationRole};
use std::sync::atomic::AtomicBool;

async fn conn() -> right_db::Connection {
    let conn = right_db::Connection::open_in_memory().await.unwrap();
    right_db::MIGRATIONS.to_latest(&conn).await.unwrap();
    conn
}

fn runtime() -> LearningEpisodeRuntime {
    LearningEpisodeRuntime {
        agent_dir: std::path::PathBuf::from("/tmp/right-agent-test-agent"),
        agent_db_dir: std::path::PathBuf::from("/tmp/right-agent-test-agent"),
        agent_name: "right".to_owned(),
        inherited_model: None,
        ssh_config_path: None,
        resolved_sandbox: None,
        debug: std::sync::Arc::new(AtomicBool::new(false)),
        learning: right_agent::agent::types::LearningConfig::default(),
        scheduler: None,
        bot: None,
    }
}

fn runtime_for_dir(path: &std::path::Path) -> LearningEpisodeRuntime {
    LearningEpisodeRuntime {
        agent_dir: path.to_path_buf(),
        agent_db_dir: path.to_path_buf(),
        agent_name: "right".to_owned(),
        inherited_model: None,
        ssh_config_path: None,
        resolved_sandbox: None,
        debug: std::sync::Arc::new(AtomicBool::new(false)),
        learning: right_agent::agent::types::LearningConfig::default(),
        scheduler: None,
        bot: None,
    }
}

#[tokio::test]
async fn completion_seed_capture_is_noop_for_deprecated_stage2() {
    let conn = conn().await;
    let mut runtime = runtime();
    runtime.learning.background_review_enabled = Some(true);

    let episode_id = runtime
        .capture_completion_seed(
            &conn,
            LearningEpisodeKind::CronRun,
            EpisodeSeedTriggerKind::Cron,
            "cron:deprecated",
            Some(10),
            Some(20),
        )
        .unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM learning_episodes", [], |r| r.get(0))
        .await
        .unwrap();
    assert_eq!(episode_id, 0);
    assert_eq!(count, 0);
}

async fn claimed_episode(
    conn: &right_db::Connection,
    kind: LearningEpisodeKind,
    seed_trigger_kind: EpisodeSeedTriggerKind,
    seed_ref: &str,
) -> LearningEpisodeRow {
    capture_episode_seed(
        conn,
        EpisodeSeedInput {
            agent_name: "right",
            kind,
            seed_trigger_kind,
            seed_ref,
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            settle_seconds: 90,
            now: "2026-05-19T10:00:00Z",
        },
    )
    .await
    .unwrap();
    right_agent::learning_episodes::claim_ready_episode(conn, "right", "2026-05-19T10:01:30Z")
        .await
        .unwrap()
        .unwrap()
}

fn panic_selector(
    _: LearningEpisodeRuntime,
    _: LearningEpisodeRow,
    _: SelectorCorpus,
) -> EpisodeSelectorFuture {
    Box::pin(async { panic!("selector should not run") })
}

fn successful_message_review(
    _: LearningEpisodeRuntime,
    bundle: crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture {
    Box::pin(async move {
        assert_eq!(bundle.learning_episode_id, Some(1));
        assert_eq!(bundle.source_invocation_id, "inv-review-success");
        assert_eq!(bundle.episode_messages.len(), 1);
        assert_eq!(bundle.episode_messages[0].content, "remember this workflow");
        crate::learning_review::ReviewOutput::parse(serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "rightx-review-success",
            "candidate_summary": "Remember the workflow.",
            "evidence_refs": [bundle.episode_messages[0].ref_id],
            "user_notice": null
        }))
        .map_err(anyhow::Error::msg)
    })
}

fn thinking_only_review(
    _: LearningEpisodeRuntime,
    bundle: crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture {
    Box::pin(async move {
        assert_eq!(bundle.episode_execution_events.len(), 1);
        assert_eq!(
            bundle.episode_execution_events[0].event_kind,
            right_agent::learning_episodes::ExecutionEventKind::Thinking
        );
        assert_eq!(
            bundle.episode_execution_events[0].trust_label,
            right_agent::learning_episodes::TrustLabel::Secondary
        );
        crate::learning_review::ReviewOutput::parse(serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "rightx-thinking-only",
            "candidate_summary": "Thinking is not enough.",
            "evidence_refs": [bundle.episode_execution_events[0].ref_id],
            "user_notice": null
        }))
        .map_err(anyhow::Error::msg)
    })
}

fn low_trust_message_review(
    _: LearningEpisodeRuntime,
    bundle: crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture {
    Box::pin(async move {
        assert_eq!(bundle.episode_messages.len(), 1);
        assert_eq!(
            bundle.episode_messages[0].trust_label,
            right_agent::learning_episodes::TrustLabel::LowTrust
        );
        crate::learning_review::ReviewOutput::parse(serde_json::json!({
            "status": "nothing_to_learn",
            "confidence": "low",
            "candidate_skill_name": null,
            "candidate_summary": null,
            "evidence_refs": [],
            "user_notice": null
        }))
        .map_err(anyhow::Error::msg)
    })
}

fn failed_structured_review(
    _: LearningEpisodeRuntime,
    _: crate::learning_review::ReviewBundle,
) -> EpisodeReviewInvocationFuture {
    Box::pin(async move {
        crate::learning_review::ReviewOutput::parse(serde_json::json!({
            "status": "failed",
            "confidence": "low",
            "candidate_skill_name": null,
            "candidate_summary": null,
            "evidence_refs": [],
            "user_notice": null
        }))
        .map_err(anyhow::Error::msg)
    })
}

async fn prepare_selected_episode(
    conn: &right_db::Connection,
    seed_ref: &str,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
) -> i64 {
    let episode = claimed_episode(
        conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        seed_ref,
    )
    .await;
    right_agent::learning_episodes::mark_episode_selected(
        conn,
        episode.id,
        &SelectedEpisodeUpdate {
            start_ref: message_refs
                .first()
                .or_else(|| execution_event_refs.first())
                .cloned(),
            end_ref: message_refs
                .last()
                .or_else(|| execution_event_refs.last())
                .cloned(),
            message_refs,
            execution_event_refs,
            selector_model: None,
            selector_output_json: serde_json::json!({"status": "selected"}),
            boundary_rationale: Some("test".to_owned()),
            confidence: Some("high".to_owned()),
            context_incomplete: false,
            episode_hash: None,
            last_evidence_at: None,
        },
    )
    .await
    .unwrap();
    right_agent::learned_skills::ensure_nudge_state(conn, "right")
        .await
        .unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running=1 WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();
    episode.id
}

async fn insert_review_message(conn: &right_db::Connection, content: &str) -> i64 {
    insert_review_message_with_route(conn, content, true, true, Some("session-review"), Some(1))
        .await
}

async fn insert_review_message_with_route(
    conn: &right_db::Connection,
    content: &str,
    addressed_to_bot: bool,
    routed_to_agent: bool,
    root_session_id: Option<&str>,
    turn_id: Option<u64>,
) -> i64 {
    right_db::conversation::archive_message(
        conn,
        ConversationMessage {
            platform: "telegram",
            chat_id: 10,
            thread_id: 20,
            message_id: None,
            sender_user_id: None,
            sender_name: None,
            addressed_to_bot,
            routed_to_agent,
            root_session_id,
            turn_id,
            role: ConversationRole::User,
            content,
        },
    )
    .await
    .unwrap()
}

async fn insert_review_execution_event(
    conn: &right_db::Connection,
    event_kind: ExecutionEventKind,
    content: &str,
) -> i64 {
    right_agent::learning_episodes::insert_execution_event(
        conn,
        &NewExecutionEvent {
            agent_name: "right".to_owned(),
            root_session_id: Some("session-review".to_owned()),
            invocation_id: Some("inv-review".to_owned()),
            turn_id: Some(1),
            async_run_id: None,
            cron_job_name: None,
            cron_run_id: None,
            seq: 1,
            event_kind,
            tool_name: None,
            content_json: serde_json::json!({ "text": content }),
            content_text: content.to_owned(),
            trust_label: TrustLabel::Primary,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn accepted_signal_creates_pending_seed_without_cooldown() {
    let conn = conn().await;
    capture_episode_seed(
        &conn,
        EpisodeSeedInput {
            agent_name: "right",
            kind: LearningEpisodeKind::ForegroundThread,
            seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
            seed_ref: "inv:inv-1",
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            settle_seconds: 90,
            now: "2026-05-19T10:00:00Z",
        },
    )
    .await
    .unwrap();
    let row: (String, String) = conn
        .query_row(
            "SELECT status, ready_after FROM learning_episodes WHERE seed_ref='inv:inv-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(
        row,
        ("pending".to_owned(), "2026-05-19T10:01:30Z".to_owned())
    );
}

#[tokio::test]
async fn selector_rejects_refs_outside_corpus() {
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:2"], vec![]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}

#[tokio::test]
async fn selector_rejects_thinking_only_episode() {
    let corpus = SelectorCorpus::for_test(vec![], vec![("exec:10", ExecutionEventKind::Thinking)]);
    let output = EpisodeSelectorOutput::for_test_selected(vec![], vec!["exec:10"]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}

#[tokio::test]
async fn cron_and_async_seed_triggers_start_review_gate_without_effort_threshold() {
    assert_eq!(
        review_trigger_for_episode(EpisodeSeedTriggerKind::Cron),
        Some(ReviewTriggerKind::EffortThreshold)
    );
    assert_eq!(
        review_trigger_for_episode(EpisodeSeedTriggerKind::AsyncResult),
        Some(ReviewTriggerKind::EffortThreshold)
    );
}

#[tokio::test]
async fn terminal_selector_output_rejects_refs_outside_corpus_before_marking_terminal() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-terminal-invalid",
    )
    .await;
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let output = EpisodeSelectorOutput {
        status: "no_episode".to_owned(),
        kind: LearningEpisodeKind::ForegroundThread,
        start_ref: Some("msg:2".to_owned()),
        end_ref: Some("msg:2".to_owned()),
        message_refs: vec!["msg:2".to_owned()],
        execution_event_refs: Vec::new(),
        boundary_rationale: Some("invalid terminal refs".to_owned()),
        confidence: "low".to_owned(),
        context_incomplete: false,
        raw: serde_json::json!({
            "status": "no_episode",
            "kind": "foreground_thread",
            "start_ref": "msg:2",
            "end_ref": "msg:2",
            "message_refs": ["msg:2"],
            "execution_event_refs": [],
            "boundary_rationale": "invalid terminal refs",
            "confidence": "low",
            "context_incomplete": false
        }),
    };

    let err = record_selector_output(&conn, &runtime(), &episode, &corpus, output)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("outside corpus"));
    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(status, "selecting");
}

#[tokio::test]
async fn effective_selector_model_prefers_learning_override_then_inherited_model() {
    let mut runtime = runtime();
    runtime.inherited_model = Some("claude-sonnet-inherited".to_owned());
    assert_eq!(
        effective_selector_model(&runtime),
        Some("claude-sonnet-inherited".to_owned())
    );

    runtime.learning.episode_selector_model = Some("claude-opus-selector".to_owned());
    assert_eq!(
        effective_selector_model(&runtime),
        Some("claude-opus-selector".to_owned())
    );
}

#[tokio::test]
async fn selected_episode_persists_effective_selector_model() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-selected-model",
    )
    .await;
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let mut runtime = runtime();
    runtime.inherited_model = Some("claude-sonnet-inherited".to_owned());
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1"], vec![]);

    record_selector_output(&conn, &runtime, &episode, &corpus, output)
        .await
        .unwrap();

    let selector_model: Option<String> = conn
        .query_row(
            "SELECT selector_model FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(selector_model, Some("claude-sonnet-inherited".to_owned()));
}

#[tokio::test]
async fn selected_selector_output_requires_non_null_boundaries_in_corpus() {
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let mut output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1"], vec![]);
    output.start_ref = None;

    let err = validate_selector_output(&corpus, &output).unwrap_err();

    assert!(err.contains("selected output requires start_ref and end_ref"));
}

#[tokio::test]
async fn selected_episode_persists_episode_hash() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-selected-hash",
    )
    .await;
    let corpus = SelectorCorpus::for_test(
        vec!["msg:1", "msg:2"],
        vec![("exec:3", ExecutionEventKind::ToolResult)],
    );
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1", "msg:2"], vec!["exec:3"]);

    record_selector_output(&conn, &runtime(), &episode, &corpus, output)
        .await
        .unwrap();

    let episode_hash: Option<String> = conn
        .query_row(
            "SELECT episode_hash FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert!(episode_hash.as_deref().is_some_and(|hash| !hash.is_empty()));
}

#[tokio::test]
async fn duplicate_selected_episode_is_suppressed_before_review() {
    let conn = conn().await;
    let existing = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-duplicate-existing",
    )
    .await;
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1"], vec![]);
    record_selector_output(&conn, &runtime(), &existing, &corpus, output)
        .await
        .unwrap();

    let duplicate = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-duplicate-current",
    )
    .await;
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1"], vec![]);

    let should_review = record_selector_output(&conn, &runtime(), &duplicate, &corpus, output)
        .await
        .expect("duplicate should be terminal, not an error");

    let row: (String, String) = conn
        .query_row(
            "SELECT status, selector_output_json FROM learning_episodes WHERE id=?1",
            [duplicate.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert!(!should_review);
    assert_eq!(row.0, "no_episode");
    assert!(row.1.contains("duplicate_episode"));
}

#[tokio::test]
async fn selector_corpus_includes_execution_events_for_message_turns() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:seed-invocation",
    )
    .await;
    insert_review_message_with_route(
        &conn,
        "nearby correction",
        true,
        true,
        Some("session-nearby"),
        Some(9),
    )
    .await;
    let event_id = right_agent::learning_episodes::insert_execution_event(
        &conn,
        &NewExecutionEvent {
            agent_name: "right".to_owned(),
            root_session_id: Some("session-nearby".to_owned()),
            invocation_id: Some("different-invocation".to_owned()),
            turn_id: Some(9),
            async_run_id: None,
            cron_job_name: None,
            cron_run_id: None,
            seq: 7,
            event_kind: ExecutionEventKind::ToolResult,
            tool_name: None,
            content_json: serde_json::json!({ "text": "tool evidence" }),
            content_text: "tool evidence".to_owned(),
            trust_label: TrustLabel::Primary,
        },
    )
    .await
    .unwrap();

    let corpus = load_selector_corpus(&conn, &episode).await.unwrap();

    assert!(
        corpus
            .execution_events
            .iter()
            .any(|event| event.id == event_id),
        "execution event sharing the included message turn must be in corpus"
    );
}

#[tokio::test]
async fn episode_reviewer_inserts_report_and_marks_reviewed() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    let message_id = insert_review_message(&conn, "remember this workflow").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-review-success",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    drop(conn);

    run_episode_reviewer_with_invocation(
        runtime_for_dir(temp.path()),
        episode_id,
        successful_message_review,
    )
    .await
    .unwrap();

    let conn = right_db::open_connection(temp.path(), false).await.unwrap();
    let row: (String, i64, String, String, i64) = conn
        .query_row(
            "SELECT e.status, r.learning_episode_id, r.source_invocation_id, r.evidence_refs_json, s.review_running
             FROM learning_episodes e
             JOIN skill_review_reports r ON r.learning_episode_id=e.id
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
        .unwrap();
    assert_eq!(row.0, "reviewed");
    assert_eq!(row.1, episode_id);
    assert_eq!(row.2, "inv-review-success");
    assert_eq!(
        row.3,
        serde_json::json!([format!("msg:{message_id}")]).to_string()
    );
    assert_eq!(row.4, 0);
}

#[tokio::test]
async fn episode_reviewer_failed_output_marks_episode_failed_and_clears_gate() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    let message_id = insert_review_message(&conn, "reviewer cannot decide").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-review-failed-output",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    drop(conn);

    run_episode_reviewer_with_invocation(
        runtime_for_dir(temp.path()),
        episode_id,
        failed_structured_review,
    )
    .await
    .unwrap();

    let conn = right_db::open_connection(temp.path(), false).await.unwrap();
    let row: (String, String, i64, i64) = conn
        .query_row(
            "SELECT e.status, r.status, r.learning_episode_id, s.review_running
             FROM learning_episodes e
             JOIN skill_review_reports r ON r.learning_episode_id=e.id
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .await
        .unwrap();
    assert_eq!(
        row,
        ("failed".to_owned(), "failed".to_owned(), episode_id, 0)
    );
}

#[tokio::test]
async fn episode_reviewer_rejects_thinking_only_candidate_and_clears_gate() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    let event_id = insert_review_execution_event(
        &conn,
        ExecutionEventKind::Thinking,
        "private reasoning only",
    )
    .await;
    let episode_id = prepare_selected_episode(
        &conn,
        "async:async-review-thinking",
        Vec::new(),
        vec![format!("exec:{event_id}")],
    )
    .await;
    drop(conn);

    let err = run_episode_reviewer_with_invocation(
        runtime_for_dir(temp.path()),
        episode_id,
        thinking_only_review,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("observable episode ref"),
        "{err:#}"
    );

    let conn = right_db::open_connection(temp.path(), false).await.unwrap();
    let row: (String, i64, i64) = conn
        .query_row(
            "SELECT e.status, s.review_running, COUNT(r.id)
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             LEFT JOIN skill_review_reports r ON r.learning_episode_id=e.id
             WHERE e.id=?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(row, ("failed".to_owned(), 0, 0));
}

#[tokio::test]
async fn episode_reviewer_preserves_low_trust_message_label_in_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    let message_id = insert_review_message_with_route(
        &conn,
        "nearby unaddressed correction",
        false,
        false,
        Some("session-review"),
        Some(1),
    )
    .await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-review-low-trust",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    drop(conn);

    run_episode_reviewer_with_invocation(
        runtime_for_dir(temp.path()),
        episode_id,
        low_trust_message_review,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn drain_marks_failed_and_clears_gate_when_corpus_load_fails_after_gate_start() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    capture_episode_seed(
        &conn,
        EpisodeSeedInput {
            agent_name: "right",
            kind: LearningEpisodeKind::ForegroundThread,
            seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
            seed_ref: "inv:inv-corpus-fails",
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            settle_seconds: 90,
            now: "2020-01-01T00:00:00Z",
        },
    )
    .await
    .unwrap();
    conn.execute("DROP TABLE conversation_messages", [])
        .await
        .unwrap();
    drop(conn);

    let result = drain_ready_learning_episodes_once_with_selector(
        runtime_for_dir(temp.path()),
        panic_selector,
    )
    .await;

    assert!(result.is_err());
    let conn = right_db::open_connection(temp.path(), false).await.unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE seed_ref='inv:inv-corpus-fails'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    let review_running: i64 = conn
        .query_row(
            "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(review_running, 0);
}

#[tokio::test]
async fn drain_requeues_when_review_already_running() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).await.unwrap();
    right_agent::learned_skills::ensure_nudge_state(&conn, "right")
        .await
        .unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running=1 WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();
    capture_episode_seed(
        &conn,
        EpisodeSeedInput {
            agent_name: "right",
            kind: LearningEpisodeKind::ForegroundThread,
            seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
            seed_ref: "inv:inv-already-running",
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            settle_seconds: 90,
            now: "2020-01-01T00:00:00Z",
        },
    )
    .await
    .unwrap();
    drop(conn);

    drain_ready_learning_episodes_once_with_selector(runtime_for_dir(temp.path()), panic_selector)
        .await
        .unwrap();

    let conn = right_db::open_connection(temp.path(), false).await.unwrap();
    let row: (String, i64) = conn
        .query_row(
            "SELECT e.status, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.seed_ref='inv:inv-already-running'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(row, ("pending".to_owned(), 1));
}

#[tokio::test]
async fn requeue_episode_or_fail_requests_follow_up_drain() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-requeue-follow-up",
    )
    .await;

    requeue_episode_or_fail(
        &conn,
        &runtime(),
        episode.id,
        chrono::DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        90,
        "2026-05-19T10:00:00Z",
    )
    .await
    .unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn requeue_episode_or_fail_preserves_gate_when_no_row_matched() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-requeue-no-row",
    )
    .await;
    // Simulate a concurrent writer moving the row out of 'selecting' while a
    // separate review owns the review_running gate. The requeue helper is used
    // after Skip(AlreadyRunning), so it must not clear a gate it did not acquire.
    right_agent::learned_skills::ensure_nudge_state(&conn, "right")
        .await
        .unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running=1 WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();
    conn.execute(
        "UPDATE learning_episodes SET status='pending' WHERE id=?1",
        [episode.id],
    )
    .await
    .unwrap();

    requeue_episode_or_fail(
        &conn,
        &runtime(),
        episode.id,
        chrono::DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        90,
        "2026-05-19T10:00:00Z",
    )
    .await
    .unwrap();

    let review_running: i64 = conn
        .query_row(
            "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(review_running, 1);
}

#[tokio::test]
async fn startup_recovery_requeues_selecting_episode_and_clears_gate() {
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-recover-selecting",
    )
    .await;
    right_agent::learned_skills::ensure_nudge_state(&conn, "right")
        .await
        .unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running=1 WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();

    let recovered = recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let row: (String, String, i64) = conn
        .query_row(
            "SELECT e.status, e.ready_after, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(
        row,
        ("pending".to_owned(), "2026-05-19T11:00:00Z".to_owned(), 0)
    );
}

#[tokio::test]
async fn startup_recovery_marks_reviewing_episode_failed_from_latest_failed_report() {
    let conn = conn().await;
    let message_id = insert_review_message(&conn, "failed review evidence").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-recover-failed-report",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    conn.execute(
        "UPDATE learning_episodes SET status='reviewing' WHERE id=?1",
        [episode_id],
    )
    .await
    .unwrap();
    right_agent::learned_skills::insert_skill_review_report(
        &conn,
        &right_agent::learned_skills::SkillReviewReport {
            agent_name: "right".to_owned(),
            source_invocation_id: "inv-recover-failed-report".to_owned(),
            learning_episode_id: Some(episode_id),
            root_session_id: None,
            chat_id: Some(10),
            thread_id: Some(20),
            trigger_kind: ReviewTriggerKind::LearningSignal,
            status: ReviewStatus::Failed,
            confidence: right_agent::learned_skills::ReviewConfidence::Low,
            candidate_skill_name: None,
            candidate_summary: None,
            evidence_refs: Vec::new(),
            review_output_json: serde_json::json!({"status":"failed"}),
            telegram_notified: false,
        },
    )
    .await
    .unwrap();

    recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE id=?1",
            [episode_id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn startup_recovery_marks_reviewing_episode_reviewed_from_successful_report() {
    let conn = conn().await;
    let message_id = insert_review_message(&conn, "successful review evidence").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-recover-success-report",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    conn.execute(
        "UPDATE learning_episodes SET status='reviewing' WHERE id=?1",
        [episode_id],
    )
    .await
    .unwrap();
    right_agent::learned_skills::insert_skill_review_report(
        &conn,
        &right_agent::learned_skills::SkillReviewReport {
            agent_name: "right".to_owned(),
            source_invocation_id: "inv-recover-success-report".to_owned(),
            learning_episode_id: Some(episode_id),
            root_session_id: None,
            chat_id: Some(10),
            thread_id: Some(20),
            trigger_kind: ReviewTriggerKind::LearningSignal,
            status: ReviewStatus::NothingToLearn,
            confidence: right_agent::learned_skills::ReviewConfidence::Low,
            candidate_skill_name: None,
            candidate_summary: None,
            evidence_refs: Vec::new(),
            review_output_json: serde_json::json!({"status":"nothing_to_learn"}),
            telegram_notified: false,
        },
    )
    .await
    .unwrap();

    recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE id=?1",
            [episode_id],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(status, "reviewed");
}

#[tokio::test]
async fn startup_recovery_preserves_review_running_when_prior_review_reported_status() {
    // Regression: `recover_stale_inflight_episodes` must NOT clear
    // `review_running` if a legitimate review has already reported a
    // status (i.e. `last_review_status IS NOT NULL`). Only the stranded
    // case — gate set with no prior status — should be cleared.
    let conn = conn().await;
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-recover-preserves-gate",
    )
    .await;
    conn.execute(
        "UPDATE learning_episodes SET status='reviewing' WHERE id=?1",
        [episode.id],
    )
    .await
    .unwrap();
    right_agent::learned_skills::ensure_nudge_state(&conn, "right")
        .await
        .unwrap();
    conn.execute(
        "UPDATE skill_nudge_state \
         SET review_running=1, last_review_status='succeeded' \
         WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();

    let recovered = recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let (episode_status, ready_after, review_running): (String, String, i64) = conn
        .query_row(
            "SELECT e.status, e.ready_after, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(episode_status, "pending");
    assert_eq!(ready_after, "2026-05-19T11:00:00Z");
    assert_eq!(
        review_running, 1,
        "review_running must stay 1 because last_review_status is not NULL",
    );
}

#[tokio::test]
async fn recovery_requeues_selected_episode_with_no_review_report_to_pending() {
    // Regression: there is a narrow window between `mark_episode_selected`
    // (which transitions `selecting` -> `selected` and is followed by the
    // reviewer being invoked) where the bot can die. End state on disk is
    // status='selected', review_running=1, and no skill_review_reports row.
    // Startup recovery must requeue the episode to 'pending' and clear the
    // stranded gate.
    let conn = conn().await;
    let message_id = insert_review_message(&conn, "selector-to-reviewer crash evidence").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-recover-selected-no-report",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;

    let recovered = recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let (episode_status, ready_after, review_running): (String, String, i64) = conn
        .query_row(
            "SELECT e.status, e.ready_after, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(episode_status, "pending");
    assert_eq!(ready_after, "2026-05-19T11:00:00Z");
    assert_eq!(
        review_running, 0,
        "review_running must clear because no review reported a status",
    );
}

#[tokio::test]
async fn recovery_marks_selected_episode_with_review_report_as_reviewed() {
    // Regression: if the reviewer actually inserted a `skill_review_reports`
    // row but the bot died before finalizing the episode status, startup
    // recovery must finalize the episode (status='reviewed') rather than
    // requeue it. The gate-clear in `recover_stale_inflight_episodes` is
    // scoped to `last_review_status IS NULL`, so a populated
    // `last_review_status` here keeps `review_running=1`.
    let conn = conn().await;
    let message_id = insert_review_message(&conn, "selected-with-report evidence").await;
    let episode_id = prepare_selected_episode(
        &conn,
        "inv:inv-recover-selected-with-report",
        vec![format!("msg:{message_id}")],
        Vec::new(),
    )
    .await;
    right_agent::learned_skills::insert_skill_review_report(
        &conn,
        &right_agent::learned_skills::SkillReviewReport {
            agent_name: "right".to_owned(),
            source_invocation_id: "inv-recover-selected-with-report".to_owned(),
            learning_episode_id: Some(episode_id),
            root_session_id: None,
            chat_id: Some(10),
            thread_id: Some(20),
            trigger_kind: ReviewTriggerKind::LearningSignal,
            status: ReviewStatus::NothingToLearn,
            confidence: right_agent::learned_skills::ReviewConfidence::Low,
            candidate_skill_name: None,
            candidate_summary: None,
            evidence_refs: Vec::new(),
            review_output_json: serde_json::json!({"status":"nothing_to_learn"}),
            telegram_notified: false,
        },
    )
    .await
    .unwrap();
    // Simulate the reviewer's `mark_review_finished_in_tx` having stamped
    // `last_review_status` before the crash. Without this, the recovery
    // gate-clear (`last_review_status IS NULL`) would still fire.
    conn.execute(
        "UPDATE skill_nudge_state \
         SET last_review_status='nothing_to_learn' \
         WHERE agent_name='right'",
        [],
    )
    .await
    .unwrap();

    let recovered = recover_stale_inflight_episodes(&conn, "right", "2026-05-19T11:00:00Z")
        .await
        .unwrap();

    let (episode_status, review_running): (String, i64) = conn
        .query_row(
            "SELECT e.status, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.id=?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(episode_status, "reviewed");
    assert_eq!(
        review_running, 1,
        "review_running must stay 1 because last_review_status is not NULL",
    );
}

#[tokio::test]
async fn drain_scheduler_joins_within_timeout_after_shutdown() {
    // Regression: `DrainScheduler::spawn` must return a `JoinHandle` whose
    // task observes `shutdown.cancelled()` and exits promptly. Previously the
    // task was detached and could outlive `run_telegram`'s return while still
    // holding a CC selector/reviewer child via `ProcessGroupChild`.
    let temp = tempfile::tempdir().unwrap();
    // Migrate the DB so a stray drain pass on shutdown finds an empty,
    // schema-current corpus (it should not be reached given the cancel race).
    let _ = right_db::open_connection(temp.path(), true).await.unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let (scheduler, handle) = DrainScheduler::spawn(
        runtime_for_dir(temp.path()),
        // Long settle so the task is parked on the debounce sleep when we
        // cancel — exercises the `shutdown.cancelled()` arm in the sleep
        // `select!` rather than the recv arm.
        std::time::Duration::from_secs(60),
        shutdown.clone(),
    );

    scheduler.schedule_drain();
    // Give the task a moment to enter its debounce sleep.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("drain task did not exit within 1s of shutdown")
        .expect("drain task panicked");
}

#[tokio::test]
async fn drain_scheduler_noop_when_background_disabled() {
    use crate::learning_episode::DrainScheduler;
    use tokio_util::sync::CancellationToken;

    let cancel = CancellationToken::new();
    let scheduler = DrainScheduler::noop();
    scheduler.schedule_drain();
    // No panic, no work — the noop variant must be cheap.
    drop(scheduler);
    cancel.cancel();
}
