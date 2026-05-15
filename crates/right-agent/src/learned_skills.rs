#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningAction {
    Create,
    Update,
}

impl LearningAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningPhase {
    Start,
    Finish,
}

impl LearningPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Created,
    Updated,
    Aborted,
    Failed,
}

impl LearningStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Created | Self::Updated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningEvent {
    pub invocation_id: String,
    pub agent_name: String,
    pub action: LearningAction,
    pub skill_name: String,
    pub phase: LearningPhase,
    pub status: Option<LearningStatus>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub event_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSignalKind {
    Learning,
    SkillIssue,
}

impl NudgeSignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Learning => "learning",
            Self::SkillIssue => "skill_issue",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NudgeSignalRecord {
    pub invocation_id: String,
    pub agent_name: String,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub signal_kind: NudgeSignalKind,
    pub payload_json: serde_json::Value,
}

pub fn insert_learning_event(
    conn: &rusqlite::Connection,
    event: &LearningEvent,
) -> Result<(), rusqlite::Error> {
    let event_refs_json = serde_json::to_string(&event.event_refs)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO skill_learning_events \
         (invocation_id, agent_name, action, skill_name, phase, status, reason, message, summary, event_refs_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            event.invocation_id,
            event.agent_name,
            event.action.as_str(),
            event.skill_name,
            event.phase.as_str(),
            event.status.map(LearningStatus::as_str),
            event.reason,
            event.message,
            event.summary,
            event_refs_json,
        ],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [event.agent_name.as_str()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn successful_finish_exists(
    conn: &rusqlite::Connection,
    invocation_id: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events \
         WHERE invocation_id=?1 AND phase='finish' AND status IN ('created','updated')",
        [invocation_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn select_reply_signal(
    conn: &rusqlite::Connection,
    invocation_id: &str,
    learning_signal: Option<serde_json::Value>,
    skill_issue_signal: Option<serde_json::Value>,
) -> Result<Option<(NudgeSignalKind, serde_json::Value)>, rusqlite::Error> {
    if successful_finish_exists(conn, invocation_id)? {
        return Ok(None);
    }

    match (learning_signal, skill_issue_signal) {
        (Some(_), Some(_)) => Ok(None),
        (Some(signal), None) => Ok(validate_nudge_signal(NudgeSignalKind::Learning, signal)),
        (None, Some(signal)) => Ok(validate_nudge_signal(NudgeSignalKind::SkillIssue, signal)),
        (None, None) => Ok(None),
    }
}

fn validate_nudge_signal(
    signal_kind: NudgeSignalKind,
    signal: serde_json::Value,
) -> Option<(NudgeSignalKind, serde_json::Value)> {
    let expected_kind = match signal_kind {
        NudgeSignalKind::Learning => "create_candidate",
        NudgeSignalKind::SkillIssue => "update_candidate",
    };
    if signal.get("kind").and_then(|v| v.as_str()) != Some(expected_kind) {
        return None;
    }

    let has_text = match signal_kind {
        NudgeSignalKind::Learning => signal
            .get("summary")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty()),
        NudgeSignalKind::SkillIssue => signal
            .get("patch_hint")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty()),
    };
    if !has_text {
        return None;
    }

    let event_ref_count = signal
        .get("event_refs")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);
    let is_explicit_user_request = signal_kind == NudgeSignalKind::Learning
        && signal.get("trigger").and_then(|v| v.as_str()) == Some("explicit_user_request");
    let required_refs = if is_explicit_user_request { 1 } else { 2 };
    if event_ref_count < required_refs {
        return None;
    }

    Some((signal_kind, signal))
}

pub fn ensure_nudge_state(
    conn: &rusqlite::Connection,
    agent_name: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [agent_name],
    )?;
    Ok(())
}

pub fn increment_turn_nudge_counters(
    conn: &rusqlite::Connection,
    agent_name: &str,
    tool_iters: i64,
) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [agent_name],
    )?;
    tx.execute(
        "UPDATE skill_nudge_state \
         SET turns_since_review = turns_since_review + 1, \
             tool_iters_since_review = tool_iters_since_review + ?2 \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, tool_iters.max(0)],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn record_nudge_signal(
    conn: &rusqlite::Connection,
    record: &NudgeSignalRecord,
) -> Result<(), rusqlite::Error> {
    let payload = serde_json::to_string(&record.payload_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO skill_nudge_state (agent_name) VALUES (?1)",
        [record.agent_name.as_str()],
    )?;
    tx.execute(
        "INSERT INTO skill_nudge_signals \
         (invocation_id, agent_name, root_session_id, chat_id, thread_id, signal_kind, payload_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            record.invocation_id,
            record.agent_name,
            record.root_session_id,
            record.chat_id,
            record.thread_id,
            record.signal_kind.as_str(),
            payload,
        ],
    )?;
    if matches!(record.signal_kind, NudgeSignalKind::SkillIssue) {
        tx.execute(
            "UPDATE skill_nudge_state \
             SET skill_issue_hints_since_review = skill_issue_hints_since_review + 1 \
             WHERE agent_name = ?1",
            [record.agent_name.as_str()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn successful_finish_exists_only_for_created_or_updated() {
        let conn = conn();
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rl-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Failed),
                reason: None,
                message: None,
                summary: Some("write failed".to_owned()),
                event_refs: vec![],
            },
        )
        .unwrap();
        assert!(!successful_finish_exists(&conn, "inv-1").unwrap());

        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-1".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rl-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Created),
                reason: None,
                message: Some("Learned skill: rl-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["e1".to_owned(), "e2".to_owned()],
            },
        )
        .unwrap();
        assert!(successful_finish_exists(&conn, "inv-1").unwrap());
    }

    #[test]
    fn record_nudge_signal_persists_payload_and_updates_counter() {
        let conn = conn();
        record_nudge_signal(
            &conn,
            &NudgeSignalRecord {
                invocation_id: "inv-2".to_owned(),
                agent_name: "right".to_owned(),
                root_session_id: Some("root-1".to_owned()),
                chat_id: Some(10),
                thread_id: Some(20),
                signal_kind: NudgeSignalKind::SkillIssue,
                payload_json: serde_json::json!({"kind":"update_candidate"}),
            },
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_nudge_signals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let payload_json: String = conn
            .query_row("SELECT payload_json FROM skill_nudge_signals", [], |r| {
                r.get(0)
            })
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({"kind":"update_candidate"}),
            "payload_json should persist the accepted signal payload"
        );

        let hints: i64 = conn
            .query_row(
                "SELECT skill_issue_hints_since_review FROM skill_nudge_state WHERE agent_name='right'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hints, 1);
    }

    fn learning_signal(trigger: &str, event_refs: Vec<&str>, summary: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "create_candidate",
            "package_name_hint": "right-demo",
            "trigger": trigger,
            "reason_not_written": "needs review",
            "event_refs": event_refs,
            "summary": summary,
        })
    }

    fn skill_issue_signal(event_refs: Vec<&str>, patch_hint: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "update_candidate",
            "skill_name": "right-demo",
            "issue": "stale command",
            "reason_not_patched": "needs review",
            "observed_effect": "user had to retry",
            "event_refs": event_refs,
            "patch_hint": patch_hint,
        })
    }

    #[test]
    fn nudge_signal_is_dropped_when_successful_finish_exists() {
        let conn = conn();
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-success".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "right-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Created),
                reason: None,
                message: Some("Learned skill: right-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["event-1".to_owned()],
            },
        )
        .unwrap();

        let selected = select_reply_signal(
            &conn,
            "inv-success",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_is_dropped_when_both_signals_present() {
        let conn = conn();
        let selected = select_reply_signal(
            &conn,
            "inv-both",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            Some(skill_issue_signal(
                vec!["event-2"],
                "Patch the stale command.",
            )),
        )
        .unwrap();

        assert!(selected.is_none());
    }

    #[test]
    fn nudge_signal_requires_two_event_refs_unless_explicit_user_request() {
        let conn = conn();
        let dropped = select_reply_signal(
            &conn,
            "inv-short",
            Some(learning_signal(
                "agent_observed_repetition",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert!(dropped.is_none());

        let accepted = select_reply_signal(
            &conn,
            "inv-explicit",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "Capture this workflow.",
            )),
            None,
        )
        .unwrap();
        assert_eq!(
            accepted.map(|(kind, _)| kind),
            Some(NudgeSignalKind::Learning)
        );

        let empty_summary = select_reply_signal(
            &conn,
            "inv-empty-summary",
            Some(learning_signal(
                "explicit_user_request",
                vec!["event-1"],
                "",
            )),
            None,
        )
        .unwrap();
        assert!(empty_summary.is_none());

        let empty_patch_hint = select_reply_signal(
            &conn,
            "inv-empty-patch",
            None,
            Some(skill_issue_signal(vec!["event-1", "event-2"], "")),
        )
        .unwrap();
        assert!(empty_patch_hint.is_none());
    }
}
