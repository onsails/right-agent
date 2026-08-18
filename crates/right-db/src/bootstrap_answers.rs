use crate::{Connection, DbError};

const STAGES: [&str; 5] = ["user_name", "agent_name", "nature", "vibe", "emoji"];

type Result<T, E = DbError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapOwner {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOwnerOutcome {
    Claimed,
    AlreadyOwned(BootstrapOwner),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedAnswer {
    pub stage: &'static str,
    pub answer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCurrentAnswerOutcome {
    Recorded {
        stage: &'static str,
        next_stage: Option<&'static str>,
    },
    NotOwner {
        owner: Option<BootstrapOwner>,
    },
    QuestionNotIssued {
        stage: &'static str,
    },
    SourceMessageNotAfterQuestion {
        stage: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAnswerOutcome {
    Recorded,
    OutOfOrder { expected: Option<&'static str> },
    NotOwner { owner: Option<BootstrapOwner> },
    QuestionNotIssued,
    SourceMessageAlreadyUsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordQuestionIssueOutcome {
    Recorded,
    OutOfOrder { expected: Option<&'static str> },
    NotOwner { owner: Option<BootstrapOwner> },
}

pub async fn claim_owner(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<ClaimOwnerOutcome> {
    let tx = conn.transaction().await?;
    let inserted = tx
        .execute(
            "INSERT INTO bootstrap_interview (id, chat_id, thread_id)
             VALUES (1, ?, ?)
             ON CONFLICT (id) DO NOTHING",
            crate::params![chat_id, thread_id],
        )
        .await?;
    let claimed_owner = owner(&tx)
        .await?
        .ok_or_else(|| DbError::Constraint("bootstrap owner missing after atomic claim".into()))?;
    tx.commit().await?;

    if inserted == 1 {
        Ok(ClaimOwnerOutcome::Claimed)
    } else {
        Ok(ClaimOwnerOutcome::AlreadyOwned(claimed_owner))
    }
}

pub async fn owner(conn: &Connection) -> Result<Option<BootstrapOwner>> {
    Ok(conn
        .query_all(
            "SELECT chat_id, thread_id FROM bootstrap_interview WHERE id = 1 LIMIT 1",
            [],
            |row| {
                Ok(BootstrapOwner {
                    chat_id: row.get(0)?,
                    thread_id: row.get(1)?,
                })
            },
        )
        .await?
        .into_iter()
        .next())
}
pub async fn issued_question_stage(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<&'static str>> {
    conn.query_all(
        "SELECT stage
         FROM bootstrap_questions
         WHERE chat_id = ? AND thread_id = ?
         LIMIT 1",
        crate::params![chat_id, thread_id],
        |row| row.get::<_, String>(0),
    )
    .await?
    .into_iter()
    .next()
    .map(|stage| canonical_stage(&stage))
    .transpose()
}

pub async fn record_question_issue(
    conn: &Connection,
    stage: &str,
    chat_id: i64,
    thread_id: i64,
    assistant_message_id: i32,
) -> Result<RecordQuestionIssueOutcome> {
    let stage = validated_stage(stage)?;
    let scope = BootstrapOwner { chat_id, thread_id };
    let tx = conn.transaction().await?;
    let current_owner = owner(&tx).await?;
    if current_owner != Some(scope) {
        tx.rollback().await?;
        return Ok(RecordQuestionIssueOutcome::NotOwner {
            owner: current_owner,
        });
    }

    let recorded = recorded_stages(&tx, chat_id, thread_id).await?;
    let expected = first_missing_from_recorded(&recorded);
    if expected != Some(stage) {
        tx.rollback().await?;
        return Ok(RecordQuestionIssueOutcome::OutOfOrder { expected });
    }

    tx.execute(
        "INSERT INTO bootstrap_questions (
            chat_id, thread_id, stage, assistant_message_id, issued_at
         ) VALUES (?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT (chat_id, thread_id, stage) DO UPDATE SET
            assistant_message_id = excluded.assistant_message_id,
            issued_at = excluded.issued_at",
        crate::params![chat_id, thread_id, stage, assistant_message_id],
    )
    .await?;
    tx.commit().await?;
    Ok(RecordQuestionIssueOutcome::Recorded)
}

pub async fn record_current_answer(
    conn: &Connection,
    answer: &str,
    chat_id: i64,
    thread_id: i64,
    source_message_id: i32,
) -> Result<RecordCurrentAnswerOutcome> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Err(DbError::InvalidParameter(
            "bootstrap answer must not be empty".into(),
        ));
    }

    let scope = BootstrapOwner { chat_id, thread_id };
    let tx = conn.transaction().await?;
    let current_owner = owner(&tx).await?;
    if current_owner != Some(scope) {
        tx.rollback().await?;
        return Ok(RecordCurrentAnswerOutcome::NotOwner {
            owner: current_owner,
        });
    }

    let recorded = recorded_stages(&tx, chat_id, thread_id).await?;
    let Some(stage) = first_missing_from_recorded(&recorded) else {
        tx.rollback().await?;
        return Ok(RecordCurrentAnswerOutcome::QuestionNotIssued { stage: "emoji" });
    };
    let issued_assistant_message_id = tx
        .query_all(
            "SELECT assistant_message_id
             FROM bootstrap_questions
             WHERE chat_id = ? AND thread_id = ? AND stage = ?
             LIMIT 1",
            crate::params![chat_id, thread_id, stage],
            |row| row.get::<_, i32>(0),
        )
        .await?
        .into_iter()
        .next();
    let Some(issued_assistant_message_id) = issued_assistant_message_id else {
        tx.rollback().await?;
        return Ok(RecordCurrentAnswerOutcome::QuestionNotIssued { stage });
    };
    if source_message_id <= issued_assistant_message_id {
        tx.rollback().await?;
        return Ok(RecordCurrentAnswerOutcome::SourceMessageNotAfterQuestion { stage });
    }

    tx.execute(
        "INSERT INTO bootstrap_answers (
            chat_id, thread_id, stage, answer, source_message_id, recorded_at
         ) VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        crate::params![chat_id, thread_id, stage, answer, source_message_id],
    )
    .await?;
    tx.execute(
        "DELETE FROM bootstrap_questions
         WHERE chat_id = ? AND thread_id = ? AND stage = ?",
        crate::params![chat_id, thread_id, stage],
    )
    .await?;
    let mut recorded = recorded;
    recorded.push(stage.to_owned());
    let next_stage = first_missing_from_recorded(&recorded);
    tx.commit().await?;
    Ok(RecordCurrentAnswerOutcome::Recorded { stage, next_stage })
}

pub async fn record_answer(
    conn: &Connection,
    stage: &str,
    answer: &str,
    chat_id: i64,
    thread_id: i64,
    source_message_id: i32,
) -> Result<RecordAnswerOutcome> {
    let stage = validated_stage(stage)?;
    let expected = first_missing_stage(conn, chat_id, thread_id).await?;
    if expected != Some(stage) {
        return Ok(RecordAnswerOutcome::OutOfOrder { expected });
    }

    match record_current_answer(conn, answer, chat_id, thread_id, source_message_id).await {
        Ok(RecordCurrentAnswerOutcome::Recorded { .. }) => Ok(RecordAnswerOutcome::Recorded),
        Ok(RecordCurrentAnswerOutcome::NotOwner { owner }) => {
            Ok(RecordAnswerOutcome::NotOwner { owner })
        }
        Ok(RecordCurrentAnswerOutcome::QuestionNotIssued { .. })
        | Ok(RecordCurrentAnswerOutcome::SourceMessageNotAfterQuestion { .. }) => {
            Ok(RecordAnswerOutcome::QuestionNotIssued)
        }
        Err(error) if error.is_constraint_violation() => {
            Ok(RecordAnswerOutcome::SourceMessageAlreadyUsed)
        }
        Err(error) => Err(error),
    }
}

pub async fn recorded_answers(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<RecordedAnswer>> {
    let rows = conn
        .query_all(
            "SELECT stage, answer
             FROM bootstrap_answers
             WHERE chat_id = ? AND thread_id = ?
             ORDER BY CASE stage
                 WHEN 'user_name' THEN 0
                 WHEN 'agent_name' THEN 1
                 WHEN 'nature' THEN 2
                 WHEN 'vibe' THEN 3
                 WHEN 'emoji' THEN 4
             END",
            crate::params![chat_id, thread_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .await?;
    rows.into_iter()
        .map(|(stage, answer)| {
            Ok(RecordedAnswer {
                stage: canonical_stage(&stage)?,
                answer,
            })
        })
        .collect()
}

pub async fn missing_stages(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<&'static str>> {
    let recorded = recorded_stages(conn, chat_id, thread_id).await?;
    Ok(STAGES
        .into_iter()
        .filter(|stage| !recorded.iter().any(|recorded| recorded == stage))
        .collect())
}

pub async fn first_missing_stage(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<&'static str>> {
    let recorded = recorded_stages(conn, chat_id, thread_id).await?;
    Ok(first_missing_from_recorded(&recorded))
}

async fn recorded_stages(conn: &Connection, chat_id: i64, thread_id: i64) -> Result<Vec<String>> {
    conn.query_all(
        "SELECT stage FROM bootstrap_answers WHERE chat_id = ? AND thread_id = ?",
        crate::params![chat_id, thread_id],
        |row| row.get(0),
    )
    .await
}

fn validated_stage(stage: &str) -> Result<&'static str> {
    canonical_stage(stage.trim())
}

fn canonical_stage(stage: &str) -> Result<&'static str> {
    STAGES
        .into_iter()
        .find(|candidate| *candidate == stage)
        .ok_or_else(|| DbError::InvalidParameter(format!("unknown bootstrap stage: {stage}")))
}

fn first_missing_from_recorded(recorded: &[String]) -> Option<&'static str> {
    STAGES
        .into_iter()
        .find(|stage| !recorded.iter().any(|recorded| recorded == stage))
}

pub async fn clear(conn: &Connection) -> Result<usize> {
    let tx = conn.transaction().await?;
    tx.execute("DELETE FROM bootstrap_questions", []).await?;
    let answers = tx.execute("DELETE FROM bootstrap_answers", []).await?;
    tx.execute("DELETE FROM bootstrap_interview", []).await?;
    tx.commit().await?;
    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn claim(conn: &Connection, chat_id: i64, thread_id: i64) {
        assert_eq!(
            claim_owner(conn, chat_id, thread_id).await.unwrap(),
            ClaimOwnerOutcome::Claimed
        );
    }

    #[tokio::test]
    async fn first_scope_atomically_claims_the_single_global_owner() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();

        claim(&conn, 10, 3).await;
        assert_eq!(
            owner(&conn).await.unwrap(),
            Some(BootstrapOwner {
                chat_id: 10,
                thread_id: 3
            })
        );
        assert_eq!(
            claim_owner(&conn, 10, 3).await.unwrap(),
            ClaimOwnerOutcome::AlreadyOwned(BootstrapOwner {
                chat_id: 10,
                thread_id: 3
            })
        );
        assert_eq!(
            claim_owner(&conn, 11, 4).await.unwrap(),
            ClaimOwnerOutcome::AlreadyOwned(BootstrapOwner {
                chat_id: 10,
                thread_id: 3
            })
        );
    }

    #[tokio::test]
    async fn non_owner_cannot_issue_or_record_questions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;
        let claimed = Some(BootstrapOwner {
            chat_id: 10,
            thread_id: 3,
        });

        assert_eq!(
            record_question_issue(&conn, "user_name", 11, 4, 100)
                .await
                .unwrap(),
            RecordQuestionIssueOutcome::NotOwner { owner: claimed }
        );
        assert_eq!(
            record_current_answer(&conn, "Grace", 11, 4, 101)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::NotOwner { owner: claimed }
        );
    }

    #[tokio::test]
    async fn current_message_records_without_conversation_archive_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;

        assert_eq!(
            record_current_answer(&conn, "Ada", 10, 3, 101)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::QuestionNotIssued { stage: "user_name" }
        );
        assert_eq!(
            record_question_issue(&conn, "user_name", 10, 3, 102)
                .await
                .unwrap(),
            RecordQuestionIssueOutcome::Recorded
        );
        assert_eq!(
            record_current_answer(&conn, "Ada", 10, 3, 102)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::SourceMessageNotAfterQuestion { stage: "user_name" }
        );
        assert_eq!(
            record_current_answer(&conn, "  Ada Lovelace  ", 10, 3, 103)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::Recorded {
                stage: "user_name",
                next_stage: Some("agent_name")
            }
        );
        assert_eq!(
            recorded_answers(&conn, 10, 3).await.unwrap(),
            vec![RecordedAnswer {
                stage: "user_name",
                answer: "Ada Lovelace".to_owned()
            }]
        );
        assert_eq!(
            record_current_answer(&conn, "replay", 10, 3, 104)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::QuestionNotIssued {
                stage: "agent_name"
            }
        );
    }
    #[tokio::test]
    async fn issued_question_lookup_is_scoped_and_cleared_when_answered() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;

        assert_eq!(issued_question_stage(&conn, 10, 3).await.unwrap(), None);
        assert_eq!(
            record_question_issue(&conn, "user_name", 10, 3, 100)
                .await
                .unwrap(),
            RecordQuestionIssueOutcome::Recorded
        );
        assert_eq!(
            issued_question_stage(&conn, 10, 3).await.unwrap(),
            Some("user_name")
        );
        assert_eq!(issued_question_stage(&conn, 10, 4).await.unwrap(), None);

        assert!(matches!(
            record_current_answer(&conn, "Ada", 10, 3, 101)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::Recorded { .. }
        ));
        assert_eq!(issued_question_stage(&conn, 10, 3).await.unwrap(), None);
    }

    #[tokio::test]
    async fn answers_are_consumed_and_returned_in_canonical_stage_order() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;
        let answers = [
            ("user_name", "Ada"),
            ("agent_name", "Claw"),
            ("nature", "daemon"),
            ("vibe", "warm"),
            ("emoji", "🦀"),
        ];

        for (index, (stage, answer)) in answers.into_iter().enumerate() {
            let assistant_message_id = i32::try_from(index * 2 + 100).unwrap();
            let next_stage = STAGES.get(index + 1).copied();
            assert_eq!(
                record_question_issue(&conn, stage, 10, 3, assistant_message_id)
                    .await
                    .unwrap(),
                RecordQuestionIssueOutcome::Recorded
            );
            assert_eq!(
                record_current_answer(&conn, answer, 10, 3, assistant_message_id + 1)
                    .await
                    .unwrap(),
                RecordCurrentAnswerOutcome::Recorded { stage, next_stage }
            );
        }

        assert_eq!(
            recorded_answers(&conn, 10, 3).await.unwrap(),
            answers
                .into_iter()
                .map(|(stage, answer)| RecordedAnswer {
                    stage,
                    answer: answer.to_owned(),
                })
                .collect::<Vec<_>>()
        );
        assert_eq!(first_missing_stage(&conn, 10, 3).await.unwrap(), None);
    }

    #[tokio::test]
    async fn question_issue_enforces_first_missing_order() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;

        assert_eq!(
            record_question_issue(&conn, "nature", 10, 3, 100)
                .await
                .unwrap(),
            RecordQuestionIssueOutcome::OutOfOrder {
                expected: Some("user_name")
            }
        );
    }

    #[tokio::test]
    async fn clear_removes_owner_answers_and_outstanding_questions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::open_connection(dir.path(), true).await.unwrap();
        claim(&conn, 10, 3).await;
        record_question_issue(&conn, "user_name", 10, 3, 100)
            .await
            .unwrap();
        assert!(matches!(
            record_current_answer(&conn, "Ada", 10, 3, 101)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::Recorded { .. }
        ));
        record_question_issue(&conn, "agent_name", 10, 3, 102)
            .await
            .unwrap();

        assert_eq!(clear(&conn).await.unwrap(), 1);
        assert_eq!(owner(&conn).await.unwrap(), None);
        assert_eq!(missing_stages(&conn, 10, 3).await.unwrap(), STAGES);
        assert_eq!(
            record_current_answer(&conn, "Claw", 10, 3, 103)
                .await
                .unwrap(),
            RecordCurrentAnswerOutcome::NotOwner { owner: None }
        );
        claim(&conn, 11, 4).await;
    }
}
