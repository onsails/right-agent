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
use rightclaw::mcp::proxy::{BackendStatus, ProxyBackend};
use tokio_util::sync::CancellationToken;

use crate::memory_server_http::{AgentInfo, AgentTokenMap, bearer_auth_middleware};
use crate::right_backend::RightBackend;

/// Maximum characters per backend in merged instructions.
const INSTRUCTIONS_TRUNCATION_LIMIT: usize = 4000;

/// Split tool name on first `__` delimiter.
/// Returns `None` if no `__` found (tool belongs to RightBackend, unprefixed).
pub(crate) fn split_prefix(tool_name: &str) -> Option<(&str, &str)> {
    tool_name.split_once("__")
}

/// Handle wrapping a `ProxyBackend` for registration in `BackendRegistry`.
pub(crate) struct ProxyHandle {
    pub backend: ProxyBackend,
}

/// Per-agent backend management: built-in tools + external proxy handles.
pub(crate) struct BackendRegistry {
    pub right: RightBackend,
    pub proxies: HashMap<String, Arc<ProxyHandle>>,
    pub agent_name: String,
    pub agent_dir: PathBuf,
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
        let proxy = self.proxies.get(proxy_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Server '{proxy_name}' not found. It may have been removed."
            )
        })?;
        Ok(proxy.backend.tools_call(tool, args).await?)
    }

    /// List all registered proxy backends with status info.
    pub(crate) async fn do_mcp_list(&self) -> Result<CallToolResult, anyhow::Error> {
        if self.proxies.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No external MCP servers registered. (none)",
            )]));
        }

        let mut lines = Vec::with_capacity(self.proxies.len());
        for (name, handle) in &self.proxies {
            let status = handle.backend.status().await;
            let tool_count = handle.backend.tools().await.len();
            let status_str = match status {
                BackendStatus::Connected => "connected",
                BackendStatus::NeedsAuth => "needs_auth",
                BackendStatus::Unreachable => "unreachable",
            };
            lines.push(format!(
                "- {name}: {status_str} ({tool_count} tools) url={url}",
                url = handle.backend.url()
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(lines.join("\n"))]))
    }

    /// Return the tool definition for `rightmeta__mcp_list`.
    pub(crate) fn mcp_list_tool_def() -> Tool {
        Tool::new(
            "rightmeta__mcp_list",
            "List all registered external MCP servers with connection status, tool count, and URL.",
            serde_json::Map::new(),
        )
    }

    /// Merge instructions from all backends, truncating each to [`INSTRUCTIONS_TRUNCATION_LIMIT`].
    pub(crate) async fn build_instructions(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Right backend instructions (static).
        parts.push("## RightClaw Built-in Tools\nMemory, cron, and MCP management tools.".into());

        for (name, handle) in &self.proxies {
            if let Some(ref instr) = handle.backend.instructions().await {
                let truncated = if instr.len() > INSTRUCTIONS_TRUNCATION_LIMIT {
                    format!(
                        "## {name}\n{}... (truncated)",
                        &instr[..INSTRUCTIONS_TRUNCATION_LIMIT]
                    )
                } else {
                    format!("## {name}\n{instr}")
                };
                parts.push(truncated);
            }
        }

        parts.join("\n\n")
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
                // Unprefixed → RightBackend
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

        // Add rightmeta__mcp_list
        tools.push(BackendRegistry::mcp_list_tool_def());

        // Add prefixed proxy tools
        for (proxy_name, handle) in &registry.proxies {
            // We read the tools using try_read on the internal lock via a
            // synchronous accessor. tools_list is not async to keep the
            // ServerHandler impl simple. Fallback: skip if lock is contended.
            if let Some(proxy_tools) = handle.backend.try_tools() {
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

    /// Build merged instructions for a given agent.
    pub(crate) async fn instructions(&self, agent_name: &str) -> String {
        let Some(registry) = self.agents.get(agent_name) else {
            return String::new();
        };
        registry.build_instructions().await
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
                 and connected external MCP servers via prefix-based dispatch.",
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
) -> miette::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let ct = CancellationToken::new();

    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct.clone());

    let session_manager = Arc::new(LocalSessionManager::default());
    let factory = Aggregator::factory(dispatcher);

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

    tracing::info!(port, agents = ?agents_dir, "MCP Aggregator listening");

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

        let right = RightBackend::new(agents_dir, tmp.to_path_buf());
        BackendRegistry {
            right,
            proxies: HashMap::new(),
            agent_name: "test-agent".into(),
            agent_dir,
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
        assert!(names.contains(&"store_record"), "missing store_record");
        assert!(names.contains(&"query_records"), "missing query_records");

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
