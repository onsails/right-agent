//! Dashboard routes for per-conversation operator focus. In-bot-process
//! direct `data.db` access (like `handle_delete_cron`); no internal socket -
//! `thread_focus` is bot-owned runtime state, not aggregator state.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::mcp::parse_json_body;
use super::{DashboardState, authenticate_api, json_error};

/// Upper bound on operator-set focus length. Operator focus is injected
/// verbatim and unwrapped into the system prompt on every foreground turn, so
/// an unbounded value would inflate the cached prompt indefinitely. Kept more
/// generous than the agent's self-set cap (`THREAD_FOCUS_MAX_CHARS` = 2000) but
/// still bounded.
pub(super) const OPERATOR_FOCUS_MAX_CHARS: usize = 4000;

const FOCUS_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(10);

type FocusNotificationFuture =
    Pin<Box<dyn Future<Output = Result<(), FocusNotificationError>> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FocusNotification {
    pub(crate) chat_id: i64,
    pub(crate) thread_id: i64,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FocusNotificationError {
    detail: String,
}

impl FocusNotificationError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for FocusNotificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for FocusNotificationError {}

#[derive(Clone)]
pub(crate) struct FocusNotifier {
    send_fn: Arc<dyn Fn(FocusNotification) -> FocusNotificationFuture + Send + Sync>,
}

impl FocusNotifier {
    fn new<F>(send_fn: F) -> Self
    where
        F: Fn(FocusNotification) -> FocusNotificationFuture + Send + Sync + 'static,
    {
        Self {
            send_fn: Arc::new(send_fn),
        }
    }

    pub(crate) fn telegram(bot: crate::telegram::BotType) -> Self {
        Self::new(move |notification| {
            let bot = bot.clone();
            Box::pin(async move { send_focus_notification_with_bot(&bot, notification).await })
        })
    }

    pub(crate) async fn send(
        &self,
        notification: FocusNotification,
    ) -> Result<(), FocusNotificationError> {
        (self.send_fn)(notification).await
    }

    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self::new(|_| Box::pin(async { Ok(()) }))
    }

    #[cfg(test)]
    pub(crate) fn capture(sent: Arc<tokio::sync::Mutex<Vec<FocusNotification>>>) -> Self {
        Self::new(move |notification| {
            let sent = Arc::clone(&sent);
            Box::pin(async move {
                sent.lock().await.push(notification);
                Ok(())
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn fail(detail: &'static str) -> Self {
        let detail = detail.to_string();
        Self::new(move |_| {
            let detail = detail.clone();
            Box::pin(async move { Err(FocusNotificationError::new(detail)) })
        })
    }
}

async fn send_focus_notification_with_bot(
    bot: &crate::telegram::BotType,
    notification: FocusNotification,
) -> Result<(), FocusNotificationError> {
    let thread = (notification.thread_id != 0).then_some(notification.thread_id as i32);
    let send = bot.send_message_opts(
        notification.chat_id,
        &notification.text,
        false,
        thread,
        None,
        None,
    );

    tokio::time::timeout(FOCUS_NOTIFICATION_TIMEOUT, send)
        .await
        .map_err(|_| FocusNotificationError::new("telegram focus notification timed out"))?
        .map(|_| ())
        .map_err(|error| {
            FocusNotificationError::new(format!("telegram focus notification failed: {error:#}"))
        })
}

fn focus_notification_text(value: Option<&str>) -> String {
    match value {
        Some(focus) => format!("Focus set: {focus}"),
        None => "Focus cleared".to_string(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct FocusScopeQuery {
    pub chat_id: i64,
    pub thread_id: i64,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FocusUpdateBody {
    pub chat_id: i64,
    pub thread_id: i64,
    pub token: Option<String>,
    pub operator_focus: String,
}

pub(crate) async fn handle_get(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Query(scope): Query<FocusScopeQuery>,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    if !super::focus_scope_token_valid(
        &state.bot_token,
        &state.agent_name,
        scope.chat_id,
        scope.thread_id,
        scope.token.as_deref().unwrap_or(""),
    ) {
        return json_error(
            StatusCode::FORBIDDEN,
            "invalid_focus_scope",
            Some("invalid conversation focus scope"),
        );
    }
    let conn = match right_db::open_connection_readonly(&state.agent_dir).await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "focus get: open db failed: {error:#}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_open_failed",
                Some("failed to open database"),
            );
        }
    };
    match right_db::thread_focus::get(&conn, scope.chat_id, scope.thread_id).await {
        Ok(row) => Json(serde_json::json!({
            "operator_focus": row.and_then(|r| r.operator_focus),
        }))
        .into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "focus get: query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "focus_read_failed",
                Some("failed to read focus"),
            )
        }
    }
}

pub(crate) async fn handle_update(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let req: FocusUpdateBody = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if !super::focus_scope_token_valid(
        &state.bot_token,
        &state.agent_name,
        req.chat_id,
        req.thread_id,
        req.token.as_deref().unwrap_or(""),
    ) {
        return json_error(
            StatusCode::FORBIDDEN,
            "invalid_focus_scope",
            Some("invalid conversation focus scope"),
        );
    }
    let trimmed = req.operator_focus.trim();
    if trimmed.chars().count() > OPERATOR_FOCUS_MAX_CHARS {
        return json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "focus_too_long",
            Some("conversation focus is too long"),
        );
    }
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    let conn = match right_db::open_connection(&state.agent_dir, false).await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "focus update: open db failed: {error:#}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_open_failed",
                Some("failed to open database"),
            );
        }
    };
    if let Err(error) =
        right_db::thread_focus::set_operator(&conn, req.chat_id, req.thread_id, value).await
    {
        tracing::error!(agent = %state.agent_name, "focus update: write failed: {error:#}");
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "focus_write_failed",
            Some("failed to write focus"),
        );
    }

    let notification = FocusNotification {
        chat_id: req.chat_id,
        thread_id: req.thread_id,
        text: focus_notification_text(value),
    };
    if let Err(error) = state.focus_notifier.send(notification).await {
        tracing::warn!(
            agent = %state.agent_name,
            chat_id = req.chat_id,
            thread_id = req.thread_id,
            "focus update: notification failed after save: {error:#}"
        );
        return json_error(
            StatusCode::BAD_GATEWAY,
            "focus_notification_failed",
            Some("Focus saved, but notification could not be sent"),
        );
    }

    Json(serde_json::json!({ "operator_focus": value })).into_response()
}
