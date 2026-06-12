//! Dashboard routes for per-conversation operator focus. In-bot-process
//! direct `data.db` access (like `handle_delete_cron`); no internal socket -
//! `thread_focus` is bot-owned runtime state, not aggregator state.

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

#[derive(Debug, Deserialize)]
pub(crate) struct FocusScopeQuery {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FocusUpdateBody {
    pub chat_id: i64,
    pub thread_id: i64,
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
    let conn = match right_db::open_connection(&state.agent_dir, false).await {
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
    let trimmed = req.operator_focus.trim();
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
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
    Json(serde_json::json!({ "operator_focus": value })).into_response()
}
