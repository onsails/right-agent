//! Dashboard route tests restored after the single-owner DB cutover.
//!
//! The former inline module seeded `data.db` directly and then exercised bot
//! routes that also opened the file. Dashboard DB projections and mutations
//! are now typed owner IPC. These tests therefore split the same observable
//! contracts across the proper boundary:
//!
//! * pure/auth/static/sandbox tests still exercise the dashboard router here;
//! * bot → owner routing tests use `FakeInternalApi` and assert typed request
//!   DTOs plus HTTP status mapping;
//! * real owner → SQL semantics and populated projection assertions live in
//!   `crates/right/src/internal_api_db_tests.rs` and the focused
//!   `right-dashboard::read_model` tests. No bot test opens `data.db`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use dashmap::DashMap;
use hmac::{Hmac, KeyInit as _, Mac as _};
use right_agent::agent::allowlist::{AllowedUser, AllowlistFile, AllowlistHandle, AllowlistState};
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt as _;

use right_mcp::internal_db as wire;

use crate::test_support::{FakeInternalApi, db_error, ok, sync_handler};

const BOT_TOKEN: &str = "123456:test-token";

struct DashboardFixture {
    _home: tempfile::TempDir,
    agent_dir: std::path::PathBuf,
    fake: FakeInternalApi,
}

impl DashboardFixture {
    fn start(handler: crate::test_support::Handler) -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let agent_dir = home.path().join("agents").join("alpha");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let run_dir = home.path().join("run");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let fake = FakeInternalApi::start_at(run_dir.join("internal.sock"), handler);
        Self {
            _home: home,
            agent_dir,
            fake,
        }
    }

    fn state(&self) -> super::DashboardState {
        test_state(
            self.agent_dir.clone(),
            Arc::new(right_mcp::internal_client::InternalClient::new(
                self.fake.socket_path(),
            )),
        )
    }
}

fn test_state(
    agent_dir: std::path::PathBuf,
    internal_client: Arc<right_mcp::internal_client::InternalClient>,
) -> super::DashboardState {
    let added_at = "2026-05-20T12:00:00Z".parse().expect("valid UTC timestamp");
    let allowlist = AllowlistHandle::new(AllowlistState::from_file(AllowlistFile {
        version: 1,
        users: vec![AllowedUser {
            id: 42,
            label: None,
            added_by: None,
            added_at,
        }],
        groups: Vec::new(),
    }));

    super::DashboardState {
        agent_name: "alpha".to_string(),
        bot_token: BOT_TOKEN.to_string(),
        focus_notifier: super::FocusNotifier::noop(),
        home: agent_dir.clone(),
        agent_dir: agent_dir.clone(),
        sandbox_name: "right-test-agent".to_owned(),
        sandbox_runtime: {
            let (handle, _rx) = crate::sandbox_runtime::SandboxRuntimeHandle::new(Err(Arc::new(
                right_sandbox::SandboxCause::HypervisorUnavailable.diagnose(),
            )));
            handle
        },
        allowlist,
        foreground: Arc::new(DashMap::new()),
        internal_client,
        provider_bindings: None,
        provider_mutation: Arc::new(tokio::sync::Mutex::new(())),
        provider_config: Arc::new(arc_swap::ArcSwap::from_pointee(
            right_agent::agent::types::AgentConfig {
                allowed_chat_ids: vec![],
                telegram_token: None,
                restart: Default::default(),
                max_restarts: 3,
                backoff_seconds: 5,
                model: None,
                debug: None,
                sandbox: None,
                env: Default::default(),
                secret: None,
                attachments: Default::default(),
                network_policy: Default::default(),
                show_thinking: true,
                learning: Default::default(),
                memory: None,
                stt: Default::default(),
            },
        )),
        pending_auth: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        oauth_status: super::super::oauth_status::OAuthFlowStatusStore::default(),
        mcp_oauth_allow_private_urls: false,
        doctor_checks: Some(vec![
            right_agent::doctor::DoctorCheck {
                name: "right".to_string(),
                status: right_agent::doctor::CheckStatus::Pass,
                detail: "found".to_string(),
                fix: None,
            },
            right_agent::doctor::DoctorCheck {
                name: right_agent::doctor::SANDBOX_RUNTIME_CHECK.to_string(),
                status: right_agent::doctor::CheckStatus::Warn,
                detail: "not running".to_string(),
                fix: Some("start the runtime".to_string()),
            },
        ]),
    }
}

fn disconnected_state(agent_dir: std::path::PathBuf) -> super::DashboardState {
    test_state(
        agent_dir.clone(),
        Arc::new(right_mcp::internal_client::InternalClient::new(
            agent_dir.join("missing-internal.sock"),
        )),
    )
}

fn signed_init_data(user_id: i64) -> String {
    signed_init_data_at(user_id, chrono::Utc::now().timestamp())
}

fn signed_init_data_at(user_id: i64, auth_date: i64) -> String {
    let user = json!({
        "id": user_id,
        "username": "tester",
        "first_name": "Test"
    })
    .to_string();
    let mut fields = vec![
        ("auth_date".to_string(), auth_date.to_string()),
        ("query_id".to_string(), "AAEAAAE".to_string()),
        ("user".to_string(), user),
    ];
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    let data_check_string = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let secret_key = Hmac::<Sha256>::new_from_slice(b"WebAppData")
        .expect("HMAC accepts key")
        .chain_update(BOT_TOKEN.as_bytes())
        .finalize()
        .into_bytes();
    let hash = Hmac::<Sha256>::new_from_slice(&secret_key)
        .expect("HMAC accepts key")
        .chain_update(data_check_string.as_bytes())
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in fields {
        serializer.append_pair(&key, &value);
    }
    serializer.append_pair("hash", &hash).finish()
}

fn signed_focus_scope_token(
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
    expires_unix: i64,
) -> String {
    super::focus_scope_token_for_expires(BOT_TOKEN, agent_name, chat_id, thread_id, expires_unix)
}

async fn request(
    state: super::DashboardState,
    method: &str,
    path: &str,
    auth: Option<String>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let router = super::build_dashboard_router(state);
    let mut builder = Request::builder().uri(path).method(method);
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
    }
    let body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = router
        .oneshot(builder.body(body).expect("valid request"))
        .await
        .expect("router response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read response body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[test]
fn focus_scope_token_valid_rejects_invalid_scope_and_token_shapes() {
    let expires_unix = chrono::Utc::now().timestamp() + 60;
    let token = signed_focus_scope_token("alpha", 7, 11, expires_unix);
    assert!(super::focus_scope_token_valid(
        BOT_TOKEN, "alpha", 7, 11, &token
    ));
    assert!(!super::focus_scope_token_valid(
        BOT_TOKEN, "beta", 7, 11, &token
    ));
    assert!(!super::focus_scope_token_valid(
        BOT_TOKEN, "alpha", 8, 11, &token
    ));
    assert!(!super::focus_scope_token_valid(
        BOT_TOKEN, "alpha", 7, 12, &token
    ));
    assert!(!super::focus_scope_token_valid(
        BOT_TOKEN,
        "alpha",
        7,
        11,
        "malformed"
    ));
    assert!(!super::focus_scope_token_valid(
        BOT_TOKEN,
        "alpha",
        7,
        11,
        &signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() - 1)
    ));
}

#[test]
fn dashboard_url_normalizes_and_rejects_non_bare_hosts() {
    let url = super::dashboard_url("https://right.example.com/", "alpha").unwrap();
    assert_eq!(url.as_str(), "https://right.example.com/dashboard/alpha/");
    let url = super::dashboard_url("right.example.com", "bot-one").unwrap();
    assert_eq!(url.path(), "/dashboard/bot-one/");

    for invalid in [
        "right.example.com/some-path",
        "https://right.example.com/extra",
        "right.example.com/?token=abc",
        "right.example.com/#frag",
        "user@right.example.com",
    ] {
        assert!(
            super::dashboard_url(invalid, "alpha").is_err(),
            "{invalid} must be rejected"
        );
    }
}

#[tokio::test]
async fn static_index_loads_without_auth() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (status, _) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn bootstrap_exposes_learning_capabilities() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/bootstrap",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["features"]["learning_metrics"], true);
    assert_eq!(body["features"]["learning_evidence_snippets"], false);
    assert_eq!(body["features"]["learning_commands"], false);
    assert_eq!(body["features"]["commands_enabled"], false);
    assert_eq!(body["features"]["activity"], true);
    assert_eq!(body["features"]["knowledge_skills"], true);
    assert_eq!(body["features"]["usage"], true);
}

#[tokio::test]
async fn api_authentication_order_and_errors_are_preserved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = || disconnected_state(temp.path().to_path_buf());

    let (status, body) = request(
        state(),
        "GET",
        "/dashboard/alpha/api/v1/bootstrap",
        Some(String::new()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    assert_eq!(body["detail"], "missing Telegram Mini App authorization");

    let expired = chrono::Utc::now().timestamp() - super::INIT_DATA_MAX_AGE_SECS - 1;
    let (status, body) = request(
        state(),
        "GET",
        "/dashboard/alpha/api/v1/bootstrap",
        Some(signed_init_data_at(42, expired)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["detail"].as_str().unwrap().contains("reopen"));

    let (status, _) = request(
        state(),
        "GET",
        "/dashboard/beta/api/v1/bootstrap",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = request(
        state(),
        "GET",
        "/dashboard/beta/api/v1/bootstrap",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        state(),
        "GET",
        "/dashboard/alpha/api/v1/bootstrap",
        Some(signed_init_data(7)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

type FocusPair = (Option<String>, Option<String>);
type FocusFixtureState = Arc<Mutex<FocusPair>>;

fn focus_fixture(
    initial_operator: Option<&str>,
    agent_focus: Option<&str>,
) -> (DashboardFixture, FocusFixtureState) {
    let focus = Arc::new(Mutex::new((
        initial_operator.map(str::to_owned),
        agent_focus.map(str::to_owned),
    )));
    let state = Arc::clone(&focus);
    let fixture = DashboardFixture::start(sync_handler(move |route, body| match route {
        wire::ROUTE_THREAD_FOCUS_GET => {
            let (operator_focus, agent_focus) = state.lock().expect("focus state").clone();
            ok(wire::ThreadFocusGetResponse {
                focus: (operator_focus.is_some() || agent_focus.is_some()).then_some(
                    wire::ThreadFocusDto {
                        operator_focus,
                        agent_focus,
                        updated_at: "2026-05-20T12:00:00Z".to_owned(),
                    },
                ),
            })
        }
        wire::ROUTE_THREAD_FOCUS_SET_OPERATOR => {
            state.lock().expect("focus state").0 = body["value"].as_str().map(str::to_owned);
            ok(wire::OkResponse {})
        }
        other => panic!("unexpected focus route {other}"),
    }));
    (fixture, focus)
}

#[tokio::test]
async fn dashboard_focus_patch_trims_clears_and_preserves_agent_focus() {
    let (fixture, focus) = focus_fixture(None, Some("agent-managed focus"));
    let auth = signed_init_data(42);
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        fixture.state(),
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(auth.clone()),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token.clone(),
            "operator_focus": "  operator focus  ",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["operator_focus"], "operator focus");
    assert_eq!(
        *focus.lock().expect("focus state"),
        (
            Some("operator focus".to_owned()),
            Some("agent-managed focus".to_owned())
        )
    );

    let (status, body) = request(
        fixture.state(),
        "GET",
        &format!("/dashboard/alpha/api/v1/focus?chat_id=7&thread_id=11&token={token}"),
        Some(auth.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": "operator focus" }));

    let (status, body) = request(
        fixture.state(),
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(auth),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": " \n\t ",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": null }));
    assert_eq!(
        *focus.lock().expect("focus state"),
        (None, Some("agent-managed focus".to_owned()))
    );
}

#[tokio::test]
async fn focus_scope_token_prevents_cross_chat_reads_and_writes_before_ipc() {
    let (fixture, focus) = focus_fixture(Some("other chat focus"), None);
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        fixture.state(),
        "GET",
        &format!("/dashboard/alpha/api/v1/focus?chat_id=8&thread_id=11&token={token}"),
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "invalid_focus_scope");

    let (status, body) = request(
        fixture.state(),
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        Some(json!({
            "chat_id": 8,
            "thread_id": 11,
            "token": token,
            "operator_focus": "tampered focus",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "invalid_focus_scope");
    assert_eq!(
        *focus.lock().expect("focus state"),
        (Some("other chat focus".to_owned()), None)
    );
    assert!(fixture.fake.recorded().await.is_empty());
}

#[tokio::test]
async fn dashboard_focus_patch_rejects_overlong_operator_focus_and_accepts_boundary() {
    let (fixture, focus) = focus_fixture(None, None);
    let auth = signed_init_data(42);
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        fixture.state(),
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(auth.clone()),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token.clone(),
            "operator_focus": "x".repeat(super::focus::OPERATOR_FOCUS_MAX_CHARS + 1),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "focus_too_long");
    assert_eq!(*focus.lock().expect("focus state"), (None, None));

    let at_cap = "x".repeat(super::focus::OPERATOR_FOCUS_MAX_CHARS);
    let (status, _) = request(
        fixture.state(),
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(auth),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": at_cap.clone(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        focus.lock().expect("focus state").0.as_deref(),
        Some(at_cap.as_str())
    );
}

#[tokio::test]
async fn dashboard_focus_patch_sends_notification_to_scope() {
    let (fixture, _) = focus_fixture(None, None);
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut state = fixture.state();
    state.focus_notifier = super::FocusNotifier::capture(Arc::clone(&sent));
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        state,
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": "  operator focus  ",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": "operator focus" }));
    assert_eq!(
        sent.lock().await.clone(),
        vec![super::FocusNotification {
            chat_id: 7,
            thread_id: 11,
            text: "Focus set: operator focus".to_string(),
        }]
    );
}

#[tokio::test]
async fn dashboard_focus_patch_sends_clear_notification_to_scope() {
    let (fixture, _) = focus_fixture(Some("old focus"), None);
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut state = fixture.state();
    state.focus_notifier = super::FocusNotifier::capture(Arc::clone(&sent));
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        state,
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": " \n\t ",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "operator_focus": null }));
    assert_eq!(
        sent.lock().await.clone(),
        vec![super::FocusNotification {
            chat_id: 7,
            thread_id: 11,
            text: "Focus cleared".to_string(),
        }]
    );
}

#[tokio::test]
async fn dashboard_focus_patch_reports_notification_failure_after_saving() {
    let (fixture, focus) = focus_fixture(None, None);
    let mut state = fixture.state();
    state.focus_notifier = super::FocusNotifier::fail("telegram unavailable");
    let token = signed_focus_scope_token("alpha", 7, 11, chrono::Utc::now().timestamp() + 60);

    let (status, body) = request(
        state,
        "PATCH",
        "/dashboard/alpha/api/v1/focus",
        Some(signed_init_data(42)),
        Some(json!({
            "chat_id": 7,
            "thread_id": 11,
            "token": token,
            "operator_focus": "operator focus",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"], "focus_notification_failed");
    assert_eq!(
        body["detail"],
        "Focus saved, but notification could not be sent"
    );
    assert_eq!(
        focus.lock().expect("focus state").0.as_deref(),
        Some("operator focus")
    );
}

#[tokio::test]
async fn overview_owner_unavailable_returns_500_without_creating_db() {
    // Semantic mapping: the old test asserted a missing `data.db` was not
    // created by a dashboard read. The bot no longer opens it under any
    // circumstances; this test proves the owner transport error is surfaced
    // and the local path remains untouched.
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("data.db");
    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/overview",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "overview_failed");
    assert!(
        !db_path.exists(),
        "dashboard IPC read must not create data.db"
    );
}

#[tokio::test]
async fn activity_and_learning_owner_errors_map_to_route_errors() {
    let fixture = DashboardFixture::start(sync_handler(|_route, _body| {
        db_error(503, wire::DbErrorCategory::Unavailable)
    }));
    for (path, expected) in [
        (
            "/dashboard/alpha/api/v1/activity/overview",
            "overview_failed",
        ),
        (
            "/dashboard/alpha/api/v1/knowledge/learning/overview",
            "learning_overview_failed",
        ),
        ("/dashboard/alpha/api/v1/usage", "usage_failed"),
    ] {
        let (status, body) = request(
            fixture.state(),
            "GET",
            path,
            Some(signed_init_data(42)),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert_eq!(body["error"], expected, "{path}");
    }
}

#[tokio::test]
async fn activity_run_detail_returns_not_found_for_unknown_run() {
    let fixture = DashboardFixture::start(sync_handler(|route, body| {
        assert_eq!(route, wire::ROUTE_DASHBOARD_RUN_DETAIL);
        assert_eq!(body["run_id"], "missing-run");
        ok(wire::DashboardRunDetailResponse { detail: None })
    }));
    let (status, body) = request(
        fixture.state(),
        "GET",
        "/dashboard/alpha/api/v1/activity/runs/missing-run",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    assert_eq!(body["detail"], "run not found");
}

#[tokio::test]
async fn delete_cron_uses_typed_owner_and_is_idempotent_404() {
    // Semantic mapping: the old test seeded `cron_specs` directly, then read
    // the row count after deletion. The bot's observable contract is route
    // auth plus one typed delete request and NotFound on replay. The real row
    // deletion is exercised in the owner API tests.
    let exists = Arc::new(Mutex::new(true));
    let state = Arc::clone(&exists);
    let fixture = DashboardFixture::start(sync_handler(move |route, body| {
        assert_eq!(route, wire::ROUTE_CRON_DELETE_SPEC);
        assert_eq!(body["job_name"], "daily");
        let mut exists = state.lock().expect("cron state");
        if *exists {
            *exists = false;
            ok(wire::OkResponse {})
        } else {
            db_error(404, wire::DbErrorCategory::NotFound)
        }
    }));

    let (status, _) = request(
        fixture.state(),
        "DELETE",
        "/dashboard/alpha/api/v1/crons/daily",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let auth = signed_init_data(42);
    let (status, _) = request(
        fixture.state(),
        "DELETE",
        "/dashboard/alpha/api/v1/crons/daily",
        Some(auth.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!*exists.lock().expect("cron state"));

    let (status, _) = request(
        fixture.state(),
        "DELETE",
        "/dashboard/alpha/api/v1/crons/daily",
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn skill_routes_authenticate_before_body_or_owner_work() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "PATCH",
        "/dashboard/alpha/api/v1/knowledge/skills/rightx-oauth-debugging/pin",
        None,
        Some(json!({ "pinned": true })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/knowledge/skills/..secret",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_skill_name");
}

#[tokio::test]
async fn sandboxless_routes_report_explicit_unavailability() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();
    std::fs::write(temp.path().join("SOUL.md"), "# Soul\n").unwrap();

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/identity",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "sandbox_unreachable");

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/knowledge/skills",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "skills_failed");
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("sandbox_unreachable")
    );
    assert!(body.get("groups").is_none());

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/health/sandbox",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "unavailable");
    assert!(body["disk"].is_null());
    assert!(body["memory"].is_null());
}

#[tokio::test]
async fn identity_file_and_doctor_routes_keep_host_metadata_contracts() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("IDENTITY.md"), "# Identity\n").unwrap();

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/identity/IDENTITY.md",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["file"]["name"], "IDENTITY.md");
    assert_eq!(body["file"]["content_preview"], "# Identity\n");

    let (status, body) = request(
        disconnected_state(temp.path().to_path_buf()),
        "GET",
        "/dashboard/alpha/api/v1/health/doctor",
        Some(signed_init_data(42)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent"], "alpha");
    assert!(body["pass"].is_array());
    assert!(body["warn"].is_array());
    assert!(body["fail"].is_array());
}

#[tokio::test]
async fn dashboard_state_resolves_the_sandbox_per_request_not_at_construction() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = disconnected_state(temp.path().to_path_buf());
    assert_eq!(state.sandbox_runtime.sandbox_reads(), 0);
    assert!(state.sandbox().is_none());
    assert!(super::skills::skills_response(&state).await.is_err());
    assert!(super::identity::identity_response(&state).await.is_ok());
    assert_eq!(
        state.sandbox_runtime.sandbox_reads(),
        3,
        "every guest-touching path resolves the live handle when it runs"
    );
}

#[test]
fn apply_sandbox_diagnosis_overrides_gateway_check_when_unavailable() {
    use right_agent::doctor::{CheckStatus, DoctorCheck};
    let mut checks = vec![
        DoctorCheck {
            name: right_agent::doctor::SANDBOX_RUNTIME_CHECK.into(),
            status: CheckStatus::Pass,
            detail: "ok".into(),
            fix: None,
        },
        DoctorCheck {
            name: "tunnel".into(),
            status: CheckStatus::Pass,
            detail: "ok".into(),
            fix: None,
        },
    ];
    let health = crate::sandbox_runtime::SandboxHealth::Unavailable {
        diagnosis: Arc::new(right_sandbox::SandboxCause::HypervisorUnavailable.diagnose()),
    };
    super::apply_sandbox_diagnosis(&mut checks, &health);
    let gateway = checks
        .iter()
        .find(|check| check.name == right_agent::doctor::SANDBOX_RUNTIME_CHECK)
        .unwrap();
    assert!(matches!(gateway.status, CheckStatus::Fail));
    assert!(gateway.detail.to_lowercase().contains("microvm"));
    let fix = gateway.fix.as_deref().unwrap().to_lowercase();
    assert!(fix.contains("apple silicon") || fix.contains("kvm"));
    assert!(matches!(
        checks
            .iter()
            .find(|check| check.name == "tunnel")
            .unwrap()
            .status,
        CheckStatus::Pass
    ));
}

#[test]
fn apply_sandbox_diagnosis_noop_when_ready() {
    use right_agent::doctor::{CheckStatus, DoctorCheck};
    let mut checks = vec![DoctorCheck {
        name: right_agent::doctor::SANDBOX_RUNTIME_CHECK.into(),
        status: CheckStatus::Pass,
        detail: "host can run microVMs".into(),
        fix: None,
    }];
    super::apply_sandbox_diagnosis(&mut checks, &crate::sandbox_runtime::SandboxHealth::Ready);
    assert!(matches!(checks[0].status, CheckStatus::Pass));
}
