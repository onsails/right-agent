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
use right_agent::mcp::proxy::ProxyBackend;
use right_agent::mcp::refresh::RefreshMessage;
use right_agent::mcp::tool_error::tool_error;
use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
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
pub(crate) type ReconnectManagers =
    Arc<HashMap<String, tokio::sync::Mutex<right_agent::mcp::reconnect::ReconnectManager>>>;

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
    client: std::sync::Arc<right_agent::memory::ResilientHindsight>,
}

impl HindsightBackend {
    pub fn new(client: std::sync::Arc<right_agent::memory::ResilientHindsight>) -> Self {
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
                let res = self
                    .client
                    .retain(
                        content,
                        context,
                        None,
                        None,
                        None,
                        right_agent::memory::resilient::POLICY_MCP_RETAIN,
                    )
                    .await;
                match res {
                    Ok(result) => {
                        let json = serde_json::json!({
                            "status": "accepted",
                            "operation_id": result.operation_id,
                        });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json)?,
                        )]))
                    }
                    Err(right_agent::memory::ResilientError::Upstream(e)) => {
                        // ResilientHindsight::retain enqueues for later drain on
                        // Transient/RateLimited. Surface that as a success with a
                        // "queued" marker so the agent does not report a hard
                        // failure nor retry (which would double-enqueue — the
                        // pending_retains queue does not dedup).
                        match e.classify() {
                            right_agent::memory::ErrorKind::Transient
                            | right_agent::memory::ErrorKind::RateLimited => {
                                let json = serde_json::json!({
                                    "status": "queued",
                                    "reason": "upstream degraded, queued for retry on next drain tick",
                                    "detail": format!("{e:#}"),
                                });
                                Ok(CallToolResult::success(vec![Content::text(
                                    serde_json::to_string_pretty(&json)?,
                                )]))
                            }
                            right_agent::memory::ErrorKind::Auth => Ok(
                                tool_error(
                                    "upstream_auth",
                                    format!("{e:#}"),
                                    None,
                                ),
                            ),
                            right_agent::memory::ErrorKind::Client
                            | right_agent::memory::ErrorKind::Malformed => Ok(
                                tool_error(
                                    "upstream_invalid",
                                    format!("{e:#}"),
                                    None,
                                ),
                            ),
                        }
                    }
                    Err(right_agent::memory::ResilientError::CircuitOpen { retry_after }) => {
                        // AuthFailed circuits won't recover without user action — queueing
                        // would grow the backlog indefinitely, so return a hard auth error
                        // rather than a silent queue. retain() honors the same distinction.
                        if matches!(
                            self.client.status(),
                            right_agent::memory::MemoryStatus::AuthFailed { .. }
                        ) {
                            Ok(tool_error(
                                "upstream_auth",
                                "memory auth failed; retain rejected",
                                None,
                            ))
                        } else {
                            let json = serde_json::json!({
                                "status": "queued",
                                "reason": "circuit breaker open; queued for retry on next drain tick",
                                "retry_after_secs": retry_after.map(|d| d.as_secs()),
                            });
                            Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string_pretty(&json)?,
                            )]))
                        }
                    }
                }
            }
            "memory_recall" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let res = self
                    .client
                    .recall(
                        query,
                        None,
                        None,
                        right_agent::memory::resilient::POLICY_MCP_RECALL,
                    )
                    .await;
                match res {
                    Ok(results) => {
                        let json = serde_json::json!({ "results": results });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json)?,
                        )]))
                    }
                    Err(e) => Ok(self.classify_resilient_error(e)),
                }
            }
            "memory_reflect" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let res = self
                    .client
                    .reflect(query, right_agent::memory::resilient::POLICY_MCP_REFLECT)
                    .await;
                match res {
                    Ok(result) => {
                        let json = serde_json::json!({ "text": result.text });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json)?,
                        )]))
                    }
                    Err(e) => Ok(self.classify_resilient_error(e)),
                }
            }
            other => bail!("unknown hindsight tool: {other}"),
        }
    }

    /// Map a `ResilientError` from `recall` / `reflect` to a structured
    /// operation error. The `retain` path has its own queueing semantics and
    /// does not use this helper.
    fn classify_resilient_error(
        &self,
        e: right_agent::memory::ResilientError,
    ) -> CallToolResult {
        match e {
            right_agent::memory::ResilientError::Upstream(ref inner) => match inner.classify() {
                right_agent::memory::ErrorKind::Transient
                | right_agent::memory::ErrorKind::RateLimited => {
                    tool_error(
                        "upstream_unreachable",
                        format!("{e:#}"),
                        None,
                    )
                }
                right_agent::memory::ErrorKind::Auth => tool_error(
                    "upstream_auth",
                    format!("{e:#}"),
                    None,
                ),
                right_agent::memory::ErrorKind::Client
                | right_agent::memory::ErrorKind::Malformed => {
                    tool_error(
                        "upstream_invalid",
                        format!("{e:#}"),
                        None,
                    )
                }
            },
            right_agent::memory::ResilientError::CircuitOpen { retry_after } => {
                if matches!(
                    self.client.status(),
                    right_agent::memory::MemoryStatus::AuthFailed { .. }
                ) {
                    tool_error(
                        "upstream_auth",
                        format!("{e:#}"),
                        None,
                    )
                } else {
                    let details = retry_after
                        .map(|d| serde_json::json!({ "retry_after_secs": d.as_secs() }));
                    tool_error(
                        "circuit_open",
                        format!("{e:#}"),
                        details,
                    )
                }
            }
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
        let Some(proxy) = proxies.get(proxy_name) else {
            return Ok(tool_error(
                "server_not_found",
                format!("Server '{proxy_name}' not found. It may have been removed."),
                None,
            ));
        };
        match proxy.tools_call(tool, args).await {
            Ok(result) => Ok(result),
            Err(e) => Ok(CallToolResult::from(e)),
        }
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
                url = right_agent::mcp::credentials::redact_url(handle.url())
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
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
                if let Some(ref hs) = registry.hindsight
                    && matches!(
                        tool_name,
                        "memory_retain" | "memory_recall" | "memory_reflect"
                    )
                {
                    return hs.tools_call(tool_name, args).await;
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
        parts.extensions.get::<AgentInfo>().cloned().ok_or_else(|| {
            McpError::internal_error("agent context not found in request extensions", None)
        })
    }
}

impl rmcp::ServerHandler for Aggregator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("right", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Right Agent MCP Aggregator — routes tool calls to built-in Right Agent tools \
                 and connected external MCP servers via prefix-based dispatch.\n\n\
                 Memory tools (when Hindsight is configured):\n\
                 - memory_retain: Store facts to long-term memory\n\
                 - memory_recall: Search memory by relevance\n\
                 - memory_reflect: Synthesize reasoned answers from memory\n\
                 (Errors follow the aggregator-level error convention; see below.)\n\n\
                 Error convention (operation errors):\n\
                 On operation failure, tools return is_error: true with content\n  \
                 { \"error\": { \"code\": \"<code>\", \"message\": \"<human readable>\", \"details\"?: {...} } }\n\
                 Cross-cutting codes any tool may emit:\n  \
                 upstream_unreachable — backend service unreachable / transport failure\n  \
                 upstream_auth        — backend authentication required or rejected\n  \
                 upstream_invalid     — backend rejected the request (4xx, malformed)\n  \
                 circuit_open         — local circuit breaker open; retry later\n  \
                 invalid_argument     — semantic argument validation failed\n  \
                 tool_failed          — upstream tool returned its own error (see details)\n  \
                 server_not_found     — referenced MCP server is not registered\n\
                 Tool-specific codes are documented in each tool's description.",
            )
    }

    // Matches rmcp trait signature `-> impl Future<..> + Send + '_`; rewriting
    // as `async fn` changes the desugared `Send` bound placement.
    #[allow(clippy::manual_async_fn)]
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let agent = Self::agent_from_context(&context)?;
            let tools = self.dispatcher.tools_list(&agent.name);
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            })
        }
    }

    // Matches rmcp trait signature `-> impl Future<..> + Send + '_`; rewriting
    // as `async fn` changes the desugared `Send` bound placement.
    #[allow(clippy::manual_async_fn)]
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
                .map(serde_json::Value::Object)
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

/// Build the `StreamableHttpServerConfig` the Aggregator runs with.
///
/// Since rmcp v1.4.0, the default config enforces a DNS-rebinding Host-header
/// allowlist (`localhost`, `127.0.0.1`, `::1` only). Sandbox clients reach the
/// aggregator as `host.openshell.internal:<port>` and other non-loopback names,
/// so the default 403s every authenticated request. This helper:
/// - empty `allowed_hosts` → `.disable_allowed_hosts()` (host check off). Safe
///   because per-agent Bearer already authenticates every request; DNS
///   rebinding only bites browser-ambient-auth scenarios that don't apply.
/// - non-empty → `.with_allowed_hosts(...)`. Use when the aggregator is
///   exposed on a fixed public hostname and defence-in-depth is wanted.
fn build_streamable_config(
    ct: CancellationToken,
    allowed_hosts: &[String],
) -> StreamableHttpServerConfig {
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct);
    if allowed_hosts.is_empty() {
        config.disable_allowed_hosts()
    } else {
        config.with_allowed_hosts(allowed_hosts.iter().cloned())
    }
}

/// Run the MCP Aggregator over HTTP with per-agent Bearer authentication.
///
/// Replaces `run_memory_server_http` — same auth middleware, but dispatches
/// through the prefix-based `ToolDispatcher` instead of `HttpMemoryServer`.
// internal helper; refactor to a config struct is out of scope for this cleanup pass
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_aggregator_http(
    port: u16,
    token_map: AgentTokenMap,
    token_map_path: PathBuf,
    dispatcher: Arc<ToolDispatcher>,
    agents_dir: PathBuf,
    home: PathBuf,
    refresh_senders: RefreshSenders,
    reconnect_managers: ReconnectManagers,
    allowed_hosts: Vec<String>,
) -> miette::Result<()> {
    let ct = CancellationToken::new();

    let config = build_streamable_config(ct.clone(), &allowed_hosts);

    let session_manager = Arc::new(LocalSessionManager::default());
    let factory = Aggregator::factory(dispatcher.clone());

    let mcp_service = StreamableHttpService::new(factory, session_manager, config);

    let token_map_for_reload = token_map.clone();
    let app = axum::Router::new().nest_service("/mcp", mcp_service).layer(
        axum::middleware::from_fn_with_state(token_map, bearer_auth_middleware),
    );

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

    let internal_app = crate::internal_api::internal_router(
        dispatcher,
        refresh_senders,
        reconnect_managers,
        token_map_for_reload,
        token_map_path,
        agents_dir.clone(),
    );
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

    fn aggregator_test_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
        let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
            panic!("expected text content, got {:?}", result.content[0].raw);
        };
        serde_json::from_str(&t.text).expect("body must be valid JSON")
    }

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
            .dispatch("test-agent", "bootstrap_done", serde_json::json!({}))
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
            .await
            .expect("dispatch should return Ok with operation error");
        assert_eq!(result.is_error, Some(true));
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "server_not_found");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Server 'notion' not found"),
            "unexpected message: {body:?}"
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

    // ---- build_streamable_config: regression for rmcp 1.4+ Host-header 403 ----

    #[test]
    fn build_streamable_config_empty_disables_host_check() {
        let config = build_streamable_config(CancellationToken::new(), &[]);
        assert!(
            config.allowed_hosts.is_empty(),
            "empty input must produce empty allowed_hosts (host check disabled), got: {:?}",
            config.allowed_hosts
        );
    }

    #[test]
    fn build_streamable_config_populates_host_check_when_provided() {
        let hosts = vec![
            "mcp.example.com".to_string(),
            "mcp.example.com:8100".to_string(),
        ];
        let config = build_streamable_config(CancellationToken::new(), &hosts);
        assert_eq!(config.allowed_hosts, hosts);
    }

    #[test]
    fn build_streamable_config_rejects_rmcp_default_that_caused_outage() {
        // Regression: rmcp 1.4.0 added a DNS-rebinding check and
        // `StreamableHttpServerConfig::default()` ships with
        // `["localhost", "127.0.0.1", "::1"]`. That breaks every sandbox
        // request (Host: host.openshell.internal:<port>) with
        // 403 "Forbidden: Host header is not allowed".
        // The empty-list helper must NOT leak that default through.
        let config = build_streamable_config(CancellationToken::new(), &[]);
        for banned in ["localhost", "127.0.0.1", "::1"] {
            assert!(
                !config.allowed_hosts.iter().any(|h| h == banned),
                "default loopback-only allowlist leaked: {banned} present in {:?}",
                config.allowed_hosts
            );
        }
    }

    // ---- HindsightBackend mock-server tests ----

    /// Mock HTTP server that responds to each incoming connection with the given
    /// status + body. Mirrors the helper from `right-agent::memory::resilient`
    /// tests; copied (not exposed) to avoid test-only public API growth.
    async fn mock_hindsight(body: &str, status: u16) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }

    fn make_hindsight_backend(
        url: &str,
    ) -> (tempfile::TempDir, std::sync::Arc<HindsightBackend>) {
        use right_agent::memory::ResilientHindsight;
        use right_agent::memory::hindsight::HindsightClient;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let _ = right_db::open_connection(&dir, true).unwrap();
        let client = HindsightClient::new("hs_x", "bank-1", "high", 1024, Some(url));
        let resilient = std::sync::Arc::new(ResilientHindsight::new(client, dir, "test"));
        (tmp, std::sync::Arc::new(HindsightBackend::new(resilient)))
    }

    #[tokio::test]
    async fn memory_retain_auth_returns_upstream_auth() {
        let (_h, url) = mock_hindsight(r#"{"error": "unauthorized"}"#, 401).await;
        let (_tmp, backend) = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok with operation error");
        assert_eq!(result.is_error, Some(true));
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }

    #[tokio::test]
    async fn memory_retain_client_returns_upstream_invalid() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad request"}"#, 400).await;
        let (_tmp, backend) = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_invalid");
    }

    #[tokio::test]
    async fn memory_retain_transient_remains_queued_success() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad gateway"}"#, 502).await;
        let (_tmp, backend) = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok success with queued status");
        // is_error is either None or Some(false) — both are acceptable success
        assert!(matches!(result.is_error, None | Some(false)));
        let body = aggregator_test_body(&result);
        assert_eq!(body["status"], "queued");
    }

    #[tokio::test]
    async fn memory_recall_auth_returns_upstream_auth() {
        let (_h, url) = mock_hindsight(r#"{"error": "unauthorized"}"#, 401).await;
        let (_tmp, backend) = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_recall",
                serde_json::json!({ "query": "test" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }

    #[tokio::test]
    async fn memory_recall_transient_returns_upstream_unreachable() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad gateway"}"#, 502).await;
        let (_tmp, backend) = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_recall",
                serde_json::json!({ "query": "test" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }
}
