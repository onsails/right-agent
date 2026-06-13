use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use dashmap::DashMap;
use right_mcp::internal_client::{
    ForumTopicCreateRequest, ForumTopicCreateResponse, ForumTopicEditRequest, ForumTopicOkResponse,
    ForumTopicThreadRequest, PROGRESS_MESSAGE_MAX_CHARS, ProgressSendRequest, ProgressSendResponse,
};
use serde::Serialize;
use subtle::ConstantTimeEq;
use teloxide::payloads::{CreateForumTopicSetters, EditForumTopicSetters, SendMessageSetters};
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, CustomEmojiId, MessageId, ParseMode, Rgb, ThreadId};

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
    pub(crate) agent_dir: std::path::PathBuf,
    pub(crate) ssh_config_path: Option<std::path::PathBuf>,
    pub(crate) resolved_sandbox: Option<String>,
}

impl std::fmt::Debug for ProgressTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressTarget")
            .field("invocation_id", &self.invocation_id)
            .field("chat_id", &self.chat_id)
            .field("thread_id", &self.thread_id)
            .field("token", &"<redacted>")
            .field("agent_dir", &self.agent_dir)
            .field("ssh_config_path", &self.ssh_config_path)
            .field("resolved_sandbox", &self.resolved_sandbox)
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
    pub(crate) bot: super::BotType,
    pub(crate) progress: ProgressState,
}

/// Map an internal-client `MessageAttachmentDto` (received over the UDS
/// `/message/send` route) to the bot's `OutboundAttachment`, reusing the same
/// downloader/sender path as CC-produced attachments.
pub(crate) fn message_dto_to_outbound(
    dto: &right_mcp::internal_client::MessageAttachmentDto,
) -> crate::cc::attachments_dto::OutboundAttachment {
    use crate::cc::attachments_dto::OutboundKind as O;
    use right_mcp::internal_client::MessageAttachmentKind as K;
    let kind = match dto.kind {
        K::Photo => O::Photo,
        K::Document => O::Document,
        K::Video => O::Video,
        K::Audio => O::Audio,
        K::Voice => O::Voice,
        K::VideoNote => O::VideoNote,
        K::Sticker => O::Sticker,
        K::Animation => O::Animation,
    };
    crate::cc::attachments_dto::OutboundAttachment {
        kind,
        path: dto.path.clone(),
        filename: dto.filename.clone(),
        caption: dto.caption.clone(),
        media_group_id: dto.media_group_id.clone(),
    }
}

#[derive(Serialize)]
struct ProgressErrorResponse {
    error: String,
}

pub(crate) fn build_progress_router(state: ProgressEndpointState) -> Router {
    Router::new()
        .route("/progress/send", post(handle_progress_send))
        .route("/message/send", post(handle_message_send))
        .route("/forum-topic/create", post(handle_forum_topic_create))
        .route("/forum-topic/edit", post(handle_forum_topic_edit))
        .route("/forum-topic/close", post(handle_forum_topic_close))
        .route("/forum-topic/reopen", post(handle_forum_topic_reopen))
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

    // Scrub Telegram error details before they surface to the agent. The
    // teloxide error description can include chat IDs and message ids; the
    // agent only needs to learn that the send failed, plus enough category
    // info (`telegram_send_failed` vs `telegram_send_timeout`) to react.
    // Full error is logged on the bot side via `tracing::warn!`.
    let outcome = send_text_message(&state.bot, &target, message).await;

    match outcome {
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

/// Shared text-send path for the progress and message endpoints.
///
/// Renders `message` as Telegram HTML and sends it to the target chat/thread.
/// On a deterministic, pre-delivery formatting rejection (entity/URL parse
/// failure or too-long) it strips the HTML tags and retries once as plain text;
/// network/5xx/timeout errors are NOT retried because they may have been
/// delivered and retrying would double-post. Each attempt is bounded by
/// `PROGRESS_SEND_TIMEOUT`; the outer `Elapsed` distinguishes a timeout from a
/// Telegram error so callers can map them to distinct statuses.
async fn send_text_message(
    bot: &super::BotType,
    target: &ProgressTarget,
    message: &str,
) -> Result<Result<teloxide::types::Message, teloxide::RequestError>, tokio::time::error::Elapsed> {
    let html = crate::telegram::markdown::md_to_telegram_html(message);
    let thread = if target.thread_id != 0 {
        Some(ThreadId(MessageId(target.thread_id as i32)))
    } else {
        None
    };

    // First attempt: HTML parse mode.
    let mut send = bot
        .send_message(ChatId(target.chat_id), html.clone())
        .parse_mode(ParseMode::Html);
    if let Some(tid) = thread {
        send = send.message_thread_id(tid);
    }

    let outcome = tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send).await;
    match outcome {
        // Only retry on a deterministic, pre-delivery formatting rejection
        // (entity/URL parse failure or too-long). Network/5xx/timeout errors
        // may have been delivered, so retrying them would double-post.
        Ok(Err(e)) if super::attachments::is_retryable_format_error(&format!("{e}")) => {
            tracing::warn!(
                invocation_id = %target.invocation_id,
                "HTML send failed, retrying as plain text: {e:#}",
            );
            // Fallback: strip HTML tags and retry without parse mode.
            let plain = crate::telegram::markdown::strip_html_tags(&html);
            let mut send = bot.send_message(ChatId(target.chat_id), plain);
            if let Some(tid) = thread {
                send = send.message_thread_id(tid);
            }
            tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send).await
        }
        other => other,
    }
}

/// Standalone rich-message delivery: optional text content plus zero or more
/// attachments, sent to the invocation's chat/thread. Reuses `send_text_message`
/// for text and `attachments::send_attachments` for files.
async fn handle_message_send(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<right_mcp::internal_client::SendMessageRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ProgressErrorResponse {
                error: "message invocation not found".to_owned(),
            }),
        )
            .into_response();
    };
    if !target.token_matches(&req.token) {
        return (
            StatusCode::FORBIDDEN,
            Json(ProgressErrorResponse {
                error: "message token mismatch".to_owned(),
            }),
        )
            .into_response();
    }

    let mut message_ids: Vec<i32> = Vec::new();

    if let Some(content) = req
        .content
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        match send_text_message(&state.bot, &target, content).await {
            Ok(Ok(message)) => message_ids.push(message.id.0),
            Ok(Err(e)) => {
                tracing::warn!(
                    invocation_id = %req.invocation_id,
                    "message_send text failed: {e:#}",
                );
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ProgressErrorResponse {
                        error: "telegram_send_failed".to_owned(),
                    }),
                )
                    .into_response();
            }
            Err(_) => {
                tracing::warn!(
                    invocation_id = %req.invocation_id,
                    "message_send text timed out after {}s",
                    PROGRESS_SEND_TIMEOUT.as_secs(),
                );
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(ProgressErrorResponse {
                        error: "telegram_send_timeout".to_owned(),
                    }),
                )
                    .into_response();
            }
        }
    }

    if !req.attachments.is_empty() {
        let outbound: Vec<_> = req
            .attachments
            .iter()
            .map(message_dto_to_outbound)
            .collect();
        if let Err(e) = crate::telegram::attachments::send_attachments(
            &outbound,
            &state.bot,
            ChatId(target.chat_id),
            target.thread_id,
            &target.agent_dir,
            target.ssh_config_path.as_deref(),
            target.resolved_sandbox.as_deref(),
        )
        .await
        {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "message_send attachments failed: {e:#}",
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(ProgressErrorResponse {
                    error: "attachment_send_failed".to_owned(),
                }),
            )
                .into_response();
        }
    }

    (
        StatusCode::OK,
        Json(right_mcp::internal_client::SendMessageResponse {
            ok: true,
            message_ids,
        }),
    )
        .into_response()
}

/// Map a teloxide forum error description to a clear, actionable sentence for
/// the agent to relay. Falls back to the raw description.
fn forum_error_message(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("not enough rights") || lower.contains("manage topics") {
        "I don't have the \"Manage Topics\" admin right in this group.".to_owned()
    } else if lower.contains("not a forum") {
        "Forum topics exist only in forum supergroups (enable Topics in group settings).".to_owned()
    } else {
        raw.to_owned()
    }
}

async fn handle_forum_topic_create(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicCreateRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let mut call = state
        .bot
        .create_forum_topic(ChatId(target.chat_id), req.name);
    if let Some(color) = req.icon_color {
        call = call.icon_color(Rgb::from_u32(color));
    }
    if let Some(emoji) = req.icon_custom_emoji_id {
        call = call.icon_custom_emoji_id(CustomEmojiId(emoji));
    }
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(topic)) => (
            StatusCode::OK,
            Json(ForumTopicCreateResponse {
                ok: true,
                message_thread_id: topic.thread_id.0.0,
            }),
        )
            .into_response(),
        Ok(Err(e)) => forum_telegram_error(&req.invocation_id, e),
        Err(_) => forum_timeout(&req.invocation_id),
    }
}

async fn handle_forum_topic_edit(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicEditRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    let mut call = state.bot.edit_forum_topic(ChatId(target.chat_id), thread);
    if let Some(name) = req.name {
        call = call.name(name);
    }
    if let Some(emoji) = req.icon_custom_emoji_id {
        call = call.icon_custom_emoji_id(CustomEmojiId(emoji));
    }
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(_)) => forum_ok(),
        Ok(Err(e)) => forum_telegram_error(&req.invocation_id, e),
        Err(_) => forum_timeout(&req.invocation_id),
    }
}

async fn handle_forum_topic_close(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicThreadRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    let call = state.bot.close_forum_topic(ChatId(target.chat_id), thread);
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(_)) => forum_ok(),
        Ok(Err(e)) => forum_telegram_error(&req.invocation_id, e),
        Err(_) => forum_timeout(&req.invocation_id),
    }
}

async fn handle_forum_topic_reopen(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<ForumTopicThreadRequest>,
) -> axum::response::Response {
    let Some(target) = state.progress.get(&req.invocation_id) else {
        return forum_not_found();
    };
    if !target.token_matches(&req.token) {
        return forum_forbidden();
    }
    let thread = ThreadId(MessageId(req.message_thread_id));
    let call = state.bot.reopen_forum_topic(ChatId(target.chat_id), thread);
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(_)) => forum_ok(),
        Ok(Err(e)) => forum_telegram_error(&req.invocation_id, e),
        Err(_) => forum_timeout(&req.invocation_id),
    }
}

fn forum_ok() -> axum::response::Response {
    (StatusCode::OK, Json(ForumTopicOkResponse { ok: true })).into_response()
}

fn forum_not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(ProgressErrorResponse {
            error: "forum invocation not found".to_owned(),
        }),
    )
        .into_response()
}

fn forum_forbidden() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(ProgressErrorResponse {
            error: "forum token mismatch".to_owned(),
        }),
    )
        .into_response()
}

fn forum_timeout(invocation_id: &str) -> axum::response::Response {
    tracing::warn!(
        invocation_id = %invocation_id,
        "forum topic op timed out after {}s",
        PROGRESS_SEND_TIMEOUT.as_secs(),
    );
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(ProgressErrorResponse {
            error: "forum_op_timeout".to_owned(),
        }),
    )
        .into_response()
}

fn forum_telegram_error(
    invocation_id: &str,
    e: teloxide::RequestError,
) -> axum::response::Response {
    let raw = format!("{e:#}");
    tracing::warn!(invocation_id = %invocation_id, "forum topic op failed: {raw}");
    (
        StatusCode::BAD_GATEWAY,
        Json(ProgressErrorResponse {
            error: forum_error_message(&raw),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_message_attachment_dto_to_outbound() {
        use right_mcp::internal_client::{MessageAttachmentDto, MessageAttachmentKind};
        let dto = MessageAttachmentDto {
            kind: MessageAttachmentKind::Document,
            path: "/sandbox/outbox/r.csv".into(),
            filename: Some("results.csv".into()),
            caption: Some("data".into()),
            media_group_id: None,
        };
        let out = super::message_dto_to_outbound(&dto);
        assert!(matches!(
            out.kind,
            crate::cc::attachments_dto::OutboundKind::Document
        ));
        assert_eq!(out.path, "/sandbox/outbox/r.csv");
        assert_eq!(out.filename.as_deref(), Some("results.csv"));
    }

    #[test]
    fn forum_error_message_maps_known_cases() {
        assert!(
            super::forum_error_message("Bad Request: not enough rights to manage forum topics")
                .contains("Manage Topics")
        );
        assert!(
            super::forum_error_message("Bad Request: the chat is not a forum")
                .contains("forum supergroups")
        );
        assert_eq!(super::forum_error_message("weird error"), "weird error");
    }

    #[tokio::test]
    async fn progress_state_register_get_unregister_roundtrip() {
        let state = ProgressState::default();
        let target = ProgressTarget {
            invocation_id: "inv-1".to_owned(),
            token: "secret-token".to_owned(),
            chat_id: 42,
            thread_id: 7,
            agent_dir: std::path::PathBuf::from("/tmp/agent"),
            ssh_config_path: None,
            resolved_sandbox: None,
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
            agent_dir: std::path::PathBuf::from("/tmp/agent"),
            ssh_config_path: None,
            resolved_sandbox: None,
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
            agent_dir: std::path::PathBuf::from("/tmp/agent"),
            ssh_config_path: None,
            resolved_sandbox: None,
        };

        assert!(target.token_matches("secret-token"));
        assert!(!target.token_matches("wrong-token"));
    }

    /// Build a `ProgressEndpointState` with a dummy bot for router tests.
    /// `build_bot` only constructs the adaptor; it performs no network I/O, so
    /// a placeholder token is safe for the lookup/auth gates which never reach
    /// a real Telegram send.
    fn test_state(progress: ProgressState) -> ProgressEndpointState {
        ProgressEndpointState {
            bot: crate::telegram::bot::build_bot("123:test".to_owned()),
            progress,
        }
    }

    fn message_send_request_json(invocation_id: &str, token: &str) -> Vec<u8> {
        let req = right_mcp::internal_client::SendMessageRequest {
            invocation_id: invocation_id.to_owned(),
            token: token.to_owned(),
            content: Some("hi".to_owned()),
            attachments: Vec::new(),
        };
        serde_json::to_vec(&req).expect("serialize SendMessageRequest")
    }

    async fn post_message_send(state: ProgressEndpointState, body: Vec<u8>) -> StatusCode {
        use tower::ServiceExt as _;
        let app = build_progress_router(state);
        let request = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/message/send")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .expect("build request");
        let response = app.oneshot(request).await.expect("router oneshot");
        response.status()
    }

    #[tokio::test]
    async fn message_send_unknown_invocation_is_404() {
        let state = test_state(ProgressState::default());
        let body = message_send_request_json("missing", "t");
        let status = post_message_send(state, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn message_send_wrong_token_is_403() {
        let progress = ProgressState::default();
        progress.register(ProgressTarget {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: std::path::PathBuf::from("/tmp"),
            ssh_config_path: None,
            resolved_sandbox: None,
        });
        let state = test_state(progress);
        let body = message_send_request_json("inv", "wrong");
        let status = post_message_send(state, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
