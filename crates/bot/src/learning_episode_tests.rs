use super::*;
use right_agent::learning_episodes::{
    EpisodeSeedTriggerKind, ExecutionEventKind, LearningEpisodeKind,
};
use std::sync::atomic::AtomicBool;

fn conn() -> rusqlite::Connection {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
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
    }
}

fn claimed_episode(
    conn: &rusqlite::Connection,
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
    .unwrap();
    right_agent::learning_episodes::claim_ready_episode(conn, "right", "2026-05-19T10:01:30Z")
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

#[test]
fn accepted_signal_creates_pending_seed_without_cooldown() {
    let conn = conn();
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
    .unwrap();
    let row: (String, String) = conn
        .query_row(
            "SELECT status, ready_after FROM learning_episodes WHERE seed_ref='inv:inv-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        ("pending".to_owned(), "2026-05-19T10:01:30Z".to_owned())
    );
}

#[test]
fn selector_rejects_refs_outside_corpus() {
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:2"], vec![]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}

#[test]
fn selector_rejects_thinking_only_episode() {
    let corpus = SelectorCorpus::for_test(vec![], vec![("exec:10", ExecutionEventKind::Thinking)]);
    let output = EpisodeSelectorOutput::for_test_selected(vec![], vec!["exec:10"]);
    assert!(validate_selector_output(&corpus, &output).is_err());
}

#[test]
fn cron_and_async_seed_triggers_start_review_gate_without_effort_threshold() {
    assert_eq!(
        review_trigger_for_episode(EpisodeSeedTriggerKind::Cron),
        Some(ReviewTriggerKind::EffortThreshold)
    );
    assert_eq!(
        review_trigger_for_episode(EpisodeSeedTriggerKind::AsyncResult),
        Some(ReviewTriggerKind::EffortThreshold)
    );
}

#[test]
fn terminal_selector_output_rejects_refs_outside_corpus_before_marking_terminal() {
    let conn = conn();
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-terminal-invalid",
    );
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

    let err = record_selector_output(&conn, &runtime(), &episode, &corpus, output).unwrap_err();
    assert!(err.to_string().contains("outside corpus"));
    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "selecting");
}

#[test]
fn effective_selector_model_prefers_learning_override_then_inherited_model() {
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

#[test]
fn selected_episode_persists_effective_selector_model() {
    let conn = conn();
    let episode = claimed_episode(
        &conn,
        LearningEpisodeKind::ForegroundThread,
        EpisodeSeedTriggerKind::LearningSignal,
        "inv:inv-selected-model",
    );
    let corpus = SelectorCorpus::for_test(vec!["msg:1"], vec![]);
    let mut runtime = runtime();
    runtime.inherited_model = Some("claude-sonnet-inherited".to_owned());
    let output = EpisodeSelectorOutput::for_test_selected(vec!["msg:1"], vec![]);

    record_selector_output(&conn, &runtime, &episode, &corpus, output).unwrap();

    let selector_model: Option<String> = conn
        .query_row(
            "SELECT selector_model FROM learning_episodes WHERE id=?1",
            [episode.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(selector_model, Some("claude-sonnet-inherited".to_owned()));
}

#[tokio::test]
async fn drain_marks_failed_and_clears_gate_when_corpus_load_fails_after_gate_start() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).unwrap();
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
    .unwrap();
    conn.execute("DROP TABLE conversation_messages", [])
        .unwrap();
    drop(conn);

    let result = drain_ready_learning_episodes_once_with_selector(
        runtime_for_dir(temp.path()),
        panic_selector,
    )
    .await;

    assert!(result.is_err());
    let conn = right_db::open_connection(temp.path(), false).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM learning_episodes WHERE seed_ref='inv:inv-corpus-fails'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let review_running: i64 = conn
        .query_row(
            "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(review_running, 0);
}

#[tokio::test]
async fn drain_requeues_when_review_already_running() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).unwrap();
    right_agent::learned_skills::ensure_nudge_state(&conn, "right").unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running=1 WHERE agent_name='right'",
        [],
    )
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
    .unwrap();
    drop(conn);

    drain_ready_learning_episodes_once_with_selector(runtime_for_dir(temp.path()), panic_selector)
        .await
        .unwrap();

    let conn = right_db::open_connection(temp.path(), false).unwrap();
    let row: (String, i64) = conn
        .query_row(
            "SELECT e.status, s.review_running
             FROM learning_episodes e
             JOIN skill_nudge_state s ON s.agent_name=e.agent_name
             WHERE e.seed_ref='inv:inv-already-running'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("pending".to_owned(), 1));
}
