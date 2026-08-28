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

use super::tg_bot::TgError;

/// End-to-end timeout for the Telegram `send_message` call invoked by the
/// progress UDS endpoint. Bounds how long the caller (aggregator's
/// `call_send_progress`) can wait if Telegram stalls. Mirrors the aggregator's
/// own timeout — both ends must bound independently.
const PROGRESS_SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Local content ceiling for a channel post, matching Telegram's 4096-char
/// message limit. Rejected before any send so an oversized post fails with a
/// deterministic local error instead of a Telegram API rejection.
const CHANNEL_POST_MAX_CHARS: usize = 4096;

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

    /// Atomically claim one `channel_post` attempt slot for `invocation_id`.
    /// Returns false when the invocation is unknown or the per-turn cap
    /// (`MAX_CHANNEL_POST_PER_TURN`) is exhausted. Attempts are never rolled
    /// back on delivery failure, mirroring the aggregator-side gate; this is
    /// the authoritative cap for any caller that reaches the bot UDS directly.
    pub(crate) fn claim_channel_post(&self, invocation_id: &str) -> bool {
        let Some(entry) = self.targets.get(invocation_id) else {
            return false;
        };
        let claimed = entry
            .channel_post_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        claimed < right_mcp::internal_client::MAX_CHANNEL_POST_PER_TURN
    }
}

#[derive(Clone)]
pub(crate) struct ProgressTarget {
    pub(crate) invocation_id: String,
    pub(crate) token: String,
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
    pub(crate) agent_dir: std::path::PathBuf,
    /// Sandbox the registering invocation runs in — the only place its
    /// outbound attachments exist. `None` once the backend has degraded: the
    /// registry outlives any single turn, and this endpoint is reachable
    /// independently of it, so the attachment path re-checks rather than
    /// assuming the handle from registration time is still there.
    pub(crate) sandbox: Option<crate::sandbox::Sandbox>,
    /// Per-turn `channel_post` attempts; capped at MAX_CHANNEL_POST_PER_TURN.
    pub(crate) channel_post_count: Arc<std::sync::atomic::AtomicU32>,
}

impl std::fmt::Debug for ProgressTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressTarget")
            .field("invocation_id", &self.invocation_id)
            .field("chat_id", &self.chat_id)
            .field("thread_id", &self.thread_id)
            .field("token", &"<redacted>")
            .field("agent_dir", &self.agent_dir)
            .field("sandbox", &self.sandbox.as_ref().map(|s| s.name()))
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
        .route("/channel/post", post(handle_channel_post))
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
                message_id: Some(message.message_id),
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
) -> Result<Result<frankenstein::types::Message, TgError>, tokio::time::error::Elapsed> {
    let html = crate::telegram::markdown::md_to_telegram_html(message);
    let thread = (target.thread_id != 0).then_some(target.thread_id as i32);

    // First attempt: HTML parse mode.
    let outcome = tokio::time::timeout(
        PROGRESS_SEND_TIMEOUT,
        bot.send_message_opts(target.chat_id, &html, true, thread, None, None),
    )
    .await;
    match outcome {
        // Only retry on a deterministic, pre-delivery formatting rejection
        // (entity/URL parse failure or too-long). Network/5xx/timeout errors
        // may have been delivered, so retrying them would double-post.
        Ok(Err(e)) if super::attachments::is_retryable_format_error(&e) => {
            tracing::warn!(
                invocation_id = %target.invocation_id,
                "HTML send failed, retrying as plain text: {e:#}",
            );
            // Fallback: strip HTML tags and retry without parse mode.
            let plain = crate::telegram::markdown::strip_html_tags(&html);
            tokio::time::timeout(
                PROGRESS_SEND_TIMEOUT,
                bot.send_message_opts(target.chat_id, &plain, false, thread, None, None),
            )
            .await
        }
        other => other,
    }
}

fn is_retryable_rich_markdown_format_error(err: &TgError) -> bool {
    let TgError::Api(frankenstein::Error::Api(response)) = err else {
        return false;
    };
    if response.error_code != 400 {
        return false;
    }
    let description = response.description.to_ascii_lowercase();
    description.contains("parse") || description.contains("too long")
}

/// Channel-only Markdown delivery through Telegram's Rich Message parser.
/// A deterministic pre-delivery formatting rejection falls back once to a
/// regular plain message containing the unchanged original Markdown. Bounded
/// 429 retries remain internal to `RightBot`; network, timeout, and 5xx failures
/// never trigger the plain fallback.
async fn send_channel_post_text(
    bot: &super::BotType,
    chat_id: i64,
    markdown: &str,
) -> Result<Result<frankenstein::types::Message, TgError>, tokio::time::error::Elapsed> {
    let outcome = tokio::time::timeout(
        PROGRESS_SEND_TIMEOUT,
        bot.send_rich_markdown(chat_id, markdown, None),
    )
    .await;
    match outcome {
        Ok(Err(e)) if is_retryable_rich_markdown_format_error(&e) => {
            tracing::warn!(
                chat_id,
                "Rich Markdown channel post failed, retrying original text as plain text: {e:#}",
            );
            tokio::time::timeout(
                PROGRESS_SEND_TIMEOUT,
                bot.send_message_opts(chat_id, markdown, false, None, None, None),
            )
            .await
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
            Ok(Ok(message)) => message_ids.push(message.message_id),
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
        // Outbound attachments live inside the guest; a degraded backend means
        // there is nothing to fetch them from. Fail closed rather than
        // silently dropping the files from an otherwise-successful send.
        let Some(sandbox) = target.sandbox.as_ref() else {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "message_send attachments refused: sandbox unavailable",
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ProgressErrorResponse {
                    error: "sandbox_unavailable".to_owned(),
                }),
            )
                .into_response();
        };
        let outbound: Vec<_> = req
            .attachments
            .iter()
            .map(message_dto_to_outbound)
            .collect();
        if let Err(e) = crate::telegram::attachments::send_attachments(
            &outbound,
            &state.bot,
            target.chat_id,
            target.thread_id,
            &target.agent_dir,
            sandbox,
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

/// Publish a Markdown post to an opened Telegram channel. The invocation
/// token and the current allowlist are both authoritative: the aggregator's
/// earlier validation is only a fast rejection before the UDS round-trip.
async fn handle_channel_post(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<right_mcp::internal_client::ChannelPostRequest>,
) -> axum::response::Response {
    let fail = |status: StatusCode, message: String| {
        (
            status,
            Json(right_mcp::internal_client::ChannelPostResponse {
                ok: false,
                message_id: None,
                error: Some(message),
            }),
        )
            .into_response()
    };

    let Some(target) = state.progress.get(&req.invocation_id) else {
        return fail(StatusCode::NOT_FOUND, "unknown invocation".to_owned());
    };
    if !target.token_matches(&req.token) {
        return fail(StatusCode::FORBIDDEN, "token mismatch".to_owned());
    }

    // Content validation before consuming an attempt slot.
    if req.text.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "empty_post".to_owned());
    }
    if req.text.chars().count() > CHANNEL_POST_MAX_CHARS {
        return fail(
            StatusCode::BAD_REQUEST,
            format!("post_too_long: exceeds {CHANNEL_POST_MAX_CHARS} chars"),
        );
    }

    // Bot-side per-turn cap. The aggregator enforces the same cap before the
    // UDS round-trip; this is authoritative for direct UDS callers.
    if !state.progress.claim_channel_post(&req.invocation_id) {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "channel_post_cap_reached".to_owned(),
        );
    }

    let is_channel = match right_agent::agent::allowlist::read_file(&target.agent_dir) {
        Ok(Some(file)) => file.groups.iter().any(|group| {
            group.id == req.chat_id
                && group.kind == right_agent::agent::allowlist::GroupKind::Channel
        }),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "channel post allowlist read failed: {e:#}",
            );
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "channel allowlist unavailable".to_owned(),
            );
        }
    };
    if !is_channel {
        return fail(StatusCode::BAD_REQUEST, "channel_not_opened".to_owned());
    }

    // Channel posts alone use Telegram's Rich Markdown parser. Deterministic
    // formatting rejection falls back to the unchanged original text.
    match send_channel_post_text(&state.bot, req.chat_id, &req.text).await {
        Ok(Ok(message)) => {
            if let Err(e) = crate::telegram::archive::archive_outbound_channel_post(
                &target.agent_dir,
                req.chat_id,
                message.message_id,
                &req.text,
            )
            .await
            {
                tracing::warn!(
                    invocation_id = %req.invocation_id,
                    chat_id = req.chat_id,
                    "channel post archive failed: {e:#}",
                );
                // The post IS published; report the archive loss explicitly so
                // the agent knows `channel_read` is missing this message.
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(right_mcp::internal_client::ChannelPostResponse {
                        ok: false,
                        message_id: Some(message.message_id),
                        error: Some(format!("published but archive failed: {e:#}")),
                    }),
                )
                    .into_response();
            }
            Json(right_mcp::internal_client::ChannelPostResponse {
                ok: true,
                message_id: Some(message.message_id),
                error: None,
            })
            .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                chat_id = req.chat_id,
                "channel post failed: {e:#}",
            );
            fail(StatusCode::BAD_GATEWAY, format!("{e:#}"))
        }
        Err(_) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                chat_id = req.chat_id,
                "channel post timed out after {}s",
                PROGRESS_SEND_TIMEOUT.as_secs(),
            );
            fail(
                StatusCode::GATEWAY_TIMEOUT,
                "telegram_send_timeout".to_owned(),
            )
        }
    }
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
    let call = state.bot.create_forum_topic(
        target.chat_id,
        req.name,
        req.icon_color,
        req.icon_custom_emoji_id,
    );
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(message_thread_id)) => (
            StatusCode::OK,
            Json(ForumTopicCreateResponse {
                ok: true,
                message_thread_id,
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
    let call = state.bot.edit_forum_topic(
        target.chat_id,
        req.message_thread_id,
        req.name,
        req.icon_custom_emoji_id,
    );
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(())) => forum_ok(),
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
    let call = state
        .bot
        .close_forum_topic(target.chat_id, req.message_thread_id);
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(())) => forum_ok(),
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
    let call = state
        .bot
        .reopen_forum_topic(target.chat_id, req.message_thread_id);
    match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, call).await {
        Ok(Ok(())) => forum_ok(),
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

fn forum_telegram_error(invocation_id: &str, e: TgError) -> axum::response::Response {
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
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
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
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
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
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
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

    fn channel_post_request_json(invocation_id: &str, token: &str, chat_id: i64) -> Vec<u8> {
        channel_post_request_json_with_text(invocation_id, token, chat_id, "hello")
    }

    fn channel_post_request_json_with_text(
        invocation_id: &str,
        token: &str,
        chat_id: i64,
        text: &str,
    ) -> Vec<u8> {
        let req = right_mcp::internal_client::ChannelPostRequest {
            invocation_id: invocation_id.to_owned(),
            token: token.to_owned(),
            chat_id,
            text: text.to_owned(),
        };
        serde_json::to_vec(&req).expect("serialize ChannelPostRequest")
    }

    async fn post_channel_post(state: ProgressEndpointState, body: Vec<u8>) -> StatusCode {
        use tower::ServiceExt as _;
        let app = build_progress_router(state);
        let request = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/channel/post")
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
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
        let state = test_state(progress);
        let body = message_send_request_json("inv", "wrong");
        let status = post_message_send(state, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn handle_channel_post_unknown_invocation_is_404() {
        let status = post_channel_post(
            test_state(ProgressState::default()),
            channel_post_request_json("missing", "t", -100),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_channel_post_wrong_token_is_403() {
        let progress = ProgressState::default();
        progress.register(ProgressTarget {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: std::path::PathBuf::from("/tmp"),
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });

        let status = post_channel_post(
            test_state(progress),
            channel_post_request_json("inv", "wrong", -100),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    fn registered_target(agent_dir: &std::path::Path) -> ProgressTarget {
        ProgressTarget {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: agent_dir.to_owned(),
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    #[test]
    fn claim_channel_post_caps_attempts_per_invocation() {
        let progress = ProgressState::default();
        progress.register(registered_target(std::path::Path::new("/tmp")));
        for _ in 0..right_mcp::internal_client::MAX_CHANNEL_POST_PER_TURN {
            assert!(progress.claim_channel_post("inv"));
        }
        assert!(
            !progress.claim_channel_post("inv"),
            "attempt beyond the cap must be rejected"
        );
        assert!(
            !progress.claim_channel_post("missing"),
            "unknown invocation must be rejected"
        );
    }

    #[tokio::test]
    async fn handle_channel_post_rejects_empty_post_before_claiming() {
        let agent_dir = tempfile::tempdir().expect("agent dir");
        let progress = ProgressState::default();
        let target = registered_target(agent_dir.path());
        let counter = target.channel_post_count.clone();
        progress.register(target);

        let status = post_channel_post(
            test_state(progress),
            channel_post_request_json_with_text("inv", "right", -100, "   "),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a rejected post must not consume an attempt slot"
        );
    }

    #[tokio::test]
    async fn handle_channel_post_rejects_non_channel_allowlist_entry() {
        let agent_dir = tempfile::tempdir().expect("agent dir");
        right_agent::agent::allowlist::write_file(
            agent_dir.path(),
            &right_agent::agent::allowlist::AllowlistFile {
                version: right_agent::agent::allowlist::CURRENT_VERSION,
                users: Vec::new(),
                groups: vec![right_agent::agent::allowlist::AllowedGroup {
                    id: -100,
                    label: None,
                    opened_by: None,
                    opened_at: chrono::Utc::now(),
                    mode: right_agent::agent::allowlist::ResponseMode::Addressed,
                    topics: Vec::new(),
                    kind: right_agent::agent::allowlist::GroupKind::Group,
                }],
            },
        )
        .expect("write allowlist");

        let progress = ProgressState::default();
        progress.register(ProgressTarget {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: agent_dir.path().to_owned(),
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });

        let status = post_channel_post(
            test_state(progress),
            channel_post_request_json("inv", "right", -100),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn channel_rich_markdown_format_rejection_retries_original_text_as_plain_message() {
        use axum::Router;
        use axum::body::Bytes;
        use axum::response::IntoResponse as _;
        use axum::routing::post;

        const ORIGINAL: &str = "Original *Markdown* with ~$18.5M and <literal>";
        let (plain_tx, mut plain_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new()
            .route(
                "/botTEST-TOKEN/sendRichMessage",
                post(|| async {
                    (
                        StatusCode::BAD_REQUEST,
                        r#"{"ok":false,"error_code":400,"description":"Bad Request: can't parse rich Markdown"}"#,
                    )
                        .into_response()
                }),
            )
            .route(
                "/botTEST-TOKEN/sendMessage",
                post(move |body: Bytes| {
                    let plain_tx = plain_tx.clone();
                    async move {
                        plain_tx
                            .send(body)
                            .expect("test must receive plain fallback request");
                        (
                            StatusCode::OK,
                            r#"{"ok":true,"result":{"message_id":654,"date":0,"chat":{"id":-1001234567890,"type":"channel"}}}"#,
                        )
                            .into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram API");
        let address = listener.local_addr().expect("read mock API address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock Telegram API");
        });
        let bot = crate::telegram::tg_bot::RightBot::new_for_test(format!(
            "http://{address}/botTEST-TOKEN"
        ));

        let message = send_channel_post_text(&bot, -1001234567890, ORIGINAL)
            .await
            .expect("send must not time out")
            .expect("plain fallback must succeed");

        assert_eq!(message.message_id, 654);
        let body = plain_rx.recv().await.expect("receive plain fallback body");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("decode plain fallback JSON");
        assert_eq!(payload["text"], ORIGINAL);
        assert!(payload.get("parse_mode").is_none());
    }

    #[tokio::test]
    async fn channel_rich_markdown_server_error_does_not_retry_plain_message() {
        use axum::Router;
        use axum::http::Request;
        use axum::middleware::{self, Next};
        use axum::response::IntoResponse as _;
        use axum::routing::post;

        let request_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = Arc::clone(&request_count);
        let app = Router::new()
            .route(
                "/botTEST-TOKEN/sendRichMessage",
                post(|| async {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        r#"{"ok":false,"error_code":500,"description":"Internal Server Error: can't parse entities"}"#,
                    )
                        .into_response()
                }),
            )
            .route(
                "/botTEST-TOKEN/sendMessage",
                post(|| async { StatusCode::OK }),
            )
            .layer(middleware::from_fn(move |request: Request<axum::body::Body>, next: Next| {
                let count = Arc::clone(&count);
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    next.run(request).await
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram API");
        let address = listener.local_addr().expect("read mock API address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock Telegram API");
        });
        let bot = crate::telegram::tg_bot::RightBot::new_for_test(format!(
            "http://{address}/botTEST-TOKEN"
        ));

        let error = send_channel_post_text(&bot, -1001234567890, "*Markdown*")
            .await
            .expect("send must not time out")
            .expect_err("Telegram 5xx must propagate without fallback");

        assert!(matches!(error, TgError::Api(_)));
        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "ambiguous 5xx must not trigger /sendMessage"
        );
    }
}
