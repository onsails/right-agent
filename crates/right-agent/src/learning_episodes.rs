use right_db::params;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionEventKind {
    AssistantText,
    Thinking,
    ToolCall,
    ToolResult,
    ToolError,
    InvocationResult,
    Other,
    /// Legacy bot-side stream-derived event kind used by the legacy
    /// foreground review path (event-* refs read from the NDJSON stream
    /// log instead of the execution_events DB table). Never written to
    /// `execution_events.event_kind`.
    StreamEvent,
}

impl ExecutionEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssistantText => "assistant_text",
            Self::Thinking => "thinking",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::ToolError => "tool_error",
            Self::InvocationResult => "invocation_result",
            Self::Other => "other",
            Self::StreamEvent => "stream_event",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, InvalidDbValue> {
        match value {
            "assistant_text" => Ok(Self::AssistantText),
            "thinking" => Ok(Self::Thinking),
            "tool_call" => Ok(Self::ToolCall),
            "tool_result" => Ok(Self::ToolResult),
            "tool_error" => Ok(Self::ToolError),
            "invocation_result" => Ok(Self::InvocationResult),
            "other" => Ok(Self::Other),
            "stream_event" => Ok(Self::StreamEvent),
            _ => Err(InvalidDbValue::new("ExecutionEventKind", value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLabel {
    Primary,
    Secondary,
    LowTrust,
}

impl TrustLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::LowTrust => "low_trust",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, InvalidDbValue> {
        match value {
            "primary" => Ok(Self::Primary),
            "secondary" => Ok(Self::Secondary),
            "low_trust" => Ok(Self::LowTrust),
            _ => Err(InvalidDbValue::new("TrustLabel", value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningEpisodeKind {
    ForegroundThread,
    AsyncContinuation,
    CronRun,
}

impl LearningEpisodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ForegroundThread => "foreground_thread",
            Self::AsyncContinuation => "async_continuation",
            Self::CronRun => "cron_run",
        }
    }

    fn from_db(value: String) -> Result<Self, InvalidDbValue> {
        match value.as_str() {
            "foreground_thread" => Ok(Self::ForegroundThread),
            "async_continuation" => Ok(Self::AsyncContinuation),
            "cron_run" => Ok(Self::CronRun),
            _ => Err(InvalidDbValue::new("LearningEpisodeKind", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeSeedTriggerKind {
    LearningSignal,
    SkillIssueSignal,
    EffortThreshold,
    Cron,
    AsyncResult,
}

impl EpisodeSeedTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LearningSignal => "learning_signal",
            Self::SkillIssueSignal => "skill_issue_signal",
            Self::EffortThreshold => "effort_threshold",
            Self::Cron => "cron",
            Self::AsyncResult => "async_result",
        }
    }

    fn from_db(value: String) -> Result<Self, InvalidDbValue> {
        match value.as_str() {
            "learning_signal" => Ok(Self::LearningSignal),
            "skill_issue_signal" => Ok(Self::SkillIssueSignal),
            "effort_threshold" => Ok(Self::EffortThreshold),
            "cron" => Ok(Self::Cron),
            "async_result" => Ok(Self::AsyncResult),
            _ => Err(InvalidDbValue::new("EpisodeSeedTriggerKind", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningEpisodeStatus {
    Pending,
    Selecting,
    Selected,
    Reviewing,
    Reviewed,
    NoEpisode,
    InsufficientContext,
    Failed,
}

impl LearningEpisodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Selecting => "selecting",
            Self::Selected => "selected",
            Self::Reviewing => "reviewing",
            Self::Reviewed => "reviewed",
            Self::NoEpisode => "no_episode",
            Self::InsufficientContext => "insufficient_context",
            Self::Failed => "failed",
        }
    }

    pub fn is_selector_terminal(self) -> bool {
        matches!(self, Self::NoEpisode | Self::InsufficientContext)
    }

    fn from_db(value: String) -> Result<Self, InvalidDbValue> {
        match value.as_str() {
            "pending" => Ok(Self::Pending),
            "selecting" => Ok(Self::Selecting),
            "selected" => Ok(Self::Selected),
            "reviewing" => Ok(Self::Reviewing),
            "reviewed" => Ok(Self::Reviewed),
            "no_episode" => Ok(Self::NoEpisode),
            "insufficient_context" => Ok(Self::InsufficientContext),
            "failed" => Ok(Self::Failed),
            _ => Err(InvalidDbValue::new("LearningEpisodeStatus", value)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewExecutionEvent {
    pub agent_name: String,
    pub root_session_id: Option<String>,
    pub invocation_id: Option<String>,
    pub turn_id: Option<i64>,
    pub async_run_id: Option<String>,
    pub cron_job_name: Option<String>,
    pub cron_run_id: Option<String>,
    pub seq: i64,
    pub event_kind: ExecutionEventKind,
    pub tool_name: Option<String>,
    pub content_json: serde_json::Value,
    pub content_text: String,
    pub trust_label: TrustLabel,
}

#[derive(Debug, Clone)]
pub struct NewLearningEpisodeSeed {
    pub agent_name: String,
    pub kind: LearningEpisodeKind,
    pub seed_trigger_kind: EpisodeSeedTriggerKind,
    pub seed_ref: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub ready_after: String,
}

#[derive(Debug, Clone)]
pub struct SelectedEpisodeUpdate {
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub message_refs: Vec<String>,
    pub execution_event_refs: Vec<String>,
    pub selector_model: Option<String>,
    pub selector_output_json: serde_json::Value,
    pub boundary_rationale: Option<String>,
    pub confidence: Option<String>,
    pub context_incomplete: bool,
    pub episode_hash: Option<String>,
    pub last_evidence_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LearningEpisodeRow {
    pub id: i64,
    pub agent_name: String,
    pub kind: LearningEpisodeKind,
    pub seed_trigger_kind: EpisodeSeedTriggerKind,
    pub seed_ref: String,
    pub status: LearningEpisodeStatus,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub message_refs: Vec<String>,
    pub execution_event_refs: Vec<String>,
    pub selector_model: Option<String>,
    pub selector_output_json: Option<serde_json::Value>,
    pub boundary_rationale: Option<String>,
    pub confidence: Option<String>,
    pub context_incomplete: bool,
    pub episode_hash: Option<String>,
    pub ready_after: String,
    pub last_evidence_at: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn insert_execution_event(
    conn: &right_db::Connection,
    event: &NewExecutionEvent,
) -> Result<i64, right_db::DbError> {
    let content_json = serde_json::to_string(&event.content_json)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let trust_label = if matches!(event.event_kind, ExecutionEventKind::Thinking) {
        TrustLabel::Secondary
    } else {
        event.trust_label
    };
    conn.execute(
        "INSERT INTO execution_events \
         (agent_name, root_session_id, invocation_id, turn_id, async_run_id, cron_job_name, cron_run_id, seq, event_kind, tool_name, content_json, content_text, trust_label) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            event.agent_name.as_str(),
            event.root_session_id.as_deref(),
            event.invocation_id.as_deref(),
            event.turn_id,
            event.async_run_id.as_deref(),
            event.cron_job_name.as_deref(),
            event.cron_run_id.as_deref(),
            event.seq,
            event.event_kind.as_str(),
            event.tool_name.as_deref(),
            content_json,
            event.content_text.as_str(),
            trust_label.as_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_pending_episode(
    conn: &right_db::Connection,
    seed: &NewLearningEpisodeSeed,
) -> Result<i64, right_db::DbError> {
    let tx = conn.transaction()?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO learning_episodes \
         (agent_name, kind, seed_trigger_kind, seed_ref, status, target_chat_id, target_thread_id, ready_after) \
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
        params![
            seed.agent_name.as_str(),
            seed.kind.as_str(),
            seed.seed_trigger_kind.as_str(),
            seed.seed_ref.as_str(),
            seed.target_chat_id,
            seed.target_thread_id,
            seed.ready_after.as_str(),
        ],
    )?;
    let id = if inserted == 1 {
        tx.last_insert_rowid()
    } else {
        let id = tx.query_row(
            "SELECT id FROM learning_episodes \
             WHERE agent_name=?1 AND kind=?2 AND seed_trigger_kind=?3 AND seed_ref=?4",
            params![
                seed.agent_name.as_str(),
                seed.kind.as_str(),
                seed.seed_trigger_kind.as_str(),
                seed.seed_ref.as_str(),
            ],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE learning_episodes \
             SET ready_after=?2, last_evidence_at=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
             WHERE id=?1 AND status='pending'",
            params![id, seed.ready_after.as_str()],
        )?;
        id
    };
    tx.commit()?;
    Ok(id)
}

pub fn claim_ready_episode(
    conn: &right_db::Connection,
    agent_name: &str,
    now: &str,
) -> Result<Option<LearningEpisodeRow>, right_db::DbError> {
    let tx = conn.transaction()?;
    let id = match tx.query_row(
        "SELECT id FROM learning_episodes \
             WHERE agent_name=?1 AND status='pending' AND ready_after <= ?2 \
             ORDER BY ready_after ASC, id ASC \
             LIMIT 1",
        params![agent_name, now],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(id) => Some(id),
        Err(right_db::DbError::NotFound) => None,
        Err(error) => return Err(error),
    };
    let Some(id) = id else {
        tx.commit()?;
        return Ok(None);
    };
    let updated = tx.execute(
        "UPDATE learning_episodes \
         SET status='selecting', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status='pending'",
        [id],
    )?;
    if updated != 1 {
        tx.commit()?;
        return Ok(None);
    }
    let episode = select_episode_in_tx(&tx, id)?;
    tx.commit()?;
    Ok(Some(episode))
}

pub fn mark_episode_selected(
    conn: &right_db::Connection,
    episode_id: i64,
    selection: &SelectedEpisodeUpdate,
) -> Result<(), right_db::DbError> {
    let message_refs_json = serde_json::to_string(&selection.message_refs)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let execution_event_refs_json = serde_json::to_string(&selection.execution_event_refs)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let selector_output_json = serde_json::to_string(&selection.selector_output_json)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status='selected', \
             start_ref=?2, \
             end_ref=?3, \
             message_refs_json=?4, \
             execution_event_refs_json=?5, \
             selector_model=?6, \
             selector_output_json=?7, \
             boundary_rationale=?8, \
             confidence=?9, \
             context_incomplete=?10, \
             episode_hash=?11, \
             last_evidence_at=COALESCE(?12, last_evidence_at), \
             updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status='selecting'",
        params![
            episode_id,
            selection.start_ref.as_deref(),
            selection.end_ref.as_deref(),
            message_refs_json,
            execution_event_refs_json,
            selection.selector_model.as_deref(),
            selector_output_json,
            selection.boundary_rationale.as_deref(),
            selection.confidence.as_deref(),
            if selection.context_incomplete {
                1_i64
            } else {
                0_i64
            },
            selection.episode_hash.as_deref(),
            selection.last_evidence_at.as_deref(),
        ],
    )?;
    require_one_row_updated(updated)
}

pub fn mark_episode_terminal(
    conn: &right_db::Connection,
    episode_id: i64,
    status: LearningEpisodeStatus,
    output_json: &serde_json::Value,
) -> Result<(), right_db::DbError> {
    if !status.is_selector_terminal() {
        return Err(right_db::DbError::InvalidParameter("invalid query".into()));
    }
    let output_json = serde_json::to_string(output_json)
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status=?2, selector_output_json=?3, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status IN ('pending','selecting')",
        params![episode_id, status.as_str(), output_json],
    )?;
    require_one_row_updated(updated)
}

pub fn mark_episode_failed(
    conn: &right_db::Connection,
    episode_id: i64,
    reason: &str,
) -> Result<(), right_db::DbError> {
    let output_json = serde_json::to_string(&serde_json::json!({ "error": reason }))
        .map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))?;
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status='failed', selector_output_json=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status IN ('pending','selecting','selected','reviewing')",
        params![episode_id, output_json],
    )?;
    require_one_row_updated(updated)
}

pub fn mark_episode_reviewing(
    conn: &right_db::Connection,
    episode_id: i64,
) -> Result<(), right_db::DbError> {
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status='reviewing', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status='selected'",
        [episode_id],
    )?;
    require_one_row_updated(updated)
}

pub fn mark_episode_reviewed(
    conn: &right_db::Connection,
    episode_id: i64,
) -> Result<(), right_db::DbError> {
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status='reviewed', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status='reviewing'",
        [episode_id],
    )?;
    require_one_row_updated(updated)
}

pub fn requeue_episode(
    conn: &right_db::Connection,
    episode_id: i64,
    ready_after: &str,
) -> Result<(), right_db::DbError> {
    let updated = conn.execute(
        "UPDATE learning_episodes \
         SET status='pending', ready_after=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE id=?1 AND status='selecting'",
        params![episode_id, ready_after],
    )?;
    require_one_row_updated(updated)
}

/// Only clears review_running when the gate is set with no prior review report (stranded case).
/// Concurrent legitimate reviews are not affected.
pub fn recover_stale_inflight_episodes(
    conn: &right_db::Connection,
    agent_name: &str,
    now: &str,
) -> Result<usize, right_db::DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, status FROM learning_episodes \
         WHERE agent_name=?1 AND status IN ('selecting','selected','reviewing') \
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([agent_name], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut episodes = Vec::new();
    for row in rows {
        episodes.push(row?);
    }
    drop(stmt);

    let tx = conn.transaction()?;
    let mut recovered = 0;
    for (episode_id, status) in episodes {
        match status.as_str() {
            "selecting" => {
                recovered += tx.execute(
                    "UPDATE learning_episodes \
                     SET status='pending', ready_after=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                     WHERE id=?1 AND status='selecting'",
                    params![episode_id, now],
                )?;
            }
            "selected" | "reviewing" => {
                let report_status: Option<String> = match tx.query_row(
                    "SELECT status FROM skill_review_reports \
                         WHERE learning_episode_id=?1 \
                         ORDER BY id DESC \
                         LIMIT 1",
                    [episode_id],
                    |row| row.get(0),
                ) {
                    Ok(status) => Some(status),
                    Err(right_db::DbError::NotFound) => None,
                    Err(error) => return Err(error),
                };
                let updated = match report_status.as_deref() {
                    Some("failed") => tx.execute(
                        "UPDATE learning_episodes \
                         SET status='failed', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                         WHERE id=?1 AND status IN ('selected','reviewing')",
                        [episode_id],
                    )?,
                    Some(_) => tx.execute(
                        "UPDATE learning_episodes \
                         SET status='reviewed', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                         WHERE id=?1 AND status IN ('selected','reviewing')",
                        [episode_id],
                    )?,
                    None => tx.execute(
                        "UPDATE learning_episodes \
                         SET status='pending', ready_after=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                         WHERE id=?1 AND status IN ('selected','reviewing')",
                        params![episode_id, now],
                    )?,
                };
                recovered += updated;
            }
            _ => {}
        }
    }
    if recovered > 0 {
        tx.execute(
            "UPDATE skill_nudge_state SET review_running = 0 \
             WHERE agent_name = ?1 AND review_running = 1 AND last_review_status IS NULL",
            [agent_name],
        )?;
    }
    tx.commit()?;
    Ok(recovered)
}

fn require_one_row_updated(updated: usize) -> Result<(), right_db::DbError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(right_db::DbError::NotFound)
    }
}

fn select_episode_in_tx(
    tx: &right_db::Transaction<'_>,
    episode_id: i64,
) -> Result<LearningEpisodeRow, right_db::DbError> {
    tx.query_row(
        "SELECT id, agent_name, kind, seed_trigger_kind, seed_ref, status, \
                target_chat_id, target_thread_id, start_ref, end_ref, \
                message_refs_json, execution_event_refs_json, selector_model, \
                selector_output_json, boundary_rationale, confidence, \
                context_incomplete, episode_hash, ready_after, last_evidence_at, \
                created_at, updated_at \
         FROM learning_episodes WHERE id=?1",
        [episode_id],
        learning_episode_from_row,
    )
}

fn learning_episode_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<LearningEpisodeRow, right_db::DbError> {
    let kind = LearningEpisodeKind::from_db(row.get(2)?).map_err(to_sql_conversion_error)?;
    let seed_trigger_kind =
        EpisodeSeedTriggerKind::from_db(row.get(3)?).map_err(to_sql_conversion_error)?;
    let status = LearningEpisodeStatus::from_db(row.get(5)?).map_err(to_sql_conversion_error)?;
    let message_refs_json: String = row.get(10)?;
    let execution_event_refs_json: String = row.get(11)?;
    let selector_output_json: Option<String> = row.get(13)?;
    let context_incomplete: i64 = row.get(16)?;
    Ok(LearningEpisodeRow {
        id: row.get(0)?,
        agent_name: row.get(1)?,
        kind,
        seed_trigger_kind,
        seed_ref: row.get(4)?,
        status,
        target_chat_id: row.get(6)?,
        target_thread_id: row.get(7)?,
        start_ref: row.get(8)?,
        end_ref: row.get(9)?,
        message_refs: parse_json_column(&message_refs_json)?,
        execution_event_refs: parse_json_column(&execution_event_refs_json)?,
        selector_model: row.get(12)?,
        selector_output_json: parse_optional_json_column(selector_output_json)?,
        boundary_rationale: row.get(14)?,
        confidence: row.get(15)?,
        context_incomplete: context_incomplete != 0,
        episode_hash: row.get(17)?,
        ready_after: row.get(18)?,
        last_evidence_at: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
    })
}

fn parse_json_column<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, right_db::DbError> {
    serde_json::from_str(value).map_err(|e| right_db::DbError::InvalidParameter(e.to_string()))
}

fn parse_optional_json_column(
    value: Option<String>,
) -> Result<Option<serde_json::Value>, right_db::DbError> {
    value.map(|value| parse_json_column(&value)).transpose()
}

fn to_sql_conversion_error(error: InvalidDbValue) -> right_db::DbError {
    right_db::DbError::InvalidParameter(error.to_string())
}

#[derive(Debug)]
pub struct InvalidDbValue {
    type_name: &'static str,
    value: String,
}

impl InvalidDbValue {
    fn new(type_name: &'static str, value: String) -> Self {
        Self { type_name, value }
    }
}

impl fmt::Display for InvalidDbValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} value {:?}", self.type_name, self.value)
    }
}

impl std::error::Error for InvalidDbValue {}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> right_db::Connection {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).unwrap();
        std::mem::forget(dir);
        conn
    }

    fn seed(seed_ref: &str) -> NewLearningEpisodeSeed {
        NewLearningEpisodeSeed {
            agent_name: "right".to_owned(),
            kind: LearningEpisodeKind::ForegroundThread,
            seed_trigger_kind: EpisodeSeedTriggerKind::LearningSignal,
            seed_ref: seed_ref.to_owned(),
            target_chat_id: Some(10),
            target_thread_id: Some(20),
            ready_after: "2026-05-19T00:00:00Z".to_owned(),
        }
    }

    fn selected_update() -> SelectedEpisodeUpdate {
        SelectedEpisodeUpdate {
            start_ref: Some("msg:1".to_owned()),
            end_ref: Some("msg:3".to_owned()),
            message_refs: vec!["msg:1".to_owned(), "msg:3".to_owned()],
            execution_event_refs: vec!["event:7".to_owned()],
            selector_model: Some("selector-model".to_owned()),
            selector_output_json: serde_json::json!({"selected": true}),
            boundary_rationale: Some("contains the correction".to_owned()),
            confidence: Some("high".to_owned()),
            context_incomplete: false,
            episode_hash: Some("episode-hash".to_owned()),
            last_evidence_at: Some("2026-05-19T00:00:03Z".to_owned()),
        }
    }

    fn assert_query_returned_no_rows(result: Result<(), right_db::DbError>) {
        assert!(matches!(result, Err(right_db::DbError::NotFound)));
    }

    fn assert_invalid_query(result: Result<(), right_db::DbError>) {
        assert!(matches!(
            result,
            Err(right_db::DbError::InvalidParameter(ref message)) if message == "invalid query"
        ));
    }

    #[test]
    fn execution_event_insert_round_trips_thinking_as_secondary() {
        let conn = conn();
        let id = insert_execution_event(
            &conn,
            &NewExecutionEvent {
                agent_name: "right".to_owned(),
                root_session_id: Some("session-1".to_owned()),
                invocation_id: Some("inv-1".to_owned()),
                turn_id: Some(7),
                async_run_id: None,
                cron_job_name: None,
                cron_run_id: None,
                seq: 3,
                event_kind: ExecutionEventKind::Thinking,
                tool_name: None,
                content_json: serde_json::json!({"text":"considering route"}),
                content_text: "considering route".to_owned(),
                trust_label: TrustLabel::Secondary,
            },
        )
        .unwrap();
        let row: (String, String) = conn
            .query_row(
                "SELECT event_kind, trust_label FROM execution_events WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("thinking".to_owned(), "secondary".to_owned()));
    }

    #[test]
    fn claim_ready_episode_moves_pending_to_selecting() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &seed("inv:inv-1")).unwrap();
        let claimed = claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z").unwrap();
        assert_eq!(claimed.map(|e| e.id), Some(id));
    }

    #[test]
    fn duplicate_pending_seed_extends_ready_after_and_last_evidence_at() {
        let conn = conn();
        let mut seed = seed("inv:inv-dup");
        seed.ready_after = "2026-05-19T00:01:00Z".to_owned();
        let id = insert_pending_episode(&conn, &seed).unwrap();

        seed.ready_after = "2026-05-19T00:05:00Z".to_owned();
        let duplicate_id = insert_pending_episode(&conn, &seed).unwrap();

        let row: (String, String) = conn
            .query_row(
                "SELECT ready_after, last_evidence_at FROM learning_episodes WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(duplicate_id, id);
        assert_eq!(row.0, "2026-05-19T00:05:00Z");
        assert_eq!(row.1, "2026-05-19T00:05:00Z");
    }

    #[test]
    fn mark_episode_selected_moves_selecting_to_selected() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &seed("inv:inv-2")).unwrap();
        claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z")
            .unwrap()
            .unwrap();

        mark_episode_selected(&conn, id, &selected_update()).unwrap();

        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT status, start_ref, message_refs_json, execution_event_refs_json \
                 FROM learning_episodes WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "selected");
        assert_eq!(row.1, "msg:1");
        assert_eq!(row.2, r#"["msg:1","msg:3"]"#);
        assert_eq!(row.3, r#"["event:7"]"#);
    }

    #[test]
    fn mark_episode_selected_rejects_missing_or_non_selecting_episode() {
        let conn = conn();
        let pending_id = insert_pending_episode(&conn, &seed("inv:inv-3")).unwrap();

        assert_query_returned_no_rows(mark_episode_selected(&conn, pending_id, &selected_update()));
        assert_query_returned_no_rows(mark_episode_selected(&conn, 999, &selected_update()));
    }

    #[test]
    fn mark_episode_terminal_moves_pending_and_selecting_to_terminal_status() {
        let conn = conn();
        let pending_id = insert_pending_episode(&conn, &seed("inv:inv-4")).unwrap();
        mark_episode_terminal(
            &conn,
            pending_id,
            LearningEpisodeStatus::NoEpisode,
            &serde_json::json!({"reason": "no bounded episode"}),
        )
        .unwrap();

        let selecting_id = insert_pending_episode(&conn, &seed("inv:inv-5")).unwrap();
        claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z")
            .unwrap()
            .unwrap();
        mark_episode_terminal(
            &conn,
            selecting_id,
            LearningEpisodeStatus::InsufficientContext,
            &serde_json::json!({"reason": "missing events"}),
        )
        .unwrap();

        let statuses: Vec<String> = conn
            .prepare("SELECT status FROM learning_episodes ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(statuses, vec!["no_episode", "insufficient_context"]);
    }

    #[test]
    fn mark_episode_terminal_rejects_missing_or_non_pending_selecting_episode() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &seed("inv:inv-6")).unwrap();
        claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z")
            .unwrap()
            .unwrap();
        mark_episode_selected(&conn, id, &selected_update()).unwrap();

        assert_query_returned_no_rows(mark_episode_terminal(
            &conn,
            id,
            LearningEpisodeStatus::NoEpisode,
            &serde_json::json!({"reason": "too late"}),
        ));
        assert_query_returned_no_rows(mark_episode_terminal(
            &conn,
            999,
            LearningEpisodeStatus::NoEpisode,
            &serde_json::json!({}),
        ));
    }

    #[test]
    fn mark_episode_terminal_rejects_non_terminal_destination_status() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &seed("inv:inv-7")).unwrap();

        assert_invalid_query(mark_episode_terminal(
            &conn,
            id,
            LearningEpisodeStatus::Selected,
            &serde_json::json!({"invalid": true}),
        ));

        let status: String = conn
            .query_row(
                "SELECT status FROM learning_episodes WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn mark_episode_failed_updates_in_flight_episode_and_rejects_missing_episode() {
        let conn = conn();
        let id = insert_pending_episode(&conn, &seed("inv:inv-8")).unwrap();
        claim_ready_episode(&conn, "right", "2026-05-19T00:00:01Z")
            .unwrap()
            .unwrap();
        conn.execute(
            "UPDATE learning_episodes SET status='reviewing' WHERE id=?1",
            [id],
        )
        .unwrap();

        mark_episode_failed(&conn, id, "selector crashed").unwrap();
        assert_query_returned_no_rows(mark_episode_failed(&conn, 999, "missing"));

        let row: (String, String) = conn
            .query_row(
                "SELECT status, selector_output_json FROM learning_episodes WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, r#"{"error":"selector crashed"}"#);
    }

    #[test]
    fn mark_episode_failed_does_not_overwrite_final_statuses() {
        let conn = conn();
        let reviewed_id = insert_pending_episode(&conn, &seed("inv:inv-9")).unwrap();
        let no_episode_id = insert_pending_episode(&conn, &seed("inv:inv-10")).unwrap();
        conn.execute(
            "UPDATE learning_episodes SET status='reviewed' WHERE id=?1",
            [reviewed_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE learning_episodes SET status='no_episode' WHERE id=?1",
            [no_episode_id],
        )
        .unwrap();

        assert_query_returned_no_rows(mark_episode_failed(&conn, reviewed_id, "late failure"));
        assert_query_returned_no_rows(mark_episode_failed(&conn, no_episode_id, "late failure"));

        let statuses: Vec<String> = conn
            .prepare("SELECT status FROM learning_episodes ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(statuses, vec!["reviewed", "no_episode"]);
    }
}
