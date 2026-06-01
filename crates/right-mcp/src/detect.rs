use crate::credentials::{is_loopback_url, is_public_url};
use crate::oauth::{
    OAuthDiscovery, OAuthError, canonical_resource_uri, discover_oauth_with_url_policy,
};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Privacy class of a resolved base-URL host. Drives the detection
/// recommendation so private/loopback hostnames never reach OAuth discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedHostClass {
    Loopback,
    PrivateLan,
    PublicOrUnknown,
}

/// Classify resolved addresses using the canonical `ssrf` predicates (the same
/// ones the connect tier enforces). Loopback wins over private-LAN; an empty
/// list (resolution failed) is `PublicOrUnknown` so the caller falls through to
/// discovery, which surfaces the real connect error.
fn classify_resolved_host(addrs: &[IpAddr]) -> ResolvedHostClass {
    if addrs.iter().copied().any(crate::ssrf::is_loopback_addr) {
        ResolvedHostClass::Loopback
    } else if addrs.iter().copied().any(crate::ssrf::is_user_private_lan) {
        ResolvedHostClass::PrivateLan
    } else {
        ResolvedHostClass::PublicOrUnknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpAuthMode {
    #[serde(rename = "oauth")]
    OAuth,
    #[serde(rename = "headers")]
    Headers,
    #[serde(rename = "url_as_is")]
    UrlAsIs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionReason {
    #[serde(rename = "query_string_present")]
    QueryStringPresent,
    #[serde(rename = "loopback_or_private")]
    LoopbackOrPrivate,
    #[serde(rename = "private_network_no_oauth")]
    PrivateNetworkNoOauth,
    #[serde(rename = "oauth_discovered")]
    OAuthDiscovered,
    #[serde(rename = "no_oauth_metadata")]
    NoOAuthMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthDetectionSummary {
    pub resource: String,
    pub scopes: Vec<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAuthDetection {
    /// Query-stripped URL used for OAuth probes and OAuth/header registration.
    /// If `recommended_mode == UrlAsIs` because `reason == QueryStringPresent`,
    /// callers should register the original URL when the user chooses URL-as-is.
    pub bare_url: String,
    pub oauth_discovered: bool,
    pub recommended_mode: McpAuthMode,
    pub reason: DetectionReason,
    pub oauth: Option<OAuthDetectionSummary>,
}

/// Detect the safest MCP authentication mode for a server URL.
///
/// Security contract: the supplied `reqwest::Client` is used for probing
/// untrusted URLs. Callers must configure redirect handling, DNS resolution,
/// and private-address guards appropriate to their trust boundary before
/// passing the client here.
pub async fn detect_mcp_auth(
    client: &reqwest::Client,
    original_url: &str,
) -> Result<McpAuthDetection, OAuthError> {
    detect_mcp_auth_with_url_policy(client, original_url, |_| Ok(())).await
}

/// Detect MCP authentication mode, applying `url_policy` before every OAuth discovery fetch.
pub async fn detect_mcp_auth_with_url_policy<F>(
    client: &reqwest::Client,
    original_url: &str,
    url_policy: F,
) -> Result<McpAuthDetection, OAuthError>
where
    F: Fn(&str) -> Result<(), OAuthError>,
{
    let parsed = reqwest::Url::parse(original_url).map_err(|_| invalid_server_url())?;
    validate_detection_url(&parsed)?;
    let has_query = parsed.query().is_some();
    let mut bare = parsed.clone();
    bare.set_query(None);
    let bare_url = bare.to_string();

    if has_query {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::QueryStringPresent,
            oauth: None,
        });
    }

    if is_loopback_url(original_url) {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::LoopbackOrPrivate,
            oauth: None,
        });
    }

    if !is_public_url(&bare_url) {
        // Private / LAN / Tailscale base URL: OAuth is not supported for local
        // servers (its token_endpoint would be private and rejected by the strict
        // metadata policy). Recommend Headers and skip OAuth discovery.
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::PrivateNetworkNoOauth,
            oauth: None,
        });
    }

    match discover_oauth_with_url_policy(client, &bare_url, url_policy).await {
        Ok(discovery) => Ok(oauth_detected(bare_url, discovery)),
        Err(error) if error.is_no_as_metadata() => Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::NoOAuthMetadata,
            oauth: None,
        }),
        Err(error) => Err(error),
    }
}

fn validate_detection_url(parsed: &reqwest::Url) -> Result<(), OAuthError> {
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_server_url());
    }

    Ok(())
}

fn invalid_server_url() -> OAuthError {
    OAuthError::InvalidServerUrl("unsupported URL form".to_string())
}

fn oauth_detected(bare_url: String, discovery: OAuthDiscovery) -> McpAuthDetection {
    let resource = if discovery.resource.trim().is_empty() {
        canonical_resource_uri(&bare_url).unwrap_or_else(|_| bare_url.clone())
    } else {
        discovery.resource.clone()
    };
    McpAuthDetection {
        bare_url,
        oauth_discovered: true,
        recommended_mode: McpAuthMode::OAuth,
        reason: DetectionReason::OAuthDiscovered,
        oauth: Some(OAuthDetectionSummary {
            resource,
            scopes: discovery.scopes,
            authorization_endpoint: discovery.metadata.authorization_endpoint,
            token_endpoint: discovery.metadata.token_endpoint,
            registration_endpoint: discovery.metadata.registration_endpoint,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        crate::ensure_crypto_provider();
        reqwest::Client::new()
    }

    fn client_resolving(host: &str, addr: std::net::SocketAddr) -> reqwest::Client {
        crate::ensure_crypto_provider();
        reqwest::Client::builder()
            .resolve(host, addr)
            .build()
            .expect("test client should build")
    }

    async fn request_count(server: &wiremock::MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    #[test]
    fn mcp_auth_mode_serializes_with_api_strings() {
        assert_eq!(
            serde_json::to_value(McpAuthMode::OAuth).unwrap(),
            serde_json::json!("oauth")
        );
        assert_eq!(
            serde_json::to_value(McpAuthMode::Headers).unwrap(),
            serde_json::json!("headers")
        );
        assert_eq!(
            serde_json::to_value(McpAuthMode::UrlAsIs).unwrap(),
            serde_json::json!("url_as_is")
        );
    }

    #[test]
    fn detection_reason_serializes_with_api_strings() {
        assert_eq!(
            serde_json::to_value(DetectionReason::QueryStringPresent).unwrap(),
            serde_json::json!("query_string_present")
        );
        assert_eq!(
            serde_json::to_value(DetectionReason::LoopbackOrPrivate).unwrap(),
            serde_json::json!("loopback_or_private")
        );
        assert_eq!(
            serde_json::to_value(DetectionReason::PrivateNetworkNoOauth).unwrap(),
            serde_json::json!("private_network_no_oauth")
        );
        assert_eq!(
            serde_json::to_value(DetectionReason::OAuthDiscovered).unwrap(),
            serde_json::json!("oauth_discovered")
        );
        assert_eq!(
            serde_json::to_value(DetectionReason::NoOAuthMetadata).unwrap(),
            serde_json::json!("no_oauth_metadata")
        );
    }

    #[tokio::test]
    async fn detect_recommends_url_as_is_for_query_string() {
        let server = wiremock::MockServer::start().await;
        let test_host = "mcp-query.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp?key=secret", server_addr.port());

        let result = detect_mcp_auth(&client, &server_url)
            .await
            .expect("query URL should classify without network");

        assert_eq!(
            result.bare_url,
            format!("http://{test_host}:{}/mcp", server_addr.port())
        );
        assert_eq!(result.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(result.reason, DetectionReason::QueryStringPresent);
        assert!(!result.oauth_discovered);
        assert_eq!(request_count(&server).await, 0);
    }

    #[tokio::test]
    async fn detect_rejects_unsupported_scheme_before_query_short_circuit() {
        let client = client();

        let result = detect_mcp_auth(&client, "ftp://example.com/mcp?token=secret").await;

        assert_invalid_server_url(result, &["ftp://example.com/mcp?token=secret", "secret"]);
    }

    #[tokio::test]
    async fn detect_rejects_missing_host_without_echoing_path() {
        let client = client();

        let result = detect_mcp_auth(&client, "https://").await;

        assert_invalid_server_url(result, &["https://"]);
    }

    #[tokio::test]
    async fn detect_rejects_url_userinfo_without_echoing_secret() {
        let client = client();

        let result = detect_mcp_auth(&client, "https://user:secret@example.com/mcp").await;

        assert_invalid_server_url(result, &["user:secret", "secret"]);
    }

    #[tokio::test]
    async fn detect_rejects_fragment_without_echoing_fragment() {
        let client = client();

        let result = detect_mcp_auth(&client, "https://example.com/mcp#secret-fragment").await;

        assert_invalid_server_url(result, &["secret-fragment"]);
    }

    #[tokio::test]
    async fn detect_recommends_url_as_is_for_loopback() {
        let server = wiremock::MockServer::start().await;
        let client = client();
        let server_url = format!("http://127.0.0.1:{}/mcp", server.address().port());

        let result = detect_mcp_auth(&client, &server_url)
            .await
            .expect("loopback URL should classify without network");

        assert_eq!(result.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(result.reason, DetectionReason::LoopbackOrPrivate);
        assert_eq!(request_count(&server).await, 0);
    }

    #[tokio::test]
    async fn detect_recommends_headers_for_private_address() {
        let client = client();

        let result = detect_mcp_auth(&client, "https://192.168.1.1/mcp")
            .await
            .expect("private URL should classify without network");

        assert_eq!(result.bare_url, "https://192.168.1.1/mcp");
        assert!(!result.oauth_discovered);
        assert_eq!(result.recommended_mode, McpAuthMode::Headers);
        assert_eq!(result.reason, DetectionReason::PrivateNetworkNoOauth);
        assert_eq!(result.oauth, None);
    }

    #[tokio::test]
    async fn detect_recommends_headers_for_link_local_address() {
        let client = client();

        let result = detect_mcp_auth(&client, "https://169.254.1.1/mcp")
            .await
            .expect("link-local URL should classify without network");

        assert_eq!(result.bare_url, "https://169.254.1.1/mcp");
        assert!(!result.oauth_discovered);
        assert_eq!(result.recommended_mode, McpAuthMode::Headers);
        assert_eq!(result.reason, DetectionReason::PrivateNetworkNoOauth);
        assert_eq!(result.oauth, None);
    }

    #[tokio::test]
    async fn detect_recommends_oauth_when_discovered() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let test_host = "mcp-oauth.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp", server_addr.port());

        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/mcp/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri())
            })))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&client, &server_url)
            .await
            .expect("detect should complete");

        assert!(result.oauth_discovered);
        assert_eq!(result.bare_url, server_url);
        assert_eq!(result.recommended_mode, McpAuthMode::OAuth);
        assert_eq!(result.reason, DetectionReason::OAuthDiscovered);
        let oauth = result.oauth.expect("OAuth summary should be present");
        assert_eq!(oauth.resource, result.bare_url);
        assert_eq!(
            oauth.authorization_endpoint,
            format!("{}/authorize", server.uri())
        );
        assert_eq!(oauth.token_endpoint, format!("{}/token", server.uri()));
        assert_eq!(oauth.registration_endpoint, None);
        assert_eq!(request_count(&server).await, 6);
    }

    #[tokio::test]
    async fn detect_recommends_headers_when_no_oauth_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let test_host = "mcp-headers.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp", server_addr.port());
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&client, &server_url)
            .await
            .expect("detect should complete");

        assert!(!result.oauth_discovered);
        assert_eq!(result.recommended_mode, McpAuthMode::Headers);
        assert_eq!(result.reason, DetectionReason::NoOAuthMetadata);
    }

    #[tokio::test]
    async fn guarded_detection_rejects_private_resource_metadata_before_fetch() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let test_host = "mcp-guarded-resource.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp", server_addr.port());
        let private_resource_metadata_url =
            format!("http://127.0.0.1:{}/private-resource", server_addr.port());

        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!(r#"Bearer resource_metadata="{private_resource_metadata_url}""#),
            ))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/private-resource"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_servers": [server.uri()]
            })))
            .mount(&server)
            .await;

        let result = detect_mcp_auth_with_url_policy(&client, &server_url, |url| {
            if url.contains("127.0.0.1") {
                Err(OAuthError::DiscoveryFailed(
                    "blocked private literal".to_string(),
                ))
            } else {
                Ok(())
            }
        })
        .await;

        match result {
            Err(OAuthError::DiscoveryFailed(detail)) => {
                assert!(
                    detail.contains("blocked private literal"),
                    "unexpected discovery error: {detail}"
                );
            }
            Ok(result) => panic!("private resource metadata URL must not classify as {result:?}"),
            Err(error) => panic!("unexpected OAuth error: {error}"),
        }
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn detect_propagates_probe_hard_failure_when_no_oauth_metadata() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let test_host = "mcp-hard-failure.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp", server_addr.port());

        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&client, &server_url).await;

        match result {
            Err(OAuthError::DiscoveryFailed(detail)) => {
                assert!(
                    detail.contains("no AS metadata found"),
                    "unexpected discovery error: {detail}"
                );
            }
            Ok(result) => panic!("hard probe failure must not be classified as {result:?}"),
            Err(error) => panic!("unexpected OAuth error: {error}"),
        }
    }

    #[tokio::test]
    async fn detect_propagates_malformed_oauth_metadata() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let test_host = "mcp-malformed.test";
        let server_addr = *server.address();
        let client = client_resolving(test_host, server_addr);
        let server_url = format!("http://{test_host}:{}/mcp", server_addr.port());

        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/mcp/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_endpoint": format!("{}/authorize", server.uri())
            })))
            .mount(&server)
            .await;

        let result = detect_mcp_auth(&client, &server_url).await;

        match result {
            Err(OAuthError::DiscoveryFailed(detail)) => {
                assert!(
                    detail.contains("failed to parse AS metadata response"),
                    "unexpected discovery error: {detail}"
                );
            }
            Ok(result) => panic!("malformed metadata must not be classified as {result:?}"),
            Err(error) => panic!("unexpected OAuth error: {error}"),
        }
    }

    #[tokio::test]
    async fn private_base_url_recommends_headers_not_oauth() {
        let client = client();
        // 100.64/10 is RFC 6598 CGNAT (Tailscale) — !is_public_url, not loopback.
        let d = detect_mcp_auth(&client, "http://100.85.147.49:27123/mcp")
            .await
            .unwrap();
        assert_eq!(d.recommended_mode, McpAuthMode::Headers);
        assert_eq!(d.reason, DetectionReason::PrivateNetworkNoOauth);
        assert!(!d.oauth_discovered);
        assert!(d.oauth.is_none());

        // RFC 1918 private range routes the same way.
        let rfc1918 = detect_mcp_auth(&client, "http://192.168.10.50:27123/mcp")
            .await
            .unwrap();
        assert_eq!(rfc1918.recommended_mode, McpAuthMode::Headers);
        assert_eq!(rfc1918.reason, DetectionReason::PrivateNetworkNoOauth);
    }

    #[tokio::test]
    async fn loopback_base_url_still_recommends_url_as_is() {
        let client = client();
        let d = detect_mcp_auth(&client, "http://127.0.0.1:8080/mcp")
            .await
            .unwrap();
        assert_eq!(d.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(d.reason, DetectionReason::LoopbackOrPrivate);
    }

    #[test]
    fn classify_resolved_host_maps_addresses() {
        use std::net::IpAddr;
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();

        // loopback takes precedence
        assert_eq!(
            classify_resolved_host(&[ip("127.0.0.1")]),
            ResolvedHostClass::Loopback
        );
        assert_eq!(
            classify_resolved_host(&[ip("::1")]),
            ResolvedHostClass::Loopback
        );
        // private-LAN families
        assert_eq!(
            classify_resolved_host(&[ip("10.0.0.5")]),
            ResolvedHostClass::PrivateLan
        );
        assert_eq!(
            classify_resolved_host(&[ip("100.85.147.49")]),
            ResolvedHostClass::PrivateLan
        );
        assert_eq!(
            classify_resolved_host(&[ip("fc00::1")]),
            ResolvedHostClass::PrivateLan
        );
        // public, empty, and unknown all fall through
        assert_eq!(
            classify_resolved_host(&[ip("8.8.8.8")]),
            ResolvedHostClass::PublicOrUnknown
        );
        assert_eq!(
            classify_resolved_host(&[]),
            ResolvedHostClass::PublicOrUnknown
        );
        // mixed: any loopback wins; else any private-LAN
        assert_eq!(
            classify_resolved_host(&[ip("8.8.8.8"), ip("127.0.0.1")]),
            ResolvedHostClass::Loopback
        );
        assert_eq!(
            classify_resolved_host(&[ip("8.8.8.8"), ip("10.0.0.5")]),
            ResolvedHostClass::PrivateLan
        );
    }

    fn assert_invalid_server_url(
        result: Result<McpAuthDetection, OAuthError>,
        forbidden_details: &[&str],
    ) {
        match result {
            Err(OAuthError::InvalidServerUrl(detail)) => {
                assert!(
                    !detail.trim().is_empty(),
                    "invalid URL error should include a neutral reason"
                );
                for forbidden in forbidden_details {
                    assert!(
                        !detail.contains(forbidden),
                        "invalid URL error must not echo {forbidden:?}: {detail}"
                    );
                }
            }
            Ok(result) => panic!("invalid URL must not be classified as {result:?}"),
            Err(error) => panic!("unexpected OAuth error: {error}"),
        }
    }
}
