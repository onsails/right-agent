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
                "RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run",
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::ServerHandler;
    use tempfile::tempdir;

    fn setup_server() -> (MemoryServer, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let conn = rightclaw::memory::open_connection(dir.path()).expect("open_connection");
        let server = MemoryServer::new(conn, "test-agent".to_string());
        (server, dir)
    }

    fn insert_cron_run(
        server: &MemoryServer,
        id: &str,
        job_name: &str,
        started_at: &str,
        status: &str,
    ) {
        let conn = server.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, status, log_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, job_name, started_at, status, format!("/tmp/{id}.log")],
        )
        .expect("insert cron_run");
    }

    fn call_result_text(result: CallToolResult) -> String {
        result
            .content
            .into_iter()
            .filter_map(|c| {
                if let rmcp::model::RawContent::Text(t) = c.raw {
                    Some(t.text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn test_get_info_server_name() {
        let (server, _dir) = setup_server();
        let info = server.get_info();
        assert_eq!(info.server_info.name, "rightclaw");
    }

    #[tokio::test]
    async fn test_cron_list_runs_empty() {
        let (server, _dir) = setup_server();
        let result = server
            .cron_list_runs(Parameters(CronListRunsParams {
                job_name: None,
                limit: None,
            }))
            .await
            .expect("cron_list_runs ok");
        let text = call_result_text(result);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed, serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_cron_list_runs_two_rows() {
        let (server, _dir) = setup_server();
        insert_cron_run(&server, "run-001", "deploy-check", "2026-04-01T10:00:00Z", "success");
        insert_cron_run(&server, "run-002", "health-ping", "2026-04-01T11:00:00Z", "success");

        let result = server
            .cron_list_runs(Parameters(CronListRunsParams {
                job_name: None,
                limit: None,
            }))
            .await
            .expect("cron_list_runs ok");
        let text = call_result_text(result);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed.len(), 2);
        // Ordered by started_at DESC — run-002 first
        assert_eq!(parsed[0]["id"], "run-002");
        assert_eq!(parsed[1]["id"], "run-001");
    }

    #[tokio::test]
    async fn test_cron_list_runs_filter_job_name() {
        let (server, _dir) = setup_server();
        insert_cron_run(&server, "run-a1", "job-a", "2026-04-01T10:00:00Z", "success");
        insert_cron_run(&server, "run-b1", "job-b", "2026-04-01T10:01:00Z", "success");

        let result = server
            .cron_list_runs(Parameters(CronListRunsParams {
                job_name: Some("job-a".to_string()),
                limit: None,
            }))
            .await
            .expect("cron_list_runs ok");
        let text = call_result_text(result);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["job_name"], "job-a");
        assert_eq!(parsed[0]["id"], "run-a1");
    }

    #[tokio::test]
    async fn test_cron_list_runs_limit() {
        let (server, _dir) = setup_server();
        for i in 0..5 {
            insert_cron_run(
                &server,
                &format!("run-{i:03}"),
                "batch-job",
                &format!("2026-04-01T{i:02}:00:00Z"),
                "success",
            );
        }
        let result = server
            .cron_list_runs(Parameters(CronListRunsParams {
                job_name: None,
                limit: Some(2),
            }))
            .await
            .expect("cron_list_runs ok");
        let text = call_result_text(result);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed.len(), 2);
    }

    #[tokio::test]
    async fn test_cron_show_run_found() {
        let (server, _dir) = setup_server();
        insert_cron_run(&server, "run-xyz", "nightly-report", "2026-04-01T02:00:00Z", "success");

        let result = server
            .cron_show_run(Parameters(CronShowRunParams {
                run_id: "run-xyz".to_string(),
            }))
            .await
            .expect("cron_show_run ok");
        let text = call_result_text(result);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(parsed["id"], "run-xyz");
        assert_eq!(parsed["job_name"], "nightly-report");
        assert!(parsed["log_path"].as_str().unwrap().contains("run-xyz"));
    }

    #[tokio::test]
    async fn test_cron_show_run_not_found() {
        let (server, _dir) = setup_server();

        let result = server
            .cron_show_run(Parameters(CronShowRunParams {
                run_id: "nonexistent-id".to_string(),
            }))
            .await
            .expect("cron_show_run returns Ok (not error) for missing");
        let text = call_result_text(result);
        assert!(
            text.contains("not found"),
            "Expected 'not found' in output, got: {text}"
        );
    }
}
