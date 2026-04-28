//! Integration tests for the per-agent webhook router.
//!
//! Exercises `right_bot::telegram::webhook::build_webhook_router` directly:
//! - 401 on missing `X-Telegram-Bot-Api-Secret-Token` header.
//! - 401 on wrong secret value.
//! - 200 + UpdateListener emits the parsed Update on correct secret.
//!
//! These tests do not start a full bot binary or hit live Telegram; they
//! exercise the in-process axum router via `tower::ServiceExt::oneshot`.

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use right_bot::telegram::webhook::build_webhook_router;
use serde_json::json;
use tower::ServiceExt as _;
use url::Url;

fn dummy_url() -> Url {
    Url::parse("https://example.com/tg/test/").unwrap()
}

/// A minimal Update JSON that teloxide's webhook handler will accept and parse.
fn fake_update() -> serde_json::Value {
    json!({
        "update_id": 1,
        "message": {
            "message_id": 1,
            "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "test"},
            "from": {"id": 1, "is_bot": false, "first_name": "test"},
            "text": "hello"
        }
    })
}

#[tokio::test]
async fn webhook_router_401_on_missing_secret() {
    let (_listener, _stop, router) = build_webhook_router("the-secret".to_string(), dummy_url());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("Content-Type", "application/json")
        .body(Body::from(fake_update().to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_router_401_on_wrong_secret() {
    let (_listener, _stop, router) = build_webhook_router("the-secret".to_string(), dummy_url());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("Content-Type", "application/json")
        .header(
            "X-Telegram-Bot-Api-Secret-Token",
            HeaderValue::from_static("wrong"),
        )
        .body(Body::from(fake_update().to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_router_200_on_correct_secret_emits_update() {
    use futures::StreamExt as _;
    use teloxide::update_listeners::AsUpdateStream as _;

    let (mut listener, _stop, router) =
        build_webhook_router("the-secret".to_string(), dummy_url());

    // Spawn the POST through the router on a separate task so we can pull from
    // the listener stream in parallel — the handler awaits the channel send,
    // which only completes once a consumer is reading.
    let router_clone = router.clone();
    let post_task = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("Content-Type", "application/json")
            .header(
                "X-Telegram-Bot-Api-Secret-Token",
                HeaderValue::from_static("the-secret"),
            )
            .body(Body::from(fake_update().to_string()))
            .unwrap();
        router_clone.oneshot(req).await.unwrap()
    });

    let stream = listener.as_stream();
    tokio::pin!(stream);
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await;

    let resp = post_task.await.unwrap();
    assert!(
        resp.status().is_success(),
        "expected 2xx, got {}",
        resp.status()
    );
    let received = received
        .expect("listener didn't yield within 2s")
        .expect("listener stream ended");
    let update = received.expect("listener error");
    assert_eq!(update.id.0, 1);
}
