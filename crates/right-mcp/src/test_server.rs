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
    serve_handler(TwoToolServer).await
}

/// In-test MCP server whose `call_tool` always succeeds. Used to exercise the
/// reconnect-and-retry self-heal path in `ProxyBackend::tools_call` against a
/// live upstream.
#[derive(Clone)]
pub(crate) struct CallableServer;

impl ServerHandler for CallableServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

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
            Ok(ListToolsResult {
                tools: vec![Tool::new("alpha".to_string(), "test tool", schema)],
                next_cursor: None,
                meta: None,
            })
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        _request: rmcp::model::CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
    + Send
    + '_ {
        async move {
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::Content::text("ok"),
            ]))
        }
    }
}

/// Bind the `CallableServer` on a loopback port; return its
/// `http://127.0.0.1:<port>/mcp` URL. The join handle keeps the server alive.
pub(crate) async fn serve_callable_server() -> (tokio::task::JoinHandle<()>, String) {
    serve_handler(CallableServer).await
}

/// Bind an arbitrary `ServerHandler` on a loopback port; return its
/// `http://127.0.0.1:<port>/mcp` URL. The join handle keeps the server alive.
async fn serve_handler<H>(handler: H) -> (tokio::task::JoinHandle<()>, String)
where
    H: ServerHandler + Clone + 'static,
{
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
        move || Ok::<_, std::io::Error>(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (handle, format!("http://127.0.0.1:{port}/mcp"))
}
