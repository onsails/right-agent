use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngExt as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;

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
}
