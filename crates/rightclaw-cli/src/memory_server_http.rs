//! HTTP-based MCP memory server with per-agent Bearer token authentication.
//!
//! One HTTP server process on the host, all agents share it.
//! Each agent is identified by a unique Bearer token mapped to [`AgentInfo`].
//! Auth is validated in an axum middleware layer before reaching
//! `StreamableHttpService`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;
use dashmap::DashMap;
use rmcp::{
    handler::server::{tool::Extension, tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::local::LocalSessionManager,
    },
    ErrorData as McpError,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::memory_server::{
    CronCreateParams, CronDeleteParams, CronListParams, CronListRunsParams, CronShowRunParams,
    DeleteRecordParams, McpAddParams, McpAuthParams, McpListParams, McpRemoveParams,
    QueryRecordsParams, SearchRecordsParams, StoreRecordParams, cron_run_to_json, entry_to_json,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Token -> agent mapping for multi-agent HTTP mode.
pub type AgentTokenMap = Arc<RwLock<HashMap<String, AgentInfo>>>;

/// Agent identity resolved from a Bearer token.
#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub name: String,
    pub dir: PathBuf,
}

// ---------------------------------------------------------------------------
// HttpMemoryServer — per-request agent resolution via Extensions
// ---------------------------------------------------------------------------

/// Connection cache keyed by agent name.
type ConnCache = Arc<DashMap<String, Arc<Mutex<rusqlite::Connection>>>>;

#[derive(Clone)]
pub struct HttpMemoryServer {
    tool_router: ToolRouter<Self>,
    conn_cache: ConnCache,
    agents_dir: PathBuf,
    #[allow(dead_code)]
    rightclaw_home: PathBuf,
}

impl HttpMemoryServer {
    fn get_conn_for_agent(&self, agent: &AgentInfo) -> Result<Arc<Mutex<rusqlite::Connection>>, McpError> {
        // Fast path: already cached.
        if let Some(entry) = self.conn_cache.get(&agent.name) {
            return Ok(Arc::clone(entry.value()));
        }
        // Slow path: open DB and cache.
        let db_dir = self.agents_dir.join(&agent.name);
        let conn = rightclaw::memory::open_connection(&db_dir)
            .map_err(|e| McpError::internal_error(format!("failed to open memory DB for {}: {e:#}", agent.name), None))?;
        let conn = Arc::new(Mutex::new(conn));
        self.conn_cache.insert(agent.name.clone(), Arc::clone(&conn));
        Ok(conn)
    }

    /// Extract `AgentInfo` from the HTTP request parts injected by rmcp.
    fn agent_from_parts(parts: &http::request::Parts) -> Result<AgentInfo, McpError> {
        parts
            .extensions
            .get::<AgentInfo>()
            .cloned()
            .ok_or_else(|| McpError::internal_error("agent context not found in request extensions", None))
    }
}

#[tool_router]
impl HttpMemoryServer {
    pub fn new(
        conn_cache: ConnCache,
        agents_dir: PathBuf,
        rightclaw_home: PathBuf,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            conn_cache,
            agents_dir,
            rightclaw_home,
        }
    }

    #[tool(description = "Store a tagged record. Content is scanned for prompt injection. Use for structured data (cron results, audit entries, explicit facts) — not for general conversation context. Returns record ID.")]
    async fn store_record(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::store_memory(
            &conn,
            &params.content,
            params.tags.as_deref(),
            Some(agent.name.as_str()),
            Some("mcp:store_record"),
        ) {
            Ok(id) => Ok(CallToolResult::success(vec![Content::text(format!("stored record id={id}"))])),
            Err(rightclaw::memory::MemoryError::InjectionDetected) => Err(McpError::invalid_params(
                "content rejected: possible prompt injection detected",
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "Look up records by tag or keyword. Returns matching active records.")]
    async fn query_records(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<QueryRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::recall_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Full-text search records using FTS5. Returns BM25-ranked results.")]
    async fn search_records(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<SearchRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::search_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Soft-delete a record by ID. Entry is excluded from queries but preserved in audit log.")]
    async fn delete_record(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<DeleteRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let id = params.id;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::forget_memory(&conn, id, Some(agent.name.as_str())) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!("deleted record id={id}"))])),
            Err(rightclaw::memory::MemoryError::NotFound(_)) => Err(McpError::invalid_params(
                format!("record id={id} not found or already deleted"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "List recent cron job runs. Returns runs sorted by started_at descending. Optionally filter by job_name and/or limit the count. Each result includes log_path -- use bash to read the log file directly.")]
    async fn cron_list_runs(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CronListRunsParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let limit = params.limit.unwrap_or(20);
        let mut stmt = conn
            .prepare(
                "SELECT id, job_name, started_at, finished_at, exit_code, status, log_path
                 FROM cron_runs
                 WHERE (?1 IS NULL OR job_name = ?1)
                 ORDER BY started_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| McpError::internal_error(format!("prepare failed: {e:#}"), None))?;
        let rows: Vec<serde_json::Value> = stmt
            .query_map(rusqlite::params![params.job_name, limit], |row| {
                Ok(cron_run_to_json(
                    &row.get::<_, String>(0)?,
                    &row.get::<_, String>(1)?,
                    &row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?.as_deref(),
                    row.get::<_, Option<i64>>(4)?,
                    &row.get::<_, String>(5)?,
                    &row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| McpError::internal_error(format!("query failed: {e:#}"), None))?
            .filter_map(|r| r.ok())
            .collect();
        let output = serde_json::to_string_pretty(&rows)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Get full metadata for a single cron job run by its run_id (UUID). Returns the same fields as cron_list_runs. Use log_path with bash to read subprocess output.")]
    async fn cron_show_run(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CronShowRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = conn.query_row(
            "SELECT id, job_name, started_at, finished_at, exit_code, status, log_path
             FROM cron_runs WHERE id = ?1",
            rusqlite::params![params.run_id],
            |row| {
                Ok(cron_run_to_json(
                    &row.get::<_, String>(0)?,
                    &row.get::<_, String>(1)?,
                    &row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?.as_deref(),
                    row.get::<_, Option<i64>>(4)?,
                    &row.get::<_, String>(5)?,
                    &row.get::<_, String>(6)?,
                ))
            },
        );
        match result {
            Ok(val) => {
                let output = serde_json::to_string_pretty(&val)
                    .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "cron run '{}' not found",
                    params.run_id
                ))]))
            }
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "Create a new cron job spec. The job will be picked up by the cron engine on its next reload cycle.")]
    async fn cron_create(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CronCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = rightclaw::cron_spec::create_spec(
            &conn,
            &params.job_name,
            &params.schedule,
            &params.prompt,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
        )
        .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    #[tool(description = "Update an existing cron job spec (full replacement). All fields are overwritten.")]
    async fn cron_update(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CronCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = rightclaw::cron_spec::update_spec(
            &conn,
            &params.job_name,
            &params.schedule,
            &params.prompt,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
        )
        .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    #[tool(description = "Delete a cron job spec. Also removes its lock file if present.")]
    async fn cron_delete(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CronDeleteParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let agent_dir = self.agents_dir.join(&agent.name);
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let msg = rightclaw::cron_spec::delete_spec(&conn, &params.job_name, &agent_dir)
            .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "List all current cron job specs. Returns a JSON array of all configured cron jobs.")]
    async fn cron_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<CronListParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let output = rightclaw::cron_spec::list_specs(&conn)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Add an HTTP MCP server to this agent's mcp.json. Use /mcp auth <name> in Telegram to complete OAuth if the server requires authentication.")]
    async fn mcp_add(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<McpAddParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        if !params.url.starts_with("https://") {
            return Err(McpError::invalid_params(
                format!("URL must start with 'https://' -- got: {}", params.url),
                None,
            ));
        }
        let mcp_json_path = agent.dir.join("mcp.json");
        rightclaw::mcp::credentials::add_http_server(&mcp_json_path, &params.name, &params.url)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Added MCP server '{}' ({}).",
            params.name, params.url
        ))]))
    }

    #[tool(description = "Remove an HTTP MCP server from this agent's mcp.json. The 'right' server is protected and cannot be removed.")]
    async fn mcp_remove(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<McpRemoveParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        if params.name == rightclaw::mcp::PROTECTED_MCP_SERVER {
            return Err(McpError::invalid_params(
                format!("Cannot remove '{}' -- required for core agent functionality", params.name),
                None,
            ));
        }
        let mcp_json_path = agent.dir.join("mcp.json");
        match rightclaw::mcp::credentials::remove_http_server(&mcp_json_path, &params.name) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Removed MCP server '{}'.",
                params.name
            ))])),
            Err(rightclaw::mcp::credentials::CredentialError::ServerNotFound(_)) => {
                Err(McpError::invalid_params(
                    format!("Server '{}' not found in mcp.json.", params.name),
                    None,
                ))
            }
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "List all configured MCP servers for this agent. Shows name, URL, auth state (present/auth required), source (.claude.json or mcp.json), and kind (http/stdio). Never exposes token values.")]
    async fn mcp_list(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<McpListParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let statuses = rightclaw::mcp::detect::mcp_auth_status(&agent.dir)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let items: Vec<serde_json::Value> = statuses
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "url": s.url,
                    "auth": s.state.to_string(),
                    "source": s.source.to_string(),
                    "kind": s.kind.to_string(),
                })
            })
            .collect();
        let output = serde_json::to_string_pretty(&items)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist.")]
    async fn bootstrap_done(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let agent_dir = self.agents_dir.join(&agent.name);
        let required = ["IDENTITY.md", "SOUL.md", "USER.md"];
        let missing: Vec<&str> = required
            .iter()
            .filter(|f| !agent_dir.join(f).exists())
            .copied()
            .collect();

        if missing.is_empty() {
            let bootstrap_path = agent_dir.join("BOOTSTRAP.md");
            if bootstrap_path.exists() {
                std::fs::remove_file(&bootstrap_path).ok();
            }
            Ok(CallToolResult::success(vec![Content::text(
                "Bootstrap complete! IDENTITY.md, SOUL.md, and USER.md verified. \
                 Your identity files are now active.",
            )]))
        } else {
            Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot complete bootstrap — missing files: {}. \
                 Create them first, then call bootstrap_done again.",
                missing.join(", ")
            ))]))
        }
    }

    #[tool(description = "Discover the OAuth authorization server for an HTTP MCP server and return its authorization endpoint URL. Use this to confirm the server supports OAuth. To complete authentication, use the Telegram bot command: /mcp auth <server_name>")]
    async fn mcp_auth(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<McpAuthParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let mcp_json_path = agent.dir.join("mcp.json");
        let servers = rightclaw::mcp::credentials::list_http_servers(&mcp_json_path)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let server_url = servers
            .iter()
            .find(|(name, _)| name == &params.server_name)
            .map(|(_, url)| url.clone())
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "Server '{}' not found in mcp.json. Add it first with mcp_add.",
                        params.server_name
                    ),
                    None,
                )
            })?;

        let http_client = reqwest::Client::new();
        let metadata = rightclaw::mcp::oauth::discover_as(&http_client, &server_url)
            .await
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Server '{}' supports OAuth. Authorization endpoint: {}\n\nTo authenticate, run in Telegram: /mcp auth {}",
            params.server_name, metadata.authorization_endpoint, params.server_name
        ))]))
    }
}

#[tool_handler]
impl rmcp::ServerHandler for HttpMemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "rightclaw",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "RightClaw agent MCP server.\n\n\
                 ## Memory\n\
                 - store_record: Store tagged records for persistent memory\n\
                 - query_records: Query records by tag or keyword\n\
                 - search_records: Full-text search with BM25 ranking\n\
                 - delete_record: Soft-delete a record (preserves audit trail)\n\n\
                 ## Cron\n\
                 - cron_create: Create a new cron job spec\n\
                 - cron_update: Update an existing cron job spec (full replacement)\n\
                 - cron_delete: Delete a cron job spec\n\
                 - cron_list: List all current cron job specs\n\
                 - cron_list_runs: List recent cron job executions\n\
                 - cron_show_run: Get details of a specific cron run\n\n\
                 ## MCP Management\n\
                 - mcp_add: Add an external HTTP MCP server\n\
                 - mcp_remove: Remove an MCP server (cannot remove 'right')\n\
                 - mcp_list: List all configured MCP servers\n\
                 - mcp_auth: Initiate OAuth for an HTTP MCP server\n\n\
                 ## Bootstrap\n\
                 - bootstrap_done: Signal onboarding completion. Verifies IDENTITY.md, SOUL.md, USER.md exist. Call AFTER creating all three files.",
            )
    }
}

// ---------------------------------------------------------------------------
// Bearer auth middleware
// ---------------------------------------------------------------------------

async fn bearer_auth_middleware(
    axum::extract::State(token_map): axum::extract::State<AgentTokenMap>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
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

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the MCP memory server over HTTP with per-agent Bearer authentication.
///
/// - One server process, multiple agents share it.
/// - Token -> agent mapping in `token_map`.
/// - DB connections opened lazily and cached per agent name.
/// - Stateless mode (no session persistence) with JSON responses (no SSE).
pub async fn run_memory_server_http(
    port: u16,
    token_map: AgentTokenMap,
    agents_dir: PathBuf,
    rightclaw_home: PathBuf,
) -> miette::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let ct = CancellationToken::new();
    let conn_cache: ConnCache = Arc::new(DashMap::new());

    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct.clone());

    let session_manager = Arc::new(LocalSessionManager::default());

    let conn_cache_factory = conn_cache.clone();
    let agents_dir_factory = agents_dir.clone();
    let rightclaw_home_factory = rightclaw_home.clone();

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(HttpMemoryServer::new(
                conn_cache_factory.clone(),
                agents_dir_factory.clone(),
                rightclaw_home_factory.clone(),
            ))
        },
        session_manager,
        config,
    );

    let app = axum::Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(axum::middleware::from_fn_with_state(
            token_map.clone(),
            bearer_auth_middleware,
        ));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| miette::miette!("bind to 0.0.0.0:{port} failed: {e:#}"))?;

    tracing::info!(port, agents = ?agents_dir, "right HTTP MCP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { ct.cancelled().await })
        .await
        .map_err(|e| miette::miette!("HTTP server error: {e:#}"))
}

#[cfg(test)]
#[path = "memory_server_http_tests.rs"]
mod tests;
