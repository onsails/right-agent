#![warn(unreachable_pub)]

pub mod credentials;
pub mod internal_client;
pub mod oauth;
pub mod proxy;
pub mod reconnect;
pub mod refresh;
pub mod tool_error;

/// Name of the built-in MCP server that right-agent manages.
/// Protected from `/mcp remove` - required for core functionality.
pub const PROTECTED_MCP_SERVER: &str = "right";

/// Generate a random 32-byte agent secret, base64url-encoded (no padding).
///
/// Stored persistently in `agent.yaml`. Used to derive Bearer tokens for
/// the HTTP MCP server and future services.
pub fn generate_agent_secret() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive a Bearer token from an agent secret using HMAC-SHA256.
///
/// `secret_b64` is the base64url-encoded agent secret from `agent.yaml`.
/// `label` identifies the service (e.g., `"right-mcp"`).
///
/// Returns base64url-encoded HMAC digest (no padding), 43 characters.
pub fn derive_token(secret_b64: &str, label: &str) -> miette::Result<String> {
    use base64::Engine as _;
    use hmac::{Hmac, KeyInit as _, Mac as _};
    use sha2::Sha256;

    let secret_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(secret_b64)
        .map_err(|e| miette::miette!("invalid agent secret (bad base64url): {e:#}"))?;

    let mut mac = Hmac::<Sha256>::new_from_slice(&secret_bytes)
        .map_err(|e| miette::miette!("HMAC init failed: {e:#}"))?;
    mac.update(label.as_bytes());
    let result = mac.finalize().into_bytes();

    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_agent_secret_is_43_chars() {
        let secret = generate_agent_secret();
        assert_eq!(secret.len(), 43);
    }

    #[test]
    fn generate_agent_secret_unique() {
        let a = generate_agent_secret();
        let b = generate_agent_secret();
        assert_ne!(a, b);
    }

    #[test]
    fn derive_token_deterministic() {
        let secret = generate_agent_secret();
        let t1 = derive_token(&secret, "right-mcp").unwrap();
        let t2 = derive_token(&secret, "right-mcp").unwrap();
        assert_eq!(t1, t2);
    }

    #[test]
    fn derive_token_different_labels_differ() {
        let secret = generate_agent_secret();
        let t1 = derive_token(&secret, "right-mcp").unwrap();
        let t2 = derive_token(&secret, "right-cron").unwrap();
        assert_ne!(t1, t2);
    }

    #[test]
    fn derive_token_is_43_chars() {
        let secret = generate_agent_secret();
        let token = derive_token(&secret, "right-mcp").unwrap();
        assert_eq!(token.len(), 43);
    }

    #[test]
    fn derive_token_rejects_invalid_base64() {
        let result = derive_token("not!valid!base64", "right-mcp");
        assert!(result.is_err());
    }

    #[test]
    fn derive_token_for_tg_webhook_matches_telegram_secret_format() {
        let secret = generate_agent_secret();
        let webhook_secret = derive_token(&secret, "tg-webhook").unwrap();
        assert!(
            !webhook_secret.is_empty() && webhook_secret.len() <= 256,
            "len out of Telegram's 1-256 range: {}",
            webhook_secret.len()
        );
        assert!(
            webhook_secret
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "char outside Telegram's [A-Za-z0-9_-]: {webhook_secret}"
        );
    }
}
