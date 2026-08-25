//! Shared post-turn skill-learning pipeline: budget gate → Haiku prefilter →
//! probe-writer fork. Called by the foreground worker (Normal turns) and by
//! recurring cron runs. Pure sequence; callers wrap it in `tokio::spawn`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::telegram::worker::ProbeAnchor;

/// Default Haiku model for the skill-learning prefilter when the agent's
/// `learning.prefilter_model` is unset. Shared by the foreground worker and the
/// cron learning path.
pub(crate) const DEFAULT_PREFILTER_MODEL: &str = "claude-haiku-4-5-20251001";

/// Everything `run_post_turn` needs, owned so it can move into a spawned task.
pub(crate) struct PostTurnLearningCtx {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    /// `None` when the sandbox backend is degraded; the prefilter and
    /// probe-writer legs both skip rather than run anything on the host.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub session_locks: crate::telegram::SessionLocks,
    pub debug_flag: Arc<AtomicBool>,
    /// Resolved Haiku model for the prefilter.
    pub prefilter_model: String,
    pub probe_writer_enabled: bool,
    /// Explicit probe-writer model override (from learning config).
    pub probe_writer_model_override: Option<String>,
    /// Fallback probe-writer model (the agent's current model) used when the
    /// override is absent.
    pub probe_writer_model_fallback: Option<String>,
    pub daily_budget: f64,
    pub baseline_window_days: u32,
    pub baseline_min_sample: u32,
}

/// True when a `rightx-*` skill was successfully created/updated during this
/// turn (so the async probe must not run — the agent already captured the how).
/// `None` invocation (progress/learning disabled) → false; query error → false.
pub(crate) async fn authored_skill_this_turn(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    learning_invocation_id: Option<&str>,
) -> bool {
    let Some(invocation_id) = learning_invocation_id else {
        return false;
    };
    match client
        .learning_authored_skill_this_turn(
            &right_mcp::internal_db::LearningAuthoredSkillThisTurnRequest {
                agent: agent.to_owned(),
                invocation_id: invocation_id.to_owned(),
            },
        )
        .await
    {
        Ok(response) => response.result,
        Err(error) => {
            tracing::warn!("learning pipeline: owner authored-skill query failed: {error:#}");
            false
        }
    }
}

/// Run the budget gate, prefilter, and (on a non-Skip decision) the
/// probe-writer fork for one captured turn. A database-owner mutation error
/// propagates to the spawned-task boundary so the pipeline operation fails;
/// non-database skip decisions remain normal `Ok(())` outcomes.
pub(crate) async fn run_post_turn(
    ctx: PostTurnLearningCtx,
    anchor: ProbeAnchor,
) -> Result<(), right_mcp::internal_db::InternalDbError> {
    let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if authored_skill_this_turn(
        &ctx.internal_client,
        &ctx.agent_name,
        anchor.learning_invocation_id.as_deref(),
    )
    .await
    {
        tracing::debug!(agent = %ctx.agent_name, "learning pipeline skipped: skill authored/patched this turn");
        return Ok(());
    }
    let today_spend = match ctx
        .internal_client
        .learning_today_spend(&right_mcp::internal_db::LearningTodaySpendRequest {
            agent: ctx.agent_name.clone(),
            now_utc,
        })
        .await
    {
        Ok(response) => response.usd,
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "learning pipeline: owner spend query failed: {error:#}");
            return Ok(());
        }
    };
    if today_spend >= ctx.daily_budget {
        tracing::debug!(
            agent = %ctx.agent_name,
            spend = today_spend,
            budget = ctx.daily_budget,
            "learning pipeline skipped: daily budget exhausted"
        );
        // Skip learning when today's spend already exceeds the budget. A
        // failed skip record fails the pipeline run: without the row there
        // is no audit trail explaining why learning did not happen.
        record_budget_skip(
            &ctx.internal_client,
            &ctx.agent_name,
            anchor.chat_id,
            anchor.thread_id,
        )
        .await?;
        return Ok(());
    }

    let prefilter_ctx = crate::learning_prefilter::PrefilterContext {
        agent_dir: ctx.agent_dir.clone(),
        agent_db_dir: ctx.agent_db_dir.clone(),
        agent_name: ctx.agent_name.clone(),
        sandbox: ctx.sandbox.clone(),
        model: ctx.prefilter_model.clone(),
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        baseline_window_days: ctx.baseline_window_days,
        baseline_min_sample: ctx.baseline_min_sample,
        internal_client: Arc::clone(&ctx.internal_client),
    };
    let decision = crate::learning_prefilter::run(prefilter_ctx, anchor.clone()).await;
    let hint = match decision {
        crate::learning_prefilter::PrefilterDecision::Skip { reason } => {
            tracing::debug!(reason = %reason, "prefilter skipped");
            return Ok(());
        }
        crate::learning_prefilter::PrefilterDecision::PatchExisting {
            target_skill,
            reason,
        } => crate::learning_probe_writer::ProbeWriterHint::PatchExisting {
            target_skill,
            reason,
        },
        crate::learning_prefilter::PrefilterDecision::CreateNew { topic_hint, reason } => {
            crate::learning_probe_writer::ProbeWriterHint::CreateNew { topic_hint, reason }
        }
    };
    if !ctx.probe_writer_enabled {
        return Ok(());
    }
    let probe_writer_model = match ctx
        .probe_writer_model_override
        .or(ctx.probe_writer_model_fallback)
    {
        Some(m) if !m.is_empty() => m,
        _ => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer model unresolved, skipping");
            return Ok(());
        }
    };

    // No sandbox, no skill index: the `rightx-*` skills live on the guest
    // filesystem and there is nowhere else to read them from.
    let skill_index = match ctx.sandbox.as_ref() {
        Some(sandbox) => match crate::learning_prefilter::collect_rightx_skill_index(sandbox).await
        {
            Ok(entries) => entries
                .into_iter()
                .map(|s| format!("- {}: {}", s.name, summary_first_line(&s.excerpt)))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => {
                tracing::warn!(agent = %ctx.agent_name, "collect_rightx_skill_index failed: {e:#}");
                String::new()
            }
        },
        None => String::new(),
    };

    let writer_ctx = crate::learning_probe_writer::ProbeWriterContext {
        agent_dir: ctx.agent_dir,
        agent_db_dir: ctx.agent_db_dir,
        agent_name: ctx.agent_name,
        sandbox: ctx.sandbox,
        internal_client: ctx.internal_client,
        model: probe_writer_model,
        debug_flag: ctx.debug_flag,
        session_locks: ctx.session_locks,
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        incoming_hint: hint,
    };
    crate::learning_probe_writer::run(writer_ctx, anchor, skill_index).await;
    Ok(())
}

/// Record a `learning_skip(reason='budget')` row.
pub(crate) async fn record_budget_skip(
    client: &right_mcp::internal_client::InternalClient,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
) -> Result<(), right_mcp::internal_db::InternalDbError> {
    let request = right_mcp::internal_db::LearningRecordBudgetSkipRequest {
        agent: agent_name.to_owned(),
        request_id: crate::db::request_id(),
        chat_id,
        thread_id,
        reason: "budget".to_string(),
        intended_kind: None,
    };
    // Propagate: silently dropping this row would erase the only record of
    // why the learning turn was skipped.
    client.learning_record_budget_skip(&request).await?;
    Ok(())
}
/// First non-empty line of a skill excerpt (truncated to 200 chars), for the
/// one-line index summary. Moved verbatim from worker.
pub(crate) fn summary_first_line(excerpt: &str) -> String {
    excerpt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget-skip record failure must propagate to the caller: silently
    /// continuing drops the observability row that explains why the learning
    /// turn was skipped, with no signal anywhere.
    #[tokio::test]
    async fn record_budget_skip_propagates_owner_error() {
        let client = right_mcp::internal_client::InternalClient::new(std::path::PathBuf::from(
            "/nonexistent-right-test-internal.sock",
        ));
        let result = record_budget_skip(&client, "alpha", 7, 0).await;
        assert!(
            matches!(
                result,
                Err(right_mcp::internal_db::InternalDbError::Transport(_))
            ),
            "budget-skip failure must propagate as a typed error, got {result:?}"
        );
    }
}
