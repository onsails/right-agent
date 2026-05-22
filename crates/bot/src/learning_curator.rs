//! Periodic skill curator: backup + automatic transitions + LLM consolidation pass.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct CuratorState {
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CuratorConfig {
    pub enabled: bool,
    pub paused: bool,
    pub interval_hours: u32,
    pub min_idle_hours: u32,
    pub stale_after_days: u32,
    pub archive_after_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CuratorGateDecision {
    Run,
    SkipDisabled,
    SkipPaused,
    SkipIntervalNotElapsed,
    SkipChatNotIdle,
}

/// Pure gate decision. No I/O.
pub(crate) fn should_run_now(
    config: CuratorConfig,
    state: &CuratorState,
    now: DateTime<Utc>,
    latest_user_activity_at: Option<DateTime<Utc>>,
) -> CuratorGateDecision {
    if !config.enabled {
        return CuratorGateDecision::SkipDisabled;
    }
    if config.paused {
        return CuratorGateDecision::SkipPaused;
    }
    if let Some(last) = state.last_run_at.as_deref() {
        if let Ok(last_dt) = DateTime::parse_from_rfc3339(last) {
            let last_dt = last_dt.with_timezone(&Utc);
            if now - last_dt < Duration::hours(config.interval_hours as i64) {
                return CuratorGateDecision::SkipIntervalNotElapsed;
            }
        }
    } else {
        // First-ever run: caller seeds last_run_at and defers one interval.
        return CuratorGateDecision::SkipIntervalNotElapsed;
    }
    if let Some(latest) = latest_user_activity_at
        && now - latest < Duration::hours(config.min_idle_hours as i64)
    {
        return CuratorGateDecision::SkipChatNotIdle;
    }
    CuratorGateDecision::Run
}

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration as StdDuration;

use crate::telegram::SessionLocks;

const CURATOR_TIMEOUT: StdDuration = StdDuration::from_secs(900);
const CURATOR_MAX_TURNS: u32 = 9999;

pub(crate) fn state_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join(".claude/skills/.curator_state.json")
}

pub(crate) fn load_state(path: &Path) -> CuratorState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_state(path: &Path, state: &CuratorState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_vec_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[derive(Debug, Clone)]
pub(crate) struct CuratorContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub session_locks: SessionLocks,
    pub config: CuratorConfig,
}

/// Gate, snapshot, transitions, and LLM fork. Best-effort: every failure path
/// logs a warn and continues. Updates `last_run_at` after a Run-gated invocation.
pub(crate) async fn run_if_due(
    ctx: CuratorContext,
    latest_user_activity_at: Option<DateTime<Utc>>,
) {
    let state_path = state_path(&ctx.agent_dir);
    let mut state = load_state(&state_path);

    // Seed first-run timestamp if missing (Hermes pattern: defer one interval on cold start).
    if state.last_run_at.is_none() {
        state.last_run_at = Some(Utc::now().to_rfc3339());
        if let Err(e) = save_state(&state_path, &state) {
            tracing::warn!(agent = %ctx.agent_name, "curator seed state failed: {e:#}");
        }
        return;
    }

    let now = Utc::now();
    let decision = should_run_now(ctx.config, &state, now, latest_user_activity_at);
    if decision != CuratorGateDecision::Run {
        tracing::debug!(agent = %ctx.agent_name, "curator gate: {:?}", decision);
        return;
    }

    let skills_dir = ctx.agent_dir.join(".claude/skills");
    let backups_dir = ctx.agent_dir.join("curator_backups");
    let now_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    if let Err(e) = crate::lifecycle::snapshot::snapshot_skills(&skills_dir, &backups_dir, &now_str)
    {
        tracing::warn!(agent = %ctx.agent_name, "curator snapshot failed: {e:#}");
    }

    let usage_path = skills_dir.join(".usage.json");
    let mut index = match crate::lifecycle::usage::read_index(&usage_path) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator usage read failed: {e:#}");
            return;
        }
    };
    let transition_changes = crate::lifecycle::transitions::apply_automatic_transitions(
        &mut index,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after_days: ctx.config.stale_after_days as i64,
            archive_after_days: ctx.config.archive_after_days as i64,
        },
    );
    if let Err(e) = crate::lifecycle::usage::write_index(&usage_path, &index) {
        tracing::warn!(agent = %ctx.agent_name, "curator usage write failed: {e:#}");
    }
    tracing::info!(
        agent = %ctx.agent_name,
        transitions = transition_changes,
        "curator auto-transitions applied"
    );

    // LLM consolidation fork.
    let invocation = build_curator_invocation(&ctx, &index);
    let args = invocation.into_args();

    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        &ctx.agent_dir,
        ctx.ssh_config_path.as_deref(),
        ctx.resolved_sandbox.as_deref(),
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    match tokio::time::timeout(CURATOR_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
                && let Ok(conn) = right_db::open_connection(&ctx.agent_db_dir, false)
                && let Err(e) = right_agent::usage::insert::insert_learning_curator(&conn, &b)
            {
                tracing::warn!(agent = %ctx.agent_name, "curator usage insert failed: {e:#}");
            }
            if !output.status.success() {
                tracing::warn!(
                    agent = %ctx.agent_name,
                    status = ?output.status,
                    "curator exited non-zero"
                );
            }
        }
        Ok(Err(e)) => tracing::warn!(agent = %ctx.agent_name, "curator spawn failed: {e:#}"),
        Err(_) => tracing::warn!(
            agent = %ctx.agent_name,
            "curator timed out after {}s",
            CURATOR_TIMEOUT.as_secs()
        ),
    };

    state.last_run_at = Some(now.to_rfc3339());
    if let Err(e) = save_state(&state_path, &state) {
        tracing::warn!(agent = %ctx.agent_name, "curator save state failed: {e:#}");
    }
}

fn build_curator_invocation(
    ctx: &CuratorContext,
    index: &crate::lifecycle::usage::Index,
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    let curator_session_id = uuid::Uuid::new_v4().to_string();
    let candidate_list = render_candidate_list(index);
    let user_prompt = format!(
        "{system}\n\n{candidates}",
        system = right_codegen::CURATOR_SYSTEM_PROMPT,
        candidates = candidate_list,
    );
    ClaudeInvocation {
        mcp_config_path: Some(crate::cc::invocation::mcp_config_path(
            ctx.ssh_config_path.as_deref(),
            &ctx.agent_dir,
        )),
        json_schema: None,
        output_format: OutputFormat::StreamJson,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(CURATOR_MAX_TURNS),
        resume_session_id: None,
        new_session_id: Some(curator_session_id),
        fork_session: false,
        allowed_tools: vec![
            "Read".into(),
            "Bash".into(),
            "mcp__right__skill_learning_start".into(),
            "mcp__right__skill_learning_finish".into(),
        ],
        disallowed_tools: vec![],
        extra_args: vec![],
        prompt: Some(user_prompt),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}

fn render_candidate_list(index: &crate::lifecycle::usage::Index) -> String {
    use std::fmt::Write;
    let mut s = String::from("<inventory>\n");
    for (name, r) in &index.skills {
        if matches!(
            r.created_by,
            crate::lifecycle::usage::CreatedBy::Foreground
                | crate::lifecycle::usage::CreatedBy::Bundled
        ) {
            continue;
        }
        if r.pinned {
            continue;
        }
        let _ = writeln!(
            s,
            "- {name}: state={state:?} use={used} patch={patched} created_by={by:?} pinned={pinned}",
            state = r.state,
            used = r.use_count,
            patched = r.patch_count,
            by = r.created_by,
            pinned = r.pinned,
        );
    }
    s.push_str("</inventory>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CuratorConfig {
        CuratorConfig {
            enabled: true,
            paused: false,
            interval_hours: 168,
            min_idle_hours: 2,
            stale_after_days: 30,
            archive_after_days: 90,
        }
    }

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn disabled_skips() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            should_run_now(
                c,
                &CuratorState::default(),
                dt("2026-05-22T00:00:00Z"),
                None
            ),
            CuratorGateDecision::SkipDisabled
        );
    }

    #[test]
    fn paused_skips() {
        let mut c = cfg();
        c.paused = true;
        assert_eq!(
            should_run_now(
                c,
                &CuratorState::default(),
                dt("2026-05-22T00:00:00Z"),
                None
            ),
            CuratorGateDecision::SkipPaused
        );
    }

    #[test]
    fn first_run_defers_one_interval() {
        let state = CuratorState { last_run_at: None };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::SkipIntervalNotElapsed
        );
    }

    #[test]
    fn within_interval_skips() {
        let state = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".to_owned()),
        };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::SkipIntervalNotElapsed
        );
    }

    #[test]
    fn after_interval_runs_when_idle() {
        let state = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".to_owned()),
        };
        assert_eq!(
            should_run_now(cfg(), &state, dt("2026-05-22T00:00:00Z"), None),
            CuratorGateDecision::Run
        );
    }

    #[test]
    fn chat_active_within_min_idle_skips() {
        let state = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".to_owned()),
        };
        let now = dt("2026-05-22T00:00:00Z");
        let just_now = now - Duration::minutes(30);
        assert_eq!(
            should_run_now(cfg(), &state, now, Some(just_now)),
            CuratorGateDecision::SkipChatNotIdle
        );
    }
}
