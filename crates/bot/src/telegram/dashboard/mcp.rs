use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr as _;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{DashboardState, authenticate_api, json_error};

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpServersResponse {
    pub agent: String,
    pub servers: Vec<DashboardMcpServer>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpServer {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    pub auth_type: Option<String>,
    pub header_names: Vec<String>,
    pub protected: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpDetectRequest {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpAddRequest {
    pub name: String,
    pub url: String,
    pub mode: right_mcp::detect::McpAuthMode,
    #[serde(default)]
    pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardMcpHeadersRequest {
    #[serde(default)]
    pub headers: Vec<right_mcp::internal_client::HttpHeaderInput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpMutationResponse {
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardMcpOAuthStartResponse {
    pub auth_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardOAuthCallbackRedirectUriError {
    MissingTunnel,
    InvalidTunnelHostname,
}

pub(crate) async fn handle_mcp_servers(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    match state.internal_client.mcp_list(&state.agent_name).await {
        Ok(response) => {
            let servers = response
                .servers
                .into_iter()
                .map(|server| DashboardMcpServer {
                    protected: is_protected_mcp_server(&server.name),
                    name: server.name,
                    url: server.url,
                    status: server.status,
                    tool_count: server.tool_count,
                    auth_type: server.auth_type,
                    header_names: server.header_names,
                })
                .collect();
            Json(DashboardMcpServersResponse {
                agent: state.agent_name,
                servers,
            })
            .into_response()
        }
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "mcp_unavailable",
            Some(&format!("{error:#}")),
        ),
    }
}

pub(crate) async fn handle_mcp_detect(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let request: DashboardMcpDetectRequest = match parse_json_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if !is_valid_mcp_detection_url(&request.url) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_url",
            Some("invalid MCP URL"),
        );
    }

    let client = match detection_http_client() {
        Ok(client) => client,
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "detect_client_failed",
                Some(&format!("{error:#}")),
            );
        }
    };

    match right_mcp::detect::detect_mcp_auth_with_url_policy(
        &client,
        &request.url,
        mcp_detection_url_policy,
    )
    .await
    {
        Ok(result) => Json(result).into_response(),
        Err(right_mcp::oauth::OAuthError::InvalidServerUrl(_)) => json_error(
            StatusCode::BAD_REQUEST,
            "invalid_url",
            Some("invalid MCP URL"),
        ),
        Err(right_mcp::oauth::OAuthError::DiscoveryFailed(detail))
            if detail.contains("invalid server URL") =>
        {
            json_error(
                StatusCode::BAD_REQUEST,
                "invalid_url",
                Some("invalid MCP URL"),
            )
        }
        Err(right_mcp::oauth::OAuthError::DiscoveryFailed(detail))
            if detail.contains(PUBLIC_DNS_ERROR_MARKER) =>
        {
            json_error(
                StatusCode::BAD_REQUEST,
                "invalid_url",
                Some("MCP URL must resolve to a public network address"),
            )
        }
        Err(error) => json_error(
            StatusCode::BAD_GATEWAY,
            "detect_failed",
            Some(&format!("{error:#}")),
        ),
    }
}

pub(crate) async fn handle_mcp_add(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let request: DashboardMcpAddRequest = match parse_json_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let DashboardMcpAddRequest {
        name,
        url,
        mode,
        headers: request_headers,
    } = request;
    let (auth_type, mcp_headers) = match mode {
        right_mcp::detect::McpAuthMode::OAuth => (Some("oauth"), Vec::new()),
        right_mcp::detect::McpAuthMode::Headers => (Some("headers"), request_headers),
        right_mcp::detect::McpAuthMode::UrlAsIs => (None, Vec::new()),
    };

    let add = right_mcp::internal_client::McpAddRequest {
        agent: &state.agent_name,
        name: &name,
        url: &url,
        auth_type,
        auth_header: None,
        auth_token: None,
        headers: mcp_headers,
    };

    match state.internal_client.mcp_add_request(&add).await {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => internal_api_error_response(error, "mcp_add_failed"),
    }
}

pub(crate) async fn handle_mcp_headers(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    let request: DashboardMcpHeadersRequest = match parse_json_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if is_protected_mcp_server(&server_name) {
        return json_error(
            StatusCode::FORBIDDEN,
            "protected_mcp",
            Some("protected MCP server cannot be modified"),
        );
    }

    let update = right_mcp::internal_client::McpSetHeadersRequest {
        agent: state.agent_name.clone(),
        name: server_name,
        headers: request.headers,
    };
    match state.internal_client.mcp_set_headers(&update).await {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => internal_api_error_response(error, "mcp_headers_failed"),
    }
}

pub(crate) async fn handle_mcp_oauth_start(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let server = match state.internal_client.mcp_list(&state.agent_name).await {
        Ok(list) => list
            .servers
            .into_iter()
            .find(|server| server.name == server_name),
        Err(error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "mcp_list_failed",
                Some(&format!("{error:#}")),
            );
        }
    };
    let Some(server) = server else {
        return json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            Some("MCP server not found"),
        );
    };
    let Some(server_url) = server.url else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "mcp_server_missing_url",
            Some("MCP server has no URL"),
        );
    };

    let global_config = match right_config::read_global_config(&state.home) {
        Ok(config) => config,
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_failed",
                Some(&format!("{error:#}")),
            );
        }
    };
    let redirect_uri = match dashboard_oauth_callback_redirect_uri(
        &global_config.tunnel.hostname,
        &state.agent_name,
    ) {
        Ok(uri) => uri,
        Err(DashboardOAuthCallbackRedirectUriError::MissingTunnel) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "tunnel_missing",
                Some("tunnel hostname is not configured"),
            );
        }
        Err(DashboardOAuthCallbackRedirectUriError::InvalidTunnelHostname) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_tunnel_hostname",
                Some("tunnel hostname must be a bare host"),
            );
        }
    };

    let http_client = reqwest::Client::new();
    let discovery = match right_mcp::oauth::discover_oauth(&http_client, &server_url).await {
        Ok(discovery) => discovery,
        Err(_error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "oauth_discovery_failed",
                Some("OAuth discovery failed"),
            );
        }
    };
    let scopes = discovery.scopes;
    let scope_param = right_mcp::oauth::scope_param(&scopes);
    let scope_refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let (client_id, client_secret) = match right_mcp::oauth::register_client_or_fallback(
        &http_client,
        &discovery.metadata,
        None,
        &redirect_uri,
        &scope_refs,
    )
    .await
    {
        Ok(pair) => pair,
        Err(_error) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "client_registration_failed",
                Some("OAuth client registration failed"),
            );
        }
    };

    let (code_verifier, code_challenge) = right_mcp::oauth::generate_pkce();
    let oauth_state = right_mcp::oauth::generate_state();
    state.pending_auth.lock().await.insert(
        oauth_state.clone(),
        right_mcp::oauth::PendingAuth {
            server_name: server_name.clone(),
            server_url,
            resource: discovery.resource.clone(),
            code_verifier,
            state: oauth_state.clone(),
            token_endpoint: discovery.metadata.token_endpoint.clone(),
            client_id: client_id.clone(),
            client_secret,
            redirect_uri: redirect_uri.clone(),
            created_at: std::time::Instant::now(),
        },
    );

    let auth_url = right_mcp::oauth::build_auth_url(
        &discovery.metadata,
        &client_id,
        &redirect_uri,
        &oauth_state,
        &code_challenge,
        &discovery.resource,
        scope_param.as_deref(),
    );

    Json(DashboardMcpOAuthStartResponse { auth_url }).into_response()
}

pub(crate) async fn handle_mcp_remove(
    AxumPath((agent, server_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }
    if is_protected_mcp_server(&server_name) {
        return json_error(
            StatusCode::FORBIDDEN,
            "protected_mcp",
            Some("protected MCP server cannot be removed"),
        );
    }

    match state
        .internal_client
        .mcp_remove(&state.agent_name, &server_name)
        .await
    {
        Ok(_) => Json(DashboardMcpMutationResponse { ok: true }).into_response(),
        Err(error) => internal_api_error_response(error, "mcp_remove_failed"),
    }
}

const PUBLIC_DNS_ERROR_MARKER: &str = "MCP detection DNS resolved to a non-public address";

#[derive(Debug, Clone, Copy)]
struct PublicNetworkResolver;

impl reqwest::dns::Resolve for PublicNetworkResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let public_addrs = addrs
                .filter(|addr| is_public_ip(addr.ip()))
                .collect::<Vec<_>>();
            if public_addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{PUBLIC_DNS_ERROR_MARKER}: {host}"),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(public_addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn detection_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(PublicNetworkResolver))
        .build()
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let ip = u32::from(ip);
    !([0, 10, 127].iter().any(|octet| ip >> 24 == *octet)
        || in_ipv4_cidr(ip, Ipv4Addr::new(100, 64, 0, 0), 10)
        || in_ipv4_cidr(ip, Ipv4Addr::new(169, 254, 0, 0), 16)
        || in_ipv4_cidr(ip, Ipv4Addr::new(172, 16, 0, 0), 12)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 0, 0, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 0, 2, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 88, 99, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 168, 0, 0), 16)
        || in_ipv4_cidr(ip, Ipv4Addr::new(198, 18, 0, 0), 15)
        || in_ipv4_cidr(ip, Ipv4Addr::new(198, 51, 100, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(203, 0, 113, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(224, 0, 0, 0), 4)
        || in_ipv4_cidr(ip, Ipv4Addr::new(240, 0, 0, 0), 4))
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }

    let ip = u128::from(ip);
    !(ip == 0
        || ip == 1
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("64:ff9b::").unwrap(), 96)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("64:ff9b:1::").unwrap(), 48)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("100::").unwrap(), 64)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2001:2::").unwrap(), 48)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2001:db8::").unwrap(), 32)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2002::").unwrap(), 16)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("fc00::").unwrap(), 7)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("fe80::").unwrap(), 10)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("ff00::").unwrap(), 8))
}

fn in_ipv4_cidr(ip: u32, base: Ipv4Addr, prefix: u32) -> bool {
    let mask = u32::MAX << (32 - prefix);
    (ip & mask) == (u32::from(base) & mask)
}

fn in_ipv6_cidr(ip: u128, base: Ipv6Addr, prefix: u32) -> bool {
    let mask = u128::MAX << (128 - prefix);
    (ip & mask) == (u128::from(base) & mask)
}

fn parse_json_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|error| {
        tracing::warn!("dashboard MCP request rejected malformed body: {error:#}");
        json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            Some("invalid MCP request body"),
        )
    })
}

fn internal_api_error_response(
    error: right_mcp::internal_client::InternalClientError,
    fallback_error: &'static str,
) -> Response {
    match error {
        right_mcp::internal_client::InternalClientError::Server { status, body } => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(value) => (status, Json(value)).into_response(),
                Err(_) => json_error(
                    status,
                    fallback_error,
                    Some("internal API returned an error"),
                ),
            }
        }
        error => json_error(
            StatusCode::BAD_GATEWAY,
            fallback_error,
            Some(&format!("{error:#}")),
        ),
    }
}

fn dashboard_oauth_callback_redirect_uri(
    tunnel_hostname: &str,
    agent_name: &str,
) -> Result<String, DashboardOAuthCallbackRedirectUriError> {
    let tunnel_hostname = tunnel_hostname.trim();
    if tunnel_hostname.is_empty() {
        return Err(DashboardOAuthCallbackRedirectUriError::MissingTunnel);
    }

    let base = url::Url::parse(&format!("https://{tunnel_hostname}/"))
        .map_err(|_| DashboardOAuthCallbackRedirectUriError::InvalidTunnelHostname)?;
    if base.host_str().is_none()
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(DashboardOAuthCallbackRedirectUriError::InvalidTunnelHostname);
    }

    Ok(format!(
        "https://{tunnel_hostname}/oauth/{agent_name}/callback"
    ))
}

fn is_protected_mcp_server(server_name: &str) -> bool {
    server_name == right_mcp::PROTECTED_MCP_SERVER || server_name == "rightmeta"
}

fn is_valid_mcp_detection_url(input: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(input) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
        && parsed.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => !is_localhost_domain(domain),
            url::Host::Ipv4(ip) => is_public_ipv4(ip),
            url::Host::Ipv6(ip) => is_public_ipv6(ip),
        })
}

fn is_localhost_domain(domain: &str) -> bool {
    domain
        .trim_end_matches('.')
        .eq_ignore_ascii_case("localhost")
}

fn mcp_detection_url_policy(input: &str) -> Result<(), right_mcp::oauth::OAuthError> {
    if is_valid_mcp_detection_url(input) {
        return Ok(());
    }

    Err(right_mcp::oauth::OAuthError::DiscoveryFailed(
        PUBLIC_DNS_ERROR_MARKER.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::dns::Resolve as _;
    use right_mcp::internal_client::InternalClientError;

    #[test]
    fn public_ip_guard_rejects_private_addresses() {
        assert!(!is_public_ip(IpAddr::from([127, 0, 0, 1])));
        assert!(!is_public_ip(IpAddr::from([10, 0, 0, 1])));
        assert!(!is_public_ip(IpAddr::from([100, 64, 0, 1])));
        assert!(!is_public_ip(IpAddr::from([192, 168, 1, 1])));
        assert!(!is_public_ip(IpAddr::from([169, 254, 1, 1])));
        assert!(!is_public_ip(IpAddr::from([198, 18, 0, 1])));
        assert!(!is_public_ip(IpAddr::from([203, 0, 113, 1])));
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_public_ip(IpAddr::V6(
            Ipv6Addr::from_str("fc00::1").unwrap()
        )));
        assert!(!is_public_ip(IpAddr::V6(
            Ipv6Addr::from_str("2001:db8::1").unwrap()
        )));
        assert!(!is_public_ip(IpAddr::V6(
            Ipv6Addr::from_str("64:ff9b::7f00:1").unwrap()
        )));
        assert!(is_public_ip(IpAddr::from([8, 8, 8, 8])));
    }

    #[tokio::test]
    async fn public_network_resolver_rejects_localhost() {
        let resolver = PublicNetworkResolver;
        let name = reqwest::dns::Name::from_str("localhost").unwrap();

        let result = resolver.resolve(name).await;
        let Err(err) = result else {
            panic!("localhost must be rejected");
        };

        assert!(err.to_string().contains(PUBLIC_DNS_ERROR_MARKER));
    }

    #[test]
    fn mcp_detection_url_validation_rejects_localhost_domains() {
        for url in [
            "http://localhost:8080/mcp",
            "http://localhost.:8080/mcp",
            "http://LOCALHOST:8080/mcp",
            "https://LocalHost./mcp",
        ] {
            assert!(
                !is_valid_mcp_detection_url(url),
                "localhost domain URL must be rejected: {url}"
            );
        }
    }

    #[test]
    fn mcp_detection_url_validation_allows_public_domains() {
        assert!(is_valid_mcp_detection_url("https://mcp.example.com/mcp"));
    }

    #[test]
    fn dashboard_oauth_callback_redirect_uri_accepts_bare_host() {
        let uri = dashboard_oauth_callback_redirect_uri("right.example.com:8443", "alpha").unwrap();

        assert_eq!(uri, "https://right.example.com:8443/oauth/alpha/callback");
    }

    #[test]
    fn dashboard_oauth_callback_redirect_uri_rejects_empty_hostname() {
        let error = dashboard_oauth_callback_redirect_uri("  ", "alpha")
            .expect_err("empty tunnel hostname must be rejected");

        assert_eq!(error, DashboardOAuthCallbackRedirectUriError::MissingTunnel);
    }

    #[test]
    fn dashboard_oauth_callback_redirect_uri_rejects_non_bare_hostname() {
        for hostname in [
            "https://right.example.com",
            "right.example.com/oauth",
            "right.example.com?token=abc",
            "user@right.example.com",
            "right.example.com#fragment",
        ] {
            let error = dashboard_oauth_callback_redirect_uri(hostname, "alpha")
                .expect_err("non-bare tunnel hostname must be rejected");

            assert_eq!(
                error,
                DashboardOAuthCallbackRedirectUriError::InvalidTunnelHostname
            );
        }
    }

    #[test]
    fn mcp_detection_url_policy_rejects_private_literal_without_echoing_url() {
        let error =
            mcp_detection_url_policy("http://127.0.0.1:8080/.well-known/oauth-protected-resource")
                .expect_err("private literal URLs must be rejected");

        let detail = error.to_string();
        assert!(detail.contains(PUBLIC_DNS_ERROR_MARKER));
        assert!(!detail.contains("127.0.0.1"));
    }

    #[test]
    fn mcp_detection_url_policy_rejects_localhost_domains_without_echoing_url() {
        let error =
            mcp_detection_url_policy("http://LOCALHOST.:8080/.well-known/oauth-protected-resource")
                .expect_err("localhost domain URLs must be rejected");

        let detail = error.to_string();
        assert!(detail.contains(PUBLIC_DNS_ERROR_MARKER));
        assert!(!detail.contains("LOCALHOST"));
    }

    #[tokio::test]
    async fn internal_api_error_response_preserves_json_status() {
        let response = internal_api_error_response(
            InternalClientError::Server {
                status: StatusCode::NOT_FOUND.as_u16(),
                body: serde_json::json!({
                    "error": "not_found",
                    "detail": "server not found"
                })
                .to_string(),
            },
            "mcp_add_failed",
        );

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "not_found");
        assert_eq!(value["detail"], "server not found");
    }
}
