use std::sync::{Arc, Mutex};

use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;

// --- Parameter types ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StoreParams {
    #[schemars(description = "Content to store as a memory")]
    pub content: String,
    #[schemars(description = "Comma-separated tags for categorization")]
    pub tags: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallParams {
    #[schemars(description = "Tag or keyword to search by")]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Full-text search query")]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForgetParams {
    #[schemars(description = "Memory ID to soft-delete")]
    pub id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronListRunsParams {
    #[schemars(description = "Filter by job name. Omit to return all jobs.")]
    pub job_name: Option<String>,
    #[schemars(description = "Maximum number of runs to return. Default: 20.")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronShowRunParams {
    #[schemars(description = "Run ID (UUID) to retrieve.")]
    pub run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAddParams {
    #[schemars(description = "MCP server identifier (e.g. 'notion', 'linear')")]
    pub name: String,
    #[schemars(description = "HTTP MCP server URL (must start with https://)")]
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpRemoveParams {
    #[schemars(description = "MCP server name to remove from .claude.json")]
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAuthParams {
    #[schemars(description = "MCP server name to initiate OAuth for (must exist in .claude.json)")]
    pub server_name: String,
}

// --- Server struct ---

#[derive(Clone)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    conn: Arc<Mutex<rusqlite::Connection>>,
    agent_name: String,
    agent_dir: std::path::PathBuf,
    #[allow(dead_code)] // reserved for mcp_auth tunnel URL resolution
    rightclaw_home: std::path::PathBuf,
}

#[tool_router]
impl MemoryServer {
    pub fn new(
        conn: rusqlite::Connection,
        agent_name: String,
        agent_dir: std::path::PathBuf,
        rightclaw_home: std::path::PathBuf,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            conn: Arc::new(Mutex::new(conn)),
            agent_name,
            agent_dir,
            rightclaw_home,
        }
    }

    #[tool(description = "Store a memory. Content is scanned for prompt injection. Returns memory ID.")]
    async fn store(
        &self,
        Parameters(params): Parameters<StoreParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::store_memory(
            &conn,
            &params.content,
            params.tags.as_deref(),
            Some(self.agent_name.as_str()),
            Some("mcp:store"),
        ) {
            Ok(id) => Ok(CallToolResult::success(vec![Content::text(format!(
                "stored memory id={id}"
            ))])),
            Err(rightclaw::memory::MemoryError::InjectionDetected) => Err(McpError::invalid_params(
                "content rejected: possible prompt injection detected",
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "Look up memories by tag or keyword. Returns matching active memories.")]
    async fn recall(
        &self,
        Parameters(params): Parameters<RecallParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::recall_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Full-text search memories using FTS5. Returns BM25-ranked results.")]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::search_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Soft-delete a memory by ID. Entry is excluded from recall/search but preserved in audit log.")]
    async fn forget(
        &self,
        Parameters(params): Parameters<ForgetParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.id;
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::forget_memory(
            &conn,
            id,
            Some(self.agent_name.as_str()),
        ) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "forgot memory id={id}"
            ))])),
            Err(rightclaw::memory::MemoryError::NotFound(_)) => Err(McpError::invalid_params(
                format!("memory id={id} not found or already deleted"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "List recent cron job runs. Returns runs sorted by started_at descending. Optionally filter by job_name and/or limit the count. Each result includes log_path — use bash to read the log file directly.")]
    async fn cron_list_runs(
        &self,
        Parameters(params): Parameters<CronListRunsParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
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
        Parameters(params): Parameters<CronShowRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
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

    #[tool(description = "Add an HTTP MCP server to this agent's .mcp.json. The server becomes available after the next agent restart.")]
    async fn mcp_add(
        &self,
        Parameters(params): Parameters<McpAddParams>,
    ) -> Result<CallToolResult, McpError> {
        if !params.url.starts_with("https://") {
            return Err(McpError::invalid_params(
                format!("URL must start with 'https://' — got: {}", params.url),
                None,
            ));
        }
        let mcp_json_path = self.agent_dir.join(".mcp.json");
        rightclaw::mcp::credentials::add_http_server(
            &mcp_json_path,
            &params.name,
            &params.url,
        )
        .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Added MCP server '{}' ({}). Restart agent for it to take effect.",
            params.name, params.url
        ))]))
    }

    #[tool(description = "Remove an HTTP MCP server from this agent's .mcp.json. The 'rightmemory' server is protected and cannot be removed.")]
    async fn mcp_remove(
        &self,
        Parameters(params): Parameters<McpRemoveParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.name == rightclaw::mcp::PROTECTED_MCP_SERVER {
            return Err(McpError::invalid_params(
                format!(
                    "Cannot remove '{}' — required for core agent functionality",
                    params.name
                ),
                None,
            ));
        }
        let mcp_json_path = self.agent_dir.join(".mcp.json");
        match rightclaw::mcp::credentials::remove_http_server(
            &mcp_json_path,
            &params.name,
        ) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Removed MCP server '{}'. Restart agent for change to take effect.",
                params.name
            ))])),
            Err(rightclaw::mcp::credentials::CredentialError::ServerNotFound(_)) => {
                Err(McpError::invalid_params(
                    format!(
                        "Server '{}' not found in .mcp.json.",
                        params.name
                    ),
                    None,
                ))
            }
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }

    #[tool(description = "List all configured MCP servers for this agent. Shows name, URL, auth state (present/auth required), source (.claude.json or .mcp.json), and kind (http/stdio). Never exposes token values.")]
    async fn mcp_list(
        &self,
        Parameters(_params): Parameters<McpListParams>,
    ) -> Result<CallToolResult, McpError> {
        let statuses = rightclaw::mcp::detect::mcp_auth_status(&self.agent_dir)
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

    #[tool(description = "Discover the OAuth authorization server for an HTTP MCP server and return its authorization endpoint URL. Use this to confirm the server supports OAuth. To complete authentication, use the Telegram bot command: /mcp auth <server_name>")]
    async fn mcp_auth(
        &self,
        Parameters(params): Parameters<McpAuthParams>,
    ) -> Result<CallToolResult, McpError> {
        let mcp_json_path = self.agent_dir.join(".mcp.json");
        let servers = rightclaw::mcp::credentials::list_http_servers(
            &mcp_json_path,
        )
        .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let server_url = servers
            .iter()
            .find(|(name, _)| name == &params.server_name)
            .map(|(_, url)| url.clone())
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "Server '{}' not found in .mcp.json. Add it first with mcp_add.",
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
impl rmcp::ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "rightclaw",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run, mcp_add, mcp_remove, mcp_list, mcp_auth",
            )
    }
}

/// Convert a MemoryEntry to a serde_json::Value for JSON output.
fn entry_to_json(entry: &rightclaw::memory::store::MemoryEntry) -> serde_json::Value {
    serde_json::json!({
        "id": entry.id,
        "content": entry.content,
        "tags": entry.tags,
        "stored_by": entry.stored_by,
        "created_at": entry.created_at,
    })
}

/// Convert a cron_runs row to JSON value.
fn cron_run_to_json(
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    log_path: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "job_name": job_name,
        "started_at": started_at,
        "finished_at": finished_at,
        "exit_code": exit_code,
        "status": status,
        "log_path": log_path,
    })
}

/// Run the MCP memory server over stdio.
///
/// - Tracing writes to stderr only (per D-03 — stdout is reserved for JSON-RPC).
/// - DB path: `$HOME/memory.db` (agent dir is set as HOME by shell wrapper).
/// - `RC_AGENT_NAME` env var identifies the calling agent.
pub async fn run_memory_server() -> miette::Result<()> {
    // CRITICAL: tracing to stderr only — stdout is the JSON-RPC transport channel.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();

    // DB path: $HOME/memory.db (HOME = agent dir under HOME override)
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let conn = rightclaw::memory::open_connection(&home)
        .map_err(|e| miette::miette!("failed to open memory database: {e:#}"))?;

    let agent_name = match std::env::var("RC_AGENT_NAME") {
        Ok(name) if !name.is_empty() => name,
        _ => {
            tracing::warn!("RC_AGENT_NAME not set — memories will record stored_by as 'unknown'");
            "unknown".to_string()
        }
    };

    let agent_dir = home.clone();

    let rightclaw_home = match std::env::var("RC_RIGHTCLAW_HOME") {
        Ok(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => {
            tracing::warn!("RC_RIGHTCLAW_HOME not set — mcp_auth tunnel commands will be unavailable");
            std::path::PathBuf::from(".")
        }
    };

    let server = MemoryServer::new(conn, agent_name, agent_dir, rightclaw_home);
    let service = server
        .serve(stdio())
        .await
        .map_err(|e| miette::miette!("MCP server error: {e:#}"))?;
    service
        .waiting()
        .await
        .map_err(|e| miette::miette!("MCP server wait error: {e:#}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "memory_server_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "memory_server_mcp_tests.rs"]
mod mcp_tests;
