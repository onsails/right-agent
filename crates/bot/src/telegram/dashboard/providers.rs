use std::sync::Arc;

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
pub(crate) struct ProviderShareBody {
    pub provider: String,
    pub dest_agent: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderUnshareBody {
    pub provider: String,
}

/// Pull (borrow) the inverse of share: `owner_agent` owns the record, the current
/// dashboard agent becomes the borrower. Maps to the same `provider_share` call
/// with owner/dest swapped — the backend enforces the identical both-sides trust.
#[derive(Debug, Deserialize)]
pub(crate) struct ProviderBorrowBody {
    pub owner_agent: String,
    pub provider: String,
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
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable in this bot process"),
        );
    };
    let Some(sandbox) = state.sandbox() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; provider was not created"),
        );
    };
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
    let view = match state.internal_client.provider_create(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_create_failed"),
    };
    let _agent_guard = match providers.agent_lock(&state.agent_name).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "provider was created but convergence lock failed");
            return holder_convergence_error("creation", std::slice::from_ref(&state.agent_name));
        }
    };
    let Some(provider_name) = view
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider_propagation_failed",
            Some(
                "provider was stored but its response did not identify it for sandbox propagation",
            ),
        );
    };
    let config = match load_provider_config(&state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    state.provider_config.store(Arc::new(config.clone()));
    match crate::sandbox_supervisor::apply_named_provider(
        &state.agent_name,
        &provider_name,
        &config,
        providers,
        &sandbox,
    )
    .await
    {
        Ok(applied) => {
            tracing::info!(
                agent = %state.agent_name,
                provider = %provider_name,
                disposition = ?applied.disposition,
                warnings = ?applied.warnings,
                "dashboard provider creation propagated to sandbox"
            );
            Json(view).into_response()
        }
        Err(error) => {
            tracing::error!(
                agent = %state.agent_name,
                provider = %provider_name,
                error = %format!("{error:#}"),
                "dashboard provider creation could not be propagated to sandbox"
            );
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(
                    "provider was stored in providers.db and agent.yaml but could not be made live in the sandbox",
                ),
            )
        }
    }
}

async fn converge_provider_holders(
    state: &DashboardState,
    providers: &right_providers::ProviderStore,
    owner_agent: &str,
    provider_name: &str,
) -> Result<(), Vec<String>> {
    let holders = match providers.holders(owner_agent, provider_name).await {
        Ok(holders) => holders,
        Err(error) => {
            tracing::error!(agent = %owner_agent, provider = %provider_name, error = %format!("{error:#}"), "failed to enumerate provider holders for convergence");
            return Err(vec![owner_agent.to_owned()]);
        }
    };
    let mut failed = Vec::new();
    for holder in holders {
        match converge_provider_holder(state, providers, &holder.agent, provider_name).await {
            Ok(()) => {}
            Err(error) => {
                tracing::error!(agent = %holder.agent, provider = %provider_name, error = %format!("{error:#}"), "provider holder sandbox convergence failed");
                failed.push(holder.agent);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed)
    }
}

async fn converge_provider_holder(
    state: &DashboardState,
    providers: &right_providers::ProviderStore,
    agent: &str,
    provider_name: &str,
) -> miette::Result<()> {
    let _guard = providers.agent_lock(agent).await.map_err(|error| {
        miette::miette!("locking provider convergence for agent {agent}: {error:#}")
    })?;
    let agent_dir = if agent == state.agent_name {
        state.agent_dir.clone()
    } else {
        right_config::agents_dir(&state.home).join(agent)
    };
    let config = right_agent::agent::parse_agent_config(&agent_dir)
        .map_err(|error| miette::miette!("loading agent {agent} config: {error:#}"))?
        .ok_or_else(|| miette::miette!("agent {agent} config is missing"))?;
    if agent == state.agent_name {
        state.provider_config.store(Arc::new(config.clone()));
    }
    let sandbox = if agent == state.agent_name {
        state
            .sandbox()
            .ok_or_else(|| miette::miette!("agent {agent} sandbox is unavailable"))?
    } else {
        let sandbox_name = right_sandbox::resolve_sandbox_name(
            agent,
            config
                .sandbox
                .as_ref()
                .and_then(|sandbox| sandbox.name.as_deref()),
        );
        Arc::new(
            right_sandbox::SandboxHandle::attach(&sandbox_name)
                .await
                .map_err(|error| {
                    miette::miette!("attaching agent {agent} sandbox {sandbox_name}: {error:#}")
                })?,
        )
    };
    crate::sandbox_supervisor::apply_named_provider(
        agent,
        provider_name,
        &config,
        providers,
        &sandbox,
    )
    .await?;
    Ok(())
}

fn holder_convergence_detail(operation: &str, failed: &[String]) -> String {
    format!(
        "provider {operation} was stored, but sandbox convergence failed for agents: {}",
        failed.join(", ")
    )
}

fn holder_convergence_error(operation: &str, failed: &[String]) -> Response {
    let detail = holder_convergence_detail(operation, failed);
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "provider_propagation_failed",
        Some(&detail),
    )
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
    let ProviderRotateBody { credential } = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    drop(body);
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable in this bot process"),
        );
    };
    if state.sandbox().is_none() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; credential was not rotated"),
        );
    }
    let _mutation = state.provider_mutation.lock().await;
    let config = match right_agent::agent::parse_agent_config(&state.agent_dir) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("agent.yaml is missing"),
            );
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "failed to reload agent config before provider rotation");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("failed to load the current agent config"),
            );
        }
    };
    state.provider_config.store(Arc::new(config.clone()));
    if !config
        .providers()
        .iter()
        .any(|entry| entry.name == provider_name)
    {
        return json_error(
            StatusCode::CONFLICT,
            "provider_propagation_failed",
            Some("provider is absent from the current agent config"),
        );
    }
    let record = match providers.get(&state.agent_name, &provider_name).await {
        Ok(record) => record,
        Err(error) => {
            tracing::error!(
                agent = %state.agent_name,
                provider = %provider_name,
                error = %format!("{error:#}"),
                "provider rotation target could not be loaded from the local provider store"
            );
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider could not be loaded from the provider store"),
            );
        }
    };
    if record.is_borrowed() {
        return json_error(
            StatusCode::CONFLICT,
            "borrowed_read_only",
            Some("borrowed providers can only be rotated by their owner"),
        );
    }
    let req = right_mcp::internal_client::ProviderRotateRequest {
        agent: &state.agent_name,
        name: &provider_name,
        credential: &credential,
    };
    let view = match state.internal_client.provider_rotate(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_rotate_failed"),
    };
    drop(credential);
    drop(_mutation);
    match converge_provider_holders(&state, providers, &state.agent_name, &provider_name).await {
        Ok(()) => Json(view).into_response(),
        Err(failed) => holder_convergence_error("credential rotation", &failed),
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
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable; provider was not removed"),
        );
    };
    let Some(sandbox) = state.sandbox() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; provider was not removed"),
        );
    };
    let previous = match load_current_provider_config(&state, "provider was not removed") {
        Ok(config) => config,
        Err(response) => return response,
    };
    let target = match providers.get(&state.agent_name, &provider_name).await {
        Ok(record) => record,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "failed to preflight provider removal target");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider could not be preflighted; it was not removed"),
            );
        }
    };
    if let Err(error) = sandbox.secret_env_vars().await {
        tracing::error!(error = %format!("{error:#}"), "failed to preflight sandbox provider bindings");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox bindings could not be preflighted; provider was not removed"),
        );
    }
    let req = right_mcp::internal_client::ProviderRemoveRequest {
        agent: &state.agent_name,
        name: &provider_name,
    };
    let view = match state.internal_client.provider_remove(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_remove_failed"),
    };
    let _agent_guard = match providers.agent_lock(&state.agent_name).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "provider lock failed before removal");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider lock failed; provider was not removed"),
            );
        }
    };
    let config = match load_provider_config(&state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    state.provider_config.store(Arc::new(config.clone()));
    match crate::sandbox_supervisor::hot_reconcile_providers(
        &state.agent_name,
        std::slice::from_ref(&previous),
        &config,
        providers,
        &sandbox,
    )
    .await
    {
        Ok(()) => {
            tracing::info!(
                agent = %state.agent_name,
                provider = %provider_name,
                env_var = %target.env_var,
                "dashboard provider removal revoked the sandbox binding"
            );
            Json(view).into_response()
        }
        Err(error) => {
            tracing::error!(
                agent = %state.agent_name,
                provider = %provider_name,
                env_var = %target.env_var,
                error = %format!("{error:#}"),
                "provider was durably removed but its sandbox binding revocation failed"
            );
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(
                    "provider was removed from providers.db and agent.yaml, but its live sandbox binding could not be revoked",
                ),
            )
        }
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
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable in this bot process"),
        );
    };
    if state.sandbox().is_none() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; provider configuration was not updated"),
        );
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
    obj.insert(
        "name".into(),
        serde_json::Value::String(provider_name.clone()),
    );
    let view = match state.internal_client.provider_config_update(&full).await {
        Ok(view) => view,
        Err(error) => {
            return internal_api_error_response(error, "provider_config_update_failed");
        }
    };
    let config = match load_provider_config(&state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    state.provider_config.store(Arc::new(config.clone()));
    drop(_mutation);
    match converge_provider_holders(&state, providers, &state.agent_name, &provider_name).await {
        Ok(()) => Json(view).into_response(),
        Err(failed) => holder_convergence_error("config update", &failed),
    }
}

fn load_current_provider_config(
    state: &DashboardState,
    unchanged_message: &'static str,
) -> Result<right_agent::agent::types::AgentConfig, Response> {
    match right_agent::agent::parse_agent_config(&state.agent_dir) {
        Ok(Some(config)) => Ok(config),
        Ok(None) => Err(json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider_propagation_failed",
            Some(unchanged_message),
        )),
        Err(error) => {
            tracing::error!(
                error = %format!("{error:#}"),
                "failed to load current agent config before provider mutation"
            );
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(unchanged_message),
            ))
        }
    }
}

fn load_provider_config(
    state: &DashboardState,
) -> Result<right_agent::agent::types::AgentConfig, Response> {
    match right_agent::agent::parse_agent_config(&state.agent_dir) {
        Ok(Some(config)) => Ok(config),
        Ok(None) => Err(json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider_propagation_failed",
            Some("provider mutation was stored but agent.yaml is missing"),
        )),
        Err(error) => {
            tracing::error!(
                error = %format!("{error:#}"),
                "failed to reload agent config after provider mutation"
            );
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(
                    "provider mutation was stored but the current agent config could not be loaded",
                ),
            ))
        }
    }
}

async fn preflight_agent_sandbox(
    state: &DashboardState,
    agent: &str,
    unchanged_message: &'static str,
) -> Result<
    (
        right_agent::agent::types::AgentConfig,
        right_sandbox::SandboxHandle,
        std::path::PathBuf,
    ),
    Response,
> {
    let agent_dir = right_config::agents_dir(&state.home).join(agent);
    let config = match right_agent::agent::parse_agent_config(&agent_dir) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(unchanged_message),
            ));
        }
        Err(error) => {
            tracing::error!(agent = %agent, error = %format!("{error:#}"), "failed to preflight destination agent config");
            return Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some(unchanged_message),
            ));
        }
    };
    let sandbox_name = right_sandbox::resolve_sandbox_name(
        agent,
        config
            .sandbox
            .as_ref()
            .and_then(|sandbox| sandbox.name.as_deref()),
    );
    let sandbox = match right_sandbox::SandboxHandle::attach(&sandbox_name).await {
        Ok(sandbox) => sandbox,
        Err(error) => {
            tracing::error!(agent = %agent, sandbox = %sandbox_name, error = %format!("{error:#}"), "failed to preflight destination sandbox");
            return Err(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "provider_propagation_failed",
                Some(unchanged_message),
            ));
        }
    };
    if let Err(error) = sandbox.secret_env_vars().await {
        tracing::error!(agent = %agent, error = %format!("{error:#}"), "failed to preflight destination sandbox bindings");
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some(unchanged_message),
        ));
    }
    Ok((config, sandbox, agent_dir))
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

pub(crate) async fn handle_share(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable; provider was not shared"),
        );
    };
    let body: ProviderShareBody = match parse_json_body(&body) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let (previous, sandbox, dest_dir) = match preflight_agent_sandbox(
        &state,
        &body.dest_agent,
        "destination sandbox is unavailable; provider was not shared",
    )
    .await
    {
        Ok(preflight) => preflight,
        Err(response) => return response,
    };
    let req = right_mcp::internal_client::ProviderShareRequest {
        actor_user_id: user.id,
        owner_agent: &state.agent_name,
        provider: &body.provider,
        dest_agent: &body.dest_agent,
    };
    let view = match state.internal_client.provider_share(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_share_failed"),
    };
    let config = match right_agent::agent::parse_agent_config(&dest_dir) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider was shared, but the destination agent.yaml is missing"),
            );
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "failed to reload destination config after provider share");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider was shared, but the destination config could not be loaded"),
            );
        }
    };
    let _agent_guard = match providers.agent_lock(&body.dest_agent).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(dest_agent = %body.dest_agent, error = %format!("{error:#}"), "destination provider lock failed before share");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("destination provider lock failed; provider was not shared"),
            );
        }
    };
    match crate::sandbox_supervisor::hot_reconcile_providers(
        &body.dest_agent,
        std::slice::from_ref(&previous),
        &config,
        providers,
        &sandbox,
    )
    .await
    {
        Ok(()) => Json(view).into_response(),
        Err(error) => {
            tracing::error!(
                dest_agent = %body.dest_agent,
                provider = %body.provider,
                error = %format!("{error:#}"),
                "provider was shared durably but destination sandbox convergence failed"
            );
            holder_convergence_error("share", std::slice::from_ref(&body.dest_agent))
        }
    }
}

pub(crate) async fn handle_unshare(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable; provider was not unshared"),
        );
    };
    let body: ProviderUnshareBody = match parse_json_body(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(sandbox) = state.sandbox() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; provider was not unshared"),
        );
    };
    let previous = match load_current_provider_config(&state, "provider was not unshared") {
        Ok(config) => config,
        Err(response) => return response,
    };
    let target = match providers.get(&state.agent_name, &body.provider).await {
        Ok(record) => record,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "failed to preflight provider unshare target");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider could not be preflighted; it was not unshared"),
            );
        }
    };
    if let Err(error) = sandbox.secret_env_vars().await {
        tracing::error!(error = %format!("{error:#}"), "failed to preflight sandbox provider bindings");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox bindings could not be preflighted; provider was not unshared"),
        );
    }
    let req = right_mcp::internal_client::ProviderUnshareRequest {
        actor_user_id: user.id,
        borrower_agent: &state.agent_name,
        provider: &body.provider,
    };
    let view = match state.internal_client.provider_unshare(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_unshare_failed"),
    };
    let _agent_guard = match providers.agent_lock(&state.agent_name).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "provider lock failed after unshare");
            return holder_convergence_error("unshare", std::slice::from_ref(&state.agent_name));
        }
    };
    let config = match load_provider_config(&state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    state.provider_config.store(Arc::new(config.clone()));
    match crate::sandbox_supervisor::hot_reconcile_providers(
        &state.agent_name,
        std::slice::from_ref(&previous),
        &config,
        providers,
        &sandbox,
    )
    .await
    {
        Ok(()) => {
            tracing::info!(agent = %state.agent_name, provider = %body.provider, env_var = %target.env_var, "dashboard provider unshare revoked the sandbox binding");
            Json(view).into_response()
        }
        Err(error) => {
            tracing::error!(agent = %state.agent_name, provider = %body.provider, env_var = %target.env_var, error = %format!("{error:#}"), "provider was durably unshared but its sandbox binding revocation failed");
            holder_convergence_error("unshare", std::slice::from_ref(&state.agent_name))
        }
    }
}
/// Borrow (pull): the current dashboard agent becomes the destination, the
/// body-supplied `owner_agent` is the source. This is `provider_share` with
/// owner/dest swapped relative to `handle_share`; the backend re-checks that the
/// actor is trusted on BOTH agents, so direction carries no extra privilege.
pub(crate) async fn handle_borrow(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match authenticate_api(&state, &agent, &headers) {
        Ok(u) => u,
        Err(error) => return error.into_response(),
    };
    let _mutation = state.provider_mutation.lock().await;
    let Some(providers) = state.providers.as_ref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_unavailable",
            Some("provider store is unavailable; provider was not borrowed"),
        );
    };
    let body: ProviderBorrowBody = match parse_json_body(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(sandbox) = state.sandbox() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox is unavailable; provider was not borrowed"),
        );
    };
    let previous = match load_current_provider_config(&state, "provider was not borrowed") {
        Ok(config) => config,
        Err(response) => return response,
    };
    if let Err(error) = sandbox.secret_env_vars().await {
        tracing::error!(error = %format!("{error:#}"), "failed to preflight sandbox provider bindings");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_propagation_failed",
            Some("sandbox bindings could not be preflighted; provider was not borrowed"),
        );
    }
    let req = right_mcp::internal_client::ProviderShareRequest {
        actor_user_id: user.id,
        owner_agent: &body.owner_agent,
        provider: &body.provider,
        dest_agent: &state.agent_name,
    };
    let view = match state.internal_client.provider_share(&req).await {
        Ok(view) => view,
        Err(error) => return internal_api_error_response(error, "provider_borrow_failed"),
    };
    let _agent_guard = match providers.agent_lock(&state.agent_name).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "provider lock failed before borrow");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "provider_propagation_failed",
                Some("provider lock failed; provider was not borrowed"),
            );
        }
    };
    let config = match load_provider_config(&state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    state.provider_config.store(Arc::new(config.clone()));
    match crate::sandbox_supervisor::hot_reconcile_providers(
        &state.agent_name,
        std::slice::from_ref(&previous),
        &config,
        providers,
        &sandbox,
    )
    .await
    {
        Ok(()) => Json(view).into_response(),
        Err(error) => {
            tracing::error!(provider = %body.provider, error = %format!("{error:#}"), "provider was borrowed durably but sandbox convergence failed");
            holder_convergence_error("borrow", std::slice::from_ref(&state.agent_name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn holder_failure_detail_names_failed_agents_without_secret_material() {
        let detail = holder_convergence_detail(
            "rotation",
            &[String::from("borrower-a"), String::from("borrower-b")],
        );
        assert!(detail.contains("borrower-a"));
        assert!(detail.contains("borrower-b"));
        assert!(!detail.contains("test-secret-value"));
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

    #[test]
    fn provider_borrow_body_carries_owner_and_provider() {
        // The borrow body names the SOURCE (owner) agent; the destination is the
        // current dashboard agent, supplied server-side — never from the body.
        let body: ProviderBorrowBody = serde_json::from_value(serde_json::json!({
            "owner_agent": "riskoff",
            "provider": "fal"
        }))
        .unwrap();

        assert_eq!(body.owner_agent, "riskoff");
        assert_eq!(body.provider, "fal");
    }
}
