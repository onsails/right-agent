use axum::{
    Json,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::DashboardState;
use super::authenticate_api;
use super::mcp::{internal_api_error_response, parse_json_body};

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCreateBody {
    #[serde(rename = "type")]
    pub type_: String,
    pub label: Option<String>,
    pub credential: String,
    pub generic: Option<ProviderCreateGenericBody>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderCreateGenericBody {
    pub env_var: String,
    pub header_name: Option<String>,
    pub upstream_host: String,
    pub upstream_path_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderRotateBody {
    pub credential: String,
}

pub(crate) async fn handle_list(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    match state.internal_client.provider_list(&state.agent_name).await {
        Ok(list) => Json(serde_json::json!({"providers": list})).into_response(),
        Err(error) => internal_api_error_response(error, "provider_list_failed"),
    }
}

pub(crate) async fn handle_types(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    match state.internal_client.provider_types().await {
        Ok(list) => Json(serde_json::json!({"types": list})).into_response(),
        Err(error) => internal_api_error_response(error, "provider_types_failed"),
    }
}

pub(crate) async fn handle_create(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let body: ProviderCreateBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let generic =
        body.generic
            .as_ref()
            .map(|g| right_mcp::internal_client::ProviderCreateGenericArg {
                env_var: &g.env_var,
                header_name: g.header_name.as_deref(),
                upstream_host: &g.upstream_host,
                upstream_path_prefix: g.upstream_path_prefix.as_deref(),
            });
    let req = right_mcp::internal_client::ProviderCreateRequest {
        agent: &state.agent_name,
        type_: &body.type_,
        label: body.label.as_deref(),
        credential: &body.credential,
        generic,
    };
    match state.internal_client.provider_create(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_create_failed"),
    }
}

pub(crate) async fn handle_rotate(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let body: ProviderRotateBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderRotateRequest {
        agent: &state.agent_name,
        name: &provider_name,
        credential: &body.credential,
    };
    match state.internal_client.provider_rotate(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_rotate_failed"),
    }
}

pub(crate) async fn handle_remove(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let req = right_mcp::internal_client::ProviderRemoveRequest {
        agent: &state.agent_name,
        name: &provider_name,
    };
    match state.internal_client.provider_remove(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_remove_failed"),
    }
}

pub(crate) async fn handle_config_update(
    AxumPath((agent, provider_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let raw: serde_json::Value = match parse_json_body::<serde_json::Value>(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let mut full = raw;
    let obj = match full.as_object_mut() {
        Some(o) => o,
        None => {
            return internal_api_error_response(
                right_mcp::internal_client::InternalClientError::Server {
                    status: 400,
                    body: "config-update body must be a JSON object".into(),
                },
                "provider_config_update_failed",
            );
        }
    };
    obj.insert(
        "agent".into(),
        serde_json::Value::String(state.agent_name.clone()),
    );
    obj.insert("name".into(), serde_json::Value::String(provider_name));
    match state.internal_client.provider_config_update(&full).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_config_update_failed"),
    }
}
