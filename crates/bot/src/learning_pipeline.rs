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
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
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
    conn: &right_db::Connection,
    learning_invocation_id: Option<&str>,
) -> bool {
    let Some(inv) = learning_invocation_id else {
        return false;
    };
    match right_agent::learned_skills::successful_finish_exists(conn, inv).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("learning pipeline: successful_finish_exists failed: {e:#}");
            false
        }
    }
}

/// Run the budget gate, prefilter, and (on a non-Skip decision) the
/// probe-writer fork for one captured turn. All failure paths log and return;
/// never propagates (the caller is fire-and-forget and must not be disrupted).
pub(crate) async fn run_post_turn(ctx: PostTurnLearningCtx, anchor: ProbeAnchor) {
    let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learning pipeline: open_connection failed: {e:#}");
            return;
        }
    };
    if authored_skill_this_turn(&conn, anchor.learning_invocation_id.as_deref()).await {
        tracing::debug!(
            agent = %ctx.agent_name,
            "learning pipeline skipped: skill authored/patched this turn"
        );
        return;
    }
    let today_spend = match crate::learning_prefilter::today_spend_usd(&conn, &now_utc).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learning pipeline: today_spend query failed: {e:#}");
            return;
        }
    };
    if today_spend >= ctx.daily_budget {
        tracing::debug!(
            agent = %ctx.agent_name,
            spend = today_spend,
            budget = ctx.daily_budget,
            "learning pipeline skipped: daily budget exhausted"
        );
        record_budget_skip(&conn, &ctx.agent_name, anchor.chat_id, anchor.thread_id).await;
        return;
    }

    let prefilter_ctx = crate::learning_prefilter::PrefilterContext {
        agent_dir: ctx.agent_dir.clone(),
        agent_db_dir: ctx.agent_db_dir.clone(),
        agent_name: ctx.agent_name.clone(),
        ssh_config_path: ctx.ssh_config_path.clone(),
        resolved_sandbox: ctx.resolved_sandbox.clone(),
        model: ctx.prefilter_model.clone(),
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        baseline_window_days: ctx.baseline_window_days,
        baseline_min_sample: ctx.baseline_min_sample,
    };
    let decision = crate::learning_prefilter::run(prefilter_ctx, anchor.clone()).await;
    let hint = match decision {
        crate::learning_prefilter::PrefilterDecision::Skip { reason } => {
            tracing::debug!(reason = %reason, "prefilter skipped");
            return;
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
        return;
    }
    let probe_writer_model = match ctx
        .probe_writer_model_override
        .or(ctx.probe_writer_model_fallback)
    {
        Some(m) if !m.is_empty() => m,
        _ => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer model unresolved, skipping");
            return;
        }
    };

    let skill_index = match crate::learning_prefilter::collect_rightx_skill_index(
        ctx.resolved_sandbox.as_deref(),
        &ctx.agent_dir,
    )
    .await
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
    };

    let writer_ctx = crate::learning_probe_writer::ProbeWriterContext {
        agent_dir: ctx.agent_dir,
        agent_db_dir: ctx.agent_db_dir,
        agent_name: ctx.agent_name,
        ssh_config_path: ctx.ssh_config_path,
        resolved_sandbox: ctx.resolved_sandbox,
        internal_client: ctx.internal_client,
        model: probe_writer_model,
        debug_flag: ctx.debug_flag,
        session_locks: ctx.session_locks,
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        incoming_hint: hint,
    };
    crate::learning_probe_writer::run(writer_ctx, anchor, skill_index).await;
}

/// Record a `learning_skip(reason='budget')` row. Moved verbatim from worker.
pub(crate) async fn record_budget_skip(
    conn: &right_db::Connection,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
) {
    if let Err(e) = right_agent::usage::insert::insert_learning_skip(
        conn,
        "budget",
        None,
        Some(chat_id),
        Some(thread_id),
    )
    .await
    {
        tracing::warn!(agent = %agent_name, "learning_skip insert failed: {e:#}");
    }
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

    #[tokio::test]
    async fn authored_skill_this_turn_true_after_successful_finish() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut c = right_db::open_connection(dir.path(), true).await.unwrap();
            right_db::migrations::MIGRATIONS
                .to_latest(&mut c)
                .await
                .unwrap();
        }
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        right_agent::learned_skills::insert_learning_event(
            &conn,
            &right_agent::learned_skills::LearningEvent {
                invocation_id: "inv-1".into(),
                agent_name: "a".into(),
                action: right_agent::learned_skills::LearningAction::Create,
                skill_name: "rightx-x".into(),
                phase: right_agent::learned_skills::LearningPhase::Finish,
                status: Some(right_agent::learned_skills::LearningStatus::Created),
                hint_outcome: None,
                reason: None,
                message: None,
                summary: None,
                event_refs: vec![],
            },
        )
        .await
        .unwrap();
        assert!(authored_skill_this_turn(&conn, Some("inv-1")).await);
        assert!(!authored_skill_this_turn(&conn, Some("inv-2")).await);
        assert!(!authored_skill_this_turn(&conn, None).await);
    }

    #[tokio::test]
    async fn budget_skip_records_learning_skip_row() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut c = right_db::open_connection(dir.path(), true).await.unwrap();
            right_db::migrations::MIGRATIONS
                .to_latest(&mut c)
                .await
                .unwrap();
        }
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        record_budget_skip(&conn, "agent-x", 99, 0).await;
        let (n, reason, kind): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(reason), MAX(intended_kind) FROM learning_skip",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!((n, reason.as_str(), kind), (1, "budget", None));
    }
}
