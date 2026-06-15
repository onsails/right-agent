use axum::{
    Json,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::mcp::{internal_api_error_response, parse_json_body};
use super::{DashboardState, authenticate_api, json_error};

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
    #[serde(default)]
    pub upstream_host: Option<String>,
    #[serde(default)]
    pub upstream_hosts: Vec<String>,
    pub upstream_path_prefix: Option<String>,
}

impl ProviderCreateGenericBody {
    fn normalized_upstream_hosts(&self) -> Vec<String> {
        let mut hosts = Vec::new();
        if let Some(host) = &self.upstream_host {
            let host = host.trim();
            if !host.is_empty() {
                hosts.push(host.to_string());
            }
        }
        for host in &self.upstream_hosts {
            let host = host.trim();
            if !host.is_empty() && !hosts.iter().any(|existing| existing == host) {
                hosts.push(host.to_string());
            }
        }
        hosts
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderRotateBody {
    pub credential: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderImportBody {
    pub source_agent: String,
    pub source_provider: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderExportBody {
    pub provider: String,
    pub dest_agent: String,
    #[serde(default)]
    pub overwrite: bool,
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
    let generic_hosts = if let Some(g) = &body.generic {
        let hosts = g.normalized_upstream_hosts();
        if hosts.is_empty() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                Some("at least one upstream host is required"),
            );
        }
        Some(hosts)
    } else {
        None
    };
    let generic =
        body.generic
            .as_ref()
            .map(|g| right_mcp::internal_client::ProviderCreateGenericArg {
                env_var: &g.env_var,
                upstream_hosts: generic_hosts
                    .as_deref()
                    .expect("generic hosts are normalized when generic body is present"),
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

pub(crate) async fn handle_peers(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    match state
        .internal_client
        .provider_peers(user.id, &state.agent_name)
        .await
    {
        Ok(peers) => Json(serde_json::json!({ "peers": peers })).into_response(),
        Err(error) => internal_api_error_response(error, "provider_peers_failed"),
    }
}

pub(crate) async fn handle_import(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let body: ProviderImportBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderCopyRequest {
        actor_user_id: user.id,
        source_agent: &body.source_agent,
        source_provider: &body.source_provider,
        dest_agent: &state.agent_name,
        label: body.label.as_deref(),
        overwrite: body.overwrite,
    };
    match state.internal_client.provider_copy(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_import_failed"),
    }
}

pub(crate) async fn handle_export(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let body: ProviderExportBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let req = right_mcp::internal_client::ProviderCopyRequest {
        actor_user_id: user.id,
        source_agent: &state.agent_name,
        source_provider: &body.provider,
        dest_agent: &body.dest_agent,
        label: None,
        overwrite: body.overwrite,
    };
    match state.internal_client.provider_copy(&req).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => internal_api_error_response(error, "provider_export_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_body_defaults_overwrite_false() {
        let b: ProviderImportBody = serde_json::from_value(serde_json::json!({
            "source_agent": "riskoff",
            "source_provider": "riskoff-fal"
        }))
        .unwrap();
        assert_eq!(b.source_agent, "riskoff");
        assert_eq!(b.source_provider, "riskoff-fal");
        assert!(b.label.is_none());
        assert!(!b.overwrite);
    }

    #[test]
    fn export_body_parses_overwrite() {
        let b: ProviderExportBody = serde_json::from_value(serde_json::json!({
            "provider": "current-fal",
            "dest_agent": "riskoff",
            "overwrite": true
        }))
        .unwrap();
        assert_eq!(b.provider, "current-fal");
        assert_eq!(b.dest_agent, "riskoff");
        assert!(b.overwrite);
    }

    #[test]
    fn provider_create_generic_body_accepts_legacy_upstream_host() {
        let body: ProviderCreateGenericBody = serde_json::from_value(serde_json::json!({
            "env_var": "ACME_TOKEN",
            "upstream_host": "api.acme.test"
        }))
        .unwrap();

        assert_eq!(body.normalized_upstream_hosts(), ["api.acme.test"]);
    }

    #[test]
    fn provider_create_generic_body_accepts_upstream_hosts() {
        let body: ProviderCreateGenericBody = serde_json::from_value(serde_json::json!({
            "env_var": "FAL_KEY",
            "upstream_hosts": ["fal.run", "queue.fal.run"]
        }))
        .unwrap();

        assert_eq!(
            body.normalized_upstream_hosts(),
            ["fal.run", "queue.fal.run"]
        );
    }

    #[test]
    fn provider_create_generic_body_trims_dedupes_and_merges_hosts() {
        let body: ProviderCreateGenericBody = serde_json::from_value(serde_json::json!({
            "env_var": "FAL_KEY",
            "upstream_host": " fal.run ",
            "upstream_hosts": ["queue.fal.run", "fal.run", "  "]
        }))
        .unwrap();

        assert_eq!(
            body.normalized_upstream_hosts(),
            ["fal.run", "queue.fal.run"]
        );
    }
}
