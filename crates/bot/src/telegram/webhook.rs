//! Telegram webhook HTTP handler.
//!
//! Replaces the former teloxide `axum_no_setup` update listener. Exposes an
//! `axum::Router` mounted at `/` (the caller nests it under `/tg/<agent>` on the
//! bot's UDS app) that authenticates the `X-Telegram-Bot-Api-Secret-Token`
//! header, parses a `frankenstein::Update` from the body, and routes it via
//! [`router::route_update`].

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;

use super::router::{self, HandlerCtx, WebhookOutcome};

const SECRET_HEADER: &str = "X-Telegram-Bot-Api-Secret-Token";

/// The set of update types we accept on the webhook. Explicit, not "all".
/// Add new variants here when the handler graph starts processing a new kind.
pub fn webhook_allowed_updates() -> Vec<frankenstein::types::AllowedUpdate> {
    use frankenstein::types::AllowedUpdate;
    vec![
        AllowedUpdate::Message,
        AllowedUpdate::EditedMessage,
        AllowedUpdate::CallbackQuery,
    ]
}

#[derive(Clone)]
struct WState {
    secret: String,
    ctx: Arc<HandlerCtx>,
}

/// Build the per-agent webhook router for mounting on the bot's UDS axum app.
///
/// The router serves a single POST `/`; the caller nests it under
/// `/tg/<agent_name>` so the public path is `/tg/<agent>`.
pub(crate) fn build_webhook_router(secret: String, ctx: Arc<HandlerCtx>) -> Router {
    Router::new()
        .route("/", post(handle))
        .with_state(WState { secret, ctx })
}

async fn handle(State(st): State<WState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let provided = headers.get(SECRET_HEADER).and_then(|v| v.to_str().ok());
    let parsed = serde_json::from_slice::<frankenstein::updates::Update>(&body);
    match router::webhook_outcome(provided, &st.secret, parsed.is_ok()) {
        WebhookOutcome::Unauthorized => StatusCode::UNAUTHORIZED,
        WebhookOutcome::AckIgnore => {
            tracing::warn!("webhook: unparseable update body, acking to stop Telegram retries");
            StatusCode::OK
        }
        WebhookOutcome::Routed => {
            // `parsed` is Ok here (body_parses was true). Route best-effort: a
            // single failed update must never propagate out of the handler.
            if let Ok(update) = parsed {
                router::route_update(update, &st.ctx).await;
            }
            StatusCode::OK
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderValue, Request};
    use frankenstein::types::AllowedUpdate;
    use tower::ServiceExt as _;

    /// Build a router whose state has a placeholder ctx. The secret-rejection
    /// paths short-circuit before `route_update` is reached, so an unconnected
    /// bot is fine for these tests.
    fn test_router(secret: &str) -> Router {
        let ctx = Arc::new(super::super::router::test_support::placeholder_ctx());
        build_webhook_router(secret.to_string(), ctx)
    }

    #[tokio::test]
    async fn allowed_updates_lists_message_edited_callback() {
        let allowed = webhook_allowed_updates();
        assert!(allowed.contains(&AllowedUpdate::Message));
        assert!(allowed.contains(&AllowedUpdate::EditedMessage));
        assert!(allowed.contains(&AllowedUpdate::CallbackQuery));
    }

    #[tokio::test]
    async fn webhook_router_rejects_missing_secret_header() {
        let router = test_router("the-secret");
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_router_rejects_wrong_secret_header() {
        let router = test_router("the-secret");
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header(SECRET_HEADER, HeaderValue::from_static("wrong-secret"))
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_router_acks_correct_secret_with_unparseable_body() {
        // Correct secret + body that is not a valid Update → 200 (AckIgnore),
        // short-circuiting before any routing. `"{}"` lacks `update_id`.
        let router = test_router("the-secret");
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header(SECRET_HEADER, HeaderValue::from_static("the-secret"))
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// A correct secret + a valid Update body is accepted (200) and routed. The
    /// placeholder ctx's handlers are no-ops for an unauthorized DM sender, so
    /// `route_update` returns without side effects; we assert the HTTP contract.
    ///
    /// (Adaptation note: the former teloxide integration test observed the
    /// emitted Update via an `UpdateListener` stream. The frankenstein router
    /// has no listener — it routes inline — so we assert the 200 + that a valid
    /// body is accepted, matching the test's spirit.)
    #[tokio::test]
    async fn webhook_router_200_on_correct_secret_and_valid_body() {
        let router = test_router("the-secret");
        let body = serde_json::json!({
            "update_id": 1,
            "message": {
                "message_id": 1,
                "date": 0,
                "chat": {"id": 1, "type": "private", "first_name": "test"},
                "from": {"id": 1, "is_bot": false, "first_name": "test"},
                "text": "hello"
            }
        })
        .to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header(SECRET_HEADER, HeaderValue::from_static("the-secret"))
            .body(Body::from(body))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Regression: the bot UDS server nests the webhook router under
    /// `/tg/<agent>`; axum 0.8 matches the no-trailing-slash form against the
    /// inner `/` route. A nested POST must reach the inner handler (401 here on
    /// secret mismatch), not 404.
    #[tokio::test]
    async fn nested_webhook_router_routes_no_trailing_slash() {
        let inner = test_router("the-secret");
        let outer = Router::new().nest("/tg/test", inner);
        let request = Request::builder()
            .method("POST")
            .uri("/tg/test")
            .header(SECRET_HEADER, HeaderValue::from_static("wrong"))
            .body(Body::from("{}"))
            .unwrap();
        let response = outer.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "expected nested router to reach inner handler at /tg/<agent>"
        );
    }
}
