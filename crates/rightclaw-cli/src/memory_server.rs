use std::sync::{Arc, Mutex};

use rmcp::{
    ErrorData as McpError, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

// --- Parameter types ---

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
pub struct CronCreateParams {
    #[schemars(description = "Job name (lowercase alphanumeric and hyphens, e.g. 'health-check')")]
    pub job_name: String,
    #[schemars(
        description = "5-field cron expression in UTC (e.g. '17 9 * * 1-5'). Required if run_at is not set. Mutually exclusive with run_at."
    )]
    pub schedule: Option<String>,
    #[schemars(description = "Task prompt that Claude executes when the cron fires")]
    pub prompt: String,
    #[schemars(
        description = "Whether the job fires repeatedly (true, default) or once then auto-deletes (false). Ignored if run_at is set."
    )]
    pub recurring: Option<bool>,
    #[schemars(
        description = "ISO8601 UTC datetime to fire once (e.g. '2026-04-15T15:30:00Z'). Mutually exclusive with schedule. Job auto-deletes after firing."
    )]
    pub run_at: Option<String>,
    #[schemars(description = "Lock TTL duration (e.g. '30m', '1h'). Default: 30m")]
    pub lock_ttl: Option<String>,
    #[schemars(description = "Maximum dollar spend per invocation. Default: 2.0")]
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    pub max_budget_usd: Option<f64>,
    #[schemars(
        description = "Telegram chat id to deliver this cron's results to. Required. For DMs use the user_id; for groups use the negative chat id. Must be present in the agent's allowlist (allowlist.yaml). Read this from the `chat.id` field in the incoming message YAML unless the user explicitly asks for a different chat."
    )]
    pub target_chat_id: i64,
    #[schemars(
        description = "Optional supergroup topic (message_thread_id). Set only when the cron should reply to a specific topic; leave unset for ordinary chat delivery."
    )]
    pub target_thread_id: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronUpdateParams {
    #[schemars(description = "Job name to update")]
    pub job_name: String,
    #[schemars(description = "New 5-field cron expression. Clears run_at if set.")]
    pub schedule: Option<String>,
    #[schemars(
        description = "New ISO8601 UTC datetime. Clears schedule and forces recurring=false."
    )]
    pub run_at: Option<String>,
    #[schemars(description = "New task prompt")]
    pub prompt: Option<String>,
    #[schemars(description = "Set recurring (true) or one-shot (false)")]
    pub recurring: Option<bool>,
    #[schemars(description = "New lock TTL duration (e.g. '30m', '1h')")]
    pub lock_ttl: Option<String>,
    #[schemars(description = "New maximum dollar spend per invocation")]
    #[serde(default, deserialize_with = "deserialize_lenient_f64")]
    pub max_budget_usd: Option<f64>,
    #[schemars(description = "New target_chat_id. Must be in the agent's allowlist.")]
    pub target_chat_id: Option<i64>,
    #[schemars(
        description = "New target_thread_id. Pass `null` to clear (cron will deliver to the chat without a topic). Omit the field entirely to leave unchanged."
    )]
    #[serde(default, deserialize_with = "deserialize_double_option_i64")]
    pub target_thread_id: Option<Option<i64>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronDeleteParams {
    #[schemars(description = "Job name to delete")]
    pub job_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronTriggerParams {
    #[schemars(description = "Job name to trigger for immediate execution")]
    pub job_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpListParams {}

/// Deserialize an `Option<f64>` that also accepts string representations.
/// LLMs sometimes send numbers as strings (e.g. `"2.0"` instead of `2.0`).
fn deserialize_lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(f64),
        Str(String),
        Null,
    }

    match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => Ok(Some(n)),
        NumOrStr::Str(s) if s.is_empty() => Ok(None),
        NumOrStr::Str(s) => s
            .parse::<f64>()
            .map(Some)
            .map_err(|_| de::Error::custom(format!("invalid number: {s}"))),
        NumOrStr::Null => Ok(None),
    }
}

/// Distinguish between "field absent" (`None`) and "explicit null" (`Some(None)`)
/// for nullable optional integers. Required so `cron_update` can clear a field.
///
/// When the field is present in JSON:
///   - `null`    → `Some(None)`  (clear the column)
///   - `7`       → `Some(Some(7))` (set to 7)
///
/// When the field is absent from JSON, serde's `default` kicks in → `None`.
fn deserialize_double_option_i64<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer).map(Some)
}

// --- Server struct ---

#[derive(Clone)]
#[allow(dead_code)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    conn: Arc<Mutex<rusqlite::Connection>>,
    agent_name: String,
    agent_dir: std::path::PathBuf,
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

    #[tool(
        description = "List recent cron job runs with results. Returns runs sorted by started_at descending. Optionally filter by job_name and/or limit the count. Each result includes summary and notify (the structured output produced by the cron session)."
    )]
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
                "SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at, delivery_status, no_notify_reason
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
                    row.get::<_, Option<String>>(6)?.as_deref(),
                    row.get::<_, Option<String>>(7)?.as_deref(),
                    row.get::<_, Option<String>>(8)?.as_deref(),
                    row.get::<_, Option<String>>(9)?.as_deref(),
                    row.get::<_, Option<String>>(10)?.as_deref(),
                    row.get::<_, Option<String>>(11)?.as_deref(),
                ))
            })
            .map_err(|e| McpError::internal_error(format!("query failed: {e:#}"), None))?
            .filter_map(|r| r.ok())
            .collect();
        let output = serde_json::to_string_pretty(&rows)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Get full details for a single cron job run by its run_id (UUID). Returns status, summary, and notify (the structured output with content and optional attachments)."
    )]
    async fn cron_show_run(
        &self,
        Parameters(params): Parameters<CronShowRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = conn.query_row(
            "SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at, delivery_status, no_notify_reason
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
                    row.get::<_, Option<String>>(6)?.as_deref(),
                    row.get::<_, Option<String>>(7)?.as_deref(),
                    row.get::<_, Option<String>>(8)?.as_deref(),
                    row.get::<_, Option<String>>(9)?.as_deref(),
                    row.get::<_, Option<String>>(10)?.as_deref(),
                    row.get::<_, Option<String>>(11)?.as_deref(),
                ))
            },
        );
        match result {
            Ok(val) => {
                let output = serde_json::to_string_pretty(&val).map_err(|e| {
                    McpError::internal_error(format!("serialization error: {e:#}"), None)
                })?;
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

    #[tool(
        description = "Create a new cron job spec. Supports recurring schedules and one-shot jobs (via run_at or recurring=false)."
    )]
    async fn cron_create(
        &self,
        Parameters(params): Parameters<CronCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = rightclaw::cron_spec::create_spec_v2(
            &conn,
            &params.job_name,
            params.schedule.as_deref(),
            &params.prompt,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
            params.recurring,
            params.run_at.as_deref(),
            Some(params.target_chat_id),
            params.target_thread_id,
        )
        .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    #[tool(
        description = "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values."
    )]
    async fn cron_update(
        &self,
        Parameters(params): Parameters<CronUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let result = rightclaw::cron_spec::update_spec_partial(
            &conn,
            &params.job_name,
            params.schedule.as_deref(),
            params.run_at.as_deref(),
            params.prompt.as_deref(),
            params.recurring,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
            params.target_chat_id,
            params.target_thread_id,
        )
        .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    #[tool(description = "Delete a cron job spec. Also removes its lock file if present.")]
    async fn cron_delete(
        &self,
        Parameters(params): Parameters<CronDeleteParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let msg = rightclaw::cron_spec::delete_spec(&conn, &params.job_name, &self.agent_dir)
            .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(
        description = "List all current cron job specs. Returns a JSON array of all configured cron jobs."
    )]
    async fn cron_list(
        &self,
        Parameters(_params): Parameters<CronListParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let output = rightclaw::cron_spec::list_specs(&conn)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Trigger a cron job for immediate execution. The job is queued and will run on the next engine tick (≤60s). Lock check still applies — if the job is currently running, the trigger is skipped."
    )]
    async fn cron_trigger(
        &self,
        Parameters(params): Parameters<CronTriggerParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let msg = rightclaw::cron_spec::trigger_spec(&conn, &params.job_name)
            .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(
        description = "List all registered MCP servers for this agent. Shows name, URL, and optional instructions."
    )]
    async fn mcp_list(
        &self,
        Parameters(_params): Parameters<McpListParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let servers = rightclaw::mcp::credentials::db_list_servers(&conn)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let items: Vec<serde_json::Value> = servers
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "url": s.url,
                    "instructions": s.instructions,
                })
            })
            .collect();
        let output = serde_json::to_string_pretty(&items)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist."
    )]
    async fn bootstrap_done(&self) -> Result<CallToolResult, McpError> {
        let required = ["IDENTITY.md", "SOUL.md", "USER.md"];
        let missing: Vec<&str> = required
            .iter()
            .filter(|f| !self.agent_dir.join(f).exists())
            .copied()
            .collect();

        if missing.is_empty() {
            let bootstrap_path = self.agent_dir.join("BOOTSTRAP.md");
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
                "RightClaw agent MCP server. CC exposes these tools with `mcp__right__` prefix.\n\n\
                 ## Cron\n\
                 - mcp__right__cron_create: Create a new cron job spec\n\
                 - mcp__right__cron_update: Update an existing cron job spec (partial — only changed fields)\n\
                 - mcp__right__cron_delete: Delete a cron job spec\n\
                 - mcp__right__cron_list: List all current cron job specs\n\
                 - mcp__right__cron_list_runs: List recent cron job runs with results (summary + notify)\n\
                 - mcp__right__cron_show_run: Get full details of a specific cron run (summary + notify)\n\
                 - mcp__right__cron_trigger: Trigger a cron job for immediate execution\n\n\
                 ## MCP Management\n\
                 - mcp__right__mcp_list: List all registered MCP servers (read-only — add/remove/auth via Telegram /mcp)\n\n\
                 ## Bootstrap\n\
                 - mcp__right__bootstrap_done: Signal onboarding completion. Verifies IDENTITY.md, SOUL.md, USER.md exist. Call AFTER creating all three files.",
            )
    }
}

/// Convert a cron_runs row to JSON value.
pub(crate) fn cron_run_to_json(
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    log_path: Option<&str>,
    summary: Option<&str>,
    notify_json: Option<&str>,
    delivered_at: Option<&str>,
    delivery_status: Option<&str>,
    no_notify_reason: Option<&str>,
) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": id,
        "job_name": job_name,
        "started_at": started_at,
        "finished_at": finished_at,
        "exit_code": exit_code,
        "status": status,
        "log_path": log_path,
        "delivered_at": delivered_at,
        "delivery_status": delivery_status,
        "no_notify_reason": no_notify_reason,
    });
    if let Some(s) = summary {
        val["summary"] = serde_json::Value::String(s.to_owned());
    }
    // Parse notify_json into a structured object so the agent sees content directly.
    if let Some(nj) = notify_json
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(nj)
    {
        val["notify"] = parsed;
    }
    val
}

/// Run the MCP memory server over stdio.
///
/// - Tracing writes to stderr only (per D-03 — stdout is reserved for JSON-RPC).
/// - DB path: `$HOME/data.db` (agent dir is set as HOME by shell wrapper).
/// - `RC_AGENT_NAME` env var identifies the calling agent.
pub async fn run_memory_server() -> miette::Result<()> {
    // CRITICAL: tracing to stderr only — stdout is the JSON-RPC transport channel.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();

    // DB path: $HOME/data.db (HOME = agent dir under HOME override)
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let conn = rightclaw::memory::open_connection(&home, true)
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
            tracing::warn!(
                "RC_RIGHTCLAW_HOME not set — mcp_auth tunnel commands will be unavailable"
            );
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
