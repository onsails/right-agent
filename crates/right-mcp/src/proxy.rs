//! MCP proxy types for aggregating external MCP servers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt as _;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientJsonRpcMessage, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, StreamableHttpClient, StreamableHttpClientTransport,
    StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Error as SseError, Sse};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

/// Errors from proxy backend operations.
// allocator churn outweighs memory savings for the hot path
#[allow(clippy::large_enum_variant)]
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

    #[error(
        "Authentication required for '{server}'. Open /mcp in Telegram and re-authenticate {server} in the dashboard."
    )]
    NeedsAuth { server: String },

    #[error("Server '{server}' is currently unreachable.")]
    Unreachable { server: String },

    #[error("No active MCP session for '{server}'")]
    NoSession { server: String },
}

/// Detect whether an rmcp error string indicates upstream OAuth/auth failure.
///
/// rmcp surfaces 401-style failures from `StreamableHttpClient` as
/// `ServiceError::TransportSend(DynamicTransportError)` whose `Display`
/// includes `"Auth required"` (from `StreamableHttpError::AuthRequired`).
/// We match on the substring rather than downcasting through `Box<dyn Error>`
/// generic transports.
pub fn is_upstream_auth_error(msg: &str) -> bool {
    msg.contains("Auth required")
}

/// Outcome of a lightweight liveness probe against a backend's live session.
#[derive(Debug)]
pub(crate) enum ProbeOutcome {
    /// Session responded; tools listed successfully.
    Alive,
    /// Upstream reported auth-required (`"Auth required"`).
    AuthRequired,
    /// Any other failure (transport, 5xx, timeout, no session). Carries detail.
    Dead(String),
}

/// Strip query strings from any URL-like substrings in an error message.
/// `query_string`-auth embeds the credential in the URL, and rmcp transport
/// errors can quote the URL verbatim — this keeps that token out of logs and
/// out of the `last_connect_error` surfaced to the dashboard.
pub(crate) fn redact_query_strings(msg: &str) -> String {
    msg.split(' ')
        .map(|tok| {
            if tok.contains("://") {
                if let Some(idx) = tok.find('?') {
                    let trailing = if tok.ends_with(')') { ")" } else { "" };
                    return format!("{}?<redacted>{trailing}", &tok[..idx]);
                }
            }
            tok.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Classify a probe error string into an outcome. Pure — no I/O.
pub(crate) fn classify_probe_error(msg: &str) -> ProbeOutcome {
    if is_upstream_auth_error(msg) {
        ProbeOutcome::AuthRequired
    } else {
        ProbeOutcome::Dead(msg.to_owned())
    }
}

/// Keep only externally-callable upstream tools, dropping internal/aggregated
/// ones (names contain `__`). Shared by `connect()` and `probe_live()` so the
/// filtering rule cannot silently diverge between the two cache-write paths.
fn filter_external_tools(tools: Vec<Tool>) -> Vec<Tool> {
    tools
        .into_iter()
        .filter(|t| !t.name.contains("__"))
        .collect()
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

/// How a proxy backend authenticates with the upstream MCP server.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum AuthMethod {
    /// `Authorization: Bearer <token>` header (default for OAuth and static bearer keys).
    #[default]
    Bearer,
    /// Custom header, e.g. `X-Api-Key: <token>`.
    Header(String),
    /// Static set of HTTP headers persisted as separate encrypted secrets.
    Headers(Vec<crate::credentials::HttpHeaderSecret>),
    /// Key is embedded in the URL query string. No header injection needed.
    QueryString,
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer => f.write_str("Bearer"),
            Self::Header(name) => f.debug_tuple("Header").field(name).finish(),
            Self::Headers(headers) => f.debug_tuple("Headers").field(headers).finish(),
            Self::QueryString => f.write_str("QueryString"),
        }
    }
}

impl AuthMethod {
    /// Parse from DB string fields (auth_type + optional auth_header).
    pub fn from_db(auth_type: Option<&str>, auth_header: Option<&str>) -> Self {
        match auth_type {
            Some("header") => Self::Header(auth_header.unwrap_or("Authorization").to_string()),
            Some("query_string") => Self::QueryString,
            _ => Self::Bearer,
        }
    }

    /// Parse from DB string fields plus multi-header secrets.
    pub fn from_db_with_headers(
        auth_type: Option<&str>,
        auth_header: Option<&str>,
        headers: Vec<crate::credentials::HttpHeaderSecret>,
    ) -> Self {
        match auth_type {
            Some("headers") => Self::Headers(headers),
            _ => Self::from_db(auth_type, auth_header),
        }
    }
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer => f.write_str("bearer"),
            Self::Header(_) => f.write_str("header"),
            Self::Headers(_) => f.write_str("headers"),
            Self::QueryString => f.write_str("query_string"),
        }
    }
}

/// Wraps `reqwest::Client` with dynamic token injection based on [`AuthMethod`].
///
/// The `StreamableHttpClient` trait passes an `auth_token` parameter per-request,
/// but we need the token to come from shared mutable state (refreshed via OAuth).
/// This wrapper reads the current token from an `Arc<RwLock<Option<String>>>` and
/// injects it into every request, ignoring the trait's own `auth_token` parameter.
///
/// For [`AuthMethod::Bearer`], the token is passed as the `auth_token` parameter.
/// For [`AuthMethod::Header`], the token is injected as a custom header.
/// For [`AuthMethod::Headers`], stored header secrets are injected directly.
/// For [`AuthMethod::QueryString`], no header injection is needed.
#[derive(Clone)]
pub(crate) struct DynamicAuthClient {
    inner: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
    auth_method: AuthMethod,
}

impl DynamicAuthClient {
    pub(crate) fn new(
        client: reqwest::Client,
        token: Arc<RwLock<Option<String>>>,
        auth_method: AuthMethod,
    ) -> Self {
        Self {
            inner: client,
            token,
            auth_method,
        }
    }

    fn auth_required_error(reason: impl Into<String>) -> StreamableHttpError<reqwest::Error> {
        StreamableHttpError::AuthRequired(AuthRequiredError::new(reason.into()))
    }

    /// Build auth token and custom headers based on auth method.
    async fn build_auth(
        &self,
    ) -> Result<(Option<String>, Vec<(HeaderName, HeaderValue)>), StreamableHttpError<reqwest::Error>>
    {
        match &self.auth_method {
            AuthMethod::Bearer => {
                let token = self.token.read().await.clone();
                Ok((token, Vec::new()))
            }
            AuthMethod::Header(header_name) => {
                let mut extra = Vec::new();
                if let Some(ref token) = *self.token.read().await {
                    let name = HeaderName::from_bytes(header_name.as_bytes()).map_err(|_| {
                        Self::auth_required_error("stored legacy auth header name is invalid")
                    })?;
                    let mut value = HeaderValue::from_str(token).map_err(|_| {
                        Self::auth_required_error("stored legacy auth header value is invalid")
                    })?;
                    value.set_sensitive(true);
                    extra.push((name, value));
                }
                Ok((None, extra))
            }
            AuthMethod::Headers(headers) => {
                let mut extra = Vec::new();
                for header in headers {
                    let name = HeaderName::from_bytes(header.name().as_bytes()).map_err(|_| {
                        Self::auth_required_error("stored HTTP header name is invalid")
                    })?;
                    let mut value = HeaderValue::from_str(header.value()).map_err(|_| {
                        Self::auth_required_error("stored HTTP header value is invalid")
                    })?;
                    value.set_sensitive(true);
                    extra.push((name, value));
                }
                Ok((None, extra))
            }
            AuthMethod::QueryString => Ok((None, Vec::new())),
        }
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
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!(
                "DynamicAuthClient: ignoring caller-provided auth_token for post_message"
            );
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await?;
        for (k, v) in extra_headers {
            custom_headers.insert(k, v);
        }
        self.inner
            .post_message(uri, message, session_id, dynamic_auth, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!(
                "DynamicAuthClient: ignoring caller-provided auth_token for delete_session"
            );
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await?;
        for (k, v) in extra_headers {
            custom_headers.insert(k, v);
        }
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
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        if auth_token.is_some() {
            tracing::debug!(
                "DynamicAuthClient: ignoring caller-provided auth_token for get_stream"
            );
        }
        let (dynamic_auth, extra_headers) = self.build_auth().await?;
        for (k, v) in extra_headers {
            custom_headers.insert(k, v);
        }
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
    auth_method: AuthMethod,
    cached_tools: RwLock<Vec<Tool>>,
    status: RwLock<BackendStatus>,
    token: Arc<RwLock<Option<String>>>,
    /// Active MCP client session handle.
    client: RwLock<Option<RunningService<RoleClient, ()>>>,
    /// Serializes concurrent `connect()` calls so refresh-driven reconnects and
    /// dashboard OAuth reconnects can't race on `client`/`cached_tools`/`status`.
    connect_mutex: Mutex<()>,
    /// Wall-clock of the most recent connect() attempt (any outcome).
    last_attempt_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// Wall-clock of the most recent successful connect().
    last_success_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// Redacted detail of the most recent connect() failure; cleared on success.
    last_connect_error: RwLock<Option<String>>,
}

impl ProxyBackend {
    pub fn new(
        server_name: String,
        agent_dir: PathBuf,
        url: String,
        token: Arc<RwLock<Option<String>>>,
        auth_method: AuthMethod,
    ) -> Self {
        Self {
            server_name,
            agent_dir,
            url,
            auth_method,
            cached_tools: RwLock::new(Vec::new()),
            status: RwLock::new(BackendStatus::Unreachable),
            token,
            client: RwLock::new(None),
            connect_mutex: Mutex::new(()),
            last_attempt_at: RwLock::new(None),
            last_success_at: RwLock::new(None),
            last_connect_error: RwLock::new(None),
        }
    }

    /// Connect to upstream, initialize the MCP session, and fetch tools.
    ///
    /// Returns the server's instructions string (if any) after writing it to SQLite.
    pub async fn connect(
        &self,
        http_client: reqwest::Client,
    ) -> Result<Option<String>, ProxyError> {
        // Hold this guard for the full body — serializes concurrent `connect()` calls
        // so refresh-driven reconnects and dashboard OAuth reconnects can't
        // interleave writes to `client`/`cached_tools`/`status`.
        let _guard = self.connect_mutex.lock().await;
        let dynamic =
            DynamicAuthClient::new(http_client, self.token.clone(), self.auth_method.clone());
        let config = StreamableHttpClientTransportConfig::with_uri(self.url.clone());
        let transport =
            StreamableHttpClientTransport::<DynamicAuthClient>::with_client(dynamic, config);

        // `()` is a minimal no-op ClientHandler — we don't need server→client notifications.
        let client: RunningService<RoleClient, ()> = match ().serve(transport).await {
            Ok(client) => client,
            Err(e) => {
                let msg = format!("{e:#}");
                let safe = redact_query_strings(&msg);
                self.record_connect_failure(safe.clone()).await;
                if let Some(err) = self.auth_required_connect_error(&msg, "initialize").await {
                    return Err(err);
                }
                tracing::debug!(
                    server = %self.server_name,
                    phase = "initialize",
                    error = %safe,
                    "upstream MCP connect failed"
                );
                return Err(ProxyError::InitFailed {
                    server: self.server_name.clone(),
                    source: e,
                });
            }
        };

        // Fetch and cache upstream tools, filtering out internal tools (contain `__`).
        let tools = match client.peer().list_all_tools().await {
            Ok(tools) => tools,
            Err(e) => {
                let msg = format!("{e:#}");
                let safe = redact_query_strings(&msg);
                self.record_connect_failure(safe.clone()).await;
                if let Some(err) = self.auth_required_connect_error(&msg, "list_tools").await {
                    return Err(err);
                }
                tracing::debug!(
                    server = %self.server_name,
                    phase = "list_tools",
                    error = %safe,
                    "upstream MCP connect failed"
                );
                return Err(ProxyError::ListToolsFailed {
                    server: self.server_name.clone(),
                    source: e,
                });
            }
        };

        let filtered = filter_external_tools(tools);
        let tool_count = filtered.len();
        *self.cached_tools.write().await = filtered;

        // Extract server instructions and write to SQLite.
        let instructions = client
            .peer()
            .peer_info()
            .and_then(|info| info.instructions.clone());
        let conn = right_db::open_connection(&self.agent_dir, false)
            .await
            .map_err(|e| ProxyError::InstructionsCacheFailed {
                server: self.server_name.clone(),
                source: e.into(),
            })?;
        crate::credentials::db_update_instructions(
            &conn,
            &self.server_name,
            instructions.as_deref(),
        )
        .await
        .map_err(|e| ProxyError::InstructionsCacheFailed {
            server: self.server_name.clone(),
            source: e.into(),
        })?;

        *self.client.write().await = Some(client);
        *self.status.write().await = BackendStatus::Connected;
        self.record_connect_success().await;

        tracing::info!(
            server = %self.server_name,
            tool_count,
            "upstream MCP server connected"
        );

        Ok(instructions)
    }

    async fn auth_required_connect_error(&self, msg: &str, phase: &str) -> Option<ProxyError> {
        if !is_upstream_auth_error(msg) {
            return None;
        }

        tracing::warn!(
            server = %self.server_name,
            phase,
            "upstream returned auth-required during connect; flipping backend to NeedsAuth"
        );
        *self.status.write().await = BackendStatus::NeedsAuth;
        Some(ProxyError::NeedsAuth {
            server: self.server_name.clone(),
        })
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

        let params = CallToolRequestParams::new(tool_name.to_owned())
            .with_arguments(arguments.unwrap_or_default());

        let result = client.peer().call_tool(params).await;
        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                let msg = format!("{e:#}");
                if is_upstream_auth_error(&msg) {
                    tracing::warn!(
                        server = %self.server_name,
                        tool = tool_name,
                        "upstream returned auth-required; flipping backend to NeedsAuth"
                    );
                    *self.status.write().await = BackendStatus::NeedsAuth;
                    return Err(ProxyError::NeedsAuth {
                        server: self.server_name.clone(),
                    });
                }
                Err(ProxyError::CallToolFailed {
                    server: self.server_name.clone(),
                    tool: tool_name.to_owned(),
                    source: e,
                })
            }
        }
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

    /// Wall-clock of the most recent connect() attempt, if any.
    pub async fn last_attempt_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.last_attempt_at.read().await
    }

    /// Wall-clock of the most recent successful connect(), if any.
    pub async fn last_success_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.last_success_at.read().await
    }

    /// Redacted detail of the most recent connect() failure, if any.
    pub async fn last_connect_error(&self) -> Option<String> {
        self.last_connect_error.read().await.clone()
    }

    async fn record_connect_success(&self) {
        let now = chrono::Utc::now();
        *self.last_attempt_at.write().await = Some(now);
        *self.last_success_at.write().await = Some(now);
        *self.last_connect_error.write().await = None;
    }

    async fn record_connect_failure(&self, detail: String) {
        *self.last_attempt_at.write().await = Some(chrono::Utc::now());
        *self.last_connect_error.write().await = Some(detail);
    }

    /// Lightweight liveness probe against the live session.
    ///
    /// Lists tools on the existing rmcp session; on success refreshes
    /// `cached_tools`. Returns the outcome and does NOT mutate `status` — the
    /// health reconciler's debounce owns the flip decision.
    pub(crate) async fn probe_live(&self) -> ProbeOutcome {
        // Clone the peer handle and drop the client lock before the network
        // round-trip, so a concurrent `connect()` reconnect isn't blocked for up
        // to PROBE_TIMEOUT waiting to swap `client`.
        let peer = {
            let client_guard = self.client.read().await;
            match client_guard.as_ref() {
                Some(client) => client.peer().clone(),
                None => return ProbeOutcome::Dead("no active session".into()),
            }
        };
        match peer.list_all_tools().await {
            Ok(tools) => {
                *self.cached_tools.write().await = filter_external_tools(tools);
                ProbeOutcome::Alive
            }
            Err(e) => classify_probe_error(&format!("{e:#}")),
        }
    }

    /// Set the connection status (e.g., after an auth failure or reconnect).
    pub async fn set_status(&self, status: BackendStatus) {
        *self.status.write().await = status;
    }

    /// Atomically set status to `new` only if it currently equals `expected`.
    /// Returns `true` if the swap happened. The health reconciler uses this to
    /// avoid clobbering a `NeedsAuth` demotion set by a concurrent tool-call or
    /// refresh during its probe window (auth death is debounce-exempt).
    pub(crate) async fn compare_and_set_status(
        &self,
        expected: BackendStatus,
        new: BackendStatus,
    ) -> bool {
        let mut guard = self.status.write().await;
        if *guard == expected {
            *guard = new;
            true
        } else {
            false
        }
    }

    /// Server name this backend connects to.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Upstream URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Agent directory used for the backend's SQLite state.
    pub fn agent_dir(&self) -> &std::path::Path {
        &self.agent_dir
    }

    /// Shared token reference for external token updates (e.g., from internal API).
    pub fn token(&self) -> &Arc<RwLock<Option<String>>> {
        &self.token
    }

    /// Authentication method for this backend.
    pub fn auth_method(&self) -> &AuthMethod {
        &self.auth_method
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install ring as the rustls process-level crypto provider. Idempotent —
    /// safe to call from multiple tests in the same binary.
    fn setup_crypto() {
        // install_default returns Err(existing provider Arc) when already
        // installed by another test in the same binary — that's not a failure.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn is_upstream_auth_error_matches_rmcp_auth_required_surface() {
        // The exact wording produced by ServiceError::TransportSend wrapping
        // StreamableHttpError::AuthRequired (rmcp 1.3, `#[error("Auth required")]`).
        let real = "Transport send error: Transport [\
            rmcp::transport::worker::WorkerTransport<\
            rmcp::transport::streamable_http_client::StreamableHttpClientWorker<\
            right_mcp::proxy::DynamicAuthClient>>\
        ] error: Auth required";
        assert!(is_upstream_auth_error(real));

        // Negative cases — must NOT be misclassified as auth.
        assert!(!is_upstream_auth_error("connection refused"));
        assert!(!is_upstream_auth_error(
            "Transport send error: Transport [foo] error: timeout"
        ));
        assert!(!is_upstream_auth_error("Mcp error: invalid_params"));
    }

    #[test]
    fn classify_probe_error_maps_auth_required_to_authrequired() {
        let msg = "Transport send error: ... error: Auth required";
        assert!(matches!(
            classify_probe_error(msg),
            ProbeOutcome::AuthRequired
        ));
    }

    #[test]
    fn classify_probe_error_maps_other_to_dead() {
        assert!(matches!(
            classify_probe_error("connection refused"),
            ProbeOutcome::Dead(_)
        ));
        assert!(matches!(
            classify_probe_error("HTTP 502 Bad Gateway"),
            ProbeOutcome::Dead(_)
        ));
    }

    #[test]
    fn probe_outcome_dead_preserves_detail() {
        match classify_probe_error("HTTP 502 Bad Gateway") {
            ProbeOutcome::Dead(d) => assert!(d.contains("502")),
            other => panic!("expected Dead, got {other:?}"),
        }
    }

    #[test]
    fn auth_method_default_is_bearer() {
        assert_eq!(AuthMethod::default(), AuthMethod::Bearer);
    }

    #[test]
    fn auth_method_display() {
        assert_eq!(AuthMethod::Bearer.to_string(), "bearer");
        assert_eq!(AuthMethod::Header("X-Api-Key".into()).to_string(), "header");
        assert_eq!(AuthMethod::QueryString.to_string(), "query_string");
    }

    #[test]
    fn auth_method_display_redacts_headers() {
        let headers = vec![
            crate::credentials::HttpHeaderSecret::new("Authorization", "Bearer secret").unwrap(),
            crate::credentials::HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
        ];
        let method = AuthMethod::Headers(headers);

        assert_eq!(method.to_string(), "headers");
        let debug = format!("{method:?}");
        assert!(debug.contains("authorization"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("Bearer secret"));
        assert!(!debug.contains("conn_123"));
    }

    #[test]
    fn auth_method_from_db() {
        assert_eq!(AuthMethod::from_db(None, None), AuthMethod::Bearer);
        assert_eq!(
            AuthMethod::from_db(Some("bearer"), None),
            AuthMethod::Bearer
        );
        assert_eq!(
            AuthMethod::from_db(Some("header"), Some("X-Api-Key")),
            AuthMethod::Header("X-Api-Key".into())
        );
        assert_eq!(
            AuthMethod::from_db(Some("header"), None),
            AuthMethod::Header("Authorization".into())
        );
        assert_eq!(
            AuthMethod::from_db(Some("query_string"), None),
            AuthMethod::QueryString
        );
    }

    #[tokio::test]
    async fn proxy_backend_new_starts_unreachable() {
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "test-server".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
            AuthMethod::default(),
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
            AuthMethod::default(),
        );
        backend.set_status(BackendStatus::NeedsAuth).await;

        let result = backend.tools_call("search", serde_json::json!({})).await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Authentication required"),
            "expected auth error, got: {msg}"
        );
        assert!(
            msg.contains("Open /mcp in Telegram"),
            "expected auth instructions, got: {msg}"
        );
    }

    #[tokio::test]
    async fn proxy_backend_connect_auth_required_sets_needs_auth() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(Some("secret".to_string())));
        let backend = ProxyBackend::new(
            "notion".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
            AuthMethod::Header("bad header".into()),
        );

        let result = backend.connect(reqwest::Client::new()).await;

        let err = result.expect_err("invalid stored auth should require reauth");
        assert!(
            matches!(&err, ProxyError::NeedsAuth { server } if server == "notion"),
            "expected NeedsAuth, got: {err:#}"
        );
        assert_eq!(backend.status().await, BackendStatus::NeedsAuth);
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
            AuthMethod::default(),
        );
        // Status is Unreachable by default from `new()`.

        let result = backend.tools_call("search", serde_json::json!({})).await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unreachable"),
            "expected unreachable error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn dynamic_auth_bearer_reads_from_shared_state() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("initial-token".to_string())));
        let client =
            DynamicAuthClient::new(reqwest::Client::new(), token.clone(), AuthMethod::Bearer);

        let (auth, extra) = client.build_auth().await.unwrap();
        assert_eq!(auth, Some("initial-token".to_string()));
        assert!(extra.is_empty());

        *token.write().await = Some("refreshed-token".to_string());
        let (auth, _) = client.build_auth().await.unwrap();
        assert_eq!(auth, Some("refreshed-token".to_string()));

        *token.write().await = None;
        let (auth, _) = client.build_auth().await.unwrap();
        assert_eq!(auth, None);
    }

    #[tokio::test]
    async fn dynamic_auth_header_injects_custom_header() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("my-api-key".to_string())));
        let client = DynamicAuthClient::new(
            reqwest::Client::new(),
            token.clone(),
            AuthMethod::Header("X-Api-Key".into()),
        );

        let (auth, extra) = client.build_auth().await.unwrap();
        assert_eq!(auth, None, "Header auth should not set auth_token");
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0].0.as_str(), "x-api-key");
        assert_eq!(extra[0].1.to_str().unwrap(), "my-api-key");
        assert!(
            extra[0].1.is_sensitive(),
            "custom auth header value must be marked sensitive"
        );
    }

    #[tokio::test]
    async fn dynamic_auth_header_no_token_no_header() {
        setup_crypto();
        let token = Arc::new(RwLock::new(None));
        let client = DynamicAuthClient::new(
            reqwest::Client::new(),
            token,
            AuthMethod::Header("X-Api-Key".into()),
        );

        let (auth, extra) = client.build_auth().await.unwrap();
        assert_eq!(auth, None);
        assert!(extra.is_empty(), "no token means no custom header");
    }

    #[tokio::test]
    async fn dynamic_auth_header_invalid_legacy_header_fails_closed() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("secret".to_string())));
        let client = DynamicAuthClient::new(
            reqwest::Client::new(),
            token,
            AuthMethod::Header("bad header".into()),
        );

        let result = tokio::spawn(async move { client.build_auth().await })
            .await
            .expect("invalid stored legacy header auth must not panic");
        let err = result.expect_err("invalid stored legacy header auth must fail closed");
        let msg = format!("{err}");
        assert!(
            is_upstream_auth_error(&msg),
            "expected auth-required error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn dynamic_auth_header_invalid_legacy_token_fails_closed() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("bad\nvalue".to_string())));
        let client = DynamicAuthClient::new(
            reqwest::Client::new(),
            token,
            AuthMethod::Header("X-Api-Key".into()),
        );

        let result = tokio::spawn(async move { client.build_auth().await })
            .await
            .expect("invalid stored legacy header token must not panic");
        let err = result.expect_err("invalid stored legacy header token must fail closed");
        let msg = format!("{err}");
        assert!(
            is_upstream_auth_error(&msg),
            "expected auth-required error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn dynamic_auth_headers_injects_multiple_headers() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("unused-token".to_string())));
        let client = DynamicAuthClient::new(
            reqwest::Client::new(),
            token,
            AuthMethod::Headers(vec![
                crate::credentials::HttpHeaderSecret::new("Authorization", "Bearer env-secret")
                    .unwrap(),
                crate::credentials::HttpHeaderSecret::new("connection-id", "conn_123").unwrap(),
                crate::credentials::HttpHeaderSecret::new("provider-config-key", "github").unwrap(),
            ]),
        );

        let (auth, extra) = client.build_auth().await.unwrap();
        assert_eq!(auth, None);
        assert_eq!(extra.len(), 3);
        for (_, value) in &extra {
            assert!(
                value.is_sensitive(),
                "stored auth header values must be marked sensitive"
            );
        }
        let headers = extra
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap().to_string(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(headers["authorization"], "Bearer env-secret");
        assert_eq!(headers["connection-id"], "conn_123");
        assert_eq!(headers["provider-config-key"], "github");
    }

    #[tokio::test]
    async fn probe_live_no_session_is_dead_and_keeps_status() {
        let tmp = tempfile::tempdir().unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "composio".into(),
            tmp.path().to_path_buf(),
            "http://localhost:9999/mcp".into(),
            token,
            AuthMethod::default(),
        );
        // Pretend it was Connected but the session is actually absent.
        backend.set_status(BackendStatus::Connected).await;

        let outcome = backend.probe_live().await;

        assert!(
            matches!(outcome, ProbeOutcome::Dead(_)),
            "no session must be Dead"
        );
        // probe_live must NOT mutate status — the reconciler owns that decision.
        assert_eq!(backend.status().await, BackendStatus::Connected);
    }

    #[tokio::test]
    async fn probe_live_alive_refreshes_tool_cache() {
        let (_srv, url) = crate::test_server::serve_two_tool_server().await;
        let tmp = tempfile::tempdir().unwrap();
        // connect() writes instructions to SQLite — needs an initialized DB with
        // a matching mcp_servers row (db_update_instructions targets it by name).
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        crate::credentials::db_add_server(&conn, "twotool", &url)
            .await
            .unwrap();
        let token = Arc::new(RwLock::new(None));
        let backend = ProxyBackend::new(
            "twotool".into(),
            tmp.path().to_path_buf(),
            url,
            token,
            AuthMethod::default(),
        );

        backend
            .connect(reqwest::Client::new())
            .await
            .expect("connect should succeed");
        assert_eq!(backend.status().await, BackendStatus::Connected);
        assert_eq!(backend.tools().await.len(), 2);

        // Clear the cache to prove probe_live repopulates it.
        *backend.cached_tools.write().await = Vec::new();
        let outcome = backend.probe_live().await;

        assert!(matches!(outcome, ProbeOutcome::Alive));
        assert_eq!(
            backend.tools().await.len(),
            2,
            "probe_live must refresh the cache"
        );
        // probe_live does not write status.
        assert_eq!(backend.status().await, BackendStatus::Connected);
    }

    #[tokio::test]
    async fn dynamic_auth_query_string_no_injection() {
        setup_crypto();
        let token = Arc::new(RwLock::new(Some("key-in-url".to_string())));
        let client = DynamicAuthClient::new(reqwest::Client::new(), token, AuthMethod::QueryString);

        let (auth, extra) = client.build_auth().await.unwrap();
        assert_eq!(auth, None, "QueryString should not set auth_token");
        assert!(extra.is_empty(), "QueryString should not inject headers");
    }

    #[test]
    fn redact_query_strings_strips_url_query() {
        assert_eq!(
            redact_query_strings("error sending request for url (http://h:1/mcp?token=abc)"),
            "error sending request for url (http://h:1/mcp?<redacted>)"
        );
        assert_eq!(redact_query_strings("plain message"), "plain message");
        assert_eq!(redact_query_strings("http://h:1/mcp"), "http://h:1/mcp");
    }

    #[tokio::test]
    async fn connect_failure_records_error_and_attempt() {
        setup_crypto();
        let tmp = tempfile::tempdir().unwrap();
        let backend = ProxyBackend::new(
            "dead".into(),
            tmp.path().to_path_buf(),
            "http://127.0.0.1:1/mcp".into(),
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        );
        let result = backend.connect(reqwest::Client::new()).await;
        assert!(result.is_err(), "connect to dead port must fail");
        assert!(backend.last_attempt_at().await.is_some());
        assert!(backend.last_connect_error().await.is_some());
        assert!(backend.last_success_at().await.is_none());
    }

    #[tokio::test]
    async fn connect_success_records_success() {
        setup_crypto();
        let (_srv, url) = crate::test_server::serve_two_tool_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let conn = right_db::open_connection(tmp.path(), true).await.unwrap();
        crate::credentials::db_add_server(&conn, "srv", &url)
            .await
            .unwrap();
        let backend = ProxyBackend::new(
            "srv".into(),
            tmp.path().to_path_buf(),
            url,
            Arc::new(RwLock::new(None)),
            AuthMethod::default(),
        );
        backend.connect(reqwest::Client::new()).await.unwrap();
        assert!(backend.last_success_at().await.is_some());
        assert!(backend.last_connect_error().await.is_none());
    }
}
