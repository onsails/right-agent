//! Standalone dispatch layer for Right Agent's built-in MCP tools.
//!
//! [`RightBackend`] extracts the tool logic from [`HttpMemoryServer`] into a
//! struct that accepts `(agent_name, agent_dir, tool_name, args, context)` and
//! dispatches manually — no rmcp macro-generated parameter parsing required.
//! The Aggregator uses this to expose right-agent tools alongside proxied external
//! MCP servers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, bail};
use dashmap::DashMap;
use right_mcp::internal_client::{
    InternalClient, ProgressSendRequest, SKILL_LEARNING_FINISH_TOOL, SKILL_LEARNING_START_TOOL,
};
use right_mcp::tool_error::tool_error;
use rmcp::handler::server::tool::schema_for_type;
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

/// End-to-end timeout for `mcp__right__send_progress`. Bounds the wait on the
/// bot UDS round-trip (which in turn awaits Telegram). Keeps the
/// per-invocation rate-limit slot from being held indefinitely if Telegram
/// stalls.
const PROGRESS_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const CONVERSATION_SEARCH_DEFAULT_LIMIT: usize = 10;

use crate::learning::{
    LearningMessagePhase, SkillLearningFinishParams, SkillLearningStartParams,
    SkillPackageExpectation,
};
use crate::memory_server::{
    CronCreateParams, CronDeleteParams, CronListParams, CronListRunsParams, CronShowRunParams,
    CronTriggerParams, CronUpdateParams, McpListParams, cron_run_to_json,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationSearchParams {
    pub(crate) query: String,
    pub(crate) limit: Option<usize>,
}

/// Connection cache keyed by agent name.
type ConnCache = Arc<DashMap<String, Arc<Mutex<rusqlite::Connection>>>>;

pub struct RightBackend {
    conn_cache: ConnCache,
    agents_dir: PathBuf,
    mtls_dir: Option<PathBuf>,
    progress: crate::progress::ProgressRegistry,
}

impl RightBackend {
    pub fn new(agents_dir: PathBuf, mtls_dir: Option<PathBuf>) -> Self {
        Self {
            conn_cache: Arc::new(DashMap::new()),
            agents_dir,
            mtls_dir,
            progress: crate::progress::ProgressRegistry::default(),
        }
    }

    pub(crate) fn progress_registry(&self) -> crate::progress::ProgressRegistry {
        self.progress.clone()
    }

    /// Return static tool definitions for all built-in tools.
    /// Cached after first call — schemas are computed once via OnceLock.
    pub fn tools_list(&self) -> Vec<Tool> {
        static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
        TOOLS.get_or_init(|| vec![
            // Cron tools
            Tool::new(
                "cron_create",
                "Create a new cron job spec. Supports recurring schedules and one-shot jobs (via run_at or recurring=false). The job will be picked up by the cron engine on its next reload cycle. Errors: chat_id_not_in_allowlist (the target chat must first be approved via /allow or /allow_all).",
                schema_for_type::<CronCreateParams>(),
            ),
            Tool::new(
                "cron_update",
                "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values. Setting schedule clears run_at; setting run_at clears schedule. Errors: chat_id_not_in_allowlist (when updating target_chat_id to a chat not in the allowlist).",
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
                "List recent cron job runs with results. Returns runs sorted by started_at descending. Optionally filter by job_name and/or limit the count. Each result includes run_note and delivery (the structured output produced by the cron session).",
                schema_for_type::<CronListRunsParams>(),
            ),
            Tool::new(
                "cron_show_run",
                "Get full details for a single cron job run by its run_id (UUID). Returns status, run_note, and delivery (the structured output with notify or silent decision).",
                schema_for_type::<CronShowRunParams>(),
            ),
            Tool::new(
                "cron_trigger",
                right_agent::cron_spec::TRIGGER_TOOL_DESC,
                schema_for_type::<CronTriggerParams>(),
            ),
            // MCP management tools (read-only — write ops are user-only via Telegram /mcp)
            Tool::new(
                "mcp_list",
                "List all registered MCP servers for this agent. Shows name, URL, and optional instructions.",
                schema_for_type::<McpListParams>(),
            ),
            Tool::new(
                crate::progress::SEND_PROGRESS_TOOL,
                "Send an occasional standalone progress message to the current Telegram chat for the current foreground invocation only. Use for complex or long-running work, not routine short tasks. Rate limited to one message per 30 seconds per invocation. Max 2000 characters.",
                schema_for_type::<crate::progress::SendProgressParams>(),
            ),
            Tool::new(
                SKILL_LEARNING_START_TOOL,
                "Stage 1 foreground metadata/progress for learned skill create/update. Call before writing or patching skill package files. action=create and action=update both require rightx-* skill names. Accepts skill names only, never paths.",
                schema_for_type::<SkillLearningStartParams>(),
            ),
            Tool::new(
                SKILL_LEARNING_FINISH_TOOL,
                "Stage 1 foreground metadata/receipt for skill create/update completion. Successful statuses require a non-empty LLM-authored message argument, verify the skill package exists at .claude/skills/<skill_name>/SKILL.md, and send learned/updated receipts. Does not move files.",
                schema_for_type::<SkillLearningFinishParams>(),
            ),
            // Conversation search tools
            Tool::new(
                "thread_search",
                "Search archived Telegram conversation messages in the current chat and current thread only. Scope is server-enforced from the current foreground invocation and is not agent-controlled.",
                schema_for_type::<ConversationSearchParams>(),
            ),
            Tool::new(
                "chat_search",
                "Search archived Telegram conversation messages in the current chat across all threads. Scope is server-enforced from the current foreground invocation and is not agent-controlled.",
                schema_for_type::<ConversationSearchParams>(),
            ),
            // Bootstrap
            Tool::new(
                "bootstrap_done",
                "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist. Errors: bootstrap_files_missing (one or more identity files not yet created — see details.missing).",
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
        context: crate::progress::ToolCallContext,
    ) -> Result<CallToolResult, anyhow::Error> {
        match tool_name {
            "cron_create" => self.call_cron_create(agent_name, agent_dir, &args),
            "cron_update" => self.call_cron_update(agent_name, agent_dir, &args),
            "cron_delete" => self.call_cron_delete(agent_name, agent_dir, &args),
            "cron_list" => self.call_cron_list(agent_name),
            "cron_list_runs" => self.call_cron_list_runs(agent_name, &args),
            "cron_show_run" => self.call_cron_show_run(agent_name, &args),
            "cron_trigger" => self.call_cron_trigger(agent_name, &args),
            "mcp_list" => self.call_mcp_list(agent_name),
            crate::progress::SEND_PROGRESS_TOOL => self.call_send_progress(context, &args).await,
            SKILL_LEARNING_START_TOOL => {
                self.call_skill_learning_start(agent_name, agent_dir, context, &args)
                    .await
            }
            SKILL_LEARNING_FINISH_TOOL => {
                self.call_skill_learning_finish(agent_name, agent_dir, context, &args)
                    .await
            }
            "thread_search" => {
                self.call_conversation_search(
                    agent_name,
                    context,
                    &args,
                    ConversationSearchMode::Thread,
                )
                .await
            }
            "chat_search" => {
                self.call_conversation_search(
                    agent_name,
                    context,
                    &args,
                    ConversationSearchMode::Chat,
                )
                .await
            }
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
        let conn = right_db::open_connection(&db_dir, false)
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

    async fn lifecycle_created_by(
        &self,
        invocation_id: &str,
    ) -> Result<right_lifecycle::CreatedBy, CallToolResult> {
        let (kind, _) = match self.progress.learning_send_target(invocation_id).await {
            Ok(target) => target,
            Err(crate::progress::ProgressError::Unavailable) => {
                return Err(tool_error(
                    "learning_unavailable",
                    "learning messages are available only for a registered invocation",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::Forbidden) => {
                return Err(tool_error(
                    "learning_unavailable",
                    "learning messages are unavailable for this invocation kind",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::RateLimited { .. }) => {
                return Err(tool_error(
                    "learning_send_failed",
                    "internal error: learning target was rate limited",
                    None,
                ));
            }
        };

        Ok(match kind {
            crate::progress::ProgressInvocationKind::Foreground => {
                right_lifecycle::CreatedBy::Foreground
            }
            crate::progress::ProgressInvocationKind::BackgroundReview => {
                right_lifecycle::CreatedBy::Curator
            }
            #[cfg(test)]
            crate::progress::ProgressInvocationKind::NonForeground => {
                right_lifecycle::CreatedBy::Foreground
            }
        })
    }

    // ------------------------------------------------------------------
    // Cron tools
    // ------------------------------------------------------------------

    fn call_cron_create(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronCreateParams =
            serde_json::from_value(args.clone()).context("invalid cron_create params")?;
        if let Err(msg) = validate_target_against_allowlist(agent_dir, params.target_chat_id) {
            return Ok(tool_error("chat_id_not_in_allowlist", msg, None));
        }
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let result = right_agent::cron_spec::create_spec_v2(
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
            false,
        )
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            right_agent::cron_spec::format_result(&result),
        )]))
    }

    fn call_cron_update(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: CronUpdateParams =
            serde_json::from_value(args.clone()).context("invalid cron_update params")?;
        if let Some(chat) = params.target_chat_id
            && let Err(msg) = validate_target_against_allowlist(agent_dir, chat)
        {
            return Ok(tool_error("chat_id_not_in_allowlist", msg, None));
        }
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let result = right_agent::cron_spec::update_spec_partial(
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
        .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(
            right_agent::cron_spec::format_result(&result),
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
        let msg = right_agent::cron_spec::delete_spec(&conn, &params.job_name, agent_dir)
            .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    fn call_cron_list(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let output =
            right_agent::cron_spec::list_specs(&conn).map_err(|e| anyhow::anyhow!("{e}"))?;
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
            "SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path,
                    run_note, delivery_json, delivered_at, delivery_status
             FROM async_runs
             WHERE kind = 'cron'
               AND (?1 IS NULL OR producer_ref = ?1)
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
            "SELECT id, producer_ref, started_at, finished_at, exit_code, status, log_path,
                    run_note, delivery_json, delivered_at, delivery_status
             FROM async_runs
             WHERE kind = 'cron' AND id = ?1",
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
        let msg = right_agent::cron_spec::trigger_spec(&conn, &params.job_name)
            .map_err(|e| anyhow::anyhow!("invalid params: {e}"))?;
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    // ------------------------------------------------------------------
    // MCP management tools
    // ------------------------------------------------------------------

    fn call_mcp_list(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let servers = right_mcp::credentials::db_list_servers(&conn)?;
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

    async fn call_send_progress(
        &self,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: crate::progress::SendProgressParams = match serde_json::from_value(args.clone())
        {
            Ok(params) => params,
            Err(e) => {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("invalid send_progress params: {e:#}"),
                    None,
                ));
            }
        };
        let message = params.message.trim();
        if message.is_empty() {
            return Ok(tool_error(
                "invalid_argument",
                "progress message must not be empty",
                None,
            ));
        }
        if message.chars().count() > crate::progress::PROGRESS_MESSAGE_MAX_CHARS {
            return Ok(tool_error(
                "invalid_argument",
                format!(
                    "progress message must be at most {} characters",
                    crate::progress::PROGRESS_MESSAGE_MAX_CHARS
                ),
                None,
            ));
        }

        let Some(invocation_id) = context.invocation_id else {
            return Ok(tool_error(
                "progress_unavailable",
                "progress is available only for the current foreground invocation",
                None,
            ));
        };

        let target = match self.progress.begin_send(&invocation_id).await {
            Ok(target) => target,
            Err(crate::progress::ProgressError::Unavailable) => {
                return Ok(tool_error(
                    "progress_unavailable",
                    "progress is unavailable for this invocation",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::Forbidden) => {
                return Ok(tool_error(
                    "progress_forbidden",
                    "progress is forbidden for this invocation",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::RateLimited { retry_after }) => {
                return Ok(tool_error(
                    "progress_rate_limited",
                    "progress messages are rate limited",
                    Some(serde_json::json!({
                        "retry_after_secs": retry_after.as_secs(),
                    })),
                ));
            }
        };

        let client = InternalClient::new(target.bot_socket_path);
        let request = ProgressSendRequest {
            invocation_id: invocation_id.clone(),
            token: target.bot_send_token,
            message: message.to_owned(),
        };
        let send_fut = client.progress_send(&request);
        match tokio::time::timeout(PROGRESS_SEND_TIMEOUT, send_fut).await {
            Ok(Ok(_)) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({ "status": "sent" }).to_string(),
            )])),
            Ok(Err(e)) => {
                self.progress.mark_send_failed(&invocation_id).await;
                Ok(tool_error("progress_send_failed", format!("{e:#}"), None))
            }
            Err(_) => {
                self.progress.mark_send_failed(&invocation_id).await;
                Ok(tool_error(
                    "progress_send_failed",
                    format!(
                        "progress send timed out after {}s",
                        PROGRESS_SEND_TIMEOUT.as_secs()
                    ),
                    None,
                ))
            }
        }
    }

    async fn call_skill_learning_start(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: SkillLearningStartParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(e) => {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("invalid skill_learning_start params: {e:#}"),
                    None,
                ));
            }
        };
        if let Err(result) =
            crate::learning::validate_learning_target(agent_dir, params.action, &params.skill_name)
        {
            return Ok(result);
        }
        let expectation = match params.action {
            crate::learning::LearningActionParam::Create => SkillPackageExpectation::MustNotExist,
            crate::learning::LearningActionParam::Update => SkillPackageExpectation::MustExist,
        };
        if let Err(result) = crate::learning::validate_skill_package_state(
            agent_name,
            self.mtls_dir.as_deref(),
            agent_dir,
            &params.skill_name,
            expectation,
        )
        .await
        {
            return Ok(result);
        }
        let Some(invocation_id) = context.invocation_id else {
            return Ok(tool_error(
                "learning_unavailable",
                "learning is available only for a registered invocation",
                None,
            ));
        };
        let (kind, _) = match self.progress.learning_send_target(&invocation_id).await {
            Ok(target) => target,
            Err(crate::progress::ProgressError::Unavailable) => {
                return Ok(tool_error(
                    "learning_unavailable",
                    "learning is available only for a registered invocation",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::Forbidden) => {
                return Ok(tool_error(
                    "learning_unavailable",
                    "learning is unavailable for this invocation kind",
                    None,
                ));
            }
            Err(crate::progress::ProgressError::RateLimited { .. }) => {
                return Ok(tool_error(
                    "learning_send_failed",
                    "internal error: learning target was rate limited",
                    None,
                ));
            }
        };
        if let Err(result) =
            crate::learning::validate_start_message(kind, params.message.as_deref())
        {
            return Ok(result);
        }

        {
            let conn_arc = self.get_conn(agent_name)?;
            let conn = Self::lock_conn(&conn_arc)?;
            right_agent::learned_skills::insert_learning_event(
                &conn,
                &right_agent::learned_skills::LearningEvent {
                    invocation_id: invocation_id.clone(),
                    agent_name: agent_name.to_owned(),
                    action: params.action.as_domain(),
                    skill_name: params.skill_name.clone(),
                    phase: right_agent::learned_skills::LearningPhase::Start,
                    status: None,
                    hint_outcome: None,
                    reason: params.reason.clone(),
                    message: params.message.clone(),
                    summary: None,
                    event_refs: params.event_refs.clone().unwrap_or_default(),
                },
            )?;
        }

        if let Err(result) = crate::learning::send_learning_message(
            &self.progress,
            &invocation_id,
            LearningMessagePhase::Start,
            params.message.as_deref(),
        )
        .await
        {
            return Ok(result);
        }

        Ok(CallToolResult::success(vec![Content::text(
            crate::learning::success_json("started", &params.skill_name),
        )]))
    }

    async fn call_skill_learning_finish(
        &self,
        agent_name: &str,
        agent_dir: &Path,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: SkillLearningFinishParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(e) => {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("invalid skill_learning_finish params: {e:#}"),
                    None,
                ));
            }
        };
        if let Some(ho) = params.hint_outcome {
            tracing::info!(
                agent = %agent_name,
                skill = %params.skill_name,
                hint_outcome = %ho.as_str(),
                "probe-writer hint outcome"
            );
        }
        if let Err(result) =
            crate::learning::validate_learning_target(agent_dir, params.action, &params.skill_name)
        {
            return Ok(result);
        }
        if let Err(result) = crate::learning::validate_finish_receipt_message(
            params.status,
            params.message.as_deref(),
        ) {
            return Ok(result);
        }
        let Some(invocation_id) = context.invocation_id else {
            return Ok(tool_error(
                "learning_unavailable",
                "learning is available only for a registered invocation",
                None,
            ));
        };

        if params.status.is_success()
            && let Err(result) = crate::learning::validate_skill_package_state(
                agent_name,
                self.mtls_dir.as_deref(),
                agent_dir,
                &params.skill_name,
                SkillPackageExpectation::MustExist,
            )
            .await
        {
            return Ok(result);
        }

        {
            let conn_arc = self.get_conn(agent_name)?;
            let conn = Self::lock_conn(&conn_arc)?;
            right_agent::learned_skills::insert_learning_event(
                &conn,
                &right_agent::learned_skills::LearningEvent {
                    invocation_id: invocation_id.clone(),
                    agent_name: agent_name.to_owned(),
                    action: params.action.as_domain(),
                    skill_name: params.skill_name.clone(),
                    phase: right_agent::learned_skills::LearningPhase::Finish,
                    status: Some(params.status.as_domain()),
                    hint_outcome: params
                        .hint_outcome
                        .map(|hint_outcome| hint_outcome.as_str().to_owned()),
                    reason: None,
                    message: params.message.clone(),
                    summary: params.summary.clone(),
                    event_refs: params.event_refs.clone().unwrap_or_default(),
                },
            )?;
        }

        let lifecycle_created_by = if params.status.is_success() {
            match self.lifecycle_created_by(&invocation_id).await {
                Ok(created_by) => Some(created_by),
                Err(result) => return Ok(result),
            }
        } else {
            None
        };

        if params.status.is_success()
            && let Err(result) = crate::learning::send_learning_message(
                &self.progress,
                &invocation_id,
                LearningMessagePhase::FinishSuccess,
                params.message.as_deref(),
            )
            .await
        {
            return Ok(result);
        }

        if let Some(created_by) = lifecycle_created_by {
            let now_utc = chrono::Utc::now();
            let conn_arc = self.get_conn(agent_name)?;
            let conn = Self::lock_conn(&conn_arc)?;
            let outcome = match params.status.as_str() {
                "created" => {
                    right_lifecycle::mark_created(&conn, &params.skill_name, created_by, now_utc)
                }
                "updated" => {
                    right_lifecycle::bump_patch(&conn, &params.skill_name, created_by, now_utc)
                }
                _ => Ok(()),
            };
            if let Err(e) = outcome {
                tracing::error!(
                    agent = %agent_name,
                    skill = %params.skill_name,
                    "skill lifecycle write failed: {e:#}"
                );
                return Ok(tool_error(
                    "skill_lifecycle_write_failed",
                    format!("{e:#}"),
                    None,
                ));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            crate::learning::success_json(params.status.as_str(), &params.skill_name),
        )]))
    }

    // ------------------------------------------------------------------
    // Conversation search tools
    // ------------------------------------------------------------------

    async fn call_conversation_search(
        &self,
        agent_name: &str,
        context: crate::progress::ToolCallContext,
        args: &serde_json::Value,
        mode: ConversationSearchMode,
    ) -> Result<CallToolResult, anyhow::Error> {
        let params: ConversationSearchParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(e) => {
                return Ok(tool_error(
                    "invalid_argument",
                    format!("invalid conversation search params: {e:#}"),
                    None,
                ));
            }
        };
        let query = params.query.trim();
        if query.is_empty() {
            return Ok(tool_error(
                "invalid_argument",
                "conversation search query must not be empty",
                None,
            ));
        }
        if !query.chars().any(|ch| ch.is_alphanumeric() || ch == '_') {
            return Ok(tool_error(
                "invalid_argument",
                "conversation search query must contain a searchable term",
                None,
            ));
        }

        let Some(invocation_id) = context.invocation_id else {
            return Ok(conversation_scope_unavailable());
        };
        let scope = match self.progress.conversation_scope(&invocation_id).await {
            Ok(scope) => scope,
            Err(_) => return Ok(conversation_scope_unavailable()),
        };

        let conn_arc = self.get_conn(agent_name)?;
        let conn = Self::lock_conn(&conn_arc)?;
        let limit = params.limit.unwrap_or(CONVERSATION_SEARCH_DEFAULT_LIMIT);
        let results = match mode {
            ConversationSearchMode::Thread => right_db::conversation::search_thread(
                &conn,
                query,
                limit,
                scope.chat_id,
                scope.thread_id,
            ),
            ConversationSearchMode::Chat => {
                right_db::conversation::search_chat(&conn, query, limit, scope.chat_id)
            }
        }
        .map_err(|e| anyhow::anyhow!("conversation search failed: {e}"))?;

        let rows: Vec<serde_json::Value> = results
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "snippet": row.snippet,
                    "role": row.role,
                    "sender_user_id": row.sender_user_id,
                    "sender_name": row.sender_name,
                    "created_at": row.created_at,
                    "thread_id": row.thread_id,
                    "message_id": row.message_id,
                    "root_session_id": row.root_session_id,
                })
            })
            .collect();
        let scope_json = match mode {
            ConversationSearchMode::Thread => {
                serde_json::json!({ "type": "thread", "thread_id": scope.thread_id })
            }
            ConversationSearchMode::Chat => serde_json::json!({ "type": "chat" }),
        };
        let output = serde_json::json!({
            "scope": scope_json,
            "results": rows,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&output)?,
        )]))
    }

    // ------------------------------------------------------------------
    // Bootstrap
    // ------------------------------------------------------------------

    async fn call_bootstrap_done(&self, agent_name: &str) -> Result<CallToolResult, anyhow::Error> {
        let agent_dir = self.agents_dir.join(agent_name);
        let required = ["IDENTITY.md", "SOUL.md", "USER.md"];

        let missing: Vec<&str> = if let Some(mtls_dir) = &self.mtls_dir {
            let sandbox_name = match right_agent::agent::parse_agent_config(&agent_dir) {
                Ok(Some(config)) => {
                    let explicit_sandbox_name =
                        config.sandbox.as_ref().and_then(|s| s.name.as_deref());
                    right_openshell::openshell::resolve_sandbox_name(
                        agent_name,
                        explicit_sandbox_name,
                    )
                }
                _ => right_openshell::openshell::resolve_sandbox_name(agent_name, None),
            };
            let mut client = right_openshell::openshell::connect_grpc(mtls_dir)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .context("bootstrap_done: failed to connect to OpenShell gRPC")?;
            let sandbox_id =
                right_openshell::openshell::resolve_sandbox_id(&mut client, &sandbox_name)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
                    .context("bootstrap_done: failed to resolve sandbox ID")?;

            let mut missing = Vec::new();
            for &file in &required {
                let path = format!("/sandbox/{file}");
                let (_, exit_code) = right_openshell::openshell::exec_in_sandbox(
                    &mut client,
                    &sandbox_id,
                    &["test", "-f", &path],
                    right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
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
            let message = format!(
                "Cannot complete bootstrap — missing files: {}. \
                 Create them first, then call bootstrap_done again.",
                missing.join(", ")
            );
            Ok(tool_error(
                "bootstrap_files_missing",
                message,
                Some(serde_json::json!({ "missing": missing })),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ConversationSearchMode {
    Thread,
    Chat,
}

fn conversation_scope_unavailable() -> CallToolResult {
    tool_error(
        "conversation_scope_unavailable",
        "conversation scope is available only for the current foreground invocation",
        None,
    )
}

/// Validate that `chat_id` is in the agent's allowlist (users or groups).
/// Reads `allowlist.yaml` on demand from `agent_dir`.
fn validate_target_against_allowlist(agent_dir: &Path, chat_id: i64) -> Result<(), String> {
    let file = match right_agent::agent::allowlist::read_file(agent_dir) {
        Ok(Some(f)) => f,
        Ok(None) => {
            return Err(format!(
                "target_chat_id {chat_id} cannot be validated: allowlist.yaml does not exist for this agent"
            ));
        }
        Err(e) => {
            return Err(format!(
                "target_chat_id {chat_id} cannot be validated: failed to read allowlist.yaml: {e}"
            ));
        }
    };
    let state = right_agent::agent::allowlist::AllowlistState::from_file(file);
    if state.is_chat_allowed(chat_id) {
        Ok(())
    } else {
        Err(format!(
            "target_chat_id {chat_id} is not in allowlist; use /allow (DM) or /allow_all (group) from a trusted account first"
        ))
    }
}

#[cfg(test)]
#[path = "right_backend_tests.rs"]
mod tests;
