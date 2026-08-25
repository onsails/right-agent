//! Owner-local persistence interface for MCP runtime state.
//!
//! The Aggregator's `AgentDbOwner` is the sole live owner of every per-agent
//! `data.db`. The proxy, reconnect, and refresh modules therefore persist
//! through this narrow interface instead of opening connections themselves.
//! The Aggregator implements [`McpPersistence`] over its owner; tests supply
//! their own implementation.
//!
//! Secret-bearing payloads ([`OAuthStatePersist`]) never appear in `Debug`
//! output — the struct carries tokens and a client secret.

use std::fmt::Debug;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence failure reported to proxy/reconnect/refresh callers.
///
/// The message carries the full server-side chain; it never contains secret
/// values (implementations redact at construction).
#[derive(Debug, thiserror::Error)]
pub enum McpPersistError {
    /// The owner is not available (starting, draining, or failed).
    #[error("MCP persistence unavailable: {0}")]
    Unavailable(String),
    /// The underlying write failed.
    #[error("MCP persistence operation failed: {0}")]
    Operation(String),
}

/// Full OAuth state for one MCP server (mirrors the inputs of
/// [`crate::credentials::db_set_oauth_state`]).
///
/// `access_token`, `refresh_token`, and `client_secret` are credentials: the
/// manual [`Debug`] impl redacts them.
#[derive(Clone)]
pub struct OAuthStatePersist {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    /// RFC3339 expiry of the access token.
    pub expires_at: String,
    pub oauth_resource: String,
}

impl Debug for OAuthStatePersist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthStatePersist")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("oauth_resource", &self.oauth_resource)
            .finish()
    }
}

/// Scoped persistence operations for MCP runtime state.
///
/// One instance exists per agent inside the Aggregator, bound to that agent's
/// `AgentDbOwner`. Implementations must preserve the current transaction
/// boundaries of the underlying `crate::credentials` helpers.
pub trait McpPersistence: Send + Sync + Debug {
    /// Cache the server's `instructions` string after a successful connect.
    fn update_instructions(
        &self,
        server: &str,
        instructions: Option<&str>,
    ) -> BoxFuture<'static, Result<(), McpPersistError>>;

    /// Persist full OAuth state (dashboard OAuth callback, NewEntry).
    fn set_oauth_state(
        &self,
        server: &str,
        state: OAuthStatePersist,
    ) -> BoxFuture<'static, Result<(), McpPersistError>>;

    /// Persist a refreshed access token (refresh scheduler, reconnect).
    fn update_oauth_token(
        &self,
        server: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: &str,
    ) -> BoxFuture<'static, Result<(), McpPersistError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_state_persist_debug_redacts_secrets() {
        let state = OAuthStatePersist {
            access_token: "access-secret".to_owned(),
            refresh_token: Some("refresh-secret".to_owned()),
            token_endpoint: "https://auth.example/token".to_owned(),
            client_id: "client-id".to_owned(),
            client_secret: Some("client-secret".to_owned()),
            expires_at: "2026-01-01T00:00:00Z".to_owned(),
            oauth_resource: "https://mcp.example/".to_owned(),
        };
        let rendered = format!("{state:?}");
        for secret in ["access-secret", "refresh-secret", "client-secret"] {
            assert!(
                !rendered.contains(secret),
                "Debug must not leak {secret}: {rendered}"
            );
        }
        assert!(rendered.contains("client-id"));
        assert!(rendered.contains("<redacted>"));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test persistence: opens the per-agent DB directly. Tests own their
    //! tempdir databases outright (explicit quiescence), so a direct open is
    //! the sanctioned pattern; production always goes through the owner.

    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{BoxFuture, McpPersistError, McpPersistence, OAuthStatePersist};

    #[derive(Debug, Clone)]
    pub(crate) struct SqlitePersistence {
        dir: PathBuf,
    }

    /// Shared handle for the `Arc<dyn McpPersistence>` constructor parameters.
    pub(crate) fn sqlite(dir: &Path) -> Arc<dyn McpPersistence> {
        Arc::new(SqlitePersistence {
            dir: dir.to_path_buf(),
        })
    }

    impl McpPersistence for SqlitePersistence {
        fn update_instructions(
            &self,
            server: &str,
            instructions: Option<&str>,
        ) -> BoxFuture<'static, Result<(), McpPersistError>> {
            let dir = self.dir.clone();
            let server = server.to_owned();
            let instructions = instructions.map(str::to_owned);
            Box::pin(async move {
                let conn = right_db::open_connection(&dir, false)
                    .await
                    .map_err(|e| McpPersistError::Unavailable(format!("{e:#}")))?;
                crate::credentials::db_update_instructions(&conn, &server, instructions.as_deref())
                    .await
                    .map_err(|e| McpPersistError::Operation(format!("{e:#}")))
            })
        }

        fn set_oauth_state(
            &self,
            server: &str,
            state: OAuthStatePersist,
        ) -> BoxFuture<'static, Result<(), McpPersistError>> {
            let dir = self.dir.clone();
            let server = server.to_owned();
            Box::pin(async move {
                let conn = right_db::open_connection(&dir, false)
                    .await
                    .map_err(|e| McpPersistError::Unavailable(format!("{e:#}")))?;
                crate::credentials::db_set_oauth_state(
                    &conn,
                    &server,
                    &state.access_token,
                    state.refresh_token.as_deref(),
                    &state.token_endpoint,
                    &state.client_id,
                    state.client_secret.as_deref(),
                    &state.expires_at,
                    &state.oauth_resource,
                )
                .await
                .map_err(|e| McpPersistError::Operation(format!("{e:#}")))
            })
        }

        fn update_oauth_token(
            &self,
            server: &str,
            access_token: &str,
            refresh_token: Option<&str>,
            expires_at: &str,
        ) -> BoxFuture<'static, Result<(), McpPersistError>> {
            let dir = self.dir.clone();
            let server = server.to_owned();
            let access_token = access_token.to_owned();
            let refresh_token = refresh_token.map(str::to_owned);
            let expires_at = expires_at.to_owned();
            Box::pin(async move {
                let conn = right_db::open_connection(&dir, false)
                    .await
                    .map_err(|e| McpPersistError::Unavailable(format!("{e:#}")))?;
                crate::credentials::db_update_oauth_token(
                    &conn,
                    &server,
                    &access_token,
                    refresh_token.as_deref(),
                    &expires_at,
                )
                .await
                .map_err(|e| McpPersistError::Operation(format!("{e:#}")))
            })
        }
    }
}
