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
