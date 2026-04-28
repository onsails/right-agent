//! Telegram webhook update listener and router.
//!
//! Wraps `teloxide::update_listeners::webhooks::axum_no_setup` to expose:
//! - an `UpdateListener` for the dispatcher
//! - the inner `axum::Router` mounted at `/` (caller nests under `/tg/<agent>`)
//! - the explicit `AllowedUpdate` set used in `setWebhook`
//!
//! Secret-token enforcement is delegated to teloxide; the router returns 401
//! when `X-Telegram-Bot-Api-Secret-Token` is missing or wrong.

use std::convert::Infallible;
use std::future::Future;

use teloxide::update_listeners::{
    UpdateListener,
    webhooks::{Options, axum_no_setup},
};
use url::Url;

/// The set of update types we accept on the webhook. Explicit, not "all".
/// Add new variants here when the handler graph starts processing a new kind.
pub fn webhook_allowed_updates() -> Vec<teloxide::types::AllowedUpdate> {
    use teloxide::types::AllowedUpdate;
    vec![
        AllowedUpdate::Message,
        AllowedUpdate::EditedMessage,
        AllowedUpdate::CallbackQuery,
    ]
}

/// Build the per-agent webhook router for mounting on the bot's UDS axum app.
///
/// Returns:
///   - an `UpdateListener` for `Dispatcher::dispatch_with_listener(...)`
///   - a future that resolves on stop (drives shutdown)
///   - the `axum::Router` mounted at `/` — the caller nests it under
///     `/tg/<agent_name>` on the outer app, so the public path is
///     `/tg/<agent>/`.
///
/// The `webhook_url` is informational at this point — `setWebhook` is called
/// elsewhere with the same URL + secret. `Options::address` is unused by
/// `axum_no_setup`; we pass a dummy SocketAddr to satisfy the type.
///
/// IMPORTANT: we explicitly set `.path("/".to_string())` because
/// `Options::new` defaults the path to `url.path()` — if `webhook_url`
/// is `https://host/tg/agent/`, the default would be `/tg/agent/`, which
/// when nested under `/tg/<agent>` produces a doubled path. Setting the
/// inner path to `/` keeps the nesting clean.
pub fn build_webhook_router(
    secret: String,
    webhook_url: Url,
) -> (
    impl UpdateListener<Err = Infallible>,
    impl Future<Output = ()> + Send,
    axum::Router,
) {
    let options = Options::new(([127, 0, 0, 1], 0).into(), webhook_url)
        .path("/".to_string())
        .secret_token(secret);
    axum_no_setup(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderValue, Request, StatusCode};
    use teloxide::types::AllowedUpdate;
    use tower::ServiceExt as _;

    fn dummy_url() -> Url {
        Url::parse("https://example.com/tg/test/").unwrap()
    }

    #[test]
    fn allowed_updates_lists_message_edited_callback() {
        let allowed = webhook_allowed_updates();
        assert!(allowed.contains(&AllowedUpdate::Message));
        assert!(allowed.contains(&AllowedUpdate::EditedMessage));
        assert!(allowed.contains(&AllowedUpdate::CallbackQuery));
    }

    #[tokio::test]
    async fn webhook_router_rejects_missing_secret_header() {
        let (_listener, _stop, router) =
            build_webhook_router("the-secret".to_string(), dummy_url());
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
        let (_listener, _stop, router) =
            build_webhook_router("the-secret".to_string(), dummy_url());
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "X-Telegram-Bot-Api-Secret-Token",
                HeaderValue::from_static("wrong-secret"),
            )
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
