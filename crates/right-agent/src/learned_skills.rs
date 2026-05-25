use right_db::{Connection, DbError, params};

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
    pub hint_outcome: Option<String>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub event_refs: Vec<String>,
}

pub async fn insert_learning_event(
    conn: &Connection,
    event: &LearningEvent,
) -> Result<(), DbError> {
    let event_refs_json = serde_json::to_string(&event.event_refs)
        .map_err(|e| DbError::InvalidParameter(e.to_string()))?;
    conn.execute(
        "INSERT INTO skill_learning_events \
         (invocation_id, agent_name, action, skill_name, phase, status, hint_outcome, reason, message, summary, event_refs_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event.invocation_id.as_str(),
            event.agent_name.as_str(),
            event.action.as_str(),
            event.skill_name.as_str(),
            event.phase.as_str(),
            event.status.map(LearningStatus::as_str),
            event.hint_outcome.as_deref(),
            event.reason.as_deref(),
            event.message.as_deref(),
            event.summary.as_deref(),
            event_refs_json,
        ],
    )
    .await?;
    Ok(())
}

pub async fn successful_finish_exists(
    conn: &Connection,
    invocation_id: &str,
) -> Result<bool, DbError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events \
         WHERE invocation_id=?1 AND phase='finish' AND status IN ('created','updated')",
            [invocation_id],
            |r| r.get(0),
        )
        .await?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn conn() -> (tempfile::TempDir, Connection) {
        right_db::test_support::migrated_connection().await
    }

    fn learning_event(
        invocation_id: &str,
        phase: LearningPhase,
        status: Option<LearningStatus>,
    ) -> LearningEvent {
        LearningEvent {
            invocation_id: invocation_id.to_owned(),
            agent_name: "right".to_owned(),
            action: LearningAction::Create,
            skill_name: "rightx-demo".to_owned(),
            phase,
            status,
            hint_outcome: None,
            reason: None,
            message: None,
            summary: None,
            event_refs: vec![],
        }
    }

    #[tokio::test]
    async fn successful_finish_exists_only_for_created_or_updated() {
        let (_dir, conn) = conn().await;
        insert_learning_event(
            &conn,
            &LearningEvent {
                summary: Some("write failed".to_owned()),
                ..learning_event("inv-1", LearningPhase::Finish, Some(LearningStatus::Failed))
            },
        )
        .await
        .unwrap();
        assert!(!successful_finish_exists(&conn, "inv-1").await.unwrap());

        insert_learning_event(
            &conn,
            &LearningEvent {
                status: Some(LearningStatus::Created),
                message: Some("Learned skill: rightx-demo".to_owned()),
                summary: Some("captured workflow".to_owned()),
                event_refs: vec!["e1".to_owned(), "e2".to_owned()],
                ..learning_event("inv-1", LearningPhase::Finish, None)
            },
        )
        .await
        .unwrap();
        assert!(successful_finish_exists(&conn, "inv-1").await.unwrap());
    }

    #[tokio::test]
    async fn insert_learning_event_persists_hint_outcome() {
        let (_dir, conn) = conn().await;
        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-hint".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Create,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Aborted),
                hint_outcome: Some("refused".to_owned()),
                reason: None,
                message: Some("Refused to create skill.".to_owned()),
                summary: Some("insufficient evidence".to_owned()),
                event_refs: vec![],
            },
        )
        .await
        .unwrap();

        let hint_outcome: Option<String> = conn
            .query_row(
                "SELECT hint_outcome FROM skill_learning_events WHERE invocation_id = 'inv-hint'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(hint_outcome.as_deref(), Some("refused"));
    }

    #[tokio::test]
    async fn insert_learning_event_does_not_require_legacy_nudge_state() {
        let (_dir, conn) = conn().await;
        let legacy_table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_nudge_state'",
                [],
                |row| row.get(0),
            )
            .await
            .unwrap();
        assert_eq!(legacy_table_count, 0);

        insert_learning_event(
            &conn,
            &LearningEvent {
                invocation_id: "inv-no-nudge-state".to_owned(),
                agent_name: "right".to_owned(),
                action: LearningAction::Update,
                skill_name: "rightx-demo".to_owned(),
                phase: LearningPhase::Finish,
                status: Some(LearningStatus::Updated),
                hint_outcome: None,
                reason: None,
                message: Some("Updated skill: rightx-demo".to_owned()),
                summary: Some("captured improved workflow".to_owned()),
                event_refs: vec!["event-1".to_owned()],
            },
        )
        .await
        .unwrap();

        let row: (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), event_refs_json FROM skill_learning_events WHERE invocation_id = 'inv-no-nudge-state'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(row, (1, r#"["event-1"]"#.to_owned()));
    }
}
