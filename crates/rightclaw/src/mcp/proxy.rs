//! MCP proxy types for aggregating external MCP servers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientJsonRpcMessage, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    StreamableHttpError, StreamableHttpPostResponse,
};
use rmcp::ServiceExt as _;
use sse_stream::{Error as SseError, Sse};
use thiserror::Error;
use tokio::sync::RwLock;

/// Errors from proxy backend operations.
#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("MCP client initialization failed for '{server}': {source}")]
    InitFailed {
        server: String,
        #[source]
        source: rmcp::service::ClientInitializeError,
    },

    #[error("instructions cache failed for '{server}': {source}")]
    InstructionsCacheFailed {
        server: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("list_tools failed for '{server}': {source}")]
    ListToolsFailed {
        server: String,
        #[source]
        source: rmcp::service::ServiceError,
    },

    #[error("call_tool '{tool}' failed on '{server}': {source}")]
    CallToolFailed {
        server: String,
        tool: String,
        #[source]
        source: rmcp::service::ServiceError,
    },

    #[error("Authentication required for '{server}'. Use /mcp auth {server} in Telegram.")]
    NeedsAuth { server: String },

    #[error("Server '{server}' is currently unreachable.")]
    Unreachable { server: String },

    #[error("No active MCP session for '{server}'")]
    NoSession { server: String },
}

/// Status of a ProxyBackend connection to an upstream MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    Connected,
    NeedsAuth,
    Unreachable,
}

impl std::fmt::Display for BackendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendStatus::Connected => f.write_str("connected"),
            BackendStatus::NeedsAuth => f.write_str("needs_auth"),
            BackendStatus::Unreachable => f.write_str("unreachable"),
        }
    }
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

/// MCP client backend that connects to a single upstream HTTP MCP server.
///
/// Manages the client session lifecycle, caches the upstream tool list and
/// instructions, and forwards tool calls through the MCP client session.
pub struct ProxyBackend {
    server_name: String,
    agent_dir: PathBuf,
    url: String,
    cached_tools: RwLock<Vec<Tool>>,
    status: RwLock<BackendStatus>,
    token: Arc<RwLock<Option<String>>>,
    /// Active MCP client session handle.
    client: RwLock<Option<RunningService<RoleClient, ()>>>,
}

impl ProxyBackend {
    pub fn new(
        server_name: String,
        agent_dir: PathBuf,
        url: String,
        token: Arc<RwLock<Option<String>>>,
    ) -> Self {
        Self {
            server_name,
            agent_dir,
            url,
            cached_tools: RwLock::new(Vec::new()),
            status: RwLock::new(BackendStatus::Unreachable),
            token,
            client: RwLock::new(None),
        }
    }

    /// Connect to upstream, initialize the MCP session, and fetch tools.
    ///
    /// Returns the server's instructions string (if any) after writing it to SQLite.
    pub async fn connect(
        &self,
        http_client: reqwest::Client,
    ) -> Result<Option<String>, ProxyError> {
        let dynamic = DynamicAuthClient::new(http_client, self.token.clone());
        let config = StreamableHttpClientTransportConfig::with_uri(self.url.clone());
        let transport =
            StreamableHttpClientTransport::<DynamicAuthClient>::with_client(dynamic, config);

        // `()` is a minimal no-op ClientHandler — we don't need server→client notifications.
        let client: RunningService<RoleClient, ()> =
            ().serve(transport).await.map_err(|e| ProxyError::InitFailed {
                server: self.server_name.clone(),
                source: e,
            })?;

        // Fetch and cache upstream tools, filtering out internal tools (contain `__`).
        let tools = client
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| ProxyError::ListToolsFailed {
                server: self.server_name.clone(),
                source: e,
            })?;

        let filtered: Vec<Tool> = tools
            .into_iter()
            .filter(|t| !t.name.contains("__"))
            .collect();

        let tool_count = filtered.len();
        *self.cached_tools.write().await = filtered;

        // Extract server instructions and write to SQLite.
        let instructions = client
            .peer()
            .peer_info()
            .and_then(|info| info.instructions.clone());
        let conn = crate::memory::open_connection(&self.agent_dir)
            .map_err(|e| ProxyError::InstructionsCacheFailed {
                server: self.server_name.clone(),
                source: e.into(),
            })?;
        crate::mcp::credentials::db_update_instructions(
            &conn,
            &self.server_name,
            instructions.as_deref(),
        )
        .map_err(|e| ProxyError::InstructionsCacheFailed {
            server: self.server_name.clone(),
            source: e.into(),
        })?;

        *self.client.write().await = Some(client);
        *self.status.write().await = BackendStatus::Connected;

        tracing::info!(
            server = %self.server_name,
            tool_count,
            "upstream MCP server connected"
        );

        Ok(instructions)
    }

    /// Forward a tool call to the upstream MCP server.
    pub async fn tools_call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, ProxyError> {
        let status = *self.status.read().await;
        match status {
            BackendStatus::NeedsAuth => {
                return Err(ProxyError::NeedsAuth {
                    server: self.server_name.clone(),
                });
            }
            BackendStatus::Unreachable => {
                return Err(ProxyError::Unreachable {
                    server: self.server_name.clone(),
                });
            }
            BackendStatus::Connected => {}
        }

        let client_guard = self.client.read().await;
        let client = client_guard.as_ref().ok_or_else(|| ProxyError::NoSession {
            server: self.server_name.clone(),
        })?;

        let arguments = match args {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        };

        let params = CallToolRequestParams::new(tool_name.to_owned()).with_arguments(
            arguments.unwrap_or_default(),
        );

        let result = client
            .peer()
            .call_tool(params)
            .await
            .map_err(|e| ProxyError::CallToolFailed {
                server: self.server_name.clone(),
                tool: tool_name.to_owned(),
                source: e,
            })?;

        Ok(result)
    }

    /// Get cached tool list.
    pub async fn tools(&self) -> Vec<Tool> {
        self.cached_tools.read().await.clone()
    }

    /// Non-blocking attempt to read cached tools. Returns `None` if the lock
    /// is currently held by a writer (e.g., during a concurrent `connect`).
    pub fn try_tools(&self) -> Option<Vec<Tool>> {
        self.cached_tools.try_read().ok().map(|g| g.clone())
    }

    /// Current connection status.
    pub async fn status(&self) -> BackendStatus {
        *self.status.read().await
    }

    /// Set the connection status (e.g., after an auth failure or reconnect).
    pub async fn set_status(&self, status: BackendStatus) {
        *self.status.write().await = status;
    }

    /// Server name this backend connects to.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Upstream URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Shared token reference for external token updates (e.g., from internal API).
    pub fn token(&self) -> &Arc<RwLock<Option<String>>> {
        &self.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn proxy_backend_new_starts_unreachable() {
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "test-server".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
        );

        assert_eq!(backend.status().await, BackendStatus::Unreachable);
        assert!(backend.tools().await.is_empty());
    }

    #[tokio::test]
    async fn proxy_backend_needs_auth_rejects_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "notion".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
        );
        backend.set_status(BackendStatus::NeedsAuth).await;

        let result = backend
            .tools_call("search", serde_json::json!({}))
            .await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Authentication required"),
            "expected auth error, got: {msg}"
        );
        assert!(
            msg.contains("/mcp auth notion"),
            "expected auth instructions, got: {msg}"
        );
    }

    #[tokio::test]
    async fn proxy_backend_unreachable_rejects_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "notion".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
        );
        // Status is Unreachable by default from `new()`.

        let result = backend
            .tools_call("search", serde_json::json!({}))
            .await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unreachable"),
            "expected unreachable error, got: {msg}"
        );
    }

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
