//! Shared in-test MCP server helpers (a real two-tool rmcp server on loopback).
#![cfg(test)]

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::model::{ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool};
use rmcp::service::{RequestContext, RoleServer};

/// Install ring as the rustls process-level crypto provider. Idempotent —
/// safe to call from multiple tests in the same binary.
fn setup_crypto() {
    // install_default returns Err(existing provider Arc) when already
    // installed by another test in the same binary — that's not a failure.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Minimal in-test MCP server exposing two tools.
/// Mirrors the `StreamableHttpService` wiring in `crates/right/src/aggregator.rs`.
#[derive(Clone)]
pub(crate) struct TwoToolServer;

impl ServerHandler for TwoToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    // Match aggregator.rs's hand-desugared signature: rewriting as `async fn`
    // changes the `Send` bound placement and fails the rmcp trait.
    #[allow(clippy::manual_async_fn)]
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let schema: rmcp::model::JsonObject =
                serde_json::from_value(serde_json::json!({"type": "object"})).unwrap();
            let schema = Arc::new(schema);
            let mk = |n: &str| Tool::new(n.to_string(), "test tool", schema.clone());
            Ok(ListToolsResult {
                tools: vec![mk("alpha"), mk("beta")],
                next_cursor: None,
                meta: None,
            })
        }
    }
}

/// Bind the `TwoToolServer` on a loopback port; return its
/// `http://127.0.0.1:<port>/mcp` URL. The join handle keeps the server alive.
pub(crate) async fn serve_two_tool_server() -> (tokio::task::JoinHandle<()>, String) {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    setup_crypto();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Default config's loopback host allowlist (localhost/127.0.0.1/::1)
    // already accepts the 127.0.0.1 client used by this test.
    let config = StreamableHttpServerConfig::default();
    let service = StreamableHttpService::new(
        || Ok::<_, std::io::Error>(TwoToolServer),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (handle, format!("http://127.0.0.1:{port}/mcp"))
}
