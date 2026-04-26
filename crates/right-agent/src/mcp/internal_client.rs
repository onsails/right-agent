//! Hyper-based Unix domain socket client for bot→aggregator IPC.
//!
//! Uses raw `hyper` with `tokio::net::UnixStream` to POST JSON to the
//! internal API served on a Unix domain socket. `reqwest` doesn't support
//! UDS natively, so we use hyper's low-level HTTP/1.1 client directly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
        self.post(
            "/mcp-add",
            &serde_json::json!({
                "agent": agent,
                "name": name,
                "url": url,
                "auth_type": auth_type,
                "auth_header": auth_header,
                "auth_token": auth_token,
            }),
        )
        .await
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
}

// ---------------------------------------------------------------------------
// Response types (must match internal_api.rs on the server side)
// ---------------------------------------------------------------------------

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
}

#[derive(Debug, Serialize)]
pub struct SetTokenRequest {
    pub agent: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
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
            client_secret: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["agent"], "bot");
        assert_eq!(json["expires_in"], 3600);
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
        let json = r#"{"added":["him","test"],"removed":["gone"],"total":3}"#;
        let resp: ReloadResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.added, vec!["him", "test"]);
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
}
