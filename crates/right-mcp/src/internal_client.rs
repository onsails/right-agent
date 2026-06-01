//! Hyper-based Unix domain socket client for Right Agent internal IPC.
//!
//! Uses raw `hyper` with `tokio::net::UnixStream` to POST JSON to the
//! internal APIs served on Unix domain sockets. `reqwest` doesn't support
//! UDS natively, so we use hyper's low-level HTTP/1.1 client directly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// HTTP header set on per-invocation MCP requests so the aggregator can route
/// `send_progress` calls back to the correct in-flight invocation.
///
/// Single source of truth — the bot writes this header key into the
/// per-invocation `.mcp.json` and the aggregator reads it off incoming
/// requests. HTTP-header case-insensitive, but keeping one constant prevents
/// drift between writer and reader.
pub const PROGRESS_INVOCATION_HEADER: &str = "X-Right-Invocation";

/// Base name of the `send_progress` MCP tool exposed by `RightBackend`.
/// Agents see the CC-prefixed form `mcp__right__send_progress` (see
/// `PROGRESS_MCP_TOOL`).
pub const SEND_PROGRESS_TOOL: &str = "send_progress";

/// CC-prefixed form of the `send_progress` tool, as seen by agents in
/// `--disallowedTools` and any user-facing prose.
pub const PROGRESS_MCP_TOOL: &str = "mcp__right__send_progress";

pub const SKILL_LEARNING_START_TOOL: &str = "skill_learning_start";
pub const SKILL_LEARNING_FINISH_TOOL: &str = "skill_learning_finish";
pub const SKILL_LEARNING_START_MCP_TOOL: &str = "mcp__right__skill_learning_start";
pub const SKILL_LEARNING_FINISH_MCP_TOOL: &str = "mcp__right__skill_learning_finish";

pub const THREAD_SEARCH_MCP_TOOL: &str = "mcp__right__thread_search";
pub const CHAT_SEARCH_MCP_TOOL: &str = "mcp__right__chat_search";

pub const FORUM_TOPIC_CREATE_MCP_TOOL: &str = "mcp__right__forum_topic_create";
pub const FORUM_TOPIC_EDIT_MCP_TOOL: &str = "mcp__right__forum_topic_edit";
pub const FORUM_TOPIC_CLOSE_MCP_TOOL: &str = "mcp__right__forum_topic_close";
pub const FORUM_TOPIC_REOPEN_MCP_TOOL: &str = "mcp__right__forum_topic_reopen";
pub const FORUM_TOPIC_LIST_MCP_TOOL: &str = "mcp__right__forum_topic_list";

/// Maximum length (in Unicode scalar values) of a `send_progress` message.
///
/// Single source of truth for: the JSON-schema `maxLength` advertised in
/// `tools/list`, server-side validation in `RightBackend::call_send_progress`,
/// and the bot-side guard in `telegram::progress::handle_progress_send`. Chosen
/// well below Telegram's 4096-UTF-16 hard limit to discourage agents from
/// dumping verbose output as "progress" — the tool is for short, factual
/// status updates.
pub const PROGRESS_MESSAGE_MAX_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum InternalClientError {
    #[error("Connection to aggregator failed: {0}")]
    Connection(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Server error ({status}): {body}")]
    Server { status: u16, body: String },
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct InternalClient {
    socket_path: PathBuf,
}

impl InternalClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// POST JSON to the internal API and parse the response.
    async fn post<Req: Serialize, Res: DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Res, InternalClientError> {
        // 1. Connect to Unix socket
        let stream = tokio::net::UnixStream::connect(&self.socket_path).await?;

        // 2. HTTP/1.1 handshake via hyper
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        // Spawn connection driver — must run concurrently with request
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("internal API connection error: {e:#}");
            }
        });

        // 3. Build request
        let body_bytes = serde_json::to_vec(body)?;
        let req = hyper::Request::post(path)
            .header("content-type", "application/json")
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                body_bytes,
            )))
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        // 4. Send request
        let response = sender
            .send_request(req)
            .await
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?;

        // 5. Read response body
        let status = response.status().as_u16();
        let body_bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .map_err(|e| InternalClientError::Http(format!("{e:#}")))?
            .to_bytes();

        // 6. Handle errors
        if status >= 400 {
            let body_str = String::from_utf8_lossy(&body_bytes).to_string();
            return Err(InternalClientError::Server {
                status,
                body: body_str,
            });
        }

        // 7. Deserialize response
        serde_json::from_slice(&body_bytes).map_err(Into::into)
    }

    /// Add an MCP server for the given agent.
    pub async fn mcp_add(
        &self,
        agent: &str,
        name: &str,
        url: &str,
        auth_type: Option<&str>,
        auth_header: Option<&str>,
        auth_token: Option<&str>,
    ) -> Result<McpAddResponse, InternalClientError> {
        self.mcp_add_request(&McpAddRequest {
            agent,
            name,
            url,
            auth_type,
            auth_header,
            auth_token,
            headers: Vec::new(),
        })
        .await
    }

    /// Add an MCP server with a structured request body.
    pub async fn mcp_add_request(
        &self,
        request: &McpAddRequest<'_>,
    ) -> Result<McpAddResponse, InternalClientError> {
        self.post("/mcp-add", request).await
    }

    /// Remove an MCP server for the given agent.
    pub async fn mcp_remove(
        &self,
        agent: &str,
        name: &str,
    ) -> Result<McpRemoveResponse, InternalClientError> {
        self.post(
            "/mcp-remove",
            &serde_json::json!({
                "agent": agent, "name": name
            }),
        )
        .await
    }

    /// List MCP servers for the given agent.
    pub async fn mcp_list(&self, agent: &str) -> Result<McpListResponse, InternalClientError> {
        self.post("/mcp-list", &serde_json::json!({"agent": agent}))
            .await
    }

    /// Replace HTTP header credentials for an MCP server.
    pub async fn mcp_set_headers(
        &self,
        request: &McpSetHeadersRequest,
    ) -> Result<McpSetHeadersResponse, InternalClientError> {
        self.post("/mcp-set-headers", request).await
    }

    /// Fetch MCP server instructions markdown for the given agent.
    pub async fn mcp_instructions(
        &self,
        agent: &str,
    ) -> Result<McpInstructionsResponse, InternalClientError> {
        self.post("/mcp-instructions", &serde_json::json!({"agent": agent}))
            .await
    }

    /// Set OAuth token for an MCP server.
    pub async fn set_token(
        &self,
        request: &SetTokenRequest,
    ) -> Result<SetTokenResponse, InternalClientError> {
        self.post("/set-token", request).await
    }

    /// Tell the aggregator to re-read agent-tokens.json and register new agents.
    pub async fn reload(&self) -> Result<ReloadResponse, InternalClientError> {
        self.post("/reload", &serde_json::json!({})).await
    }

    /// Register an invocation that may send progress or learning messages.
    pub async fn progress_register(
        &self,
        request: &ProgressRegisterRequest,
    ) -> Result<ProgressRegisterResponse, InternalClientError> {
        self.post("/progress/register", request).await
    }

    /// Unregister a progress/learning-capable invocation.
    pub async fn progress_unregister(
        &self,
        request: &ProgressUnregisterRequest,
    ) -> Result<ProgressUnregisterResponse, InternalClientError> {
        self.post("/progress/unregister", request).await
    }

    /// Ask the bot-local UDS endpoint to send a progress message.
    pub async fn progress_send(
        &self,
        request: &ProgressSendRequest,
    ) -> Result<ProgressSendResponse, InternalClientError> {
        self.post("/progress/send", request).await
    }

    /// Create a forum topic via the bot-local UDS endpoint.
    pub async fn forum_topic_create(
        &self,
        request: &ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResponse, InternalClientError> {
        self.post("/forum-topic/create", request).await
    }

    /// Edit (rename / change icon) a forum topic.
    pub async fn forum_topic_edit(
        &self,
        request: &ForumTopicEditRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/edit", request).await
    }

    /// Close a forum topic.
    pub async fn forum_topic_close(
        &self,
        request: &ForumTopicThreadRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/close", request).await
    }

    /// Reopen a forum topic.
    pub async fn forum_topic_reopen(
        &self,
        request: &ForumTopicThreadRequest,
    ) -> Result<ForumTopicOkResponse, InternalClientError> {
        self.post("/forum-topic/reopen", request).await
    }

    /// List providers for the given agent.
    pub async fn provider_list(
        &self,
        agent: &str,
    ) -> Result<Vec<serde_json::Value>, InternalClientError> {
        self.post("/provider-list", &ProviderListRequest { agent })
            .await
    }

    /// List available provider types.
    pub async fn provider_types(&self) -> Result<Vec<serde_json::Value>, InternalClientError> {
        self.post("/provider-types", &serde_json::json!({})).await
    }

    /// Create a provider for the given agent.
    pub async fn provider_create(
        &self,
        req: &ProviderCreateRequest<'_>,
    ) -> Result<serde_json::Value, InternalClientError> {
        self.post("/provider-create", req).await
    }

    /// Rotate credentials for an existing provider.
    pub async fn provider_rotate(
        &self,
        req: &ProviderRotateRequest<'_>,
    ) -> Result<serde_json::Value, InternalClientError> {
        self.post("/provider-rotate", req).await
    }

    /// Remove a provider from an agent.
    pub async fn provider_remove(
        &self,
        req: &ProviderRemoveRequest<'_>,
    ) -> Result<serde_json::Value, InternalClientError> {
        self.post("/provider-remove", req).await
    }

    /// Update provider config for an agent.
    pub async fn provider_config_update(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, InternalClientError> {
        self.post("/provider-config-update", body).await
    }
}

// ---------------------------------------------------------------------------
// Provider request types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct ProviderListRequest<'a> {
    pub agent: &'a str,
}

#[derive(serde::Serialize)]
pub struct ProviderCreateRequest<'a> {
    pub agent: &'a str,
    #[serde(rename = "type")]
    pub type_: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'a str>,
    pub credential: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generic: Option<ProviderCreateGenericArg<'a>>,
}

impl std::fmt::Debug for ProviderCreateRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderCreateRequest")
            .field("agent", &self.agent)
            .field("type_", &self.type_)
            .field("label", &self.label)
            .field("credential", &"<redacted>")
            .field("generic", &self.generic)
            .finish()
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderCreateGenericArg<'a> {
    pub env_var: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_name: Option<&'a str>,
    pub upstream_host: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_path_prefix: Option<&'a str>,
}

#[derive(serde::Serialize)]
pub struct ProviderRotateRequest<'a> {
    pub agent: &'a str,
    pub name: &'a str,
    pub credential: &'a str,
}

impl std::fmt::Debug for ProviderRotateRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRotateRequest")
            .field("agent", &self.agent)
            .field("name", &self.name)
            .field("credential", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ProviderRemoveRequest<'a> {
    pub agent: &'a str,
    pub name: &'a str,
}

// ---------------------------------------------------------------------------
// Response types (must match the internal UDS handlers on the server side)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpHeaderInput {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for HttpHeaderInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpHeaderInput")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize)]
pub struct McpAddRequest<'a> {
    pub agent: &'a str,
    pub name: &'a str,
    pub url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpHeaderInput>,
}

impl std::fmt::Debug for McpAddRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth_token = self.auth_token.map(|_| "<redacted>");
        f.debug_struct("McpAddRequest")
            .field("agent", &self.agent)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("auth_type", &self.auth_type)
            .field("auth_header", &self.auth_header)
            .field("auth_token", &auth_token)
            .field("headers", &self.headers)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
pub struct McpAddResponse {
    pub tools_count: usize,
    #[serde(default)]
    pub excluded: Vec<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpRemoveResponse {
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSetHeadersRequest {
    pub agent: String,
    pub name: String,
    pub headers: Vec<HttpHeaderInput>,
}

#[derive(Debug, Deserialize)]
pub struct McpSetHeadersResponse {
    pub ok: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpListResponse {
    pub servers: Vec<McpServerStatus>,
}

#[derive(Debug, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub url: Option<String>,
    pub status: String,
    pub tool_count: usize,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub header_names: Vec<String>,
    #[serde(default)]
    pub last_connect_error: Option<String>,
    #[serde(default)]
    pub last_attempt_at: Option<String>,
    #[serde(default)]
    pub last_success_at: Option<String>,
}

#[derive(Serialize)]
pub struct SetTokenRequest {
    pub agent: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_endpoint: String,
    pub client_id: String,
    pub resource: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for SetTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let client_secret = self.client_secret.as_ref().map(|_| "<redacted>");
        f.debug_struct("SetTokenRequest")
            .field("agent", &self.agent)
            .field("server", &self.server)
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("resource", &self.resource)
            .field("client_secret", &client_secret)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
pub struct SetTokenResponse {
    pub ok: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpInstructionsResponse {
    pub instructions: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReloadResponse {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressInvocationKindDto {
    Foreground,
    BackgroundReview,
    ProbeWriter,
    Curator,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProgressRegisterRequest {
    pub agent: String,
    pub invocation_id: String,
    pub kind: ProgressInvocationKindDto,
    pub bot_send_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
}

impl std::fmt::Debug for ProgressRegisterRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressRegisterRequest")
            .field("agent", &self.agent)
            .field("invocation_id", &self.invocation_id)
            .field("kind", &self.kind)
            .field("bot_send_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
            .field("thread_id", &self.thread_id)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProgressRegisterResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUnregisterRequest {
    pub agent: String,
    pub invocation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProgressUnregisterResponse {
    pub ok: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProgressSendRequest {
    pub invocation_id: String,
    pub token: String,
    pub message: String,
}

impl std::fmt::Debug for ProgressSendRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressSendRequest")
            .field("invocation_id", &self.invocation_id)
            .field("token", &"<redacted>")
            .field("message", &self.message)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProgressSendResponse {
    pub ok: bool,
    pub message_id: Option<i32>,
}

// ---------------------------------------------------------------------------
// Forum topic management (bot-local UDS endpoints)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicCreateRequest {
    pub invocation_id: String,
    pub token: String,
    pub name: String,
    /// One of the 6 Telegram-allowed RGB ints. Always positive, so `u32`
    /// rather than `i32` keeps the value lossless from validation to the bot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

// Manual Debug redacts `token` — these wire structs carry the bot send token,
// and the project rule forbids token-bearing structs from being `{:?}`-logged
// in plaintext (mirrors `ProgressSendRequest`).
impl std::fmt::Debug for ForumTopicCreateRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumTopicCreateRequest")
            .field("invocation_id", &self.invocation_id)
            .field("token", &"<redacted>")
            .field("name", &self.name)
            .field("icon_color", &self.icon_color)
            .field("icon_custom_emoji_id", &self.icon_custom_emoji_id)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicEditRequest {
    pub invocation_id: String,
    pub token: String,
    pub message_thread_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

impl std::fmt::Debug for ForumTopicEditRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumTopicEditRequest")
            .field("invocation_id", &self.invocation_id)
            .field("token", &"<redacted>")
            .field("message_thread_id", &self.message_thread_id)
            .field("name", &self.name)
            .field("icon_custom_emoji_id", &self.icon_custom_emoji_id)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ForumTopicThreadRequest {
    pub invocation_id: String,
    pub token: String,
    pub message_thread_id: i32,
}

impl std::fmt::Debug for ForumTopicThreadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumTopicThreadRequest")
            .field("invocation_id", &self.invocation_id)
            .field("token", &"<redacted>")
            .field("message_thread_id", &self.message_thread_id)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForumTopicCreateResponse {
    pub ok: bool,
    pub message_thread_id: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForumTopicOkResponse {
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_server() {
        let err = InternalClientError::Server {
            status: 404,
            body: r#"{"error":"not_found"}"#.to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("404"), "expected 404 in: {msg}");
        assert!(msg.contains("not_found"), "expected body in: {msg}");
    }

    #[test]
    fn error_display_connection() {
        let err = InternalClientError::Connection(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn client_construction() {
        let client = InternalClient::new("/tmp/test.sock");
        assert_eq!(client.socket_path(), Path::new("/tmp/test.sock"));
    }

    #[test]
    fn set_token_request_serializes() {
        let req = SetTokenRequest {
            agent: "bot".into(),
            server: "notion".into(),
            access_token: "tok-abc".into(),
            refresh_token: "ref-abc".into(),
            expires_in: 3600,
            token_endpoint: "https://auth.example.com/token".into(),
            client_id: "my-client".into(),
            resource: "https://mcp.example.com/mcp".into(),
            client_secret: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["agent"], "bot");
        assert_eq!(json["expires_in"], 3600);
        assert_eq!(json["resource"], "https://mcp.example.com/mcp");
        // client_secret should be skipped when None
        assert!(!json.as_object().unwrap().contains_key("client_secret"));
    }

    #[test]
    fn mcp_instructions_response_deserializes() {
        let json = "{\"instructions\":\"# MCP Server Instructions\\n\\n## composio\\n\\nConnect apps.\\n\"}";
        let resp: McpInstructionsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.instructions.contains("composio"));
    }

    #[test]
    fn reload_response_deserializes() {
        let json = r#"{"added":["agent-b","test"],"removed":["gone"],"total":3}"#;
        let resp: ReloadResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.added, vec!["agent-b", "test"]);
        assert_eq!(resp.removed, vec!["gone"]);
        assert_eq!(resp.total, 3);
    }

    #[test]
    fn reload_response_empty_added() {
        let json = r#"{"added":[],"removed":[],"total":2}"#;
        let resp: ReloadResponse = serde_json::from_str(json).unwrap();
        assert!(resp.added.is_empty());
        assert!(resp.removed.is_empty());
        assert_eq!(resp.total, 2);
    }

    #[test]
    fn progress_register_request_serializes_expected_fields() {
        let request = ProgressRegisterRequest {
            agent: "agent-1".to_owned(),
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKindDto::Foreground,
            bot_send_token: "send-token".to_owned(),
            chat_id: Some(100),
            thread_id: Some(7),
        };

        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["agent"], "agent-1");
        assert_eq!(json["invocation_id"], "inv-1");
        assert_eq!(json["kind"], "foreground");
        assert_eq!(json["bot_send_token"], "send-token");
        assert_eq!(json["chat_id"], 100);
        assert_eq!(json["thread_id"], 7);
    }

    #[test]
    fn mcp_tool_name_is_prefixed_base_name() {
        // Invariant: the CC-prefixed form must match `mcp__right__<base>`.
        // A drift between these (e.g. server-side rename) would silently
        // make `--disallowedTools` ineffective for cron/delivery/reflection.
        assert_eq!(
            PROGRESS_MCP_TOOL,
            format!("mcp__right__{SEND_PROGRESS_TOOL}")
        );
    }

    #[test]
    fn progress_send_request_serializes_expected_fields() {
        let request = ProgressSendRequest {
            invocation_id: "inv-1".to_owned(),
            token: "send-token".to_owned(),
            message: "Still working".to_owned(),
        };

        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["invocation_id"], "inv-1");
        assert_eq!(json["token"], "send-token");
        assert_eq!(json["message"], "Still working");
    }

    #[test]
    fn progress_register_request_debug_redacts_token() {
        let request = ProgressRegisterRequest {
            agent: "agent-1".to_owned(),
            invocation_id: "inv-1".to_owned(),
            kind: ProgressInvocationKindDto::Foreground,
            bot_send_token: "supersecret".to_owned(),
            chat_id: None,
            thread_id: None,
        };
        let s = format!("{request:?}");
        assert!(
            !s.contains("supersecret"),
            "Debug must redact bot_send_token: {s}"
        );
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[test]
    fn progress_send_request_debug_redacts_token() {
        let request = ProgressSendRequest {
            invocation_id: "inv-1".to_owned(),
            token: "supersecret".to_owned(),
            message: "Still working".to_owned(),
        };
        let s = format!("{request:?}");
        assert!(!s.contains("supersecret"), "Debug must redact token: {s}");
        assert!(s.contains("<redacted>"), "Debug must mark redaction: {s}");
    }

    #[test]
    fn mcp_header_request_debug_redacts_secret_values() {
        let header = HttpHeaderInput {
            name: "Authorization".to_owned(),
            value: "Bearer raw-header-secret".to_owned(),
        };
        let add_request = McpAddRequest {
            agent: "agent-1",
            name: "nango",
            url: "https://mcp.example.com/mcp",
            auth_type: Some("headers"),
            auth_header: None,
            auth_token: Some("raw-auth-token"),
            headers: vec![header.clone()],
        };
        let set_headers_request = McpSetHeadersRequest {
            agent: "agent-1".to_owned(),
            name: "nango".to_owned(),
            headers: vec![header.clone()],
        };
        let set_token_request = SetTokenRequest {
            agent: "agent-1".to_owned(),
            server: "nango".to_owned(),
            access_token: "raw-access-token".to_owned(),
            refresh_token: "raw-refresh-token".to_owned(),
            expires_in: 3600,
            token_endpoint: "https://auth.example.com/token".to_owned(),
            client_id: "client-id".to_owned(),
            resource: "https://mcp.example.com/mcp".to_owned(),
            client_secret: Some("raw-client-secret".to_owned()),
        };

        let forum_create = ForumTopicCreateRequest {
            invocation_id: "inv-1".to_owned(),
            token: "forum-bot-token".to_owned(),
            name: "Bugs".to_owned(),
            icon_color: Some(7322096),
            icon_custom_emoji_id: None,
        };
        let forum_edit = ForumTopicEditRequest {
            invocation_id: "inv-1".to_owned(),
            token: "forum-bot-token".to_owned(),
            message_thread_id: 5,
            name: Some("Renamed".to_owned()),
            icon_custom_emoji_id: None,
        };
        let forum_thread = ForumTopicThreadRequest {
            invocation_id: "inv-1".to_owned(),
            token: "forum-bot-token".to_owned(),
            message_thread_id: 5,
        };

        for debug in [
            format!("{header:?}"),
            format!("{add_request:?}"),
            format!("{set_headers_request:?}"),
            format!("{set_token_request:?}"),
            format!("{forum_create:?}"),
            format!("{forum_edit:?}"),
            format!("{forum_thread:?}"),
        ] {
            assert!(
                !debug.contains("forum-bot-token"),
                "Debug must redact forum send tokens: {debug}"
            );
            assert!(
                !debug.contains("raw-header-secret"),
                "Debug must redact header values: {debug}"
            );
            assert!(
                !debug.contains("raw-auth-token"),
                "Debug must redact auth tokens: {debug}"
            );
            assert!(
                !debug.contains("raw-access-token"),
                "Debug must redact access tokens: {debug}"
            );
            assert!(
                !debug.contains("raw-refresh-token"),
                "Debug must redact refresh tokens: {debug}"
            );
            assert!(
                !debug.contains("raw-client-secret"),
                "Debug must redact client secrets: {debug}"
            );
            assert!(
                debug.contains("<redacted>"),
                "Debug must mark redaction: {debug}"
            );
        }
    }

    #[test]
    fn forum_topic_create_request_serializes_expected_fields() {
        let req = ForumTopicCreateRequest {
            invocation_id: "inv-1".to_owned(),
            token: "secret".to_owned(),
            name: "Bugs".to_owned(),
            icon_color: Some(7322096),
            icon_custom_emoji_id: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["invocation_id"], "inv-1");
        assert_eq!(v["name"], "Bugs");
        assert_eq!(v["icon_color"], 7322096);
        assert!(v.get("icon_custom_emoji_id").is_none(), "None is skipped");
    }
}
