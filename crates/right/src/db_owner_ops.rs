//! Grouped owner-scoped implementations for typed internal database IPC.
//!
//! Every operation is invoked by [`AgentDbOwner`] while its per-agent mutex is
//! held. This module is deliberately private to `db_owner`: no database
//! connection escapes the owner boundary.

use std::time::Duration;

use chrono::{DateTime, Utc};
use right_db::OptionalExtension as _;
use right_mcp::internal_db as wire;
use secrecy::ExposeSecret as _;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};

use super::{
    AgentDbOwner, DbOwnerError, InteractionStateOp, LearningMemoryOp, OwnerResponse, RunLedgerOp,
    SecretsRegistryOp,
};

const ERROR_DETAIL_TTL_SECS: i64 = 7 * 86_400;
const ALERT_DEDUP_HOURS: i64 = 24;

fn domain(error: impl std::fmt::Display) -> DbOwnerError {
    DbOwnerError::Domain(error.to_string())
}

fn invalid(error: impl std::fmt::Display) -> DbOwnerError {
    DbOwnerError::Invalid(error.to_string())
}

fn json<T: Serialize>(value: T) -> Result<serde_json::Value, DbOwnerError> {
    serde_json::to_value(value).map_err(domain)
}

async fn idempotent_response<T: DeserializeOwned>(
    conn: &right_db::Connection,
    request_id: &str,
    operation: &str,
) -> Result<Option<T>, DbOwnerError> {
    let row = conn
        .query_row(
            "SELECT operation, response_json FROM internal_ipc_requests WHERE request_id = ?1",
            [request_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .await
        .optional()?;
    let Some((recorded_operation, response)) = row else {
        return Ok(None);
    };
    if recorded_operation != operation {
        return Err(DbOwnerError::Conflict(format!(
            "request id was already used for {recorded_operation}"
        )));
    }
    serde_json::from_str(&response).map(Some).map_err(domain)
}

async fn record_idempotent<T: Serialize>(
    conn: &right_db::Connection,
    request_id: &str,
    operation: &str,
    response: &T,
) -> Result<(), DbOwnerError> {
    let response = serde_json::to_string(response).map_err(domain)?;
    conn.execute(
        "INSERT INTO internal_ipc_requests(request_id, operation, response_json) VALUES (?1, ?2, ?3)",
        right_db::params![request_id, operation, response],
    )
    .await?;
    Ok(())
}

fn retain_enqueue_operation(item: &wire::RetainEnqueueItemDto) -> Result<String, DbOwnerError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let payload = serde_json::to_vec(item).map_err(domain)?;
    let digest = Sha256::digest(payload);
    let mut operation = String::with_capacity("retain_enqueue:".len() + digest.len() * 2);
    operation.push_str("retain_enqueue:");
    for byte in digest {
        operation.push(char::from(HEX[usize::from(byte >> 4)]));
        operation.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(operation)
}

fn session_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<wire::SessionRowDto, right_db::DbError> {
    Ok(wire::SessionRowDto {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        thread_id: row.get(2)?,
        root_session_id: row.get(3)?,
        label: row.get(4)?,
        is_active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        last_used_at: row.get(7)?,
    })
}

fn bootstrap_owner(owner: right_db::bootstrap_answers::BootstrapOwner) -> wire::BootstrapOwnerDto {
    wire::BootstrapOwnerDto {
        chat_id: owner.chat_id,
        thread_id: owner.thread_id,
    }
}

impl AgentDbOwner {
    pub(crate) async fn interaction_state(
        &self,
        operation: InteractionStateOp,
    ) -> Result<OwnerResponse, DbOwnerError> {
        self.with_connection(move |conn| {
            Box::pin(async move {
                let value = match operation {
                    InteractionStateOp::ArchiveMessage(request) => {
                        let role = match request.message.role.as_str() {
                            "user" => right_db::conversation::ConversationRole::User,
                            "assistant" => right_db::conversation::ConversationRole::Assistant,
                            other => return Err(invalid(format!("invalid conversation role {other:?}"))),
                        };
                        if let Some(response) = idempotent_response::<wire::ArchiveMessageResponse>(
                            conn,
                            &request.request_id,
                            "archive_message",
                        )
                        .await?
                        {
                            json(response)?
                        } else {
                            let m = &request.message;
                            let existed = if let Some(message_id) = m.message_id {
                                conn.query_row(
                                    "SELECT id FROM conversation_messages WHERE platform = ?1 AND chat_id = ?2 AND message_id = ?3 AND role = ?4 LIMIT 1",
                                    right_db::params![m.platform.as_str(), m.chat_id, message_id, m.role.as_str()],
                                    |row| row.get::<_, i64>(0),
                                )
                                .await
                                .optional()?
                                .is_some()
                            } else {
                                false
                            };
                            let tx = conn.transaction().await?;
                            let id = right_db::conversation::archive_message(
                                &tx,
                                right_db::conversation::ConversationMessage {
                                    platform: &m.platform,
                                    chat_id: m.chat_id,
                                    thread_id: m.thread_id,
                                    message_id: m.message_id,
                                    sender_user_id: m.sender_user_id,
                                    sender_name: m.sender_name.as_deref(),
                                    addressed_to_bot: m.addressed_to_bot,
                                    routed_to_agent: m.routed_to_agent,
                                    root_session_id: m.root_session_id.as_deref(),
                                    turn_id: m.turn_id,
                                    role,
                                    content: &m.content,
                                },
                            )
                            .await?;
                            let response = wire::ArchiveMessageResponse {
                                id,
                                inserted: !existed,
                            };
                            record_idempotent(&tx, &request.request_id, "archive_message", &response)
                                .await?;
                            tx.commit().await?;
                            json(response)?
                        }
                    }
                    InteractionStateOp::MarkMessageRouted(r) => {
                        right_db::conversation::mark_routed(
                            conn,
                            &r.platform,
                            r.chat_id,
                            r.thread_id,
                            r.message_id,
                            &r.root_session_id,
                            r.turn_id,
                        )
                        .await?;
                        json(wire::OkResponse {})?
                    }
                    InteractionStateOp::GetActiveSession(r) => {
                        let session = conn.query_row(
                            "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1 LIMIT 1",
                            right_db::params![r.chat_id, r.thread_id], session_from_row,
                        ).await.optional()?;
                        json(wire::GetActiveSessionResponse { session })?
                    }
                    InteractionStateOp::CreateSession(r) => {
                        conn.execute(
                            "INSERT INTO sessions (chat_id, thread_id, root_session_id, label, is_active) VALUES (?1, ?2, ?3, ?4, 1)",
                            right_db::params![r.chat_id, r.thread_id, r.session_uuid, r.label],
                        ).await?;
                        json(wire::CreateSessionResponse { session_id: conn.last_insert_rowid() })?
                    }
                    InteractionStateOp::DeactivateCurrentSession(r) => {
                        let previous_root_session_id = conn.query_row(
                            "SELECT root_session_id FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1 LIMIT 1",
                            right_db::params![r.chat_id, r.thread_id], |row| row.get::<_, String>(0),
                        ).await.optional()?;
                        conn.execute(
                            "UPDATE sessions SET is_active = 0 WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1",
                            right_db::params![r.chat_id, r.thread_id],
                        ).await?;
                        json(wire::DeactivateCurrentSessionResponse { previous_root_session_id })?
                    }
                    InteractionStateOp::ActivateSession(r) => {
                        let tx = conn.transaction().await?;
                        let target = tx.query_row(
                            "SELECT chat_id, thread_id FROM sessions WHERE id = ?1",
                            [r.session_id],
                            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                        ).await.optional()?;
                        let Some((chat_id, thread_id)) = target else { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); };
                        tx.execute("UPDATE sessions SET is_active = 0 WHERE chat_id = ?1 AND thread_id = ?2 AND is_active = 1", right_db::params![chat_id, thread_id]).await?;
                        let changed = tx.execute("UPDATE sessions SET is_active = 1, last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1", [r.session_id]).await?;
                        if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                        tx.commit().await?;
                        json(wire::OkResponse {})?
                    }
                    InteractionStateOp::TouchSession(r) => {
                        let changed = conn.execute("UPDATE sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1", [r.session_id]).await?;
                        if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                        json(wire::OkResponse {})?
                    }
                    InteractionStateOp::ListSessions(r) => {
                        let sessions = conn.query_all(
                            "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 ORDER BY last_used_at DESC",
                            right_db::params![r.chat_id, r.thread_id], session_from_row,
                        ).await?;
                        json(wire::ListSessionsResponse { sessions })?
                    }
                    InteractionStateOp::FindSessionsByUuid(r) => {
                        let pattern = format!("%{}%", r.uuid_prefix);
                        let sessions = conn.query_all(
                            "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND (root_session_id LIKE ?3 OR label LIKE ?3)",
                            right_db::params![r.chat_id, r.thread_id, pattern], session_from_row,
                        ).await?;
                        json(wire::ListSessionsResponse { sessions })?
                    }
                    InteractionStateOp::FindSessionByRoot(r) => {
                        let session = conn.query_row(
                            "SELECT id, chat_id, thread_id, root_session_id, label, is_active, created_at, last_used_at FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND root_session_id = ?3 ORDER BY id DESC LIMIT 1",
                            right_db::params![r.chat_id, r.thread_id, r.root_session_id], session_from_row,
                        ).await.optional()?;
                        json(wire::FindSessionByRootResponse { session })?
                    }
                    InteractionStateOp::LatestAssistantIsUniqueExact(r) => json(wire::BoolResultResponse {
                        result: right_db::conversation::latest_assistant_is_unique_exact(conn, &r.root_session_id, &r.target).await?,
                    })?,
                    InteractionStateOp::IsRecentRoutedTarget(r) => json(wire::BoolResultResponse {
                        result: right_db::conversation::is_recent_routed_target(conn, right_db::conversation::RecentRoutedTargetQuery {
                            platform: &r.platform,
                            chat_id: r.chat_id,
                            thread_id: r.thread_id,
                            message_id: r.message_id,
                            root_session_id: &r.root_session_id,
                            window: r.window_secs,
                            current_turn_id: r.current_turn_id,
                        }).await?,
                    })?,
                    InteractionStateOp::FetchMessagesByIds(r) => {
                        let messages = right_db::conversation::fetch_by_ids(conn, &r.platform, r.chat_id, r.thread_id, &r.message_ids).await?
                            .into_iter().map(|m| wire::FetchedMessageDto { message_id: m.message_id, sender_name: m.sender_name, text: m.text, role: m.role }).collect();
                        json(wire::FetchMessagesByIdsResponse { messages })?
                    }
                    InteractionStateOp::ConversationLatestTurnId(r) => json(wire::ConversationLatestTurnIdResponse {
                        turn_id: right_db::conversation::latest_turn_id(conn, &r.root_session_id).await?,
                    })?,
                    InteractionStateOp::ThreadFocusGet(r) => {
                        let focus = right_db::thread_focus::get(conn, r.chat_id, r.thread_id).await?.map(|f| wire::ThreadFocusDto {
                            operator_focus: f.operator_focus, agent_focus: f.agent_focus, updated_at: f.updated_at,
                        });
                        json(wire::ThreadFocusGetResponse { focus })?
                    }
                    InteractionStateOp::ThreadFocusSetOperator(r) => {
                        right_db::thread_focus::set_operator(conn, r.chat_id, r.thread_id, r.value.as_deref()).await?;
                        json(wire::OkResponse {})?
                    }
                    InteractionStateOp::ErrorDetailInsert(r) => {
                        if let Some(response) = idempotent_response::<wire::ErrorDetailInsertResponse>(conn, &r.request_id, "error_detail_insert").await? {
                            json(response)?
                        } else {
                            let tx = conn.transaction().await?;
                            tx.execute("DELETE FROM error_details WHERE created_at < ?1", [r.created_at_unix - ERROR_DETAIL_TTL_SECS]).await?;
                            tx.execute("INSERT INTO error_details(chat_id, thread_id, raw_json, created_at) VALUES (?1, ?2, ?3, ?4)", right_db::params![r.chat_id, r.thread_id, r.raw_json, r.created_at_unix]).await?;
                            let id = tx.query_one("SELECT last_insert_rowid()", (), |row| row.get::<_, i64>(0)).await?;
                            let response = wire::ErrorDetailInsertResponse { id };
                            record_idempotent(&tx, &r.request_id, "error_detail_insert", &response).await?;
                            tx.commit().await?;
                            json(response)?
                        }
                    }
                    InteractionStateOp::ErrorDetailGet(r) => json(wire::ErrorDetailGetResponse {
                        raw_json: conn.query_row("SELECT raw_json FROM error_details WHERE id = ?1 AND chat_id = ?2", right_db::params![r.id, r.chat_id], |row| row.get::<_, String>(0)).await.optional()?,
                    })?,
                    InteractionStateOp::LifecycleBumpUseMany(r) => {
                        if let Some(response) = idempotent_response::<wire::LifecycleBumpUseManyResponse>(conn, &r.request_id, "lifecycle_bump_use_many").await? {
                            json(response)?
                        } else {
                            let now = DateTime::parse_from_rfc3339(&r.now_utc).map_err(invalid)?.with_timezone(&Utc);
                            let bumped = r.skill_names.len();
                            let tx = conn.transaction().await?;
                            right_lifecycle::bump_use_many_in_tx(&tx, &r.skill_names, now).await.map_err(domain)?;
                            let response = wire::LifecycleBumpUseManyResponse { bumped };
                            record_idempotent(&tx, &r.request_id, "lifecycle_bump_use_many", &response).await?;
                            tx.commit().await?;
                            json(response)?
                        }
                    }
                    InteractionStateOp::BootstrapOwner(_) => json(wire::BootstrapOwnerResponse {
                        owner: right_db::bootstrap_answers::owner(conn).await?.map(bootstrap_owner),
                    })?,
                    InteractionStateOp::BootstrapClaimOwner(r) => {
                        let outcome = match right_db::bootstrap_answers::claim_owner(conn, r.chat_id, r.thread_id).await? {
                            right_db::bootstrap_answers::ClaimOwnerOutcome::Claimed => wire::ClaimOwnerOutcomeDto { claimed: true, owner: None },
                            right_db::bootstrap_answers::ClaimOwnerOutcome::AlreadyOwned(owner) => wire::ClaimOwnerOutcomeDto { claimed: false, owner: Some(bootstrap_owner(owner)) },
                        };
                        json(wire::BootstrapClaimOwnerResponse { outcome })?
                    }
                    InteractionStateOp::BootstrapMissingStages(r) => json(wire::BootstrapMissingStagesResponse {
                        stages: right_db::bootstrap_answers::missing_stages(conn, r.chat_id, r.thread_id).await?.into_iter().map(str::to_owned).collect(),
                    })?,
                    InteractionStateOp::BootstrapFirstMissingStage(r) => json(wire::BootstrapStageResponse {
                        stage: right_db::bootstrap_answers::first_missing_stage(conn, r.chat_id, r.thread_id).await?.map(str::to_owned),
                    })?,
                    InteractionStateOp::BootstrapIssuedQuestionStage(r) => json(wire::BootstrapStageResponse {
                        stage: right_db::bootstrap_answers::issued_question_stage(conn, r.chat_id, r.thread_id).await?.map(str::to_owned),
                    })?,
                    InteractionStateOp::BootstrapRecordQuestionIssue(r) => {
                        let outcome = match right_db::bootstrap_answers::record_question_issue(conn, &r.stage, r.chat_id, r.thread_id, r.assistant_message_id).await? {
                            right_db::bootstrap_answers::RecordQuestionIssueOutcome::Recorded => wire::RecordQuestionIssueOutcomeDto::Recorded,
                            right_db::bootstrap_answers::RecordQuestionIssueOutcome::OutOfOrder { expected } => wire::RecordQuestionIssueOutcomeDto::OutOfOrder { expected: expected.map(str::to_owned) },
                            right_db::bootstrap_answers::RecordQuestionIssueOutcome::NotOwner { owner } => wire::RecordQuestionIssueOutcomeDto::NotOwner { owner: owner.map(bootstrap_owner) },
                        };
                        json(wire::BootstrapRecordQuestionIssueResponse { outcome })?
                    }
                    InteractionStateOp::BootstrapRecordCurrentAnswer(r) => {
                        let outcome = match right_db::bootstrap_answers::record_current_answer(conn, &r.answer, r.chat_id, r.thread_id, r.source_message_id).await? {
                            right_db::bootstrap_answers::RecordCurrentAnswerOutcome::Recorded { stage, next_stage } => wire::RecordCurrentAnswerOutcomeDto::Recorded { stage: stage.to_owned(), next_stage: next_stage.map(str::to_owned) },
                            right_db::bootstrap_answers::RecordCurrentAnswerOutcome::NotOwner { owner } => wire::RecordCurrentAnswerOutcomeDto::NotOwner { owner: owner.map(bootstrap_owner) },
                            right_db::bootstrap_answers::RecordCurrentAnswerOutcome::QuestionNotIssued { stage } => wire::RecordCurrentAnswerOutcomeDto::QuestionNotIssued { stage: stage.to_owned() },
                            right_db::bootstrap_answers::RecordCurrentAnswerOutcome::SourceMessageNotAfterQuestion { stage } => wire::RecordCurrentAnswerOutcomeDto::SourceMessageNotAfterQuestion { stage: stage.to_owned() },
                        };
                        json(wire::BootstrapRecordCurrentAnswerResponse { outcome })?
                    }
                    InteractionStateOp::BootstrapRecordAnswer(r) => {
                        let outcome = match right_db::bootstrap_answers::record_answer(conn, &r.stage, &r.answer, r.chat_id, r.thread_id, r.source_message_id).await? {
                            right_db::bootstrap_answers::RecordAnswerOutcome::Recorded => wire::RecordAnswerOutcomeDto::Recorded,
                            right_db::bootstrap_answers::RecordAnswerOutcome::OutOfOrder { expected } => wire::RecordAnswerOutcomeDto::OutOfOrder { expected: expected.map(str::to_owned) },
                            right_db::bootstrap_answers::RecordAnswerOutcome::NotOwner { owner } => wire::RecordAnswerOutcomeDto::NotOwner { owner: owner.map(bootstrap_owner) },
                            right_db::bootstrap_answers::RecordAnswerOutcome::QuestionNotIssued => wire::RecordAnswerOutcomeDto::QuestionNotIssued,
                            right_db::bootstrap_answers::RecordAnswerOutcome::SourceMessageAlreadyUsed => wire::RecordAnswerOutcomeDto::SourceMessageAlreadyUsed,
                        };
                        json(wire::BootstrapRecordAnswerResponse { outcome })?
                    }
                    InteractionStateOp::BootstrapRecordedAnswers(r) => json(wire::BootstrapRecordedAnswersResponse {
                        answers: right_db::bootstrap_answers::recorded_answers(conn, r.chat_id, r.thread_id).await?.into_iter().map(|a| wire::RecordedAnswerDto { stage: a.stage.to_owned(), answer: a.answer }).collect(),
                    })?,
                    InteractionStateOp::BootstrapClear(_) => json(wire::BootstrapClearResponse {
                        cleared: right_db::bootstrap_answers::clear(conn).await?,
                    })?,
                };
                Ok(OwnerResponse::Interaction(value))
            })
        }).await
    }
}

fn cron_spec_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<wire::CronSpecDto, right_db::DbError> {
    Ok(wire::CronSpecDto {
        job_name: row.get(0)?,
        schedule: row.get(1)?,
        prompt: row.get(2)?,
        lock_ttl: row.get(3)?,
        max_budget_usd: row.get(4)?,
        triggered_at: row.get(5)?,
        trigger_force_notify: row.get::<_, i64>(6)? != 0,
        recurring: row.get::<_, i64>(7)? != 0,
        run_at: row.get(8)?,
        target_chat_id: row.get(9)?,
        target_thread_id: row.get(10)?,
        model: row.get(11)?,
        trigger_extra_instruction: row.get(12)?,
        trigger_then_json: row.get(13)?,
        trigger_origin_chat_id: row.get(14)?,
        trigger_origin_thread_id: row.get(15)?,
    })
}

fn run_from_row(row: &right_db::row::Row<'_>) -> Result<wire::CronRunRowDto, right_db::DbError> {
    Ok(wire::CronRunRowDto {
        id: row.get(0)?,
        job_name: row.get(1)?,
        started_at: row.get(2)?,
        finished_at: row.get(3)?,
        exit_code: row.get(4)?,
        status: row.get(5)?,
        log_path: row.get(6)?,
        run_note: row.get(7)?,
        delivery_json: row.get(8)?,
        delivered_at: row.get(9)?,
        delivery_status: row.get(10)?,
    })
}

fn pending_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<wire::PendingAsyncResultDto, right_db::DbError> {
    Ok(wire::PendingAsyncResultDto {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        delivery_json: row.get(3)?,
        run_note: row.get(4)?,
        status: row.get(5)?,
        target_chat_id: row.get(6)?,
        target_thread_id: row.get(7)?,
        force_notify: row.get::<_, i64>(8)? != 0,
    })
}

impl AgentDbOwner {
    pub(crate) async fn run_ledger(
        &self,
        operation: RunLedgerOp,
    ) -> Result<OwnerResponse, DbOwnerError> {
        self.with_connection(move |conn| Box::pin(async move {
            let value = match operation {
                RunLedgerOp::CronSpecsList(_) => {
                    let specs = conn.query_all("SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, trigger_force_notify, recurring, run_at, target_chat_id, target_thread_id, model, trigger_extra_instruction, trigger_then_json, trigger_origin_chat_id, trigger_origin_thread_id FROM cron_specs ORDER BY job_name", (), cron_spec_from_row).await?;
                    json(wire::CronSpecsListResponse { specs })?
                }
                RunLedgerOp::CronRecentRuns(r) => {
                    let limit = i64::from(r.limit.clamp(1, 100));
                    let runs = conn.query_all("SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path, run_note, delivery_json, delivered_at, delivery_status FROM async_runs WHERE kind = 'cron' AND producer_ref = ?1 ORDER BY started_at DESC LIMIT ?2", right_db::params![r.job_name, limit], run_from_row).await?;
                    json(wire::CronRecentRunsResponse { runs })?
                }
                RunLedgerOp::CronSpecDetail(r) => {
                    let spec = conn.query_row("SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, trigger_force_notify, recurring, run_at, target_chat_id, target_thread_id, model, trigger_extra_instruction, trigger_then_json, trigger_origin_chat_id, trigger_origin_thread_id FROM cron_specs WHERE job_name = ?1", [&r.job_name], cron_spec_from_row).await.optional()?;
                    let detail = if let Some(spec) = spec {
                        let recent_runs = conn.query_all("SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path, run_note, delivery_json, delivered_at, delivery_status FROM async_runs WHERE kind = 'cron' AND producer_ref = ?1 ORDER BY started_at DESC LIMIT 20", [&r.job_name], run_from_row).await?;
                        let linked_skills = right_agent::cron_skill_link::list_for_job(conn, &r.job_name).await?;
                        Some(wire::CronSpecDetailDto { spec, recent_runs, linked_skills })
                    } else { None };
                    json(wire::CronSpecDetailResponse { detail })?
                }
                RunLedgerOp::CronDeleteSpec(r) => {
                    let changed = conn.execute("DELETE FROM cron_specs WHERE job_name = ?1", [&r.job_name]).await?;
                    if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::CronClearTriggered(r) => {
                    right_agent::cron_spec::clear_triggered_at(conn, &r.job_name)
                        .await
                        .map_err(domain)?;
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::EnqueueBackgroundRun(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "enqueue_background_run").await?.is_none() {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::insert_queued_background_run(&tx, right_agent::async_runs::NewBackgroundRun { id: &r.run_id, producer_ref: r.producer_ref.as_deref(), source_session_id: &r.source_session_id, run_session_id: &r.run_session_id, target_chat_id: r.target_chat_id, target_thread_id: r.target_thread_id, created_at: &r.created_at }).await?;
                        record_idempotent(&tx, &r.request_id, "enqueue_background_run", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::CronInsertRunningRun(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "cron_insert_running_run").await?.is_none() {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::insert_running_cron_run(&tx, right_agent::async_runs::NewCronRun { id: &r.run_id, job_name: &r.job_name, started_at: &r.started_at, log_path: &r.log_path, target_chat_id: r.target_chat_id, target_thread_id: r.target_thread_id, force_notify: r.force_notify }).await?;
                        record_idempotent(&tx, &r.request_id, "cron_insert_running_run", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::MarkBackgroundSpawned(r) => {
                    let changed = conn.execute("UPDATE async_runs SET status = 'running', handoff_state = 'spawned', started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%SZ','now')), delivery_required = 1, delivery_status = 'pending', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE id = ?1", [&r.run_id]).await?;
                    if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::PersistRunOutput(r) => {
                    if let Some(response) = idempotent_response::<wire::PersistRunOutputResponse>(conn, &r.request_id, "persist_run_output").await? { json(response)? } else {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::persist_run_output(&tx, &r.run_id, right_agent::async_runs::RunOutput { run_note: r.run_note.as_deref(), delivery_json: r.delivery_json.as_deref(), error_json: r.error_json.as_deref(), delivery_required: r.delivery_required }).await?;
                        right_agent::async_runs::finish_run(&tx, &r.run_id, r.exit_code, &r.status).await?;
                        let response = wire::PersistRunOutputResponse { delivery_status: if r.delivery_required { "pending" } else { "none" }.to_owned() };
                        record_idempotent(&tx, &r.request_id, "persist_run_output", &response).await?;
                        tx.commit().await?;
                        json(response)?
                    }
                }
                RunLedgerOp::FinishRun(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "finish_run").await?.is_none() {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::finish_run(&tx, &r.run_id, r.exit_code, &r.status).await?;
                        record_idempotent(&tx, &r.request_id, "finish_run", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::MarkHandoffFailed(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "mark_handoff_failed").await?.is_none() {
                        let tx = conn.transaction().await?;
                        right_agent::async_runs::persist_run_output(&tx, &r.run_id, right_agent::async_runs::RunOutput { run_note: Some(&r.run_note), delivery_json: Some(&r.delivery_json), error_json: Some(&r.error_json), delivery_required: true }).await?;
                        let changed = tx.execute("UPDATE async_runs SET handoff_state = 'failed' WHERE id = ?1", [&r.run_id]).await?;
                        if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                        right_agent::async_runs::finish_run(&tx, &r.run_id, None, "failed").await?;
                        record_idempotent(&tx, &r.request_id, "mark_handoff_failed", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::RecoverInterruptedHandoffs(_) => {
                    let tx = conn.transaction().await?;
                    let now = Utc::now().to_rfc3339();
                    let changed = tx.execute("UPDATE async_runs SET run_note = 'Background run interrupted before producing a result', delivery_json = '{\"kind\":\"notify\",\"content\":\"A background task was interrupted before it produced a result.\",\"attachments\":null}', error_json = '{\"kind\":\"background_result_unavailable\",\"reason\":\"interrupted handoff\"}', delivery_required = 1, delivery_status = 'pending', handoff_state = 'failed', finished_at = ?1, status = 'failed', updated_at = ?1 WHERE kind = 'background' AND status = 'queued' AND handoff_state = 'queued'", [&now]).await?;
                    tx.commit().await?;
                    json(wire::RecoveredCountResponse { recovered: changed })?
                }
                RunLedgerOp::CronMarkInterruptedByShutdown(r) => {
                    let now = Utc::now().to_rfc3339();
                    let error_json = serde_json::json!({"kind":"cron_shutdown_interrupted","job_name":r.job_name,"reason":r.reason}).to_string();
                    let changed = conn.execute("UPDATE async_runs SET run_note = 'Cron interrupted by shutdown', error_json = ?1, delivery_required = CASE WHEN target_chat_id != 0 THEN 1 ELSE 0 END, delivery_status = CASE WHEN target_chat_id != 0 THEN 'pending' ELSE 'none' END, finished_at = ?2, status = 'failed', updated_at = ?2 WHERE kind = 'cron' AND producer_ref = ?3 AND status = 'running'", right_db::params![error_json, now, r.job_name]).await?;
                    json(wire::RecoveredCountResponse { recovered: changed })?
                }
                RunLedgerOp::DeliveryFetchPending(r) => {
                    let pending = conn.query_all("SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, NULLIF(target_chat_id, 0), target_thread_id, force_notify FROM async_runs WHERE delivery_required = 1 AND delivery_status IN ('pending','retryable') AND status IN ('success','failed') AND delivery_json IS NOT NULL ORDER BY finished_at ASC LIMIT ?1", [i64::from(r.limit.clamp(1, 100))], pending_from_row).await?;
                    json(wire::DeliveryFetchPendingResponse { pending })?
                }
                RunLedgerOp::DeliveryMarkOutcome(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "delivery_mark_outcome").await?.is_none() {
                        let tx = conn.transaction().await?;
                        let now = Utc::now().to_rfc3339();
                        let changed = tx.execute("UPDATE async_runs SET delivery_status = ?1, delivered_at = ?2, updated_at = ?2 WHERE id = ?3", right_db::params![r.status, now, r.run_id]).await?;
                        if changed == 0 { return Err(DbOwnerError::Database(right_db::DbError::NotFound)); }
                        record_idempotent(&tx, &r.request_id, "delivery_mark_outcome", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                RunLedgerOp::DeliveryDeduplicateJob(r) => {
                    let tx = conn.transaction().await?;
                    let candidate = tx.query_row(
                        "SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status, \
                                NULLIF(target_chat_id, 0), target_thread_id, \
                                COALESCE(( \
                                    SELECT MAX(force_notify) FROM async_runs grouped \
                                    WHERE grouped.kind = 'cron' \
                                      AND grouped.producer_ref = ?1 \
                                      AND grouped.delivery_required = 1 \
                                      AND grouped.delivery_status IN ('pending','retryable') \
                                      AND grouped.status IN ('success','failed') \
                                      AND grouped.delivery_json IS NOT NULL \
                                ), 0) \
                         FROM async_runs \
                         WHERE kind = 'cron' AND producer_ref = ?1 \
                           AND delivery_required = 1 \
                           AND delivery_status IN ('pending','retryable') \
                           AND status IN ('success','failed') \
                           AND delivery_json IS NOT NULL \
                         ORDER BY finished_at DESC LIMIT 1",
                        [&r.producer_ref],
                        pending_from_row,
                    ).await.optional()?;
                    let superseded = if let Some(c) = &candidate { tx.execute("UPDATE async_runs SET delivery_status = 'superseded', delivered_at = strftime('%Y-%m-%dT%H:%M:%SZ','now'), updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE kind = 'cron' AND producer_ref = ?1 AND id != ?2 AND delivery_required = 1 AND delivery_status IN ('pending','retryable') AND status IN ('success','failed')", right_db::params![r.producer_ref, c.id.as_str()]).await? as u32 } else { 0 };
                    tx.commit().await?;
                    json(wire::DeliveryDeduplicateJobResponse { candidate, superseded })?
                }
            };
            Ok(OwnerResponse::Runs(value))
        })).await
    }

    pub(crate) async fn secrets_registry(
        &self,
        operation: SecretsRegistryOp,
    ) -> Result<OwnerResponse, DbOwnerError> {
        self.with_connection(move |conn| {
            Box::pin(async move {
                let value = match operation {
                    SecretsRegistryOp::AuthStatus(_) => json(wire::AuthStatusResponse {
                        token_present: right_mcp::credentials::get_auth_token(conn)
                            .await?
                            .is_some(),
                    })?,
                    SecretsRegistryOp::AuthTokenGet(_) => json(wire::AuthTokenGetResponse {
                        token: right_mcp::credentials::get_auth_token(conn)
                            .await?
                            .map(secrecy::SecretString::from),
                    })?,
                    SecretsRegistryOp::AuthTokenSave(r) => {
                        if idempotent_response::<wire::OkResponse>(
                            conn,
                            &r.request_id,
                            "auth_token_save",
                        )
                        .await?
                        .is_none()
                        {
                            let tx = conn.transaction().await?;
                            tx.execute("DELETE FROM auth_tokens", ()).await?;
                            tx.execute(
                                "INSERT INTO auth_tokens(token) VALUES (?1)",
                                [r.token.expose_secret()],
                            )
                            .await?;
                            record_idempotent(
                                &tx,
                                &r.request_id,
                                "auth_token_save",
                                &wire::OkResponse {},
                            )
                            .await?;
                            tx.commit().await?;
                        }
                        json(wire::OkResponse {})?
                    }
                    SecretsRegistryOp::AuthTokenDelete(r) => {
                        if idempotent_response::<wire::OkResponse>(
                            conn,
                            &r.request_id,
                            "auth_token_delete",
                        )
                        .await?
                        .is_none()
                        {
                            let tx = conn.transaction().await?;
                            right_mcp::credentials::delete_auth_token(&tx).await?;
                            record_idempotent(
                                &tx,
                                &r.request_id,
                                "auth_token_delete",
                                &wire::OkResponse {},
                            )
                            .await?;
                            tx.commit().await?;
                        }
                        json(wire::OkResponse {})?
                    }
                    SecretsRegistryOp::NoticeTokenGetOrCreate(r) => {
                        if let Some(response) = idempotent_response::<
                            wire::NoticeTokenGetOrCreateResponse,
                        >(
                            conn, &r.request_id, "notice_token_get_or_create"
                        )
                        .await?
                        {
                            json(response)?
                        } else {
                            let tx = conn.transaction().await?;
                            let response = wire::NoticeTokenGetOrCreateResponse {
                                token: secrecy::SecretString::from(
                                    right_mcp::credentials::get_or_create_notice_token(&tx).await?,
                                ),
                            };
                            record_idempotent(
                                &tx,
                                &r.request_id,
                                "notice_token_get_or_create",
                                &response,
                            )
                            .await?;
                            tx.commit().await?;
                            json(response)?
                        }
                    }
                    SecretsRegistryOp::McpOauthStateSet(r) => {
                        if idempotent_response::<wire::OkResponse>(
                            conn,
                            &r.request_id,
                            "mcp_oauth_state_set",
                        )
                        .await?
                        .is_none()
                        {
                            let tx = conn.transaction().await?;
                            right_mcp::credentials::db_set_oauth_state(
                                &tx,
                                &r.server_name,
                                r.access_token.expose_secret(),
                                r.refresh_token.as_ref().map(|v| v.expose_secret()),
                                &r.token_endpoint,
                                &r.client_id,
                                r.client_secret.as_ref().map(|v| v.expose_secret()),
                                &r.expires_at,
                                &r.oauth_resource,
                            )
                            .await?;
                            record_idempotent(
                                &tx,
                                &r.request_id,
                                "mcp_oauth_state_set",
                                &wire::OkResponse {},
                            )
                            .await?;
                            tx.commit().await?;
                        }
                        json(wire::OkResponse {})?
                    }
                };
                Ok(OwnerResponse::Secrets(value))
            })
        })
        .await
    }
}

fn usage_breakdown(dto: wire::UsageBreakdownDto) -> right_agent::usage::UsageBreakdown {
    right_agent::usage::UsageBreakdown {
        session_uuid: dto.session_uuid,
        total_cost_usd: dto.total_cost_usd,
        num_turns: dto.num_turns,
        input_tokens: dto.input_tokens,
        output_tokens: dto.output_tokens,
        cache_creation_tokens: dto.cache_creation_tokens,
        cache_read_tokens: dto.cache_read_tokens,
        web_search_requests: dto.web_search_requests,
        web_fetch_requests: dto.web_fetch_requests,
        model_usage_json: dto.model_usage_json,
        api_key_source: dto.api_key_source,
        wall_elapsed_ms: dto.wall_elapsed_ms,
    }
}

fn baseline_f64(
    value: right_agent::usage::turn_baseline::BaselineMetric<f64>,
) -> wire::BaselineMetricDto {
    match value {
        right_agent::usage::turn_baseline::BaselineMetric::Insufficient { sample_size } => {
            wire::BaselineMetricDto::Insufficient { sample_size }
        }
        right_agent::usage::turn_baseline::BaselineMetric::Available { p50, p90, p99 } => {
            wire::BaselineMetricDto::Available { p50, p90, p99 }
        }
    }
}
fn baseline_u32(
    value: right_agent::usage::turn_baseline::BaselineMetric<u32>,
) -> wire::BaselineMetricDto {
    match value {
        right_agent::usage::turn_baseline::BaselineMetric::Insufficient { sample_size } => {
            wire::BaselineMetricDto::Insufficient { sample_size }
        }
        right_agent::usage::turn_baseline::BaselineMetric::Available { p50, p90, p99 } => {
            wire::BaselineMetricDto::Available {
                p50: f64::from(p50),
                p90: f64::from(p90),
                p99: f64::from(p99),
            }
        }
    }
}
fn baseline_u64(
    value: right_agent::usage::turn_baseline::BaselineMetric<u64>,
) -> wire::BaselineMetricDto {
    match value {
        right_agent::usage::turn_baseline::BaselineMetric::Insufficient { sample_size } => {
            wire::BaselineMetricDto::Insufficient { sample_size }
        }
        right_agent::usage::turn_baseline::BaselineMetric::Available { p50, p90, p99 } => {
            wire::BaselineMetricDto::Available {
                p50: p50 as f64,
                p90: p90 as f64,
                p99: p99 as f64,
            }
        }
    }
}

fn lifecycle_dto(row: right_lifecycle::SkillLifecycleRow) -> wire::SkillLifecycleDto {
    wire::SkillLifecycleDto {
        skill_name: row.skill_name,
        state: row.state.as_db_str().to_owned(),
        pinned: row.pinned,
        created_by: row.created_by.as_db_str().to_owned(),
        use_count: row.use_count.max(0) as u32,
        patch_count: row.patch_count.max(0) as u32,
        created_at: row.created_at.map(|v| v.to_rfc3339()),
        last_used_at: row.last_used_at.map(|v| v.to_rfc3339()),
        last_patched_at: row.last_patched_at.map(|v| v.to_rfc3339()),
        archived_at: row.archived_at.map(|v| v.to_rfc3339()),
        absorbed_into: row.absorbed_into,
    }
}

impl AgentDbOwner {
    pub(crate) async fn learning_memory(
        &self,
        operation: LearningMemoryOp,
    ) -> Result<OwnerResponse, DbOwnerError> {
        let agent = self.agent.clone();
        self.with_connection(move |conn| Box::pin(async move {
            let value = match operation {
                LearningMemoryOp::UsageInsertEvent(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "usage_insert_event").await?.is_none() {
                        let tx = conn.transaction().await?;
                        let b = usage_breakdown(r.event);
                        match r.source {
                            wire::UsageSourceDto::Interactive { chat_id, thread_id } => right_agent::usage::insert::insert_interactive(&tx, &b, chat_id, thread_id).await,
                            wire::UsageSourceDto::Cron { job_name } => right_agent::usage::insert::insert_cron(&tx, &b, &job_name).await,
                            wire::UsageSourceDto::ReflectionWorker { chat_id, thread_id } => right_agent::usage::insert::insert_reflection_worker(&tx, &b, chat_id, thread_id).await,
                            wire::UsageSourceDto::ReflectionCron { job_name } => right_agent::usage::insert::insert_reflection_cron(&tx, &b, &job_name).await,
                            wire::UsageSourceDto::LearningPrefilter { chat_id, thread_id } => right_agent::usage::insert::insert_learning_prefilter(&tx, &b, chat_id, thread_id).await,
                            wire::UsageSourceDto::LearningProbeWriter { chat_id, thread_id } => right_agent::usage::insert::insert_learning_probe_writer(&tx, &b, chat_id, thread_id).await,
                            wire::UsageSourceDto::LearningCurator => right_agent::usage::insert::insert_learning_curator(&tx, &b).await,
                            wire::UsageSourceDto::IdleCompaction { chat_id, thread_id } => right_agent::usage::insert::insert_idle_compaction(&tx, &b, chat_id, thread_id).await,
                        }.map_err(domain)?;
                        record_idempotent(&tx, &r.request_id, "usage_insert_event", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::LearningEventInsert(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "learning_event_insert").await?.is_none() {
                        let action: right_agent::learned_skills::LearningAction = serde_json::from_value(serde_json::Value::String(r.event.action)).map_err(invalid)?;
                        let phase: right_agent::learned_skills::LearningPhase = serde_json::from_value(serde_json::Value::String(r.event.phase)).map_err(invalid)?;
                        let status = r.event.status.map(|v| serde_json::from_value(serde_json::Value::String(v)).map_err(invalid)).transpose()?;
                        let tx = conn.transaction().await?;
                        right_agent::learned_skills::insert_learning_event(&tx, &right_agent::learned_skills::LearningEvent { invocation_id: r.event.invocation_id, agent_name: agent.clone(), action, skill_name: r.event.skill_name, phase, status, hint_outcome: r.event.hint_outcome, reason: r.event.reason, message: r.event.message, summary: r.event.summary, event_refs: r.event.event_refs }).await?;
                        record_idempotent(&tx, &r.request_id, "learning_event_insert", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::LearningTodaySpend(r) => {
                    let date = r.now_utc.split_once('T').map_or(r.now_utc.as_str(), |(date, _)| date);
                    let start = format!("{date}T00:00:00Z");
                    let usd = conn.query_row("SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events WHERE ts >= ?1 AND source IN ('learning_prefilter','learning_probe_writer','learning_curator')", [&start], |row| row.get::<_, f64>(0)).await?;
                    json(wire::LearningTodaySpendResponse { usd })?
                }
                LearningMemoryOp::LearningRecordBudgetSkip(r) => {
                    if let Some(response) = idempotent_response::<wire::OkResponse>(conn, &r.request_id, "learning_record_budget_skip").await? {
                        json(response)?
                    } else {
                        let tx = conn.transaction().await?;
                        right_agent::usage::insert::insert_learning_skip(&tx, &r.reason, r.intended_kind.as_deref(), Some(r.chat_id), Some(r.thread_id)).await.map_err(domain)?;
                        let response = wire::OkResponse {};
                        record_idempotent(&tx, &r.request_id, "learning_record_budget_skip", &response).await?;
                        tx.commit().await?;
                        json(response)?
                    }
                }
                LearningMemoryOp::LearningAuthoredSkillThisTurn(r) => json(wire::BoolResultResponse { result: right_agent::learned_skills::successful_finish_exists(conn, &r.invocation_id).await? })?,
                LearningMemoryOp::LearningLinkCronAuthored(r) => {
                    let mut names: Vec<String> = right_agent::learned_skills::successful_finishes_for_invocation(conn, &r.invocation_id).await?.into_iter().map(|(name, _)| name).collect();
                    names.sort(); names.dedup();
                    right_agent::cron_skill_link::link_auto(conn, &r.job_name, &names).await?;
                    json(wire::LearningLinkCronAuthoredResponse { linked: names.len() })?
                }
                LearningMemoryOp::LearningLatestInteractiveContextTokens(r) => {
                    let tokens = conn.query_row("SELECT input_tokens + cache_read_tokens + cache_creation_tokens FROM usage_events WHERE chat_id = ?1 AND thread_id = ?2 AND source = 'interactive' ORDER BY ts DESC LIMIT 1", right_db::params![r.chat_id, r.thread_id], |row| row.get::<_, i64>(0)).await.optional()?.map(|v| v.max(0) as u64);
                    json(wire::LearningLatestInteractiveContextTokensResponse { tokens })?
                }
                LearningMemoryOp::LearningTurnBaselines(r) => {
                    let b = right_agent::usage::turn_baseline::compute(conn, r.window_days, r.min_sample).await.map_err(domain)?;
                    json(wire::LearningTurnBaselinesResponse { baselines: wire::TurnBaselinesDto { sample_size: b.sample_size, elapsed_sample_size: b.elapsed_sample_size, window_days: b.window_days, cost_usd: baseline_f64(b.cost_usd), num_turns: baseline_u32(b.num_turns), wall_elapsed_ms: baseline_u64(b.wall_elapsed_ms) } })?
                }
                LearningMemoryOp::LearningProbeCostSpike(r) => {
                    let now = DateTime::parse_from_rfc3339(&r.now_rfc3339).map_err(invalid)?.with_timezone(&Utc);
                    let evidence = right_agent::usage::turn_baseline::check_probe_writer_cost_spike(conn, now, r.baseline_days, r.k, r.min_floor_usd).await.map_err(domain)?.map(|e| wire::CostSpikeEvidenceDto { today_cost_usd: e.today_cost_usd, baseline_p50_usd: e.baseline_p50_usd, k: e.k, min_floor_usd: e.min_floor_usd });
                    json(wire::LearningProbeCostSpikeResponse { evidence })?
                }
                LearningMemoryOp::CuratorLoadState(_) => {
                    let state = conn.query_row("SELECT last_run_at, last_run_status, consecutive_failures, circuit_open_until, last_spike_evidence_json FROM curator_state WHERE agent_singleton_id = 1", (), |row| Ok(wire::CuratorStateDto { last_run_at: row.get(0)?, last_run_status: row.get(1)?, consecutive_failures: row.get::<_, i64>(2)?.max(0) as u32, circuit_open_until: row.get(3)?, last_spike_evidence_json: row.get(4)? })).await.optional()?.unwrap_or_default();
                    json(wire::CuratorLoadStateResponse { state })?
                }
                LearningMemoryOp::CuratorSaveState(r) => {
                    conn.execute("INSERT OR REPLACE INTO curator_state(agent_singleton_id,last_run_at,last_run_status,consecutive_failures,circuit_open_until,last_spike_evidence_json) VALUES (1,?1,?2,?3,?4,?5)", right_db::params![r.state.last_run_at, r.state.last_run_status, i64::from(r.state.consecutive_failures), r.state.circuit_open_until, r.state.last_spike_evidence_json]).await?;
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::CuratorInsertRun(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "curator_insert_run").await?.is_none() {
                        let tx = conn.transaction().await?; let v = r.record;
                        tx.execute("INSERT INTO curator_runs(run_at,trigger,trigger_evidence_json,mode,status,cost_usd,cache_read,cache_creation,consolidations,archives,summary,actions_json,invocation_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)", right_db::params![v.run_at,v.trigger,v.trigger_evidence_json,v.mode,v.status,v.cost_usd,v.cache_read,v.cache_creation,v.consolidations,v.archives,v.summary,v.actions_json,v.invocation_id]).await?;
                        record_idempotent(&tx, &r.request_id, "curator_insert_run", &wire::OkResponse {}).await?; tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::CuratorLatestChatActivity(_) => json(wire::CuratorLatestChatActivityResponse { at: conn.query_row("SELECT MAX(created_at) FROM conversation_messages", (), |row| row.get::<_, Option<String>>(0)).await? })?,
                LearningMemoryOp::CuratorChangeCount(r) => {
                    let since = DateTime::parse_from_rfc3339(&r.since_rfc3339).map_err(invalid)?.with_timezone(&Utc);
                    json(wire::CuratorChangeCountResponse { count: right_lifecycle::count_changes_since(conn, since).await.map_err(domain)? })?
                }
                LearningMemoryOp::CuratorArchivedSnapshot(_) => {
                    let skills = conn.query_all("SELECT skill_name, absorbed_into FROM skill_lifecycle WHERE state = 'archived' ORDER BY skill_name", (), |row| Ok(wire::ArchivedSkillDto { skill_name: row.get(0)?, absorbed_into: row.get(1)? })).await?;
                    json(wire::CuratorArchivedSnapshotResponse { skills })?
                }
                LearningMemoryOp::CuratorApplyTransitions(r) => {
                    let now = DateTime::parse_from_rfc3339(&r.now_rfc3339).map_err(invalid)?.with_timezone(&Utc);
                    let before: std::collections::HashSet<String> = conn.query_all("SELECT skill_name FROM skill_lifecycle WHERE state = 'archived'", (), |row| row.get::<_, String>(0)).await?.into_iter().collect();
                    let transition_changes = right_lifecycle::apply_automatic_transitions(conn, now, right_lifecycle::TransitionConfig { stale_after: chrono::Duration::days(i64::from(r.stale_after_days)), archive_after: chrono::Duration::days(i64::from(r.archive_after_days)) }).await.map_err(domain)?;
                    let archived = conn.query_all("SELECT skill_name, absorbed_into FROM skill_lifecycle WHERE state = 'archived'", (), |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))).await?;
                    let mut archived_this_pass = Vec::new();
                    for (skill, target) in archived.into_iter().filter(|(name, _)| !before.contains(name)) {
                        match target {
                            Some(target) => right_agent::cron_skill_link::redirect_skill(conn, &skill, &target).await?,
                            None => right_agent::cron_skill_link::drop_skill(conn, &skill).await?,
                        }
                        archived_this_pass.push(skill);
                    }
                    let candidates = right_lifecycle::list_curator_candidates(conn).await.map_err(domain)?.into_iter().map(lifecycle_dto).collect();
                    json(wire::CuratorApplyTransitionsResponse { transition_changes, candidates, archived_this_pass })?
                }
                LearningMemoryOp::CuratorFinalize(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "curator_finalize").await?.is_none() {
                        let tx = conn.transaction().await?;
                        tx.execute("INSERT OR REPLACE INTO curator_state(agent_singleton_id,last_run_at,last_run_status,consecutive_failures,circuit_open_until,last_spike_evidence_json) VALUES (1,?1,?2,?3,?4,?5)", right_db::params![r.state.last_run_at, r.state.last_run_status, i64::from(r.state.consecutive_failures), r.state.circuit_open_until, r.state.last_spike_evidence_json]).await?;
                        let v = r.run_record;
                        tx.execute("INSERT INTO curator_runs(run_at,trigger,trigger_evidence_json,mode,status,cost_usd,cache_read,cache_creation,consolidations,archives,summary,actions_json,invocation_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)", right_db::params![v.run_at,v.trigger,v.trigger_evidence_json,v.mode,v.status,v.cost_usd,v.cache_read,v.cache_creation,v.consolidations,v.archives,v.summary,v.actions_json,v.invocation_id]).await?;
                        for e in r.maintain_spend_entries { right_agent::usage::insert::insert_skill_spend(&tx, &e.skill_name, &e.kind, e.cost_usd, e.cache_read, e.cache_creation, e.invocation_id.as_deref()).await.map_err(domain)?; }
                        record_idempotent(&tx, &r.request_id, "curator_finalize", &wire::OkResponse {}).await?;
                        tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::LifecycleArchivedSince(r) => json(wire::LifecycleArchivedSinceResponse { skill_names: conn.query_all("SELECT skill_name FROM skill_lifecycle WHERE archived_at >= ?1 ORDER BY skill_name", [&r.since_rfc3339], |row| row.get::<_, String>(0)).await? })?,
                LearningMemoryOp::SkillLifecycleGet(r) => json(wire::SkillLifecycleGetResponse { row: right_lifecycle::get(conn, &r.skill_name).await.map_err(domain)?.map(lifecycle_dto) })?,
                LearningMemoryOp::SkillLifecycleList(_) => json(wire::SkillLifecycleListResponse { rows: right_lifecycle::list(conn).await.map_err(domain)?.into_iter().map(lifecycle_dto).collect() })?,
                LearningMemoryOp::SkillPin(r) => { right_lifecycle::set_pinned(conn, &r.skill_name, r.pinned).await.map_err(domain)?; json(wire::OkResponse {})? }
                LearningMemoryOp::SkillSpendRecord(r) => {
                    if idempotent_response::<wire::OkResponse>(conn, &r.request_id, "skill_spend_record").await?.is_none() {
                        let tx = conn.transaction().await?;
                        for e in r.entries { right_agent::usage::insert::insert_skill_spend(&tx, &e.skill_name, &e.kind, e.cost_usd, e.cache_read, e.cache_creation, e.invocation_id.as_deref()).await.map_err(domain)?; }
                        record_idempotent(&tx, &r.request_id, "skill_spend_record", &wire::OkResponse {}).await?; tx.commit().await?;
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::SkillSpendBySkill(_) | LearningMemoryOp::DashboardSkillSpend(_) => json(wire::SkillSpendBySkillResponse { rows: right_dashboard::read_model::learning::skill_spend_by_skill(conn).await.map_err(domain)? })?,
                LearningMemoryOp::AlertCheckAndRecord(r) => {
                    if let Some(response) = idempotent_response::<wire::AlertCheckAndRecordResponse>(conn, &r.request_id, "alert_check_and_record").await? { json(response)? } else {
                        let tx = conn.transaction().await?;
                        let existing = tx.query_row("SELECT first_sent_at FROM memory_alerts WHERE alert_type = ?1", [&r.alert_type], |row| row.get::<_, String>(0)).await.optional()?;
                        let now = Utc::now();
                        let should_fire = existing.and_then(|v| DateTime::parse_from_rfc3339(&v).ok()).is_none_or(|sent| now.signed_duration_since(sent.with_timezone(&Utc)) > chrono::Duration::hours(ALERT_DEDUP_HOURS));
                        if should_fire { tx.execute("INSERT INTO memory_alerts(alert_type,first_sent_at) VALUES (?1,?2) ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at", right_db::params![r.alert_type, now.to_rfc3339()]).await?; }
                        let response = wire::AlertCheckAndRecordResponse { should_fire }; record_idempotent(&tx, &r.request_id, "alert_check_and_record", &response).await?; tx.commit().await?; json(response)?
                    }
                }
                LearningMemoryOp::AlertRecord(r) => {
                    conn.execute("INSERT INTO memory_alerts(alert_type,first_sent_at) VALUES (?1,?2) ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at", right_db::params![r.alert_type,Utc::now().to_rfc3339()]).await?;
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::AlertClear(r) => {
                    match r.older_than_secs {
                        Some(seconds) => {
                            let cutoff = (Utc::now() - chrono::Duration::seconds(i64::try_from(seconds).map_err(invalid)?)).to_rfc3339();
                            conn.execute("DELETE FROM memory_alerts WHERE alert_type = ?1 AND first_sent_at < ?2", right_db::params![r.alert_type, cutoff]).await?;
                        }
                        None => {
                            conn.execute("DELETE FROM memory_alerts WHERE alert_type = ?1", [r.alert_type.as_str()]).await?;
                        }
                    }
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::RetainEnqueue(r) => {
                    let operation = retain_enqueue_operation(&r.item)?;
                    let tx = conn.transaction().await?;
                    let response = if let Some(response) =
                        idempotent_response::<wire::OkResponse>(
                            &tx,
                            &r.request_id,
                            &operation,
                        )
                        .await?
                    {
                        response
                    } else {
                        let i = r.item;
                        right_memory::retain_queue::enqueue_in_transaction(
                            &tx,
                            &i.source,
                            &i.content,
                            i.context.as_deref(),
                            i.document_id.as_deref(),
                            i.update_mode.as_deref(),
                            Some(&i.tags),
                        )
                        .await
                        .map_err(domain)?;
                        let response = wire::OkResponse {};
                        record_idempotent(&tx, &r.request_id, &operation, &response).await?;
                        response
                    };
                    tx.commit().await?;
                    json(response)?
                }
                LearningMemoryOp::RetainClaimBatch(r) => {
                    let claim = right_memory::retain_queue::claim_batch(conn, r.limit.max(1) as usize, Duration::from_secs(u64::from(r.lease_ttl_secs.max(1)))).await.map_err(domain)?;
                    let items = claim.items.into_iter().map(|i| wire::PendingRetainDto { id: i.id, content: i.content, context: i.context, document_id: i.document_id, update_mode: i.update_mode, tags: i.tags.unwrap_or_default(), attempts: i.attempts.max(0) as u32, created_at: i.created_at }).collect();
                    json(wire::RetainClaimBatchResponse { claim: wire::RetainClaimDto { claim_token: claim.claim_token, lease_expires_at: claim.lease_expires_at, items } })?
                }
                LearningMemoryOp::RetainAck(r) => {
                    let tx = conn.transaction().await?;
                    for id in r.ids {
                        right_memory::retain_queue::ack(&tx, &r.claim_token, id).await.map_err(|e| match e { right_memory::MemoryError::LeaseConflict(v) => DbOwnerError::Conflict(v), other => domain(other) })?;
                    }
                    tx.commit().await?;
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::RetainNack(r) => {
                    let tx = conn.transaction().await?;
                    for id in r.ids {
                        right_memory::retain_queue::nack(&tx, &r.claim_token, id, r.retry, &r.error).await.map_err(|e| match e { right_memory::MemoryError::LeaseConflict(v) => DbOwnerError::Conflict(v), other => domain(other) })?;
                    }
                    tx.commit().await?;
                    json(wire::OkResponse {})?
                }
                LearningMemoryOp::RetainQueueStats(_) => json(wire::RetainQueueStatsResponse { count: right_memory::retain_queue::count(conn).await.map_err(domain)?, oldest_age_secs: right_memory::retain_queue::oldest_age(conn).await.map_err(domain)?.map(|v| v.as_secs()) })?,
                LearningMemoryOp::DashboardActivity(r) => json(wire::DashboardActivityResponse { overview: right_dashboard::read_model::activity::activity_overview(conn, right_dashboard::read_model::activity::ActivityOverviewInput { agent: r.agent, generated_at: r.generated_at, refresh_interval_secs: r.refresh_interval_secs, foreground: r.foreground }).await.map_err(domain)? })?,
                LearningMemoryOp::DashboardRunDetail(r) => json(wire::DashboardRunDetailResponse { detail: right_dashboard::read_model::activity::activity_run_detail(conn, &r.run_id, r.max_lines as usize).await.map_err(domain)? })?,
                LearningMemoryOp::DashboardOverview(r) => json(wire::DashboardOverviewResponseWrapper { overview: right_dashboard::read_model::dashboard_overview::dashboard_overview(conn, right_dashboard::read_model::dashboard_overview::DashboardOverviewInput { agent: r.agent, generated_at: r.generated_at, foreground_active_count: r.foreground_active_count, sandbox: r.sandbox }).await.map_err(domain)? })?,
                LearningMemoryOp::DashboardUsage(r) => json(wire::DashboardUsageResponse { overview: right_dashboard::read_model::usage::usage_overview(conn, right_dashboard::read_model::usage::UsageOverviewInput { agent: r.agent, generated_at: r.generated_at, timezone: r.timezone, range: r.range }).await.map_err(domain)? })?,
                LearningMemoryOp::DashboardLearning(r) => json(wire::DashboardLearningResponse { overview: right_dashboard::read_model::learning::learning_overview(conn, right_dashboard::read_model::learning::LearningOverviewInput { agent: r.agent, generated_at: r.generated_at, refresh_interval_secs: r.refresh_interval_secs }).await.map_err(domain)? })?,
                LearningMemoryOp::DashboardSkillLifecycle(r) => json(wire::DashboardSkillLifecycleResponse { overview: right_dashboard::read_model::learning::skill_lifecycle_overview(conn, &r.agent).await.map_err(domain)? })?,
            };
            Ok(OwnerResponse::Learning(value))
        })).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn migrated_owner() -> (tempfile::TempDir, std::sync::Arc<AgentDbOwner>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let owner = std::sync::Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.expect("open and migrate");
        (dir, owner)
    }

    async fn query_count(owner: &AgentDbOwner, sql: &'static str) -> i64 {
        owner
            .with_connection(|conn| {
                Box::pin(async move { Ok(conn.query_row(sql, (), |row| row.get(0)).await?) })
            })
            .await
            .expect("count query")
    }

    fn interaction_value(response: OwnerResponse) -> serde_json::Value {
        match response {
            OwnerResponse::Interaction(value) => value,
            other => panic!("expected interaction response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lifecycle_bump_use_many_replay_does_not_double_count() {
        let (_dir, owner) = migrated_owner().await;
        let request = wire::LifecycleBumpUseManyRequest {
            agent: "alpha".into(),
            request_id: "bump-req-1".into(),
            skill_names: vec!["rightx-alpha".into(), "rightx-beta".into()],
            now_utc: "2026-08-25T00:00:00Z".into(),
        };
        let first = owner
            .interaction_state(InteractionStateOp::LifecycleBumpUseMany(request.clone()))
            .await
            .map(interaction_value)
            .expect("first bump");
        // Lost-response replay: the same request_id must return the recorded
        // response without incrementing the counters a second time.
        let second = owner
            .interaction_state(InteractionStateOp::LifecycleBumpUseMany(request))
            .await
            .map(interaction_value)
            .expect("replay bump");
        assert_eq!(first, second, "replay must return the recorded response");

        let count = query_count(
            &owner,
            "SELECT use_count FROM skill_lifecycle WHERE skill_name = 'rightx-alpha'",
        )
        .await;
        assert_eq!(count, 1, "replayed bump must not double-increment");

        // A genuinely new request (new request_id) still bumps.
        let fresh = wire::LifecycleBumpUseManyRequest {
            agent: "alpha".into(),
            request_id: "bump-req-2".into(),
            skill_names: vec!["rightx-alpha".into()],
            now_utc: "2026-08-25T00:01:00Z".into(),
        };
        owner
            .interaction_state(InteractionStateOp::LifecycleBumpUseMany(fresh))
            .await
            .expect("fresh bump");
        let count = query_count(
            &owner,
            "SELECT use_count FROM skill_lifecycle WHERE skill_name = 'rightx-alpha'",
        )
        .await;
        assert_eq!(count, 2, "a new request_id must apply the mutation");
    }

    #[tokio::test]
    async fn learning_record_budget_skip_replay_does_not_duplicate_rows() {
        let (_dir, owner) = migrated_owner().await;
        let request = wire::LearningRecordBudgetSkipRequest {
            agent: "alpha".into(),
            request_id: "skip-req-1".into(),
            chat_id: 7,
            thread_id: 0,
            reason: "budget".into(),
            intended_kind: Some("learning".into()),
        };
        owner
            .learning_memory(LearningMemoryOp::LearningRecordBudgetSkip(request.clone()))
            .await
            .expect("first skip");
        owner
            .learning_memory(LearningMemoryOp::LearningRecordBudgetSkip(request))
            .await
            .expect("replay skip");
        let rows = query_count(&owner, "SELECT COUNT(*) FROM learning_skip").await;
        assert_eq!(rows, 1, "replayed skip insert must not duplicate the row");

        let fresh = wire::LearningRecordBudgetSkipRequest {
            agent: "alpha".into(),
            request_id: "skip-req-2".into(),
            chat_id: 7,
            thread_id: 0,
            reason: "budget".into(),
            intended_kind: None,
        };
        owner
            .learning_memory(LearningMemoryOp::LearningRecordBudgetSkip(fresh))
            .await
            .expect("fresh skip");
        let rows = query_count(&owner, "SELECT COUNT(*) FROM learning_skip").await;
        assert_eq!(rows, 2, "a new request_id must insert a new row");
    }

    fn retain_request(request_id: &str, content: &str) -> wire::RetainEnqueueRequest {
        wire::RetainEnqueueRequest {
            agent: "alpha".into(),
            request_id: request_id.into(),
            item: wire::RetainEnqueueItemDto {
                source: "bot".into(),
                content: content.into(),
                context: Some("ctx".into()),
                document_id: Some("doc".into()),
                update_mode: Some("append".into()),
                tags: vec!["chat:7".into()],
            },
        }
    }

    #[tokio::test]
    async fn retain_enqueue_lost_response_replay_does_not_duplicate_queue_row() {
        let (_dir, owner) = migrated_owner().await;
        let request = retain_request("retain-req-1", "remember this");

        owner
            .learning_memory(LearningMemoryOp::RetainEnqueue(request.clone()))
            .await
            .expect("first enqueue");
        owner
            .learning_memory(LearningMemoryOp::RetainEnqueue(request))
            .await
            .expect("lost-response replay");

        assert_eq!(
            query_count(&owner, "SELECT COUNT(*) FROM pending_retains").await,
            1
        );
    }

    #[tokio::test]
    async fn retain_enqueue_request_id_reuse_with_different_payload_conflicts() {
        let (_dir, owner) = migrated_owner().await;
        owner
            .learning_memory(LearningMemoryOp::RetainEnqueue(retain_request(
                "retain-req-1",
                "first payload",
            )))
            .await
            .expect("first enqueue");

        let error = owner
            .learning_memory(LearningMemoryOp::RetainEnqueue(retain_request(
                "retain-req-1",
                "different payload",
            )))
            .await
            .expect_err("same request id with a different payload must conflict");

        assert!(matches!(error, DbOwnerError::Conflict(_)));
        assert_eq!(
            query_count(&owner, "SELECT COUNT(*) FROM pending_retains").await,
            1
        );
    }

    #[tokio::test]
    async fn retain_enqueue_idempotency_record_failure_rolls_back_queue_insert() {
        let (_dir, owner) = migrated_owner().await;
        owner
            .with_connection(|conn| {
                Box::pin(async move {
                    conn.execute_batch(
                        "CREATE TRIGGER fail_retain_idempotency
                         BEFORE INSERT ON internal_ipc_requests
                         WHEN NEW.operation LIKE 'retain_enqueue%'
                         BEGIN
                           SELECT RAISE(ABORT, 'simulated idempotency failure');
                         END;",
                    )
                    .await?;
                    Ok(())
                })
            })
            .await
            .expect("install failure trigger");

        owner
            .learning_memory(LearningMemoryOp::RetainEnqueue(retain_request(
                "retain-req-1",
                "must roll back",
            )))
            .await
            .expect_err("idempotency record must fail");

        assert_eq!(
            query_count(&owner, "SELECT COUNT(*) FROM pending_retains").await,
            0,
            "queue insert must roll back with the failed idempotency record"
        );
    }

    #[tokio::test]
    async fn request_id_reuse_across_operations_conflicts() {
        let (_dir, owner) = migrated_owner().await;
        let skip = wire::LearningRecordBudgetSkipRequest {
            agent: "alpha".into(),
            request_id: "shared-id".into(),
            chat_id: 1,
            thread_id: 0,
            reason: "budget".into(),
            intended_kind: None,
        };
        owner
            .learning_memory(LearningMemoryOp::LearningRecordBudgetSkip(skip))
            .await
            .expect("first op");
        let bump = wire::LifecycleBumpUseManyRequest {
            agent: "alpha".into(),
            request_id: "shared-id".into(),
            skill_names: vec!["rightx-alpha".into()],
            now_utc: "2026-08-25T00:00:00Z".into(),
        };
        let error = owner
            .interaction_state(InteractionStateOp::LifecycleBumpUseMany(bump))
            .await
            .expect_err("request_id reuse across operations must conflict");
        assert!(
            matches!(error, DbOwnerError::Conflict(_)),
            "expected conflict, got {error:?}"
        );
    }
}
