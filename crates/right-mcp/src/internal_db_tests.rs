//! Tests for the typed domain IPC surface: DTO round-trips, secret redaction,
//! error classification, and the no-raw-database-fields contract.

#![cfg(test)]

use secrecy::{ExposeSecret as _, SecretString};

use crate::internal_client::InternalClientError;
use crate::internal_db::*;
use serde::{Deserialize, Serialize};
fn sample_message() -> ConversationMessageDto {
    ConversationMessageDto {
        platform: "telegram".to_owned(),
        chat_id: -100,
        thread_id: 7,
        message_id: Some(42),
        sender_user_id: Some(99),
        sender_name: Some("alice".to_owned()),
        addressed_to_bot: true,
        routed_to_agent: false,
        root_session_id: Some("root-1".to_owned()),
        turn_id: Some(3),
        role: "user".to_owned(),
        content: "hello".to_owned(),
    }
}

fn sample_session() -> SessionRowDto {
    SessionRowDto {
        id: 5,
        chat_id: -100,
        thread_id: 7,
        root_session_id: "uuid-1".to_owned(),
        label: Some("label".to_owned()),
        is_active: true,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        last_used_at: "2026-01-01T01:00:00Z".to_owned(),
    }
}

fn sample_spec() -> CronSpecDto {
    CronSpecDto {
        job_name: "daily".to_owned(),
        schedule: "0 8 * * *".to_owned(),
        prompt: "do it".to_owned(),
        lock_ttl: Some("6h".to_owned()),
        max_budget_usd: 1.5,
        triggered_at: None,
        trigger_force_notify: false,
        recurring: true,
        run_at: None,
        target_chat_id: Some(-100),
        target_thread_id: None,
        model: Some("haiku".to_owned()),
        trigger_extra_instruction: None,
        trigger_then_json: Some(r#"{"instruction":"x","run_on":"success"}"#.to_owned()),
        trigger_origin_chat_id: None,
        trigger_origin_thread_id: None,
    }
}

fn sample_run_row() -> CronRunRowDto {
    CronRunRowDto {
        id: "run-1".to_owned(),
        job_name: "daily".to_owned(),
        started_at: "2026-01-01T00:00:00Z".to_owned(),
        finished_at: Some("2026-01-01T00:01:00Z".to_owned()),
        exit_code: Some(0),
        status: "success".to_owned(),
        log_path: Some("/log".to_owned()),
        run_note: Some("note".to_owned()),
        delivery_json: Some(r#"{"kind":"notify","content":"x","attachments":null}"#.to_owned()),
        delivered_at: None,
        delivery_status: Some("pending".to_owned()),
    }
}

fn sample_pending() -> PendingAsyncResultDto {
    PendingAsyncResultDto {
        id: "run-1".to_owned(),
        kind: "cron".to_owned(),
        producer_ref: Some("daily".to_owned()),
        delivery_json: r#"{"kind":"notify","content":"x","attachments":null}"#.to_owned(),
        run_note: "note".to_owned(),
        status: "success".to_owned(),
        target_chat_id: Some(-100),
        target_thread_id: None,
        force_notify: false,
    }
}

fn sample_usage() -> UsageBreakdownDto {
    UsageBreakdownDto {
        session_uuid: "s-1".to_owned(),
        total_cost_usd: 0.25,
        num_turns: 3,
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 20,
        web_search_requests: 1,
        web_fetch_requests: 0,
        model_usage_json: "{}".to_owned(),
        api_key_source: "none".to_owned(),
        wall_elapsed_ms: Some(1234),
    }
}

fn sample_retain() -> PendingRetainDto {
    PendingRetainDto {
        id: 9,
        content: "remember this".to_owned(),
        context: Some("ctx".to_owned()),
        document_id: None,
        update_mode: Some("append".to_owned()),
        tags: vec!["a".to_owned()],
        attempts: 1,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

fn round_trip<T>(value: &T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(*value, back);
}

#[test]
fn interaction_dtos_round_trip() {
    round_trip(&sample_message());
    round_trip(&sample_session());
    round_trip(&FetchedMessageDto {
        message_id: Some(1),
        sender_name: None,
        text: "t".to_owned(),
        role: "assistant".to_owned(),
    });
    round_trip(&ThreadFocusDto {
        operator_focus: Some("op".to_owned()),
        agent_focus: None,
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    });
    round_trip(&BootstrapOwnerDto {
        chat_id: 1,
        thread_id: 2,
    });
    round_trip(&RecordedAnswerDto {
        stage: "user_name".to_owned(),
        answer: "molt".to_owned(),
    });
    round_trip(&ClaimOwnerOutcomeDto {
        claimed: false,
        owner: Some(BootstrapOwnerDto {
            chat_id: 1,
            thread_id: 0,
        }),
    });
    round_trip(&RecordQuestionIssueOutcomeDto::OutOfOrder {
        expected: Some("vibe".to_owned()),
    });
    round_trip(&RecordQuestionIssueOutcomeDto::NotOwner { owner: None });
    round_trip(&RecordCurrentAnswerOutcomeDto::Recorded {
        stage: "nature".to_owned(),
        next_stage: Some("vibe".to_owned()),
    });
    round_trip(
        &RecordCurrentAnswerOutcomeDto::SourceMessageNotAfterQuestion {
            stage: "emoji".to_owned(),
        },
    );
    round_trip(&RecordAnswerOutcomeDto::SourceMessageAlreadyUsed);
}

#[test]
fn interaction_requests_round_trip() {
    round_trip(&ArchiveMessageRequest {
        agent: "a".to_owned(),
        request_id: "r-1".to_owned(),
        message: sample_message(),
    });
    round_trip(&ArchiveMessageResponse {
        id: 10,
        inserted: true,
    });
    round_trip(&MarkMessageRoutedRequest {
        agent: "a".to_owned(),
        platform: "telegram".to_owned(),
        chat_id: 1,
        thread_id: 0,
        message_id: 5,
        root_session_id: "r".to_owned(),
        turn_id: 2,
    });
    round_trip(&GetActiveSessionRequest {
        agent: "a".to_owned(),
        chat_id: 1,
        thread_id: 0,
    });
    round_trip(&GetActiveSessionResponse {
        session: Some(sample_session()),
    });
    round_trip(&CreateSessionRequest {
        agent: "a".to_owned(),
        chat_id: 1,
        thread_id: 0,
        session_uuid: "u".to_owned(),
        label: None,
    });
    round_trip(&CreateSessionResponse { session_id: 3 });
    round_trip(&DeactivateCurrentSessionResponse {
        previous_root_session_id: Some("r".to_owned()),
    });
    round_trip(&ActivateSessionRequest {
        agent: "a".to_owned(),
        session_id: 1,
    });
    round_trip(&ListSessionsResponse {
        sessions: vec![sample_session()],
    });
    round_trip(&FindSessionsByUuidRequest {
        agent: "a".to_owned(),
        chat_id: 1,
        thread_id: 0,
        uuid_prefix: "ab".to_owned(),
    });
    round_trip(&FindSessionByRootResponse { session: None });
    round_trip(&BoolResultResponse { result: true });
    round_trip(&IsRecentRoutedTargetRequest {
        agent: "a".to_owned(),
        platform: "telegram".to_owned(),
        chat_id: 1,
        thread_id: 0,
        message_id: 5,
        root_session_id: "r".to_owned(),
        window_secs: 30,
        current_turn_id: 9,
    });
    round_trip(&FetchMessagesByIdsRequest {
        agent: "a".to_owned(),
        platform: "telegram".to_owned(),
        chat_id: 1,
        thread_id: 0,
        message_ids: vec![1, 2],
    });
    round_trip(&ConversationLatestTurnIdResponse { turn_id: Some(7) });
    round_trip(&ThreadFocusSetOperatorRequest {
        agent: "a".to_owned(),
        chat_id: 1,
        thread_id: 0,
        value: None,
    });
    round_trip(&ErrorDetailInsertRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        chat_id: 1,
        thread_id: 0,
        raw_json: "{}".to_owned(),
        created_at_unix: 1000,
    });
    round_trip(&ErrorDetailGetResponse {
        raw_json: Some("{}".to_owned()),
    });
    round_trip(&LifecycleBumpUseManyRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        skill_names: vec!["s".to_owned()],
        now_utc: "2026-01-01T00:00:00Z".to_owned(),
    });
    round_trip(&BootstrapClaimOwnerResponse {
        outcome: ClaimOwnerOutcomeDto {
            claimed: true,
            owner: None,
        },
    });
    round_trip(&BootstrapRecordCurrentAnswerResponse {
        outcome: RecordCurrentAnswerOutcomeDto::QuestionNotIssued {
            stage: "vibe".to_owned(),
        },
    });
    round_trip(&BootstrapRecordedAnswersResponse {
        answers: vec![RecordedAnswerDto {
            stage: "emoji".to_owned(),
            answer: "🦀".to_owned(),
        }],
    });
    round_trip(&BootstrapClearResponse { cleared: 4 });
    round_trip(&OkResponse {});
}

#[test]
fn run_ledger_dtos_round_trip() {
    round_trip(&sample_spec());
    round_trip(&sample_run_row());
    round_trip(&sample_pending());
    round_trip(&CronSpecDetailDto {
        spec: sample_spec(),
        recent_runs: vec![sample_run_row()],
        linked_skills: vec!["right-x".to_owned()],
    });
    round_trip(&CronSpecsListResponse {
        specs: vec![sample_spec()],
    });
    round_trip(&EnqueueBackgroundRunRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        run_id: "run-1".to_owned(),
        producer_ref: Some("cron_then".to_owned()),
        source_session_id: "src".to_owned(),
        run_session_id: "run-1".to_owned(),
        target_chat_id: -100,
        target_thread_id: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    });
    round_trip(&CronInsertRunningRunRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        run_id: "run-1".to_owned(),
        job_name: "daily".to_owned(),
        started_at: "2026-01-01T00:00:00Z".to_owned(),
        log_path: "/log".to_owned(),
        target_chat_id: Some(-100),
        target_thread_id: None,
        force_notify: true,
    });
    round_trip(&PersistRunOutputRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        run_id: "run-1".to_owned(),
        run_note: Some("n".to_owned()),
        delivery_json: Some("{}".to_owned()),
        error_json: None,
        delivery_required: true,
        exit_code: Some(0),
        status: "success".to_owned(),
    });
    round_trip(&PersistRunOutputResponse {
        delivery_status: "pending".to_owned(),
    });
    round_trip(&FinishRunRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        run_id: "run-1".to_owned(),
        exit_code: None,
        status: "failed".to_owned(),
    });
    round_trip(&MarkHandoffFailedRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        run_id: "run-1".to_owned(),
        run_note: "n".to_owned(),
        delivery_json: "{}".to_owned(),
        error_json: "{}".to_owned(),
    });
    round_trip(&RecoveredCountResponse { recovered: 2 });
    round_trip(&DeliveryFetchPendingResponse {
        pending: vec![sample_pending()],
    });
    round_trip(&DeliveryDeduplicateJobResponse {
        candidate: Some(sample_pending()),
        superseded: 3,
    });
}

#[test]
fn learning_dtos_round_trip() {
    round_trip(&sample_usage());
    round_trip(&UsageSourceDto::Interactive {
        chat_id: 1,
        thread_id: 0,
    });
    round_trip(&UsageSourceDto::LearningCurator);
    round_trip(&UsageSourceDto::ReflectionCron {
        job_name: "j".to_owned(),
    });
    round_trip(&UsageInsertEventRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        source: UsageSourceDto::Cron {
            job_name: "j".to_owned(),
        },
        event: sample_usage(),
    });
    round_trip(&LearningEventDto {
        invocation_id: "inv".to_owned(),
        action: "create".to_owned(),
        skill_name: "rightx-a".to_owned(),
        phase: "finish".to_owned(),
        status: Some("created".to_owned()),
        hint_outcome: None,
        reason: None,
        message: None,
        summary: Some("s".to_owned()),
        event_refs: vec![],
    });
    round_trip(&BaselineMetricDto::Available {
        p50: 1.0,
        p90: 2.0,
        p99: 3.0,
    });
    round_trip(&BaselineMetricDto::Insufficient { sample_size: 2 });
    round_trip(&TurnBaselinesDto {
        sample_size: 10,
        elapsed_sample_size: 8,
        window_days: 7,
        cost_usd: BaselineMetricDto::Available {
            p50: 0.1,
            p90: 0.2,
            p99: 0.3,
        },
        num_turns: BaselineMetricDto::Insufficient { sample_size: 1 },
        wall_elapsed_ms: BaselineMetricDto::Available {
            p50: 100.0,
            p90: 200.0,
            p99: 300.0,
        },
    });
    round_trip(&CostSpikeEvidenceDto {
        today_cost_usd: 5.0,
        baseline_p50_usd: 0.5,
        k: 8.0,
        min_floor_usd: 2.0,
    });
    round_trip(&CuratorStateDto {
        last_run_at: Some("2026-01-01T00:00:00Z".to_owned()),
        last_run_status: Some("success".to_owned()),
        consecutive_failures: 0,
        circuit_open_until: None,
        last_spike_evidence_json: None,
    });
    round_trip(&CuratorRunRecordDto {
        run_at: "2026-01-01T00:00:00Z".to_owned(),
        trigger: "time_fallback".to_owned(),
        trigger_evidence_json: None,
        mode: "apply".to_owned(),
        status: "success".to_owned(),
        cost_usd: 0.1,
        cache_read: 0,
        cache_creation: 0,
        consolidations: 1,
        archives: 2,
        summary: None,
        actions_json: "[]".to_owned(),
        invocation_id: None,
    });
    round_trip(&SkillLifecycleDto {
        skill_name: "rightx-a".to_owned(),
        state: "active".to_owned(),
        pinned: true,
        created_by: "probe_writer".to_owned(),
        use_count: 5,
        patch_count: 1,
        created_at: None,
        last_used_at: None,
        last_patched_at: None,
        archived_at: None,
        absorbed_into: None,
    });
    round_trip(&SkillSpendDto {
        skill_name: "rightx-a".to_owned(),
        kind: "maintain".to_owned(),
        cost_usd: 0.01,
        cache_read: 10,
        cache_creation: 5,
        invocation_id: Some("inv".to_owned()),
    });
    round_trip(&sample_retain());
    round_trip(&RetainEnqueueItemDto {
        source: "user".to_owned(),
        content: "c".to_owned(),
        context: None,
        document_id: None,
        update_mode: None,
        tags: vec![],
    });
    round_trip(&RetainClaimDto {
        claim_token: "tok".to_owned(),
        lease_expires_at: "2026-01-01T00:05:00Z".to_owned(),
        items: vec![sample_retain()],
    });
    round_trip(&RetainQueueStatsResponse {
        count: 3,
        oldest_age_secs: Some(60),
    });
    round_trip(&AlertCheckAndRecordResponse { should_fire: true });
}

const SECRET: &str = "sk-ant-oat01-supersecretvalue";

#[test]
fn secret_dtos_expose_value_only_in_json_body() {
    // AuthTokenGetResponse
    let resp = AuthTokenGetResponse {
        token: Some(SecretString::from(SECRET)),
    };
    let debug = format!("{resp:?}");
    assert!(!debug.contains(SECRET), "Debug must redact: {debug}");
    assert!(debug.contains("<redacted>"));
    let json = serde_json::to_string(&resp).expect("serialize");
    assert!(json.contains(SECRET), "UDS body must carry the value");
    let back: AuthTokenGetResponse = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.token.as_ref().unwrap().expose_secret(), SECRET);

    // AuthTokenSaveRequest
    let req = AuthTokenSaveRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        token: SecretString::from(SECRET),
    };
    assert!(!format!("{req:?}").contains(SECRET));
    assert!(serde_json::to_string(&req).unwrap().contains(SECRET));

    // NoticeTokenGetOrCreateResponse
    let notice = NoticeTokenGetOrCreateResponse {
        token: SecretString::from(SECRET),
    };
    assert!(!format!("{notice:?}").contains(SECRET));
    assert!(serde_json::to_string(&notice).unwrap().contains(SECRET));

    // McpOauthStateSetRequest
    let oauth = McpOauthStateSetRequest {
        agent: "a".to_owned(),
        request_id: "r".to_owned(),
        server_name: "s".to_owned(),
        access_token: SecretString::from(SECRET),
        refresh_token: Some(SecretString::from("refresh-secret")),
        token_endpoint: "https://example.com/token".to_owned(),
        client_id: "cid".to_owned(),
        client_secret: None,
        expires_at: "2026-01-01T00:00:00Z".to_owned(),
        oauth_resource: "https://example.com".to_owned(),
    };
    let debug = format!("{oauth:?}");
    assert!(!debug.contains(SECRET));
    assert!(!debug.contains("refresh-secret"));
    let json = serde_json::to_string(&oauth).unwrap();
    assert!(json.contains(SECRET));
    assert!(json.contains("refresh-secret"));
    let back: McpOauthStateSetRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.refresh_token.as_ref().unwrap().expose_secret(),
        "refresh-secret"
    );

    // SecretBindingDto
    let binding = SecretBindingDto {
        provider: "openai".to_owned(),
        env_var: "OPENAI_API_KEY".to_owned(),
        source_env_var: "RIGHT_PROVIDERS_OPENAI".to_owned(),
        placeholder: "$MSB_OPENAI_API_KEY".to_owned(),
        allowed_hosts: vec!["api.openai.com".to_owned()],
        inject_query: false,
        value: SecretString::from(SECRET),
    };
    assert!(!format!("{binding:?}").contains(SECRET));
    let json = serde_json::to_string(&binding).unwrap();
    assert!(json.contains(SECRET));
    let back: SecretBindingDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back.value.expose_secret(), SECRET);

    // Provider resolution requests/responses
    let resolve = ResolveProviderBindingsRequest {
        agent: "a".to_owned(),
        auth: SecretString::from("auth-token"),
    };
    assert!(!format!("{resolve:?}").contains("auth-token"));
    let named = ResolveNamedProviderBindingResponse { binding };
    assert!(!format!("{named:?}").contains(SECRET));
}

#[test]
fn none_secret_option_serializes_as_null() {
    let resp = AuthTokenGetResponse { token: None };
    let json = serde_json::to_string(&resp).unwrap();
    assert_eq!(json, r#"{"token":null}"#);
    let back: AuthTokenGetResponse = serde_json::from_str(&json).unwrap();
    assert!(back.token.is_none());
}

#[test]
fn error_category_round_trip_snake_case() {
    for (category, text) in [
        (DbErrorCategory::Unavailable, "unavailable"),
        (DbErrorCategory::NotReady, "not_ready"),
        (DbErrorCategory::NotFound, "not_found"),
        (DbErrorCategory::Conflict, "conflict"),
        (DbErrorCategory::Transient, "transient"),
        (DbErrorCategory::Invalid, "invalid"),
        (DbErrorCategory::Internal, "internal"),
    ] {
        let json = serde_json::to_string(&category).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        let back: DbErrorCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, category);
    }
}

#[test]
fn classify_parses_typed_server_error() {
    let body = serde_json::to_string(&DbErrorResponse {
        category: DbErrorCategory::Conflict,
        message: "stale claim token".to_owned(),
    })
    .unwrap();
    let err = classify_transport_error(InternalClientError::Server { status: 409, body });
    match err {
        InternalDbError::Server {
            category,
            status,
            message,
        } => {
            assert_eq!(category, DbErrorCategory::Conflict);
            assert_eq!(status, 409);
            assert_eq!(message, "stale claim token");
        }
        other => panic!("expected typed server error, got {other:?}"),
    }
}

#[test]
fn classify_unparseable_body_is_internal_and_truncated() {
    let body = "x".repeat(ERROR_BODY_MAX_CHARS * 3);
    let err = classify_transport_error(InternalClientError::Server { status: 500, body });
    match err {
        InternalDbError::Server {
            category,
            status,
            message,
        } => {
            assert_eq!(category, DbErrorCategory::Internal);
            assert_eq!(status, 500);
            assert_eq!(message.chars().count(), ERROR_BODY_MAX_CHARS);
        }
        other => panic!("expected internal server error, got {other:?}"),
    }
}

#[test]
fn classify_transport_passthrough() {
    let err = classify_transport_error(InternalClientError::Http("boom".to_owned()));
    assert!(matches!(err, InternalDbError::Transport(_)));
}

#[test]
fn provider_binding_token_is_stable_and_label_scoped() {
    use base64::Engine as _;
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 32]);
    let t1 = provider_binding_token(&secret).unwrap();
    let t2 = provider_binding_token(&secret).unwrap();
    assert_eq!(t1, t2);
    assert_eq!(t1.len(), 43);
    let other = crate::derive_token(&secret, "right-mcp").unwrap();
    assert_ne!(t1, other);
}

/// The wire contract must never expose SQL escape hatches: no `sql`, `table`,
/// `params`, `row`, `rows`, `operation`, or `query` fields anywhere in any
/// request/response body.
#[test]
fn wire_contract_has_no_raw_database_fields() {
    const FORBIDDEN: [&str; 6] = ["sql", "table", "params", "row", "operation", "query"];

    fn assert_clean(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    assert!(
                        !FORBIDDEN.contains(&key.as_str()),
                        "forbidden field `{key}` at {path}"
                    );
                    assert_clean(child, &format!("{path}.{key}"));
                }
            }
            serde_json::Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    assert_clean(child, &format!("{path}[{i}]"));
                }
            }
            _ => {}
        }
    }

    let samples: Vec<serde_json::Value> = vec![
        serde_json::to_value(ArchiveMessageRequest {
            agent: "a".to_owned(),
            request_id: "r".to_owned(),
            message: sample_message(),
        })
        .unwrap(),
        serde_json::to_value(ArchiveMessageResponse {
            id: 1,
            inserted: true,
        })
        .unwrap(),
        serde_json::to_value(sample_session()).unwrap(),
        serde_json::to_value(sample_spec()).unwrap(),
        serde_json::to_value(CronSpecDetailDto {
            spec: sample_spec(),
            recent_runs: vec![sample_run_row()],
            linked_skills: vec![],
        })
        .unwrap(),
        serde_json::to_value(sample_pending()).unwrap(),
        serde_json::to_value(PersistRunOutputRequest {
            agent: "a".to_owned(),
            request_id: "r".to_owned(),
            run_id: "run".to_owned(),
            run_note: None,
            delivery_json: None,
            error_json: None,
            delivery_required: false,
            exit_code: None,
            status: "success".to_owned(),
        })
        .unwrap(),
        serde_json::to_value(UsageInsertEventRequest {
            agent: "a".to_owned(),
            request_id: "r".to_owned(),
            source: UsageSourceDto::IdleCompaction {
                chat_id: 1,
                thread_id: 0,
            },
            event: sample_usage(),
        })
        .unwrap(),
        serde_json::to_value(LearningEventDto {
            invocation_id: "i".to_owned(),
            action: "update".to_owned(),
            skill_name: "s".to_owned(),
            phase: "start".to_owned(),
            status: None,
            hint_outcome: None,
            reason: None,
            message: None,
            summary: None,
            event_refs: vec![],
        })
        .unwrap(),
        serde_json::to_value(TurnBaselinesDto {
            sample_size: 0,
            elapsed_sample_size: 0,
            window_days: 7,
            cost_usd: BaselineMetricDto::Insufficient { sample_size: 0 },
            num_turns: BaselineMetricDto::Insufficient { sample_size: 0 },
            wall_elapsed_ms: BaselineMetricDto::Insufficient { sample_size: 0 },
        })
        .unwrap(),
        serde_json::to_value(CuratorStateDto::default()).unwrap(),
        serde_json::to_value(SkillLifecycleDto {
            skill_name: "s".to_owned(),
            state: "active".to_owned(),
            pinned: false,
            created_by: "foreground".to_owned(),
            use_count: 0,
            patch_count: 0,
            created_at: None,
            last_used_at: None,
            last_patched_at: None,
            archived_at: None,
            absorbed_into: None,
        })
        .unwrap(),
        serde_json::to_value(SkillSpendDto {
            skill_name: "s".to_owned(),
            kind: "usage".to_owned(),
            cost_usd: 0.0,
            cache_read: 0,
            cache_creation: 0,
            invocation_id: None,
        })
        .unwrap(),
        serde_json::to_value(RetainClaimDto {
            claim_token: "t".to_owned(),
            lease_expires_at: "x".to_owned(),
            items: vec![sample_retain()],
        })
        .unwrap(),
        serde_json::to_value(SecretBindingDto {
            provider: "p".to_owned(),
            env_var: "E".to_owned(),
            source_env_var: "S".to_owned(),
            placeholder: "P".to_owned(),
            allowed_hosts: vec![],
            inject_query: false,
            value: SecretString::from("v"),
        })
        .unwrap(),
        serde_json::to_value(DbErrorResponse {
            category: DbErrorCategory::Internal,
            message: "m".to_owned(),
        })
        .unwrap(),
    ];

    for sample in &samples {
        assert_clean(sample, "$");
    }
}

#[test]
fn all_routes_are_db_scoped_and_unique() {
    let routes = [
        ROUTE_ARCHIVE_MESSAGE,
        ROUTE_MARK_MESSAGE_ROUTED,
        ROUTE_GET_ACTIVE_SESSION,
        ROUTE_CREATE_SESSION,
        ROUTE_DEACTIVATE_CURRENT_SESSION,
        ROUTE_ACTIVATE_SESSION,
        ROUTE_TOUCH_SESSION,
        ROUTE_LIST_SESSIONS,
        ROUTE_FIND_SESSIONS_BY_UUID,
        ROUTE_FIND_SESSION_BY_ROOT,
        ROUTE_LATEST_ASSISTANT_IS_UNIQUE_EXACT,
        ROUTE_IS_RECENT_ROUTED_TARGET,
        ROUTE_FETCH_MESSAGES_BY_IDS,
        ROUTE_CONVERSATION_LATEST_TURN_ID,
        ROUTE_THREAD_FOCUS_GET,
        ROUTE_THREAD_FOCUS_SET_OPERATOR,
        ROUTE_ERROR_DETAIL_INSERT,
        ROUTE_ERROR_DETAIL_GET,
        ROUTE_LIFECYCLE_BUMP_USE_MANY,
        ROUTE_BOOTSTRAP_OWNER,
        ROUTE_BOOTSTRAP_CLAIM_OWNER,
        ROUTE_BOOTSTRAP_MISSING_STAGES,
        ROUTE_BOOTSTRAP_FIRST_MISSING_STAGE,
        ROUTE_BOOTSTRAP_ISSUED_QUESTION_STAGE,
        ROUTE_BOOTSTRAP_RECORD_QUESTION_ISSUE,
        ROUTE_BOOTSTRAP_RECORD_CURRENT_ANSWER,
        ROUTE_BOOTSTRAP_RECORD_ANSWER,
        ROUTE_BOOTSTRAP_RECORDED_ANSWERS,
        ROUTE_BOOTSTRAP_CLEAR,
        ROUTE_CRON_SPECS_LIST,
        ROUTE_CRON_SPEC_DETAIL,
        ROUTE_CRON_RECENT_RUNS,
        ROUTE_CRON_DELETE_SPEC,
        ROUTE_ENQUEUE_BACKGROUND_RUN,
        ROUTE_CRON_INSERT_RUNNING_RUN,
        ROUTE_MARK_BACKGROUND_SPAWNED,
        ROUTE_PERSIST_RUN_OUTPUT,
        ROUTE_FINISH_RUN,
        ROUTE_MARK_HANDOFF_FAILED,
        ROUTE_RECOVER_INTERRUPTED_HANDOFFS,
        ROUTE_CRON_MARK_INTERRUPTED_BY_SHUTDOWN,
        ROUTE_DELIVERY_FETCH_PENDING,
        ROUTE_DELIVERY_MARK_OUTCOME,
        ROUTE_DELIVERY_DEDUPLICATE_JOB,
        ROUTE_AUTH_STATUS,
        ROUTE_AUTH_TOKEN_GET,
        ROUTE_AUTH_TOKEN_SAVE,
        ROUTE_AUTH_TOKEN_DELETE,
        ROUTE_NOTICE_TOKEN_GET_OR_CREATE,
        ROUTE_MCP_OAUTH_STATE_SET,
        ROUTE_USAGE_INSERT_EVENT,
        ROUTE_LEARNING_EVENT_INSERT,
        ROUTE_LEARNING_TODAY_SPEND,
        ROUTE_LEARNING_RECORD_BUDGET_SKIP,
        ROUTE_LEARNING_AUTHORED_SKILL_THIS_TURN,
        ROUTE_LEARNING_LINK_CRON_AUTHORED,
        ROUTE_LEARNING_LATEST_INTERACTIVE_CONTEXT_TOKENS,
        ROUTE_LEARNING_TURN_BASELINES,
        ROUTE_LEARNING_PROBE_COST_SPIKE,
        ROUTE_CURATOR_LOAD_STATE,
        ROUTE_CURATOR_SAVE_STATE,
        ROUTE_CURATOR_INSERT_RUN,
        ROUTE_CURATOR_LATEST_CHAT_ACTIVITY,
        ROUTE_LIFECYCLE_ARCHIVED_SINCE,
        ROUTE_SKILL_LIFECYCLE_GET,
        ROUTE_SKILL_LIFECYCLE_LIST,
        ROUTE_SKILL_PIN,
        ROUTE_SKILL_SPEND_RECORD,
        ROUTE_SKILL_SPEND_BY_SKILL,
        ROUTE_ALERT_CHECK_AND_RECORD,
        ROUTE_ALERT_RECORD,
        ROUTE_RETAIN_ENQUEUE,
        ROUTE_RETAIN_CLAIM_BATCH,
        ROUTE_RETAIN_ACK,
        ROUTE_RETAIN_NACK,
        ROUTE_RETAIN_QUEUE_STATS,
        ROUTE_DASHBOARD_ACTIVITY,
        ROUTE_DASHBOARD_RUN_DETAIL,
        ROUTE_DASHBOARD_OVERVIEW,
        ROUTE_DASHBOARD_USAGE,
        ROUTE_DASHBOARD_LEARNING,
        ROUTE_DASHBOARD_SKILL_LIFECYCLE,
        ROUTE_DASHBOARD_SKILL_SPEND,
        ROUTE_PROVIDER_BINDINGS_RESOLVE,
        ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED,
    ];
    let mut seen = std::collections::HashSet::new();
    for route in routes {
        assert!(route.starts_with("/db/"), "route not db-scoped: {route}");
        assert!(seen.insert(route), "duplicate route: {route}");
    }
}
