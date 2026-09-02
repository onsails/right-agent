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

/// Shared literal text-send path for the progress endpoint.
///
/// Sends the agent-authored message without a parse mode. The single attempt is
/// bounded by `PROGRESS_SEND_TIMEOUT`; `Elapsed` remains distinct from a
/// Telegram error so callers can map them to separate statuses.
async fn send_text_message(
    bot: &super::BotType,
    target: &ProgressTarget,
    message: &str,
) -> Result<Result<frankenstein::types::Message, TgError>, tokio::time::error::Elapsed> {
    let thread = (target.thread_id != 0).then_some(target.thread_id as i32);
    tokio::time::timeout(
        PROGRESS_SEND_TIMEOUT,
        bot.send_message_opts(target.chat_id, message, false, thread, None, None),
    )
    .await
}

/// Shared validated-rich send path for final, MCP, channel, and async delivery.
///
/// Deliberately NOT wrapped in an outer timeout: a single call fans out to N
/// rich parts (plus plain-fallback chunks), each gated by the per-chat
/// throttle and each independently bounded by the bot's per-attempt
/// `TELEGRAM_TEXT_TIMEOUT`. A fixed 10s ceiling here deterministically
/// truncated long multi-part sends mid-stream while the channel-post attempt
/// had already been consumed.
async fn send_rich_content(
    bot: &super::BotType,
    chat_id: i64,
    content: &right_rich_content::RichContent,
    thread: Option<i32>,
) -> super::rich_content::RichSendOutcome {
    crate::telegram::rich_content::send(bot, chat_id, content, thread, None, None).await
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
    if let Some(content) = req.content.as_ref() {
        let thread = (target.thread_id != 0).then_some(target.thread_id as i32);
        let outcome = send_rich_content(&state.bot, target.chat_id, content, thread).await;
        message_ids.extend(outcome.delivered.iter().map(|message| message.message_id));
        if !outcome.is_complete() {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                delivered_messages = outcome.delivered.len(),
                "message_send text failed partway: {}",
                outcome.error_display()
            );
            if outcome.delivered.is_empty() {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ProgressErrorResponse {
                        error: "telegram_send_failed".to_owned(),
                    }),
                )
                    .into_response();
            }
            // Partial publication: the delivered ids must travel back so the
            // agent does not blindly retry and duplicate the prefix.
            return partial_publication_response(message_ids);
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
            error: None,
        }),
    )
        .into_response()
}

/// `send_message` partial-publication reply: `ok:false` with every delivered
/// Telegram message id. Travels as HTTP 200 so the aggregator's typed client
/// can surface the ids to the agent instead of collapsing them into a
/// transport error.
fn partial_publication_response(message_ids: Vec<i32>) -> axum::response::Response {
    (
        StatusCode::OK,
        Json(right_mcp::internal_client::SendMessageResponse {
            ok: false,
            message_ids,
            error: Some(
                "partially published: some rich parts failed; do not resend the delivered message_ids"
                    .to_owned(),
            ),
        }),
    )
        .into_response()
}

fn channel_attachment_thread_id(_invoking_thread_id: i64) -> i64 {
    0
}

fn channel_post_response(
    status: StatusCode,
    ok: bool,
    message_ids: Vec<i32>,
    delivery_uncertain: bool,
    error: Option<String>,
) -> axum::response::Response {
    (
        status,
        Json(right_mcp::internal_client::ChannelPostResponse {
            ok,
            message_ids,
            delivery_uncertain,
            error,
        }),
    )
        .into_response()
}

/// Publish validated rich content to an opened Telegram channel. The invocation
/// token and the current allowlist are both authoritative.
async fn handle_channel_post(
    State(state): State<ProgressEndpointState>,
    Json(req): Json<right_mcp::internal_client::ChannelPostRequest>,
) -> axum::response::Response {
    let fail = |status: StatusCode, message: String| {
        channel_post_response(status, false, Vec::new(), false, Some(message))
    };

    let Some(target) = state.progress.get(&req.invocation_id) else {
        return fail(StatusCode::NOT_FOUND, "unknown invocation".to_owned());
    };
    if !target.token_matches(&req.token) {
        return fail(StatusCode::FORBIDDEN, "token mismatch".to_owned());
    }
    if req.content.is_none() && req.attachments.is_empty() {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "content_or_attachments_required".to_owned(),
        );
    }
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
        Err(error) => {
            tracing::warn!(
                invocation_id = %req.invocation_id,
                "channel post allowlist read failed: {error:#}",
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

    let mut message_ids = Vec::new();
    let mut archive_fragments = Vec::new();
    let mut failure: Option<(String, bool)> = None;

    if let Some(content) = req.content.as_ref() {
        let outcome = crate::telegram::rich_content::send_until_failure(
            &state.bot,
            req.chat_id,
            content,
            None,
        )
        .await;
        message_ids.extend(outcome.delivered.iter().map(|message| message.message_id));
        if !outcome.delivered_text.is_empty() {
            archive_fragments.push(outcome.delivered_text.clone());
        }
        if !outcome.is_complete() {
            let uncertain = outcome.error.as_ref().is_some_and(|error| {
                !matches!(
                    error,
                    TgError::Api(frankenstein::Error::Api(response)) if response.error_code == 400
                )
            });
            failure = Some((outcome.error_display().into_owned(), uncertain));
        }
    }

    if failure.is_none() && !req.attachments.is_empty() {
        let Some(sandbox) = target.sandbox.as_ref() else {
            failure = Some(("sandbox_unavailable".to_owned(), false));
            return finish_channel_post(&req, &target, message_ids, archive_fragments, failure)
                .await;
        };
        let outbound: Vec<_> = req
            .attachments
            .iter()
            .map(message_dto_to_outbound)
            .collect();
        let report = crate::telegram::attachments::send_attachments_reported(
            &outbound,
            &state.bot,
            req.chat_id,
            channel_attachment_thread_id(target.thread_id),
            &target.agent_dir,
            sandbox,
            true,
        )
        .await;
        message_ids.extend(report.confirmed);
        archive_fragments.extend(report.delivered_fragments);
        if let Some(send_failure) = report.failure {
            failure = Some((
                send_failure.message().to_owned(),
                send_failure.is_uncertain(),
            ));
        }
    }

    finish_channel_post(&req, &target, message_ids, archive_fragments, failure).await
}

async fn finish_channel_post(
    req: &right_mcp::internal_client::ChannelPostRequest,
    target: &ProgressTarget,
    message_ids: Vec<i32>,
    archive_fragments: Vec<String>,
    failure: Option<(String, bool)>,
) -> axum::response::Response {
    if let Some(&last_id) = message_ids.last() {
        let publication = archive_fragments
            .iter()
            .filter(|fragment| !fragment.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Err(error) = crate::telegram::archive::archive_outbound_channel_post(
            &target.agent_dir,
            req.chat_id,
            last_id,
            &publication,
        )
        .await
        {
            let (delivery_uncertain, error) = match failure.as_ref() {
                Some((delivery_error, true)) => (
                    true,
                    format!(
                        "published but archive failed: {error:#}; Telegram may have delivered the failed request: {delivery_error}; do not resend this publication or confirmed message_ids"
                    ),
                ),
                Some((delivery_error, false)) => (
                    false,
                    format!(
                        "published but archive failed: {error:#}; partially published: {delivery_error}; do not resend the confirmed message_ids"
                    ),
                ),
                None => (
                    false,
                    format!(
                        "published but archive failed: {error:#}; do not resend confirmed message_ids"
                    ),
                ),
            };
            return channel_post_response(
                StatusCode::OK,
                false,
                message_ids,
                delivery_uncertain,
                Some(error),
            );
        }
    }

    match failure {
        None => channel_post_response(StatusCode::OK, true, message_ids, false, None),
        Some((error, uncertain)) => {
            if !message_ids.is_empty() || uncertain {
                channel_post_response(
                    StatusCode::OK,
                    false,
                    message_ids,
                    uncertain,
                    Some(if uncertain {
                        format!(
                            "Telegram may have delivered the failed request: {error}; do not resend this publication or confirmed message_ids"
                        )
                    } else {
                        format!(
                            "partially published: {error}; do not resend the confirmed message_ids"
                        )
                    }),
                )
            } else {
                channel_post_response(
                    if error == "sandbox_unavailable" {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::BAD_GATEWAY
                    },
                    false,
                    message_ids,
                    false,
                    Some(error),
                )
            }
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
        let dto: right_mcp::internal_client::MessageAttachmentDto =
            serde_json::from_value(serde_json::json!({
                "type": "document",
                "path": "/sandbox/outbox/r.csv",
                "filename": "results.csv"
            }))
            .unwrap();
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

    #[tokio::test]
    async fn progress_with_paired_single_tildes_is_sent_as_literal_plain_text() {
        use axum::{body::Bytes, response::IntoResponse, routing::post};

        let (body_tx, mut body_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/botTEST-TOKEN/sendMessage",
            post(move |body: Bytes| {
                let body_tx = body_tx.clone();
                async move {
                    body_tx.send(body).expect("capture progress request");
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "ok": true,
                            "result": {
                                "message_id": 17,
                                "date": 0,
                                "chat": {"id": 42, "type": "private"}
                            }
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Telegram stub");
        let address = listener.local_addr().expect("Telegram stub address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve Telegram stub");
        });
        let bot = crate::telegram::tg_bot::RightBot::new_for_test(format!(
            "http://{address}/botTEST-TOKEN"
        ));
        let target = ProgressTarget {
            invocation_id: "inv-tilde".to_owned(),
            token: "secret-token".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: std::path::PathBuf::from("/tmp/agent"),
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        };

        let outcome = send_text_message(&bot, &target, "working on ~riskoff~ now")
            .await
            .expect("progress send must not time out")
            .expect("Telegram must accept progress");

        assert_eq!(outcome.message_id, 17);
        let payload: serde_json::Value =
            serde_json::from_slice(&body_rx.recv().await.expect("captured progress request"))
                .expect("progress request JSON");
        assert_eq!(payload["text"], "working on ~riskoff~ now");
        assert!(
            payload.get("parse_mode").is_none(),
            "literal progress must not enable HTML parsing: {payload}"
        );
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
            content: Some(right_rich_content::RichContent::literal("hi").unwrap()),
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
            content: Some(right_rich_content::RichContent::literal(text.to_owned()).unwrap()),
            attachments: Vec::new(),
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

    /// Post to `/channel/post` and return the status plus the typed body the
    /// aggregator's client deserializes.
    async fn post_channel_post_typed(
        state: ProgressEndpointState,
        body: Vec<u8>,
    ) -> (StatusCode, right_mcp::internal_client::ChannelPostResponse) {
        use tower::ServiceExt as _;
        let app = build_progress_router(state);
        let request = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/channel/post")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .expect("build request");
        let response = app.oneshot(request).await.expect("router oneshot");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        (
            status,
            serde_json::from_slice(&bytes).expect("typed channel post body"),
        )
    }

    fn channel_allowlisted_agent_dir() -> tempfile::TempDir {
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
                    kind: right_agent::agent::allowlist::GroupKind::Channel,
                }],
            },
        )
        .expect("write allowlist");
        agent_dir
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

        let body = serde_json::to_vec(&serde_json::json!({
            "invocation_id": "inv",
            "token": "right",
            "chat_id": -100,
            "content": { "text": "   " }
        }))
        .expect("serialize invalid raw request");
        let status = post_channel_post(test_state(progress), body).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a rejected post must not consume an attempt slot"
        );
    }

    #[tokio::test]
    async fn channel_post_rejects_missing_content_and_attachments_before_claiming() {
        let agent_dir = tempfile::tempdir().expect("agent dir");
        let progress = ProgressState::default();
        let target = registered_target(agent_dir.path());
        let counter = target.channel_post_count.clone();
        progress.register(target);
        let body = serde_json::to_vec(&serde_json::json!({
            "invocation_id": "inv",
            "token": "right",
            "chat_id": -100
        }))
        .unwrap();

        let status = post_channel_post(test_state(progress), body).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);
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

    #[test]
    fn channel_post_request_uses_content_not_text() {
        let value: serde_json::Value =
            serde_json::from_slice(&channel_post_request_json("inv", "token", -100)).unwrap();
        assert_eq!(value["content"]["text"], "hello");
        assert!(value.get("text").is_none());
    }

    #[test]
    fn channel_post_attachments_ignore_invoking_thread() {
        let mut target = registered_target(std::path::Path::new("/tmp"));
        target.thread_id = 73;

        assert_eq!(channel_attachment_thread_id(target.thread_id), 0);
    }

    #[tokio::test]
    async fn channel_post_archive_failure_preserves_delivery_uncertainty() {
        let req = right_mcp::internal_client::ChannelPostRequest {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: -100,
            content: None,
            attachments: Vec::new(),
        };
        let target = registered_target(std::path::Path::new("/tmp"));

        let response = finish_channel_post(
            &req,
            &target,
            vec![701, 702],
            vec!["confirmed body".to_owned()],
            Some(("attachment delivery timed out".to_owned(), true)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let response: right_mcp::internal_client::ChannelPostResponse =
            serde_json::from_slice(&bytes).expect("typed channel post body");

        assert!(!response.ok);
        assert_eq!(response.message_ids, vec![701, 702]);
        assert!(response.delivery_uncertain);
        let error = response.error.expect("combined delivery and archive error");
        assert!(
            error.contains("archive failed"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("Telegram may have delivered"),
            "unexpected error: {error}"
        );
        assert!(error.contains("do not resend"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn channel_post_archive_only_failure_stays_delivery_certain() {
        let req = right_mcp::internal_client::ChannelPostRequest {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: -100,
            content: None,
            attachments: Vec::new(),
        };
        let target = registered_target(std::path::Path::new("/tmp"));

        let response = finish_channel_post(
            &req,
            &target,
            vec![703],
            vec!["complete publication".to_owned()],
            None,
        )
        .await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let response: right_mcp::internal_client::ChannelPostResponse =
            serde_json::from_slice(&bytes).expect("typed channel post body");

        assert!(!response.ok);
        assert_eq!(response.message_ids, vec![703]);
        assert!(!response.delivery_uncertain);
        let error = response.error.expect("archive error");
        assert!(
            error.contains("archive failed"),
            "unexpected error: {error}"
        );
        assert!(error.contains("do not resend"), "unexpected error: {error}");
        assert!(
            !error.contains("Telegram may have delivered"),
            "unexpected error: {error}"
        );
    }

    /// Regression: the rich fan-out must not be wrapped in an outer wall-clock
    /// timeout. The helper's return type is `RichSendOutcome` — no `Elapsed`
    /// variant remains — and this test drives a real multi-part send through
    /// the per-chat throttle to prove every part is awaited to completion. The
    /// removed 10s wrapper raced exactly this throttle wait and truncated long
    /// sends mid-stream while the channel-post attempt had been consumed.
    #[tokio::test]
    async fn send_rich_content_completes_multi_part_fan_out_without_outer_timeout() {
        use axum::{body::Bytes, response::IntoResponse, routing::post};
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_handler = attempts.clone();
        let next_id = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(10));
        let app = axum::Router::new().route(
            "/botTEST-TOKEN/sendRichMessage",
            post(move |_body: Bytes| {
                let attempts = attempts_for_handler.clone();
                let next_id = next_id.clone();
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let id = next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "ok": true,
                            "result": {"message_id": id, "date": 0,
                                       "chat": {"id": 7, "type": "private"}}
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = crate::telegram::tg_bot::RightBot::new_for_test(format!(
            "http://{address}/botTEST-TOKEN"
        ));

        // Eight code blocks of 5,000 chars each: greedy batching fits ~6 per
        // part under the 32,768-unit limit, so delivery fans out to 2 parts —
        // part waits out the per-chat throttle (1s) before its send — exactly
        // the wait the old wrapper cut off.
        let blocks: Vec<serde_json::Value> = (0..8)
            .map(|index| {
                serde_json::json!({
                    "type": "code",
                    "text": format!("part-{index}-{}", "x".repeat(5_000))
                })
            })
            .collect();
        let content: right_rich_content::RichContent =
            serde_json::from_value(serde_json::json!({ "blocks": blocks })).unwrap();
        assert_eq!(content.delivery_parts().len(), 2);

        let outcome = send_rich_content(&bot, 7, &content, None).await;
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "both parts must be attempted and awaited to completion"
        );
        assert_eq!(outcome.delivered.len(), 2);
        assert!(outcome.is_complete());
    }

    /// Multi-part content plus a mock Telegram API that delivers the first
    /// part and fails the second: the post is partially published, and the
    /// response must be HTTP 200 `ok:false` carrying the live `message_id`
    /// (with the archive written under it) so the aggregator's typed client
    /// surfaces the id instead of a transport error.
    #[tokio::test]
    async fn channel_post_partial_publication_returns_200_with_message_ids() {
        use axum::{body::Bytes, response::IntoResponse, routing::post};
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_handler = attempts.clone();
        let app = axum::Router::new().route(
            "/botTEST-TOKEN/sendRichMessage",
            post(move |_body: Bytes| {
                let attempts = attempts_for_handler.clone();
                async move {
                    let seen = attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if seen == 0 {
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "ok": true,
                                "result": {"message_id": 777, "date": 0,
                                           "chat": {"id": -100, "type": "channel"}}
                            })),
                        )
                            .into_response()
                    } else {
                        // Not a known rich-content rejection, so it is
                        // terminal: no plain fallback, no retry.
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            axum::Json(serde_json::json!({
                                "ok": false,
                                "error_code": 400,
                                "description": "Bad Request: reply message not found"
                            })),
                        )
                            .into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // The partial branch archives the delivered prefix before replying,
        // and the archive write derives `<home>/run/internal.sock` from the
        // agent dir. Stand up a home-shaped layout and serve the archive route
        // so the response comes from the PARTIAL branch, not the archive-failed
        // one (both are HTTP 200 `ok:false` with a message_id; only the error
        // text distinguishes them).
        let home = tempfile::tempdir().expect("home dir");
        let agent_path = home.path().join("agents").join("alpha");
        std::fs::create_dir_all(&agent_path).expect("agent dir");
        right_agent::agent::allowlist::write_file(
            &agent_path,
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
                    kind: right_agent::agent::allowlist::GroupKind::Channel,
                }],
            },
        )
        .expect("write allowlist");
        let archive_socket = home.path().join("run").join("internal.sock");
        std::fs::create_dir_all(archive_socket.parent().unwrap()).expect("run dir");
        let archive_listener =
            tokio::net::UnixListener::bind(&archive_socket).expect("bind archive socket");
        tokio::spawn(async move {
            let app = axum::Router::new().route(
                right_mcp::internal_db::ROUTE_ARCHIVE_MESSAGE,
                axum::routing::post(|| async {
                    axum::Json(serde_json::json!({ "id": 1, "inserted": true }))
                }),
            );
            if let Err(e) = axum::serve(archive_listener, app).await {
                tracing::warn!("test archive server ended: {e}");
            }
        });

        let progress = ProgressState::default();
        progress.register(ProgressTarget {
            invocation_id: "inv".to_owned(),
            token: "right".to_owned(),
            chat_id: 42,
            thread_id: 0,
            agent_dir: agent_path,
            sandbox: None,
            channel_post_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
        let state = ProgressEndpointState {
            bot: crate::telegram::tg_bot::RightBot::new_for_test(format!(
                "http://{address}/botTEST-TOKEN"
            )),
            progress,
        };

        // Eight 5,000-char code blocks fan out to two parts (same shape as the
        // fan-out regression above).
        let blocks: Vec<serde_json::Value> = (0..8)
            .map(|index| {
                serde_json::json!({
                    "type": "code",
                    "text": format!("part-{index}-{}", "x".repeat(5_000))
                })
            })
            .collect();
        let body = serde_json::to_vec(&serde_json::json!({
            "invocation_id": "inv",
            "token": "right",
            "chat_id": -100,
            "content": { "blocks": blocks }
        }))
        .expect("serialize request");

        let (status, response) = post_channel_post_typed(state, body).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!response.ok, "partial publication must be ok:false");
        assert_eq!(response.message_ids, vec![777]);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("partially published")),
            "error must name the partial publication: {:?}",
            response.error
        );
    }

    /// Zero delivered messages is a genuine failure: it must stay on an error
    /// status, because there is no live message id for the agent to avoid
    /// resending.
    #[tokio::test]
    async fn channel_post_zero_delivery_stays_error_status() {
        use axum::{body::Bytes, response::IntoResponse, routing::post};
        let app = axum::Router::new().route(
            "/botTEST-TOKEN/sendRichMessage",
            post(|_body: Bytes| async move {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "ok": false,
                        "error_code": 400,
                        "description": "Bad Request: chat not found"
                    })),
                )
                    .into_response()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let agent_dir = channel_allowlisted_agent_dir();
        let progress = ProgressState::default();
        progress.register(registered_target(agent_dir.path()));
        let state = ProgressEndpointState {
            bot: crate::telegram::tg_bot::RightBot::new_for_test(format!(
                "http://{address}/botTEST-TOKEN"
            )),
            progress,
        };

        let (status, response) =
            post_channel_post_typed(state, channel_post_request_json("inv", "right", -100)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(response.message_ids.is_empty());
        assert!(!response.delivery_uncertain);
    }
}
