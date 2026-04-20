//! Standalone dispatch layer for RightClaw's built-in MCP tools.
//!
//! [`RightBackend`] extracts the tool logic from [`HttpMemoryServer`] into a
//! struct that accepts `(agent_name, agent_dir, tool_name, args)` and dispatches
//! manually — no rmcp macro-generated parameter parsing required.
//! The Aggregator uses this to expose rightclaw tools alongside proxied external
//! MCP servers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, bail};
use dashmap::DashMap;
use rmcp::handler::server::tool::schema_for_type;
use rmcp::model::{CallToolResult, Content, Tool};

use crate::memory_server::{
    CronCreateParams, CronDeleteParams, CronListParams, CronListRunsParams, CronShowRunParams,
    CronTriggerParams, CronUpdateParams, McpListParams, cron_run_to_json,
};

/// Connection cache keyed by agent name.
type ConnCache = Arc<DashMap<String, Arc<Mutex<rusqlite::Connection>>>>;

pub struct RightBackend {
    conn_cache: ConnCache,
    agents_dir: PathBuf,
    mtls_dir: Option<PathBuf>,
}

impl RightBackend {
    pub fn new(agents_dir: PathBuf, mtls_dir: Option<PathBuf>) -> Self {
        Self {
            conn_cache: Arc::new(DashMap::new()),
            agents_dir,
            mtls_dir,
        }
    }

    /// Return static tool definitions for all built-in tools.
    /// Cached after first call — schemas are computed once via OnceLock.
    pub fn tools_list(&self) -> Vec<Tool> {
        static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
        TOOLS.get_or_init(|| vec![
            // Cron tools
            Tool::new(
                "cron_create",
                "Create a new cron job spec. Supports recurring schedules and one-shot jobs (via run_at or recurring=false). The job will be picked up by the cron engine on its next reload cycle.",
                schema_for_type::<CronCreateParams>(),
            ),
            Tool::new(
                "cron_update",
                "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values. Setting schedule clears run_at; setting run_at clears schedule.",
                schema_for_type::<CronUpdateParams>(),
            ),
            Tool::new(
                "cron_delete",
                "Delete a cron job spec. Also removes its lock file if present.",
                schema_for_type::<CronDeleteParams>(),
            ),
            Tool::new(
                "cron_list",
                "List all current cron job specs. Returns a JSON array of all configured cron jobs.",
                schema_for_type::<CronListParams>(),
            ),
            Tool::new(
                "cron_list_runs",
                "List recent cron job runs with results. Returns runs sorted by started_at descending. Optionally filter by job_name and/or limit the count. Each result includes summary and notify (the structured output produced by the cron session).",
                schema_for_type::<CronListRunsParams>(),
            ),
            Tool::new(
                "cron_show_run",
                "Get full details for a single cron job run by its run_id (UUID). Returns status, summary, and notify (the structured output with content and optional attachments).",
                schema_for_type::<CronShowRunParams>(),
            ),
            Tool::new(
                "cron_trigger",
                "Trigger a cron job for immediate execution. The job is queued and will run on the next engine tick (≤60s). Lock check still applies — if the job is currently running, the trigger is skipped.",
                schema_for_type::<CronTriggerParams>(),
            ),
            // MCP management tools (read-only — write ops are user-only via Telegram /mcp)
            Tool::new(
                "mcp_list",
                "List all registered MCP servers for this agent. Shows name, URL, and optional instructions.",
                schema_for_type::<McpListParams>(),
            ),
            // Bootstrap
            Tool::new(
                "bootstrap_done",
                "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist.",
                schema_for_type::<CronListParams>(), // empty schema — no params
            ),
        ]).clone()
    }

    /// Dispatch a tool call by name.
    ///
    /// Returns `Ok(CallToolResult)` on success (including tool-level errors
    /// surfaced as `CallToolResult::error`). Returns `Err` only for
    /// infrastructure failures (DB open, mutex poisoned, unknown tool, etc.).
    pub async fn tools_call(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        match tool_name {
            "cron_create" => self.call_cron_create(agent_name, &args),
            "cron_update" => self.call_cron_update(agent_name, &args),
            "cron_delete" => self.call_cron_delete(agent_name, agent_dir, &args),
            "cron_list" => self.call_cron_list(agent_name),
            "cron_list_runs" => self.call_cron_list_runs(agent_name, &args),
            "cron_show_run" => self.call_cron_show_run(agent_name, &args),
            "cron_trigger" => self.call_cron_trigger(agent_name, &args),
            "mcp_list" => self.call_mcp_list(agent_name),
            "bootstrap_done" => self.call_bootstrap_done(agent_name).await,
            other => bail!("unknown tool: {other}"),
        }
    }

    // ------------------------------------------------------------------
    // Connection helpers
    // ------------------------------------------------------------------

    pub(crate) fn get_conn(
        &self,
        agent_name: &str,
    ) -> Result<Arc<Mutex<rusqlite::Connection>>, anyhow::Error> {
        if let Some(entry) = self.conn_cache.get(agent_name) {
            return Ok(Arc::clone(entry.value()));
        }
        let db_dir = self.agents_dir.join(agent_name);
        let conn = rightclaw::memory::open_connection(&db_dir, false)
            .with_context(|| format!("failed to open memory DB for {agent_name}"))?;
        let conn = Arc::new(Mutex::new(conn));
        self.conn_cache
            .insert(agent_name.to_owned(), Arc::clone(&conn));
        Ok(conn)
    }

    fn lock_conn(
        conn_arc: &Arc<Mutex<rusqlite::Connection>>,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, anyhow::Error> {
        conn_arc
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))
    }

    // ------------------------------------------------------------------
    // Cron tools
    // ------------------------------------------------------------------

    fn call_cron_create(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronCreateParams =
            serde_json::from_value(args.clone()).context("invalid cron_create params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let result = rightclaw::cron_spec::create_spec_v2(
            &conn,
            &params.job_name,
            params.schedule.as_deref(),
            &params.prompt,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
            params.recurring,
            params.run_at.as_deref(),
        )
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    fn call_cron_update(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronUpdateParams =
            serde_json::from_value(args.clone()).context("invalid cron_update params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let result = rightclaw::cron_spec::update_spec_partial(
            &conn,
            &params.job_name,
            params.schedule.as_deref(),
            params.run_at.as_deref(),
            params.prompt.as_deref(),
            params.recurring,
            params.lock_ttl.as_deref(),
            params.max_budget_usd,
        )
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            rightclaw::cron_spec::format_result(&result),
        )]))
    }

    fn call_cron_delete(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronDeleteParams =
            serde_json::from_value(args.clone()).context("invalid cron_delete params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let msg = rightclaw::cron_spec::delete_spec(&conn, &params.job_name, agent_dir)
            .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    fn call_cron_list(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let output = rightclaw::cron_spec::list_specs(&conn).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn call_cron_list_runs(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronListRunsParams =
            serde_json::from_value(args.clone()).context("invalid cron_list_runs params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let limit = params.limit.unwrap_or(20);
        let mut stmt = conn.prepare(
            "SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at, delivery_status, no_notify_reason
             FROM cron_runs
             WHERE (?1 IS NULL OR job_name = ?1)
             ORDER BY started_at DESC
             LIMIT ?2",
        )?;
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
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let output = serde_json::to_string_pretty(&rows)?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    fn call_cron_show_run(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronShowRunParams =
            serde_json::from_value(args.clone()).context("invalid cron_show_run params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
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
                let output = serde_json::to_string_pretty(&val)?;
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "cron run '{}' not found",
                    params.run_id
                ))]))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn call_cron_trigger(
        &self,
        agent_name: &str,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronTriggerParams =
            serde_json::from_value(args.clone()).context("invalid cron_trigger params")?;
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let msg = rightclaw::cron_spec::trigger_spec(&conn, &params.job_name)
            .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    // ------------------------------------------------------------------
    // MCP management tools
    // ------------------------------------------------------------------

    fn call_mcp_list(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let servers = rightclaw::mcp::credentials::db_list_servers(&conn)?;
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
        let output = serde_json::to_string_pretty(&items)?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    // ------------------------------------------------------------------
    // Bootstrap
    // ------------------------------------------------------------------

    async fn call_bootstrap_done(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let agent_dir = self.agents_dir.join(agent_name);
        let required = ["IDENTITY.md", "SOUL.md", "USER.md"];

        let missing: Vec<&str> = if let Some(mtls_dir) = &self.mtls_dir {
            let sandbox_name = match rightclaw::agent::parse_agent_config(&agent_dir) {
                Ok(Some(config)) => rightclaw::openshell::resolve_sandbox_name(agent_name, &config),
                _ => rightclaw::openshell::sandbox_name(agent_name),
            };
            let mut client = rightclaw::openshell::connect_grpc(mtls_dir)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .context("bootstrap_done: failed to connect to OpenShell gRPC")?;
            let sandbox_id = rightclaw::openshell::resolve_sandbox_id(&mut client, &sandbox_name)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .context("bootstrap_done: failed to resolve sandbox ID")?;

            let mut missing = Vec::new();
            for &file in &required {
                let path = format!("/sandbox/{file}");
                let (_, exit_code) = rightclaw::openshell::exec_in_sandbox(
                    &mut client,
                    &sandbox_id,
                    &["test", "-f", &path],
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .with_context(|| format!("bootstrap_done: exec test -f {path} failed"))?;
                if exit_code != 0 {
                    missing.push(file);
                }
            }
            missing
        } else {
            required
                .iter()
                .filter(|f| !agent_dir.join(f).exists())
                .copied()
                .collect()
        };

        if missing.is_empty() {
            let bootstrap_path = agent_dir.join("BOOTSTRAP.md");
            if bootstrap_path.exists() {
                std::fs::remove_file(&bootstrap_path).context("failed to remove BOOTSTRAP.md")?;
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

#[cfg(test)]
#[path = "right_backend_tests.rs"]
mod tests;
