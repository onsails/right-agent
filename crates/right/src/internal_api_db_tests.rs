use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use secrecy::SecretString;
use serde::Serialize;
use tower::ServiceExt as _;

use right_mcp::internal_db as wire;

async fn test_app(tmp: &std::path::Path) -> axum::Router {
    let agents_dir = tmp.join("agents");
    let agent_dir = agents_dir.join("alpha");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "secret: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
    )
    .unwrap();

    let owners =
        crate::db_owner::DbOwnerRegistry::open_initial([("alpha".to_owned(), agent_dir.clone())])
            .await
            .unwrap();
    owners.bundle("alpha").await.unwrap().publish();
    let dispatcher = Arc::new(crate::aggregator::ToolDispatcher {
        agents: dashmap::DashMap::new(),
    });
    let token_map = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    crate::internal_api::internal_router(crate::internal_api::InternalRouterDeps {
        dispatcher,
        refresh_senders: Arc::new(dashmap::DashMap::new()),
        reconnect_managers: Arc::new(dashmap::DashMap::new()),
        token_map,
        db_owners: owners,
        token_map_path: tmp.join("agent-tokens.json"),
        agents_dir,
        providers: crate::internal_api::open_provider_store(tmp).await.unwrap(),
    })
}

async fn post<T: Serialize>(app: axum::Router, path: &str, value: &T) -> (StatusCode, Vec<u8>) {
    let response = app
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(value).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

fn message() -> wire::ConversationMessageDto {
    wire::ConversationMessageDto {
        platform: "telegram".to_owned(),
        chat_id: 7,
        thread_id: 0,
        message_id: Some(42),
        sender_user_id: Some(9),
        sender_name: Some("Alice".to_owned()),
        addressed_to_bot: true,
        routed_to_agent: false,
        root_session_id: None,
        turn_id: None,
        role: "user".to_owned(),
        content: "hello".to_owned(),
    }
}

#[tokio::test]
async fn internal_api_db_archive_retry_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let request = wire::ArchiveMessageRequest {
        agent: "alpha".to_owned(),
        request_id: "archive-1".to_owned(),
        message: message(),
    };
    let (status, first) = post(app.clone(), wire::ROUTE_ARCHIVE_MESSAGE, &request).await;
    assert_eq!(status, StatusCode::OK);
    let first: wire::ArchiveMessageResponse = serde_json::from_slice(&first).unwrap();
    let (status, second) = post(app, wire::ROUTE_ARCHIVE_MESSAGE, &request).await;
    assert_eq!(status, StatusCode::OK);
    let second: wire::ArchiveMessageResponse = serde_json::from_slice(&second).unwrap();
    assert_eq!(first, second);
    assert!(first.inserted);

    let conn = right_db::open_connection(&tmp.path().join("agents/alpha"), false)
        .await
        .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM conversation_messages", (), |row| {
            row.get(0)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn internal_api_db_session_activation_is_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let create = |uuid: &str| wire::CreateSessionRequest {
        agent: "alpha".to_owned(),
        chat_id: 7,
        thread_id: 0,
        session_uuid: uuid.to_owned(),
        label: None,
    };
    let (_, first) = post(app.clone(), wire::ROUTE_CREATE_SESSION, &create("one")).await;
    let first: wire::CreateSessionResponse = serde_json::from_slice(&first).unwrap();
    let deactivate = wire::DeactivateCurrentSessionRequest {
        agent: "alpha".to_owned(),
        chat_id: 7,
        thread_id: 0,
    };
    assert_eq!(
        post(
            app.clone(),
            wire::ROUTE_DEACTIVATE_CURRENT_SESSION,
            &deactivate
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, second) = post(app.clone(), wire::ROUTE_CREATE_SESSION, &create("two")).await;
    let second: wire::CreateSessionResponse = serde_json::from_slice(&second).unwrap();
    assert_ne!(first.session_id, second.session_id);

    let activate = wire::ActivateSessionRequest {
        agent: "alpha".to_owned(),
        session_id: first.session_id,
    };
    assert_eq!(
        post(app.clone(), wire::ROUTE_ACTIVATE_SESSION, &activate)
            .await
            .0,
        StatusCode::OK
    );
    let list = wire::ListSessionsRequest {
        agent: "alpha".to_owned(),
        chat_id: 7,
        thread_id: 0,
    };
    let (_, body) = post(app, wire::ROUTE_LIST_SESSIONS, &list).await;
    let rows: wire::ListSessionsResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(rows.sessions.iter().filter(|s| s.is_active).count(), 1);
    assert!(
        rows.sessions
            .iter()
            .find(|s| s.id == first.session_id)
            .unwrap()
            .is_active
    );
}

#[tokio::test]
async fn internal_api_db_persist_run_output_retry_is_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let enqueue = wire::EnqueueBackgroundRunRequest {
        agent: "alpha".to_owned(),
        request_id: "enqueue-1".to_owned(),
        run_id: "run-1".to_owned(),
        producer_ref: None,
        source_session_id: "source".to_owned(),
        run_session_id: "run-1".to_owned(),
        target_chat_id: 7,
        target_thread_id: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    assert_eq!(
        post(app.clone(), wire::ROUTE_ENQUEUE_BACKGROUND_RUN, &enqueue)
            .await
            .0,
        StatusCode::OK
    );
    let finish = wire::PersistRunOutputRequest {
        agent: "alpha".to_owned(),
        request_id: "finish-1".to_owned(),
        run_id: "run-1".to_owned(),
        run_note: Some("done".to_owned()),
        delivery_json: Some("{}".to_owned()),
        error_json: None,
        delivery_required: true,
        exit_code: Some(0),
        status: "success".to_owned(),
    };
    let first = post(app.clone(), wire::ROUTE_PERSIST_RUN_OUTPUT, &finish).await;
    let second = post(app, wire::ROUTE_PERSIST_RUN_OUTPUT, &finish).await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert_eq!(first.1, second.1);

    let conn = right_db::open_connection(&tmp.path().join("agents/alpha"), false)
        .await
        .unwrap();
    let row: (String, String, String) = conn
        .query_row(
            "SELECT status, run_note, delivery_status FROM async_runs WHERE id = 'run-1'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(
        row,
        (
            "success".to_owned(),
            "done".to_owned(),
            "pending".to_owned()
        )
    );
}

#[tokio::test]
async fn internal_api_db_retain_leases_reclaim_and_reject_stale_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let enqueue = wire::RetainEnqueueRequest {
        agent: "alpha".to_owned(),
        request_id: "retain-1".to_owned(),
        item: wire::RetainEnqueueItemDto {
            source: "test".to_owned(),
            content: "remember".to_owned(),
            context: None,
            document_id: None,
            update_mode: None,
            tags: vec![],
        },
    };
    assert_eq!(
        post(app.clone(), wire::ROUTE_RETAIN_ENQUEUE, &enqueue)
            .await
            .0,
        StatusCode::OK
    );
    let claim = wire::RetainClaimBatchRequest {
        agent: "alpha".to_owned(),
        limit: 10,
        lease_ttl_secs: 1,
    };
    let (_, first) = post(app.clone(), wire::ROUTE_RETAIN_CLAIM_BATCH, &claim).await;
    let first: wire::RetainClaimBatchResponse = serde_json::from_slice(&first).unwrap();
    assert_eq!(first.claim.items.len(), 1);
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (_, second) = post(app.clone(), wire::ROUTE_RETAIN_CLAIM_BATCH, &claim).await;
    let second: wire::RetainClaimBatchResponse = serde_json::from_slice(&second).unwrap();
    assert_eq!(second.claim.items.len(), 1);
    assert_ne!(first.claim.claim_token, second.claim.claim_token);

    let stale = wire::RetainAckRequest {
        agent: "alpha".to_owned(),
        claim_token: first.claim.claim_token,
        ids: vec![first.claim.items[0].id],
    };
    assert_eq!(
        post(app.clone(), wire::ROUTE_RETAIN_ACK, &stale).await.0,
        StatusCode::CONFLICT
    );
    let fresh = wire::RetainAckRequest {
        agent: "alpha".to_owned(),
        claim_token: second.claim.claim_token,
        ids: vec![second.claim.items[0].id],
    };
    assert_eq!(
        post(app, wire::ROUTE_RETAIN_ACK, &fresh).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn internal_api_db_auth_token_round_trip_is_redacted_on_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let secret = "sk-ant-oat01-never-log-this";
    let save = wire::AuthTokenSaveRequest {
        agent: "alpha".to_owned(),
        request_id: "auth-1".to_owned(),
        token: SecretString::from(secret),
    };
    assert_eq!(
        post(app.clone(), wire::ROUTE_AUTH_TOKEN_SAVE, &save)
            .await
            .0,
        StatusCode::OK
    );
    let get = wire::AuthTokenGetRequest {
        agent: "alpha".to_owned(),
    };
    let (status, body) = post(app, wire::ROUTE_AUTH_TOKEN_GET, &get).await;
    assert_eq!(status, StatusCode::OK);
    let response: wire::AuthTokenGetResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(response.token.as_ref().unwrap()),
        secret
    );
    assert!(!format!("{response:?}").contains(secret));
}

#[tokio::test]
async fn internal_api_db_unknown_and_malformed_routes_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let unknown = app
        .clone()
        .oneshot(Request::post("/db/unknown").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let malformed = app
        .oneshot(
            Request::post(wire::ROUTE_AUTH_STATUS)
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn internal_api_db_provider_binding_requires_correct_hmac_and_redacts_debug() {
    use right_providers::{Credential, NewProvider, ProviderKind};
    use secrecy::ExposeSecret as _;

    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;
    let store = right_providers::ProviderStore::open(tmp.path())
        .await
        .unwrap();
    store
        .create(
            NewProvider {
                owner_agent: "alpha".to_owned(),
                name: "openai-main".to_owned(),
                kind: ProviderKind::Builtin("openai".to_owned()),
                label: "OpenAI".to_owned(),
            },
            Credential::from("provider-super-secret".to_owned()),
        )
        .await
        .unwrap();

    let wrong = wire::ResolveNamedProviderBindingRequest {
        agent: "alpha".to_owned(),
        provider: "openai-main".to_owned(),
        auth: SecretString::from("wrong"),
    };
    assert_eq!(
        post(
            app.clone(),
            wire::ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED,
            &wrong
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let token =
        wire::provider_binding_token("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let request = wire::ResolveNamedProviderBindingRequest {
        agent: "alpha".to_owned(),
        provider: "openai-main".to_owned(),
        auth: SecretString::from(token),
    };
    let (status, body) = post(app, wire::ROUTE_PROVIDER_BINDINGS_RESOLVE_NAMED, &request).await;
    assert_eq!(status, StatusCode::OK);
    let response: wire::ResolveNamedProviderBindingResponse =
        serde_json::from_slice(&body).unwrap();
    assert_eq!(
        response.binding.value.expose_secret(),
        "provider-super-secret"
    );
    assert!(!format!("{response:?}").contains("provider-super-secret"));
}

#[tokio::test]
async fn internal_api_db_delivery_dedup_preserves_forced_group_bypass() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app(tmp.path()).await;

    let insert =
        |run_id: &str, started_at: &str, force_notify: bool| wire::CronInsertRunningRunRequest {
            agent: "alpha".to_owned(),
            request_id: format!("insert-{run_id}"),
            run_id: run_id.to_owned(),
            job_name: "daily".to_owned(),
            started_at: started_at.to_owned(),
            log_path: format!("/{run_id}.log"),
            target_chat_id: Some(7),
            target_thread_id: None,
            force_notify,
        };
    let finish = |run_id: &str, request_id: &str| wire::PersistRunOutputRequest {
        agent: "alpha".to_owned(),
        request_id: request_id.to_owned(),
        run_id: run_id.to_owned(),
        run_note: Some(format!("note-{run_id}")),
        delivery_json: Some(format!("{{\"kind\":\"notify\",\"content\":\"{run_id}\"}}")),
        error_json: None,
        delivery_required: true,
        exit_code: Some(0),
        status: "success".to_owned(),
    };

    // The older manual run is forced; the newer scheduled run has fresher
    // content and wins the candidate selection. The candidate must still
    // carry the group-OR force flag so the user's manual verification bypass
    // is not silently held behind the bot idle gate.
    for request in [
        insert("older-forced", "2026-06-02T00:00:00Z", true),
        insert("newer-scheduled", "2026-06-02T00:05:00Z", false),
    ] {
        assert_eq!(
            post(app.clone(), wire::ROUTE_CRON_INSERT_RUNNING_RUN, &request)
                .await
                .0,
            StatusCode::OK
        );
    }
    for request in [
        finish("older-forced", "finish-older"),
        finish("newer-scheduled", "finish-newer"),
    ] {
        assert_eq!(
            post(app.clone(), wire::ROUTE_PERSIST_RUN_OUTPUT, &request)
                .await
                .0,
            StatusCode::OK
        );
    }

    let request = wire::DeliveryDeduplicateJobRequest {
        agent: "alpha".to_owned(),
        producer_ref: "daily".to_owned(),
    };
    let (status, body) = post(app, wire::ROUTE_DELIVERY_DEDUPLICATE_JOB, &request).await;
    assert_eq!(status, StatusCode::OK);
    let response: wire::DeliveryDeduplicateJobResponse = serde_json::from_slice(&body).unwrap();
    let candidate = response.candidate.expect("latest delivery candidate");
    assert_eq!(candidate.id, "newer-scheduled");
    assert!(
        candidate.force_notify,
        "a forced run's idle-gate bypass must survive dedup"
    );
    assert_eq!(response.superseded, 1);
}
