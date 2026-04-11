//! MCP proxy types for aggregating external MCP servers.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Error as SseError, Sse};
use tokio::sync::RwLock;

/// Status of a ProxyBackend connection to an upstream MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendStatus {
    Connected,
    NeedsAuth,
    Unreachable,
}

/// Wraps `reqwest::Client` with dynamic Bearer token injection.
///
/// The `StreamableHttpClient` trait passes an `auth_token` parameter per-request,
/// but we need the token to come from shared mutable state (refreshed via OAuth).
/// This wrapper reads the current token from an `Arc<RwLock<Option<String>>>` and
/// injects it into every request, ignoring the trait's own `auth_token` parameter.
#[derive(Clone)]
pub(crate) struct DynamicAuthClient {
    inner: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
}

impl DynamicAuthClient {
    pub(crate) fn new(client: reqwest::Client, token: Arc<RwLock<Option<String>>>) -> Self {
        Self {
            inner: client,
            token,
        }
    }

    async fn current_auth(&self) -> Option<String> {
        self.token.read().await.clone()
    }
}

impl StreamableHttpClient for DynamicAuthClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!("DynamicAuthClient: ignoring caller-provided auth_token for post_message");
        }
        let dynamic_auth = self.current_auth().await;
        self.inner
            .post_message(uri, message, session_id, dynamic_auth, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!(
                "DynamicAuthClient: ignoring caller-provided auth_token for delete_session"
            );
        }
        let dynamic_auth = self.current_auth().await;
        self.inner
            .delete_session(uri, session_id, dynamic_auth, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!(
                "DynamicAuthClient: ignoring caller-provided auth_token for get_stream"
            );
        }
        let dynamic_auth = self.current_auth().await;
        self.inner
            .get_stream(uri, session_id, last_event_id, dynamic_auth, custom_headers)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dynamic_auth_reads_from_shared_state() {
        let token = Arc::new(RwLock::new(Some("initial-token".to_string())));
        let client = DynamicAuthClient::new(reqwest::Client::new(), token.clone());

        assert_eq!(
            client.current_auth().await,
            Some("initial-token".to_string())
        );

        *token.write().await = Some("refreshed-token".to_string());
        assert_eq!(
            client.current_auth().await,
            Some("refreshed-token".to_string())
        );

        *token.write().await = None;
        assert_eq!(client.current_auth().await, None);
    }
}
