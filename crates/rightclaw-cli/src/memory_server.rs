use std::sync::{Arc, Mutex};

use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
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

// --- Server struct ---

#[derive(Clone)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    conn: Arc<Mutex<rusqlite::Connection>>,
    agent_name: String,
}

#[tool_router]
impl MemoryServer {
    pub fn new(conn: rusqlite::Connection, agent_name: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            conn: Arc::new(Mutex::new(conn)),
            agent_name,
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
}

#[tool_handler]
impl rmcp::ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("RightClaw memory tools: store, recall, search, forget")
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

    let server = MemoryServer::new(conn, agent_name);
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
