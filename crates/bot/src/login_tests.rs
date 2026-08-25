//! Tests for the token-based login flow.
//!
//! Semantic mapping: token persistence moved from in-bot `data.db` access to
//! typed owner IPC (`auth-token-get` / `auth-token-save`). The round-trip
//! tests below stand up a `FakeInternalApi` at the exact
//! `<home>/run/internal.sock` location `crate::db::client_for_agent_dir`
//! derives and keep an in-memory token store behind the real wire protocol.
//! Owner-side storage semantics (replace-on-save, redacted errors) are
//! covered in `crates/right/src/internal_api_db_tests.rs`
//! (`internal_api_db_auth_token_round_trip_is_redacted_on_errors`).

use std::sync::{Arc, Mutex};

use tempfile::tempdir;

use right_mcp::internal_db as wire;

use super::{
    INVALID_TOKEN_MESSAGE, auth_instruction_message, finish_token_request,
    is_plausible_setup_token, load_auth_token,
};
use crate::test_support::{FakeInternalApi, ok, sync_handler};

/// Home layout fixture: `<home>/agents/alpha` plus a fake owner socket at
/// `<home>/run/internal.sock` backed by an in-memory token store.
struct LoginFixture {
    _home: tempfile::TempDir,
    agent_dir: std::path::PathBuf,
    token_store: Arc<Mutex<Option<String>>>,
    fake: FakeInternalApi,
}

fn login_fixture() -> LoginFixture {
    let home = tempdir().expect("home tempdir");
    let agent_dir = home.path().join("agents").join("alpha");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    let run_dir = home.path().join("run");
    std::fs::create_dir_all(&run_dir).expect("run dir");

    let token_store: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let store = Arc::clone(&token_store);
    let fake = FakeInternalApi::start_at(
        run_dir.join("internal.sock"),
        sync_handler(move |route, body| {
            let store = Arc::clone(&store);
            match route {
                wire::ROUTE_AUTH_TOKEN_GET => {
                    let token = store.lock().expect("token store").clone();
                    ok(wire::AuthTokenGetResponse {
                        token: token.map(secrecy::SecretString::from),
                    })
                }
                wire::ROUTE_AUTH_TOKEN_SAVE => {
                    let token = body["token"].as_str().expect("token string").to_owned();
                    *store.lock().expect("token store") = Some(token);
                    ok(wire::OkResponse {})
                }
                other => panic!("unexpected route {other}"),
            }
        }),
    );
    LoginFixture {
        _home: home,
        agent_dir,
        token_store,
        fake,
    }
}

#[tokio::test]
async fn load_auth_token_returns_none_when_no_token() {
    let fixture = login_fixture();
    assert!(load_auth_token(&fixture.agent_dir).await.unwrap().is_none());
}

#[tokio::test]
async fn load_auth_token_propagates_owner_transport_failure() {
    // Semantic mapping: the old test made `data.db` unopenable and asserted
    // the open error propagated. The store is owner-side now; the equivalent
    // contract (#196) is that an unreachable owner endpoint propagates
    // instead of being mistaken for "agent has not authenticated".
    let home = tempdir().expect("home tempdir");
    let agent_dir = home.path().join("agents").join("alpha");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");

    let error = load_auth_token(&agent_dir)
        .await
        .expect_err("an unreachable token owner must fail");
    let chain = format!("{error:#}");
    assert!(
        chain.contains("query auth token through database owner"),
        "{chain}"
    );
}

#[tokio::test]
async fn load_auth_token_returns_saved_token() {
    let fixture = login_fixture();
    *fixture.token_store.lock().expect("token store") = Some("my-token".to_owned());
    assert_eq!(
        load_auth_token(&fixture.agent_dir)
            .await
            .unwrap()
            .as_deref(),
        Some("my-token")
    );
}

#[test]
fn plausible_setup_token_accepts_long_opaque_value() {
    assert!(is_plausible_setup_token(&"a".repeat(108)));
}

#[test]
fn plausible_setup_token_rejects_short_ordinary_text() {
    assert!(!is_plausible_setup_token("restart authentication"));
}

#[test]
fn plausible_setup_token_rejects_internal_whitespace() {
    let candidate = format!("{} {}", "a".repeat(53), "b".repeat(54));

    assert_eq!(candidate.len(), 108);
    assert!(!is_plausible_setup_token(&candidate));
}

#[tokio::test]
async fn locally_valid_token_is_saved() {
    let fixture = login_fixture();
    let token = "a".repeat(108);

    finish_token_request(&fixture.agent_dir, &token)
        .await
        .unwrap();

    assert_eq!(
        load_auth_token(&fixture.agent_dir)
            .await
            .unwrap()
            .as_deref(),
        Some(token.as_str())
    );
}

#[tokio::test]
async fn malformed_token_is_not_saved_or_diagnosed() {
    let fixture = login_fixture();
    let token = "secret-invalid\ntoken";

    let error = finish_token_request(&fixture.agent_dir, token)
        .await
        .expect_err("malformed candidate must be rejected");

    assert_eq!(error, INVALID_TOKEN_MESSAGE);
    assert!(!error.contains(token));
    assert!(
        fixture.fake.recorded().await.is_empty(),
        "a malformed candidate must never reach the owner"
    );
    assert!(load_auth_token(&fixture.agent_dir).await.unwrap().is_none());
}

#[tokio::test]
async fn malformed_replacement_preserves_existing_token() {
    let fixture = login_fixture();
    *fixture.token_store.lock().expect("token store") = Some("existing-token".to_owned());

    finish_token_request(&fixture.agent_dir, "restart authentication")
        .await
        .expect_err("malformed candidate must be rejected");

    assert_eq!(
        load_auth_token(&fixture.agent_dir)
            .await
            .unwrap()
            .as_deref(),
        Some("existing-token")
    );
}

#[tokio::test]
async fn auth_instruction_message_mentions_setup_token() {
    assert!(
        auth_instruction_message().contains("claude setup-token"),
        "instruction message must mention `claude setup-token`"
    );
}
