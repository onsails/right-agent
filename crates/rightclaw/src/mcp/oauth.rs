use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use tracing::debug;

/// Errors that can occur during the OAuth 2.1 + PKCE flow.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("AS discovery failed for server: {0}")]
    DiscoveryFailed(String),
    #[error("Dynamic Client Registration failed: {0}")]
    DcrFailed(String),
    #[error("Token exchange failed: {0}")]
    TokenExchangeFailed(String),
    #[error("No browser auth available (tunnel not configured or unreachable)")]
    NoBrowserAuth,
    #[error("Invalid OAuth state token — possible CSRF attempt")]
    InvalidState,
    #[error("Missing client_id: server has no registration_endpoint and .mcp.json has no clientId")]
    MissingClientId,
    #[error("Missing endpoint: {0}")]
    MissingEndpoint(String),
}

/// In-flight OAuth session stored in bot memory until callback arrives.
pub struct PendingAuth {
    pub server_name: String,
    pub server_url: String,
    pub code_verifier: String,
    pub state: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub created_at: std::time::Instant,
}

/// Authorization Server Metadata (RFC 8414 / OIDC well-known).
#[derive(Debug, Deserialize)]
pub struct AsMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub code_challenge_methods_supported: Option<Vec<String>>,
}

/// Resource Metadata (RFC 9728) — points to the Authorization Server.
#[derive(Debug, Deserialize)]
pub struct ResourceMetadata {
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Option<Vec<String>>,
}

/// Dynamic Client Registration response (RFC 7591).
#[derive(Debug, Deserialize)]
pub struct DcrResponse {
    pub client_id: String,
    pub client_secret: Option<String>,
}

/// Token response from the authorization server.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub expires_in: Option<u64>,
}

/// Generate a PKCE code_verifier and code_challenge (S256 method).
///
/// Returns `(code_verifier, code_challenge)` where both are base64url-no-pad encoded.
/// - verifier: 32 random bytes → 43-char base64url string
/// - challenge: SHA-256(verifier_bytes) → 43-char base64url string (per RFC 7636 §4.2)
pub fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
    let hash = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hash);
    (code_verifier, code_challenge)
}

/// Generate a cryptographically random OAuth state token.
///
/// Returns a 22-char base64url-no-pad string (16 random bytes).
pub fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Verify an OAuth state token in constant time to prevent timing attacks.
///
/// Per D-05: uses `subtle::ConstantTimeEq` to defeat side-channel attacks.
pub fn verify_state(expected: &str, received: &str) -> bool {
    expected.as_bytes().ct_eq(received.as_bytes()).into()
}

// ---------------------------------------------------------------------------
// Task 1: AS discovery — RFC 9728 → RFC 8414 → OIDC fallback chain
// ---------------------------------------------------------------------------

/// Ordered list of discovery URLs to try for the given server URL.
///
/// Returns URLs in discovery priority order:
/// 1. RFC 9728 resource metadata
/// 2. RFC 8414 AS metadata (no-path)
/// 3. OIDC well-known (no-path)
///
/// This helper is exposed for unit-testing URL construction without HTTP.
pub fn discovery_urls(server_url: &str) -> Vec<String> {
    let parsed = match reqwest::Url::parse(server_url) {
        Ok(u) => u,
        Err(_) => return vec![],
    };
    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
    let port_str = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let origin = format!("{origin}{port_str}");

    // Path component (strip trailing slash)
    let raw_path = parsed.path();
    let path = raw_path.trim_end_matches('/');

    let mut urls = Vec::new();

    // Step 1 — RFC 9728 resource metadata
    if path.is_empty() || path == "/" {
        urls.push(format!("{origin}/.well-known/oauth-protected-resource"));
    } else {
        urls.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }

    // Steps 2 & 3 — RFC 8414 and OIDC (no-path variants used here; path variants
    // require the AS URL from the RFC 9728 response, resolved at runtime)
    urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
    urls.push(format!("{origin}/.well-known/openid-configuration"));

    urls
}

/// Build the ordered list of AS metadata URLs for a given AS URL.
///
/// Used in Steps 2 & 3 of `discover_as` after obtaining the AS URL from RFC 9728.
fn as_metadata_urls(as_url: &str) -> Vec<String> {
    let parsed = match reqwest::Url::parse(as_url) {
        Ok(u) => u,
        Err(_) => return vec![],
    };
    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
    let port_str = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let origin = format!("{origin}{port_str}");

    let raw_path = parsed.path();
    let path = raw_path.trim_end_matches('/');

    let mut urls = Vec::new();

    if path.is_empty() || path == "/" {
        // No-path AS URL: try RFC 8414, then OIDC
        urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
        urls.push(format!("{origin}/.well-known/openid-configuration"));
    } else {
        // Path-bearing AS URL: try all variants
        urls.push(format!("{origin}/.well-known/oauth-authorization-server{path}"));
        urls.push(format!("{origin}/.well-known/openid-configuration{path}"));
        urls.push(format!("{origin}{path}/.well-known/openid-configuration"));
    }

    urls
}

/// Discover the Authorization Server metadata for a given MCP server URL.
///
/// Implements the 3-step fallback chain mandated by the MCP Authorization spec:
/// 1. RFC 9728 resource metadata (`/.well-known/oauth-protected-resource`)
/// 2. RFC 8414 AS metadata (`/.well-known/oauth-authorization-server`)
/// 3. OIDC discovery (`/.well-known/openid-configuration`)
///
/// Per D-07: 404 → try next fallback. 5xx → abort immediately.
pub async fn discover_as(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<AsMetadata, OAuthError> {
    let parsed = reqwest::Url::parse(server_url)
        .map_err(|e| OAuthError::DiscoveryFailed(format!("invalid server URL {server_url}: {e}")))?;
    let origin = {
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        format!("{scheme}://{host}{port}")
    };

    let raw_path = parsed.path();
    let path = raw_path.trim_end_matches('/');

    // --- Step 1: RFC 9728 resource metadata ---
    let rfc9728_url = if path.is_empty() || path == "/" {
        format!("{origin}/.well-known/oauth-protected-resource")
    } else {
        format!("{origin}/.well-known/oauth-protected-resource{path}")
    };

    debug!("discover_as: trying RFC 9728 resource metadata at {rfc9728_url}");
    let as_url: Option<String> = match client.get(&rfc9728_url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let meta: ResourceMetadata = resp.json().await.map_err(|e| {
                    OAuthError::DiscoveryFailed(format!(
                        "failed to parse RFC 9728 response from {rfc9728_url}: {e}"
                    ))
                })?;
                debug!("discover_as: RFC 9728 succeeded, AS URL = {:?}", meta.authorization_servers.first());
                Some(
                    meta.authorization_servers
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            OAuthError::DiscoveryFailed(
                                "RFC 9728 authorization_servers array is empty".to_string(),
                            )
                        })?,
                )
            } else if status.as_u16() == 404 {
                debug!("discover_as: RFC 9728 returned 404, falling back to RFC 8414");
                None
            } else {
                // 5xx or other error → abort immediately
                return Err(OAuthError::DiscoveryFailed(format!(
                    "RFC 9728 server error {status} at {rfc9728_url}"
                )));
            }
        }
        Err(e) => {
            // Connection error (server not running, etc.) → abort
            return Err(OAuthError::DiscoveryFailed(format!(
                "RFC 9728 request failed for {rfc9728_url}: {e}"
            )));
        }
    };

    // --- Steps 2 & 3: AS metadata + OIDC using the AS URL (or server_url as fallback) ---
    let as_base = as_url.as_deref().unwrap_or(server_url);
    let as_meta_urls = as_metadata_urls(as_base);

    for url in &as_meta_urls {
        debug!("discover_as: trying AS metadata at {url}");
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let meta: AsMetadata = resp.json().await.map_err(|e| {
                        OAuthError::DiscoveryFailed(format!(
                            "failed to parse AS metadata from {url}: {e}"
                        ))
                    })?;
                    debug!("discover_as: succeeded via {url}");
                    return Ok(meta);
                } else if status.as_u16() == 404 {
                    debug!("discover_as: {url} returned 404, trying next");
                    continue;
                } else {
                    return Err(OAuthError::DiscoveryFailed(format!(
                        "AS metadata server error {status} at {url}"
                    )));
                }
            }
            Err(e) => {
                return Err(OAuthError::DiscoveryFailed(format!(
                    "AS metadata request failed for {url}: {e}"
                )));
            }
        }
    }

    Err(OAuthError::DiscoveryFailed(format!(
        "no AS metadata found for {server_url}"
    )))
}

// ---------------------------------------------------------------------------
// Task 2: DCR, auth URL builder, and token exchange
// ---------------------------------------------------------------------------

/// DCR request body (RFC 7591).
#[derive(Debug, Serialize)]
struct DcrRequest<'a> {
    client_name: &'a str,
    redirect_uris: Vec<&'a str>,
    grant_types: Vec<&'a str>,
    response_types: Vec<&'a str>,
    token_endpoint_auth_method: &'a str,
    application_type: &'a str,
}

/// Register this client via Dynamic Client Registration (RFC 7591).
///
/// POSTs to `registration_endpoint` with the standard native-app fields.
/// Returns the server-assigned `client_id` (and optional `client_secret`).
pub async fn register_client(
    client: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<DcrResponse, OAuthError> {
    let body = DcrRequest {
        client_name: "RightClaw",
        redirect_uris: vec![redirect_uri],
        grant_types: vec!["authorization_code"],
        response_types: vec!["code"],
        token_endpoint_auth_method: "none",
        application_type: "native",
    };

    debug!("register_client: POST to {registration_endpoint}");
    let resp = client
        .post(registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| OAuthError::DcrFailed(format!("DCR request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::DcrFailed(format!(
            "DCR returned {status} from {registration_endpoint}: {text}"
        )));
    }

    let dcr: DcrResponse = resp.json().await.map_err(|e| {
        OAuthError::DcrFailed(format!("failed to parse DCR response from {registration_endpoint}: {e}"))
    })?;

    Ok(dcr)
}

/// Try DCR; fall back to a static `client_id` from `.mcp.json` configuration.
///
/// Per D-08: if `registration_endpoint` is absent, use `static_client_id`.
/// If neither is available, return `OAuthError::MissingClientId`.
pub async fn register_client_or_fallback(
    client: &reqwest::Client,
    metadata: &AsMetadata,
    static_client_id: Option<&str>,
    redirect_uri: &str,
) -> Result<(String, Option<String>), OAuthError> {
    if let Some(reg_ep) = &metadata.registration_endpoint {
        let dcr = register_client(client, reg_ep, redirect_uri).await?;
        Ok((dcr.client_id, dcr.client_secret))
    } else if let Some(id) = static_client_id {
        debug!("register_client_or_fallback: using static client_id (no registration_endpoint)");
        Ok((id.to_string(), None))
    } else {
        Err(OAuthError::MissingClientId)
    }
}

/// Build the authorization URL with PKCE S256 parameters.
///
/// URL-encodes all parameter values. Appends `scope` only when `Some`.
pub fn build_auth_url(
    metadata: &AsMetadata,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    scope: Option<&str>,
) -> String {
    let encode = |s: &str| {
        s.chars()
            .flat_map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    vec![c as u8]
                }
                _ => format!("%{:02X}", c as u8).into_bytes(),
            })
            .map(|b| b as char)
            .collect::<String>()
    };

    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        metadata.authorization_endpoint,
        encode(client_id),
        encode(redirect_uri),
        encode(state),
        encode(code_challenge),
    );

    if let Some(s) = scope {
        url.push_str(&format!("&scope={}", encode(s)));
    }

    url
}

/// Exchange an authorization code for tokens (authorization_code grant with PKCE).
///
/// Sends a form-encoded POST to `token_endpoint`.
/// Includes `client_secret` in the form body when `Some`.
pub async fn exchange_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code_verifier: &str,
) -> Result<TokenResponse, OAuthError> {
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
    ];

    let secret_owned;
    if let Some(s) = client_secret {
        secret_owned = s.to_string();
        params.push(("client_secret", &secret_owned));
    }

    debug!("exchange_token: POST to {token_endpoint}");
    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| OAuthError::TokenExchangeFailed(format!("token exchange request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::TokenExchangeFailed(format!(
            "token endpoint returned {status}: {text}"
        )));
    }

    let token_resp: TokenResponse = resp.json().await.map_err(|e| {
        OAuthError::TokenExchangeFailed(format!(
            "failed to parse token response from {token_endpoint}: {e}"
        ))
    })?;

    Ok(token_resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_pkce_verifier_is_43_chars() {
        let (verifier, _) = generate_pkce();
        assert_eq!(verifier.len(), 43, "code_verifier should be 43 chars (32 bytes base64url-no-pad)");
    }

    #[test]
    fn generate_pkce_challenge_is_43_chars() {
        let (_, challenge) = generate_pkce();
        assert_eq!(challenge.len(), 43, "code_challenge should be 43 chars (SHA-256 base64url-no-pad)");
    }

    #[test]
    fn generate_pkce_challenge_matches_s256_of_verifier() {
        let (verifier, challenge) = generate_pkce();
        let expected_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected_challenge, "code_challenge must be S256(code_verifier)");
    }

    #[test]
    fn generate_state_is_22_chars() {
        let state = generate_state();
        assert_eq!(state.len(), 22, "state should be 22 chars (16 bytes base64url-no-pad)");
    }

    #[test]
    fn verify_state_returns_true_for_matching() {
        let state = generate_state();
        assert!(verify_state(&state, &state), "matching states should return true");
    }

    #[test]
    fn verify_state_returns_false_for_nonmatching() {
        let a = generate_state();
        let b = generate_state();
        // Extremely unlikely to collide (128-bit random), but handle it defensively
        if a != b {
            assert!(!verify_state(&a, &b), "different states should return false");
        }
    }

    #[test]
    fn verify_state_returns_false_for_different_lengths() {
        let state = generate_state();
        let short = &state[..10];
        assert!(!verify_state(&state, short), "different-length states should return false");
    }

    // Task 1: AS discovery URL construction tests
    #[test]
    fn discovery_urls_with_path_rfc9728_appends_path() {
        let urls = discovery_urls("https://mcp.example.com/mcp");
        // RFC 9728: path appended after .well-known/oauth-protected-resource
        assert_eq!(urls[0], "https://mcp.example.com/.well-known/oauth-protected-resource/mcp");
    }

    #[test]
    fn discovery_urls_without_path_rfc9728_no_trailing_slash() {
        let urls = discovery_urls("https://api.example.com");
        assert_eq!(urls[0], "https://api.example.com/.well-known/oauth-protected-resource");
    }

    #[test]
    fn discovery_urls_rfc8414_no_path() {
        let urls = discovery_urls("https://api.example.com");
        assert!(
            urls.contains(&"https://api.example.com/.well-known/oauth-authorization-server".to_string()),
            "RFC 8414 no-path URL must be in discovery list"
        );
    }

    #[test]
    fn discovery_urls_oidc_no_path() {
        let urls = discovery_urls("https://api.example.com");
        assert!(
            urls.contains(&"https://api.example.com/.well-known/openid-configuration".to_string()),
            "OIDC no-path URL must be in discovery list"
        );
    }

    #[test]
    fn discovery_urls_rfc9728_order_is_first() {
        let urls = discovery_urls("https://mcp.example.com/mcp");
        // RFC 9728 must be first
        assert!(urls[0].contains("oauth-protected-resource"));
    }

    #[test]
    fn discovery_urls_all_contain_well_known() {
        let urls = discovery_urls("https://mcp.example.com/mcp");
        for url in &urls {
            assert!(url.contains(".well-known"), "All discovery URLs must contain .well-known: {url}");
        }
    }

    // Task 1: discover_as integration tests using mock HTTP server
    #[tokio::test]
    async fn discover_as_rfc9728_success_returns_as_metadata() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // JSON for ResourceMetadata pointing to the same host
        let resource_meta_body = r#"{"authorization_servers":["http://127.0.0.1:0"]}"#;
        let as_meta_body = r#"{"authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token"}"#;

        // Spin up a minimal HTTP server that returns ResourceMetadata on RFC 9728 path
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Patch: serve RFC 9728 response then AS metadata response
        let resource_meta_body_owned = resource_meta_body.to_string();
        let as_meta_body_owned = as_meta_body.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let request = String::from_utf8_lossy(&buf);
            let response_body = if request.contains(".well-known/oauth-protected-resource") {
                let as_url = format!(r#"{{"authorization_servers":["http://127.0.0.1:{port}"]}}"#);
                as_url
            } else {
                resource_meta_body_owned.clone()
            };
            // Return ResourceMetadata — points back to same server
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(resp.as_bytes()).await;

            // Second connection for AS metadata
            let (mut stream2, _) = listener.accept().await.unwrap();
            let mut buf2 = [0u8; 4096];
            let _ = stream2.read(&mut buf2).await;
            let resp2 = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                as_meta_body_owned.len(),
                as_meta_body_owned
            );
            let _ = stream2.write_all(resp2.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let server_url = format!("http://127.0.0.1:{port}/mcp");
        let result = discover_as(&client, &server_url).await;
        assert!(result.is_ok(), "discover_as should succeed: {result:?}");
        let meta = result.unwrap();
        assert_eq!(meta.authorization_endpoint, "https://auth.example.com/authorize");
        assert_eq!(meta.token_endpoint, "https://auth.example.com/token");
    }

    #[tokio::test]
    async fn discover_as_5xx_aborts_immediately() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let server_url = format!("http://127.0.0.1:{port}/mcp");
        let result = discover_as(&client, &server_url).await;
        assert!(matches!(result, Err(OAuthError::DiscoveryFailed(_))), "5xx should abort: {result:?}");
    }

    #[tokio::test]
    async fn discover_as_all_404_returns_discovery_failed() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Serve 404 for all requests
        tokio::spawn(async move {
            for _ in 0..10 {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                }
            }
        });

        let client = reqwest::Client::new();
        let server_url = format!("http://127.0.0.1:{port}");
        let result = discover_as(&client, &server_url).await;
        assert!(matches!(result, Err(OAuthError::DiscoveryFailed(_))), "all-404 should fail: {result:?}");
    }

    // Task 2: build_auth_url tests (pure function, no HTTP needed)
    #[test]
    fn build_auth_url_contains_required_params() {
        let metadata = AsMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_string(),
            token_endpoint: "https://auth.example.com/token".to_string(),
            registration_endpoint: None,
            code_challenge_methods_supported: Some(vec!["S256".to_string()]),
        };
        let url = build_auth_url(&metadata, "client123", "https://cb.example.com/callback", "state456", "challenge789", Some("read write"));
        assert!(url.contains("response_type=code"), "must have response_type=code");
        assert!(url.contains("client_id=client123"), "must have client_id");
        assert!(url.contains("redirect_uri="), "must have redirect_uri");
        assert!(url.contains("state=state456"), "must have state");
        assert!(url.contains("code_challenge=challenge789"), "must have code_challenge");
        assert!(url.contains("code_challenge_method=S256"), "must have S256 method");
        assert!(url.contains("scope="), "must have scope when provided");
    }

    #[test]
    fn build_auth_url_no_scope_when_none() {
        let metadata = AsMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_string(),
            token_endpoint: "https://auth.example.com/token".to_string(),
            registration_endpoint: None,
            code_challenge_methods_supported: None,
        };
        let url = build_auth_url(&metadata, "client123", "https://cb.example.com/callback", "state456", "challenge789", None);
        assert!(!url.contains("scope="), "scope param must be absent when None");
    }

    // Task 2: register_client tests
    #[tokio::test]
    async fn register_client_posts_correct_body_and_returns_client_id() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            // Verify RightClaw client_name is in body
            assert!(request.contains("RightClaw"), "DCR body must contain RightClaw");
            assert!(request.contains("authorization_code"), "DCR body must contain authorization_code");

            let body = r#"{"client_id":"dcr-client-id-123","client_secret":null}"#;
            let resp = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let reg_ep = format!("http://127.0.0.1:{port}/register");
        let result = register_client(&client, &reg_ep, "https://cb.example.com/callback").await;
        assert!(result.is_ok(), "register_client should succeed: {result:?}");
        assert_eq!(result.unwrap().client_id, "dcr-client-id-123");
    }

    #[tokio::test]
    async fn register_client_non_2xx_returns_dcr_failed() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let reg_ep = format!("http://127.0.0.1:{port}/register");
        let result = register_client(&client, &reg_ep, "https://cb.example.com/callback").await;
        assert!(matches!(result, Err(OAuthError::DcrFailed(_))), "non-2xx should be DcrFailed: {result:?}");
    }

    #[test]
    fn register_client_or_fallback_uses_static_client_id_when_no_registration_endpoint() {
        // Synchronous test: when no registration_endpoint and static_client_id provided, returns static
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::new();
            let metadata = AsMetadata {
                authorization_endpoint: "https://auth.example.com/authorize".to_string(),
                token_endpoint: "https://auth.example.com/token".to_string(),
                registration_endpoint: None,
                code_challenge_methods_supported: None,
            };
            let result = register_client_or_fallback(&client, &metadata, Some("static-id-abc"), "https://cb.example.com/callback").await;
            assert!(result.is_ok(), "static fallback should succeed");
            let (client_id, secret) = result.unwrap();
            assert_eq!(client_id, "static-id-abc");
            assert!(secret.is_none());
        });
    }

    #[test]
    fn register_client_or_fallback_returns_missing_client_id_when_neither() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::new();
            let metadata = AsMetadata {
                authorization_endpoint: "https://auth.example.com/authorize".to_string(),
                token_endpoint: "https://auth.example.com/token".to_string(),
                registration_endpoint: None,
                code_challenge_methods_supported: None,
            };
            let result = register_client_or_fallback(&client, &metadata, None, "https://cb.example.com/callback").await;
            assert!(matches!(result, Err(OAuthError::MissingClientId)), "no endpoint + no static id = MissingClientId");
        });
    }

    // Task 2: exchange_token tests
    #[tokio::test]
    async fn exchange_token_posts_form_and_returns_token_response() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            // Verify form fields
            assert!(request.contains("grant_type=authorization_code"), "must have grant_type");
            assert!(request.contains("code=auth-code-xyz"), "must have code");
            assert!(request.contains("code_verifier="), "must have code_verifier");

            let body = r#"{"access_token":"tok-abc","token_type":"Bearer","expires_in":3600}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let token_ep = format!("http://127.0.0.1:{port}/token");
        let result = exchange_token(&client, &token_ep, "auth-code-xyz", "https://cb.example.com/callback", "client123", None, "verifier-string").await;
        assert!(result.is_ok(), "exchange_token should succeed: {result:?}");
        let tok = result.unwrap();
        assert_eq!(tok.access_token, "tok-abc");
        assert_eq!(tok.expires_in, Some(3600));
    }

    #[tokio::test]
    async fn exchange_token_non_2xx_returns_token_exchange_failed() {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"error":"invalid_grant"}"#;
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        });

        let client = reqwest::Client::new();
        let token_ep = format!("http://127.0.0.1:{port}/token");
        let result = exchange_token(&client, &token_ep, "bad-code", "https://cb.example.com/callback", "client123", None, "verifier").await;
        assert!(matches!(result, Err(OAuthError::TokenExchangeFailed(_))), "non-2xx should be TokenExchangeFailed: {result:?}");
    }
}
