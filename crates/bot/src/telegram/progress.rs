use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use dashmap::DashMap;
use right_mcp::internal_client::{
    PROGRESS_MESSAGE_MAX_CHARS, ProgressSendRequest, ProgressSendResponse,
};
use serde::Serialize;
use subtle::ConstantTimeEq;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, MessageId, ThreadId};

/// End-to-end timeout for the Telegram `send_message` call invoked by the
/// progress UDS endpoint. Bounds how long the caller (aggregator's
/// `call_send_progress`) can wait if Telegram stalls. Mirrors the aggregator's
/// own timeout — both ends must bound independently.
const PROGRESS_SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub(crate) struct ProgressState {
    targets: Arc<DashMap<String, ProgressTarget>>,
}

impl ProgressState {
    pub(crate) fn register(&self, target: ProgressTarget) {
        self.targets.insert(target.invocation_id.clone(), target);
    }

    pub(crate) fn unregister(&self, invocation_id: &str) {
        self.targets.remove(invocation_id);
    }

    pub(crate) fn get(&self, invocation_id: &str) -> Option<ProgressTarget> {
        self.targets
            .get(invocation_id)
            .map(|entry| entry.value().clone())
    }
}

#[derive(Clone)]
pub(crate) struct ProgressTarget {
    pub(crate) invocation_id: String,
    pub(crate) token: String,
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
}

impl std::fmt::Debug for ProgressTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressTarget")
            .field("invocation_id", &self.invocation_id)
            .field("chat_id", &self.chat_id)
            .field("thread_id", &self.thread_id)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl ProgressTarget {
    pub(crate) fn token_matches(&self, token: &str) -> bool {
        self.token.len() == token.len() && self.token.as_bytes().ct_eq(token.as_bytes()).into()
    }
}

#[derive(Clone)]
pub(crate) struct ProgressEndpointState {
    pub(crate) bot: teloxide::Bot,
    pub(crate) progress: ProgressState,
}

#[derive(Serialize)]
struct ProgressErrorResponse {
    error: String,
}

pub(crate) fn build_progress_router(state: ProgressEndpointState) -> Router {
    Router::new()
        .route("/progress/send", post(handle_progress_send))
        .with_state(state)
}

async fn handle_progress_send(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ProgressSendRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ProgressErrorResponse {
                error: "progress invocation not found".to_owned(),
            }),
        )
            .into_response();
    };
    if !target.token_matches(&req.token) {
        return (
            StatusCode::FORBIDDEN,
            Json(ProgressErrorResponse {
                error: "progress token mismatch".to_owned(),
            }),
        )
            .into_response();
    }

    let message = req.message.trim();
    if message.is_empty() || message.chars().count() > PROGRESS_MESSAGE_MAX_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(ProgressErrorResponse {
                error: format!(
                    "progress message must be non-empty and at most {PROGRESS_MESSAGE_MAX_CHARS} characters",
                ),
            }),
        )
            .into_response();
    }

    let mut send = state.bot.send_message(ChatId(target.chat_id), message);
    if target.thread_id != 0 {
        send = send.message_thread_id(ThreadId(MessageId(target.thread_id as i32)));
    }

    // Scrub Telegram error details before they surface to the agent. The
    // teloxide error description can include chat IDs and message ids; the
    // agent only needs to learn that the send failed, plus enough category
    // info (`telegram_send_failed` vs `telegram_send_timeout`) to react.
    // Full error is logged on the bot side via `tracing::warn!`.
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send).await {
        Ok(Ok(message)) => (
            StatusCode::OK,
            Json(ProgressSendResponse {
                ok: true,
                message_id: Some(message.id.0),
            }),
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "telegram send failed: {e:#}",
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(ProgressErrorResponse {
                    error: "telegram_send_failed".to_owned(),
                }),
            )
                .into_response()
        }
        Err(_) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "telegram send timed out after {}s",
                PROGRESS_SEND_TIMEOUT.as_secs(),
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ProgressErrorResponse {
                    error: "telegram_send_timeout".to_owned(),
                }),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn progress_state_register_get_unregister_roundtrip() {
        let state = ProgressState::default();
        let target = ProgressTarget {
            invocation_id: "inv-1".to_owned(),
            token: "secret-token".to_owned(),
            chat_id: 42,
            thread_id: 7,
        };

        state.register(target.clone());

        let stored = state.get("inv-1").expect("target should be registered");
        assert_eq!(stored.invocation_id, "inv-1");
        assert_eq!(stored.chat_id, 42);
        assert_eq!(stored.thread_id, 7);

        state.unregister("inv-1");
        assert!(state.get("inv-1").is_none());
    }

    #[tokio::test]
    async fn progress_target_debug_redacts_token() {
        let target = ProgressTarget {
            invocation_id: "inv-1".to_owned(),
            token: "supersecret".to_owned(),
            chat_id: 42,
            thread_id: 7,
        };
        let s = format!("{target:?}");
        assert!(!s.contains("supersecret"), "Debug must redact token: {s}");
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[tokio::test]
    async fn progress_target_token_matches() {
        let target = ProgressTarget {
            invocation_id: "inv-1".to_owned(),
            token: "secret-token".to_owned(),
            chat_id: 42,
            thread_id: 0,
        };

        assert!(target.token_matches("secret-token"));
        assert!(!target.token_matches("wrong-token"));
    }
}
