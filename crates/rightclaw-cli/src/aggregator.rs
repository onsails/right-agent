//! MCP Aggregator: prefix-based routing across built-in and proxied backends.
//!
//! Three-layer architecture:
//! - [`Aggregator`] — top-level `ServerHandler` impl for `StreamableHttpService`
//! - [`ToolDispatcher`] — prefix parsing + per-agent routing
//! - [`BackendRegistry`] — per-agent backend management (RightBackend + proxies)

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use dashmap::DashMap;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use rmcp::ErrorData as McpError;
use rightclaw::mcp::proxy::ProxyBackend;
use rightclaw::mcp::refresh::RefreshMessage;
use tokio_util::sync::CancellationToken;

use crate::right_backend::RightBackend;

// ---------------------------------------------------------------------------
// Auth types & middleware (moved from memory_server_http.rs)
// ---------------------------------------------------------------------------

/// Token -> agent mapping for multi-agent HTTP mode.
pub(crate) type AgentTokenMap = Arc<tokio::sync::RwLock<HashMap<String, AgentInfo>>>;

/// Per-agent refresh scheduler sender map.
pub(crate) type RefreshSenders = Arc<HashMap<String, tokio::sync::mpsc::Sender<RefreshMessage>>>;

/// Per-agent reconnect manager map (one manager per agent, mutex-protected for mutable access).
pub(crate) type ReconnectManagers = Arc<HashMap<String, tokio::sync::Mutex<rightclaw::mcp::reconnect::ReconnectManager>>>;

/// Agent identity resolved from a Bearer token.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct AgentInfo {
    pub name: String,
    pub dir: PathBuf,
}

pub(crate) async fn bearer_auth_middleware(
    axum::extract::State(token_map): axum::extract::State<AgentTokenMap>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(token) = auth else {
        return (axum::http::StatusCode::UNAUTHORIZED, "Missing Bearer token").into_response();
    };

    let map = token_map.read().await;
    let agent = {
        use subtle::ConstantTimeEq;
        let token_bytes = token.as_bytes();
        let mut found: Option<AgentInfo> = None;
        for (candidate, agent_name) in map.iter() {
            let candidate_bytes = candidate.as_bytes();
            // Pad to equal length so ct_eq doesn't leak length via short-circuit.
            // A mismatch in length still results in 0, but we always iterate all entries.
            let eq = if candidate_bytes.len() == token_bytes.len() {
                candidate_bytes.ct_eq(token_bytes).into()
            } else {
                false
            };
            if eq {
                found = Some(agent_name.clone());
            }
        }
        found
    };
    let Some(agent) = agent else {
        return (axum::http::StatusCode::UNAUTHORIZED, "Invalid Bearer token").into_response();
    };
    drop(map);

    req.extensions_mut().insert(agent);
    next.run(req).await
}

/// Split tool name on first `__` delimiter.
/// Returns `None` if no `__` found (tool belongs to RightBackend, unprefixed).
pub(crate) fn split_prefix(tool_name: &str) -> Option<(&str, &str)> {
    tool_name.split_once("__")
}

/// MCP backend for Hindsight memory tools.
pub(crate) struct HindsightBackend {
    client: rightclaw::memory::hindsight::HindsightClient,
}

impl HindsightBackend {
    pub fn new(client: rightclaw::memory::hindsight::HindsightClient) -> Self {
        Self { client }
    }

    /// Convert a `serde_json::Value::Object` into a `serde_json::Map` for `Tool::new`.
    fn json_map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m,
            _ => unreachable!("expected JSON object"),
        }
    }

    pub fn tools_list() -> Vec<Tool> {
        vec![
            Tool::new(
                "memory_retain",
                "Store information to long-term memory. Hindsight automatically extracts \
                 structured facts, resolves entities, and indexes for retrieval.",
                Self::json_map(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The information to store."
                        },
                        "context": {
                            "type": "string",
                            "description": "Short label (e.g. 'user preference', 'api format', 'mistake to avoid')."
                        }
                    },
                    "required": ["content"]
                })),
            ),
            Tool::new(
                "memory_recall",
                "Search long-term memory. Returns memories ranked by relevance using \
                 semantic search, keyword matching, entity graph traversal, and reranking.",
                Self::json_map(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "What to search for."
                        }
                    },
                    "required": ["query"]
                })),
            ),
            Tool::new(
                "memory_reflect",
                "Synthesize a reasoned answer from long-term memories. Unlike recall, \
                 this reasons across all stored memories to produce a coherent response.",
                Self::json_map(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The question to reflect on."
                        }
                    },
                    "required": ["query"]
                })),
            ),
        ]
    }

    pub async fn tools_call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        match tool_name {
            "memory_retain" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: content"))?;
                let context = args["context"].as_str();
                let result = self
                    .client
                    .retain(content, context)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({
                    "status": "accepted",
                    "operation_id": result.operation_id,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            "memory_recall" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let results = self
                    .client
                    .recall(query)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "results": results });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            "memory_reflect" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let result = self
                    .client
                    .reflect(query)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "text": result.text });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            other => bail!("unknown hindsight tool: {other}"),
        }
    }
}

/// Per-agent backend management: built-in tools + external proxy backends.
pub(crate) struct BackendRegistry {
    pub right: RightBackend,
    pub proxies: Arc<tokio::sync::RwLock<HashMap<String, Arc<ProxyBackend>>>>,
    pub agent_dir: PathBuf,
    /// Hindsight memory backend (present only when agent has memory.provider=hindsight).
    pub hindsight: Option<Arc<HindsightBackend>>,
}

impl BackendRegistry {
    /// Dispatch a read-only meta tool. Currently only `mcp_list`.
    pub(crate) async fn handle_read_only_tool(
        &self,
        tool: &str,
        _args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        match tool {
            "mcp_list" => self.do_mcp_list().await,
            other => bail!("unknown rightmeta tool: {other}"),
        }
    }

    /// Dispatch a tool call to a named proxy backend.
    pub(crate) async fn dispatch_to_proxy(
        &self,
        proxy_name: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let proxies = self.proxies.read().await;
        let proxy = proxies.get(proxy_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Server '{proxy_name}' not found. It may have been removed."
            )
        })?;
        Ok(proxy.tools_call(tool, args).await?)
    }

    /// List all registered proxy backends with status info.
    pub(crate) async fn do_mcp_list(&self) -> Result<CallToolResult, anyhow::Error> {
        let proxies = self.proxies.read().await;
        if proxies.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No external MCP servers registered. (none)",
            )]));
        }

        let mut lines = Vec::with_capacity(proxies.len());
        for (name, handle) in proxies.iter() {
            let status = handle.status().await;
            let tool_count = handle.try_tools().map(|t| t.len()).unwrap_or(0);
            lines.push(format!(
                "- {name}: {status} ({tool_count} tools) url={url}",
                url = rightclaw::mcp::credentials::redact_url(handle.url())
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(lines.join("\n"))]))
    }

    /// Return the tool definition for `rightmeta__mcp_list`.
    pub(crate) fn mcp_list_tool_def() -> Tool {
        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), serde_json::Value::String("object".into()));
        Tool::new(
            "rightmeta__mcp_list",
            "List all registered external MCP servers with connection status, tool count, and URL.",
            schema,
        )
    }

}

/// Prefix-based tool routing across per-agent backend registries.
pub(crate) struct ToolDispatcher {
    pub agents: DashMap<String, BackendRegistry>,
}

impl ToolDispatcher {
    /// Route a tool call to the correct backend based on prefix parsing.
    pub(crate) async fn dispatch(
        &self,
        agent_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let registry = self
            .agents
            .get(agent_name)
            .with_context(|| format!("agent '{agent_name}' not registered in dispatcher"))?;

        match split_prefix(tool_name) {
            None => {
                // Unprefixed → check if it's a hindsight tool first, then RightBackend
                if let Some(ref hs) = registry.hindsight {
                    if matches!(
                        tool_name,
                        "memory_retain" | "memory_recall" | "memory_reflect"
                    ) {
                        return hs.tools_call(tool_name, args).await;
                    }
                }
                registry
                    .right
                    .tools_call(agent_name, &registry.agent_dir, tool_name, args)
                    .await
            }
            Some(("rightmeta", tool)) => {
                // Meta tools (read-only aggregator management)
                registry.handle_read_only_tool(tool, args).await
            }
            Some((prefix, tool)) => {
                // External proxy
                registry.dispatch_to_proxy(prefix, tool, args).await
            }
        }
    }

    /// Merge tool lists from all backends for a given agent.
    pub(crate) fn tools_list(&self, agent_name: &str) -> Vec<Tool> {
        let Some(registry) = self.agents.get(agent_name) else {
            return Vec::new();
        };

        let mut tools = registry.right.tools_list();

        if registry.hindsight.is_some() {
            tools.extend(HindsightBackend::tools_list());
        }

        // Add rightmeta__mcp_list
        tools.push(BackendRegistry::mcp_list_tool_def());

        // Add prefixed proxy tools. Use try_read to avoid blocking in sync context.
        let Some(proxies) = registry.proxies.try_read().ok() else {
            return tools;
        };
        for (proxy_name, handle) in proxies.iter() {
            // We read the tools using try_read on the internal lock via a
            // synchronous accessor. tools_list is not async to keep the
            // ServerHandler impl simple. Fallback: skip if lock is contended.
            if let Some(proxy_tools) = handle.try_tools() {
                for t in proxy_tools.iter() {
                    let prefixed_name = format!("{proxy_name}__{}", t.name);
                    let mut prefixed = t.clone();
                    prefixed.name = Cow::Owned(prefixed_name);
                    tools.push(prefixed);
                }
            }
        }

        tools
    }

}

/// Top-level aggregator: rmcp `ServerHandler` backed by prefix-based tool routing.
///
/// Each HTTP request creates a fresh `Aggregator` via the factory closure.
/// Agent identity is extracted from HTTP request extensions (set by bearer auth middleware).
pub(crate) struct Aggregator {
    pub dispatcher: Arc<ToolDispatcher>,
}

impl Aggregator {
    /// Factory closure for `StreamableHttpService::new`.
    ///
    /// In stateless mode, each HTTP POST creates a fresh `Aggregator`.
    pub(crate) fn factory(
        dispatcher: Arc<ToolDispatcher>,
    ) -> impl Fn() -> Result<Self, std::io::Error> + Send + Sync + 'static {
        move || {
            Ok(Self {
                dispatcher: dispatcher.clone(),
            })
        }
    }

    /// Extract `AgentInfo` from the rmcp request context.
    ///
    /// The bearer auth middleware injects `AgentInfo` into the HTTP request extensions.
    /// rmcp's `StreamableHttpService` then injects `http::request::Parts` into the
    /// rmcp `Extensions` on the `RequestContext`.
    fn agent_from_context(context: &RequestContext<RoleServer>) -> Result<AgentInfo, McpError> {
        let parts = context
            .extensions
            .get::<http::request::Parts>()
            .ok_or_else(|| {
                McpError::internal_error("HTTP request parts not found in context", None)
            })?;
        parts
            .extensions
            .get::<AgentInfo>()
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error("agent context not found in request extensions", None)
            })
    }
}

impl rmcp::ServerHandler for Aggregator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rightclaw", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "RightClaw MCP Aggregator — routes tool calls to built-in RightClaw tools \
                 and connected external MCP servers via prefix-based dispatch.\n\n\
                 Memory tools (when Hindsight is configured):\n\
                 - memory_retain: Store facts to long-term memory\n\
                 - memory_recall: Search memory by relevance\n\
                 - memory_reflect: Synthesize reasoned answers from memory",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let agent = Self::agent_from_context(&context)?;
            let tools = self.dispatcher.tools_list(&agent.name);
            Ok(ListToolsResult { tools, next_cursor: None, meta: None })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let agent = Self::agent_from_context(&context)?;
            let tool_name = request.name.as_ref();
            let args = request
                .arguments
                .map(|m| serde_json::Value::Object(m))
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            self.dispatcher
                .dispatch(&agent.name, tool_name, args)
                .await
                .map_err(|e| McpError::internal_error(format!("{e:#}"), None))
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        // No agent context available here (no RequestContext), so we cannot
        // do per-agent lookup. Return None to bypass task-support validation.
        // This is acceptable because all our tools use default TaskSupport::Forbidden.
        let _ = name;
        None
    }
}

// ---------------------------------------------------------------------------
// HTTP entry point
// ---------------------------------------------------------------------------

/// Run the MCP Aggregator over HTTP with per-agent Bearer authentication.
///
/// Replaces `run_memory_server_http` — same auth middleware, but dispatches
/// through the prefix-based `ToolDispatcher` instead of `HttpMemoryServer`.
pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
) -> miette::Result<()> {
    let ct = CancellationToken::new();

    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct.clone());

    let session_manager = Arc::new(LocalSessionManager::default());
    let factory = Aggregator::factory(dispatcher.clone());

    let mcp_service = StreamableHttpService::new(factory, session_manager, config);

    let app = axum::Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(axum::middleware::from_fn_with_state(
            token_map,
            bearer_auth_middleware,
        ));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| miette::miette!("bind to 0.0.0.0:{port} failed: {e:#}"))?;

    // Start internal REST API on Unix domain socket
    let socket_path = home.join("run/internal.sock");
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .map_err(|e| miette::miette!("remove stale UDS: {e:#}"))?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| miette::miette!("create UDS parent dir: {e:#}"))?;
    }

    let internal_app = crate::internal_api::internal_router(dispatcher, refresh_senders, reconnect_managers);
    let uds_listener = tokio::net::UnixListener::bind(&socket_path)
        .map_err(|e| miette::miette!("bind UDS {}: {e:#}", socket_path.display()))?;

    tracing::info!(
        port,
        uds = %socket_path.display(),
        agents = ?agents_dir,
        "MCP Aggregator listening"
    );

    let ct_uds_shutdown = ct.clone();
    let ct_uds_fail = ct.clone();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(uds_listener, internal_app.into_make_service())
            .with_graceful_shutdown(async move { ct_uds_shutdown.cancelled().await })
            .await
        {
            tracing::error!("UDS server error: {e:#}");
            ct_uds_fail.cancel(); // propagate failure — shut down the whole aggregator
        }
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { ct.cancelled().await })
        .await
        .map_err(|e| miette::miette!("HTTP server error: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_registry(tmp: &std::path::Path) -> BackendRegistry {
        let agents_dir = tmp.join("agents");
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        let right = RightBackend::new(agents_dir, None);
        BackendRegistry {
            right,
            proxies: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            agent_dir,
            hindsight: None,
        }
    }

    fn make_dispatcher(tmp: &std::path::Path) -> ToolDispatcher {
        let registry = make_test_registry(tmp);
        let agents = DashMap::new();
        agents.insert("test-agent".into(), registry);
        ToolDispatcher { agents }
    }

    // ---- split_prefix tests ----

    #[test]
    fn split_prefix_with_delimiter() {
        assert_eq!(split_prefix("notion__search"), Some(("notion", "search")));
    }

    #[test]
    fn split_prefix_without_delimiter() {
        assert_eq!(split_prefix("store_record"), None);
    }

    #[test]
    fn split_prefix_multiple_delimiters() {
        assert_eq!(
            split_prefix("notion__my__tool"),
            Some(("notion", "my__tool"))
        );
    }

    // ---- tools_list tests ----

    #[test]
    fn tools_list_includes_right_and_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let tools = dispatcher.tools_list("test-agent");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

        // RightBackend tools present (unprefixed)
        assert!(names.contains(&"cron_create"), "missing cron_create");
        assert!(names.contains(&"bootstrap_done"), "missing bootstrap_done");

        // Meta tool present
        assert!(
            names.contains(&"rightmeta__mcp_list"),
            "missing rightmeta__mcp_list"
        );
    }

    // ---- dispatch tests ----

    #[tokio::test]
    async fn dispatch_unprefixed_goes_to_right() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        // store_record requires valid params and a DB, so we use a tool that
        // exercises RightBackend dispatch. bootstrap_done checks files — should
        // return a tool-level error (missing files), not an infrastructure error.
        let result = dispatcher
            .dispatch(
                "test-agent",
                "bootstrap_done",
                serde_json::json!({}),
            )
            .await;

        assert!(result.is_ok(), "dispatch should succeed: {result:?}");
        let ctr = result.unwrap();
        // bootstrap_done returns error because IDENTITY.md etc. are missing
        assert_eq!(ctr.is_error, Some(true));
    }

    #[tokio::test]
    async fn dispatch_unknown_proxy_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let result = dispatcher
            .dispatch("test-agent", "notion__search", serde_json::json!({}))
            .await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Server 'notion' not found"),
            "unexpected error: {msg}"
        );
    }

    // ---- inputSchema validation ----

    /// CC silently drops ALL MCP tools if any tool has an invalid inputSchema.
    /// An empty `{}` is invalid — every schema must have `"type": "object"`.
    #[test]
    fn all_tools_have_valid_input_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());
        let tools = dispatcher.tools_list("test-agent");

        for tool in &tools {
            let schema = &tool.input_schema;
            assert!(
                !schema.is_empty(),
                "tool '{}' has empty inputSchema — CC will silently drop ALL tools",
                tool.name
            );
            assert!(
                schema.contains_key("type"),
                "tool '{}' inputSchema missing 'type' field — must be {{\"type\": \"object\"}}",
                tool.name
            );
            let type_val = &schema["type"];
            assert_eq!(
                type_val.as_str(),
                Some("object"),
                "tool '{}' inputSchema 'type' must be \"object\", got {:?}",
                tool.name,
                type_val
            );
        }
    }

    // ---- mcp_list tests ----

    #[tokio::test]
    async fn mcp_list_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = make_test_registry(tmp.path());

        let result = registry.do_mcp_list().await.unwrap();
        let text = match &result.content[0].raw {
            rmcp::model::RawContent::Text(t) => t.text.as_str(),
            _ => panic!("expected text content"),
        };
        assert!(text.contains("(none)"), "should mention (none): {text}");
    }
}
