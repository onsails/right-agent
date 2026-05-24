//! Periodic skill curator: backup + automatic transitions + LLM consolidation pass.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CuratorState {
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub consecutive_failures: u32,
    pub circuit_open_until: Option<String>,
    pub last_spike_evidence_json: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CuratorConfig {
    pub enabled: bool,
    pub paused: bool,
    pub interval_hours: u32,
    pub min_idle_hours: u32,
    pub min_cooldown_hours: u32,
    pub stale_after_days: u32,
    pub archive_after_days: u32,
    pub cost_spike_k: f64,
    pub cost_spike_baseline_days: u32,
    pub cost_spike_min_floor_usd: f64,
    pub skill_change_threshold: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CuratorGateDecision {
    Run { trigger: CuratorTrigger },
    SkipDisabled,
    SkipPaused,
    SkipCircuitOpen,
    SkipChatNotIdle,
    SkipCooldown,
    SkipNoTrigger,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CuratorTrigger {
    CostSpike(right_agent::usage::turn_baseline::CostSpikeEvidence),
    SkillChangeCount { count: u32, threshold: u32 },
    TimeFallback { interval_hours: u32 },
}

/// Trigger-independent skip conditions: enabled, paused, circuit open, chat
/// idle, cooldown. Returns `Some(skip)` if one fires, `None` if all pass.
/// Extracted so `run_if_due` can short-circuit BEFORE computing expensive
/// trigger signals (cost-spike SQL, skills index file read).
fn cheap_skip(
    config: CuratorConfig,
    state: &CuratorState,
    now: DateTime<Utc>,
    latest_user_activity_at: Option<DateTime<Utc>>,
) -> Option<CuratorGateDecision> {
    if !config.enabled {
        return Some(CuratorGateDecision::SkipDisabled);
    }
    if config.paused {
        return Some(CuratorGateDecision::SkipPaused);
    }
    if let Some(open_until) = state.circuit_open_until.as_deref()
        && let Ok(dt) = DateTime::parse_from_rfc3339(open_until)
        && dt.with_timezone(&Utc) > now
    {
        return Some(CuratorGateDecision::SkipCircuitOpen);
    }
    if let Some(latest) = latest_user_activity_at
        && now - latest < Duration::hours(config.min_idle_hours as i64)
    {
        return Some(CuratorGateDecision::SkipChatNotIdle);
    }
    if let Some(last_dt) = state.last_run_at.as_deref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    }) && now - last_dt < Duration::hours(config.min_cooldown_hours as i64)
    {
        return Some(CuratorGateDecision::SkipCooldown);
    }
    None
}

/// Pure gate decision. No I/O.
pub(crate) fn should_run_now(
    config: CuratorConfig,
    state: &CuratorState,
    now: DateTime<Utc>,
    latest_user_activity_at: Option<DateTime<Utc>>,
    cost_spike_evidence: Option<right_agent::usage::turn_baseline::CostSpikeEvidence>,
    skill_change_count: u32,
) -> CuratorGateDecision {
    if let Some(skip) = cheap_skip(config, state, now, latest_user_activity_at) {
        return skip;
    }

    // Trigger priority: cost spike > skill change count > time fallback.
    if let Some(ev) = cost_spike_evidence {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::CostSpike(ev),
        };
    }
    if skill_change_count >= config.skill_change_threshold {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::SkillChangeCount {
                count: skill_change_count,
                threshold: config.skill_change_threshold,
            },
        };
    }
    let last = state.last_run_at.as_deref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    });
    if let Some(last_dt) = last
        && now - last_dt >= Duration::hours(config.interval_hours as i64)
    {
        return CuratorGateDecision::Run {
            trigger: CuratorTrigger::TimeFallback {
                interval_hours: config.interval_hours,
            },
        };
    }
    // No trigger fired — covers both the first-ever-run case (Hermes defer)
    // and the post-cooldown idle case.
    CuratorGateDecision::SkipNoTrigger
}

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration as StdDuration;

use crate::telegram::SessionLocks;

const CURATOR_TIMEOUT: StdDuration = StdDuration::from_secs(900);
const CURATOR_MAX_TURNS: u32 = 9999;

pub(crate) fn load_state_db(conn: &rusqlite::Connection) -> Result<CuratorState, rusqlite::Error> {
    let row = conn.query_row(
        "SELECT last_run_at, last_run_status, consecutive_failures, \
                circuit_open_until, last_spike_evidence_json \
         FROM curator_state WHERE agent_singleton_id = 1",
        [],
        |r| {
            Ok(CuratorState {
                last_run_at: r.get(0)?,
                last_run_status: r.get(1)?,
                consecutive_failures: r.get::<_, i64>(2)? as u32,
                circuit_open_until: r.get(3)?,
                last_spike_evidence_json: r.get(4)?,
            })
        },
    );
    match row {
        Ok(s) => Ok(s),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CuratorState::default()),
        Err(e) => Err(e),
    }
}

pub(crate) fn save_state_db(
    conn: &rusqlite::Connection,
    state: &CuratorState,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO curator_state \
            (agent_singleton_id, last_run_at, last_run_status, \
             consecutive_failures, circuit_open_until, last_spike_evidence_json) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            state.last_run_at,
            state.last_run_status,
            state.consecutive_failures as i64,
            state.circuit_open_until,
            state.last_spike_evidence_json,
        ],
    )?;
    Ok(())
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
/// logs a warn and continues. Updates state after a Run-gated invocation.
pub(crate) async fn run_if_due(
    ctx: CuratorContext,
    latest_user_activity_at: Option<DateTime<Utc>>,
) {
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator open_connection failed: {e:#}");
            return;
        }
    };
    let mut state = match load_state_db(&conn) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator load_state_db failed: {e:#}");
            return;
        }
    };

    let now = Utc::now();

    // Seed first-run timestamp (Hermes defer).
    if state.last_run_at.is_none() {
        state.last_run_at = Some(now.to_rfc3339());
        if let Err(e) = save_state_db(&conn, &state) {
            tracing::warn!(agent = %ctx.agent_name, "curator seed state failed: {e:#}");
        }
        return;
    }

    // Cheap pre-gate: short-circuit before computing cost-spike SQL or
    // reading lifecycle rows. On a busy ticker (60s per agent), these are
    // wasted I/O when the chat is active or we're inside cooldown.
    if let Some(skip) = cheap_skip(ctx.config, &state, now, latest_user_activity_at) {
        tracing::debug!(agent = %ctx.agent_name, "curator gate: {:?}", skip);
        return;
    }

    // Compute trigger signals.
    let cost_spike_evidence = match right_agent::usage::turn_baseline::check_probe_writer_cost_spike(
        &conn,
        now,
        ctx.config.cost_spike_baseline_days,
        ctx.config.cost_spike_k,
        ctx.config.cost_spike_min_floor_usd,
    ) {
        Ok(ev) => ev,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator cost spike check failed: {e:#}");
            None
        }
    };

    let lifecycle_rows = match crate::lifecycle::usage::list(&conn) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator lifecycle read failed: {e:#}");
            return;
        }
    };
    let since = state.last_run_at.as_deref().unwrap_or("");
    let change_count = crate::lifecycle::usage::count_changes_since(&lifecycle_rows, since);

    let decision = should_run_now(
        ctx.config,
        &state,
        now,
        latest_user_activity_at,
        cost_spike_evidence,
        change_count,
    );

    let trigger = match decision {
        CuratorGateDecision::Run { trigger } => trigger,
        other => {
            tracing::debug!(agent = %ctx.agent_name, "curator gate: {:?}", other);
            return;
        }
    };

    // Capture evidence.
    state.last_spike_evidence_json = Some(serialize_evidence(&trigger, now));

    let skills_dir = ctx.agent_dir.join(".claude/skills");
    let backups_dir = ctx.agent_dir.join("curator_backups");
    let now_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    if let Err(e) = crate::lifecycle::snapshot::snapshot_skills(&skills_dir, &backups_dir, &now_str)
    {
        tracing::warn!(agent = %ctx.agent_name, "curator snapshot failed: {e:#}");
    }

    let transition_changes = match crate::lifecycle::transitions::apply_automatic_transitions(
        &conn,
        now,
        crate::lifecycle::transitions::TransitionConfig {
            stale_after: Duration::days(ctx.config.stale_after_days as i64),
            archive_after: Duration::days(ctx.config.archive_after_days as i64),
        },
    ) {
        Ok(changes) => changes,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator auto-transition failed: {e:#}");
            return;
        }
    };
    let lifecycle_rows = match crate::lifecycle::usage::list(&conn) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator lifecycle refresh failed: {e:#}");
            return;
        }
    };
    tracing::info!(
        agent = %ctx.agent_name,
        transitions = transition_changes,
        trigger = ?trigger,
        "curator auto-transitions applied"
    );

    // LLM consolidation fork.
    let invocation = build_curator_invocation(&ctx, &lifecycle_rows);
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

    let run_status = match tokio::time::timeout(CURATOR_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if let Some(b) = crate::cc::stream::parse_usage_full(&stdout)
                && let Err(e) = right_agent::usage::insert::insert_learning_curator(&conn, &b)
            {
                tracing::warn!(agent = %ctx.agent_name, "curator usage insert failed: {e:#}");
            }
            if output.status.success() {
                "success".to_owned()
            } else {
                tracing::warn!(
                    agent = %ctx.agent_name,
                    status = ?output.status,
                    "curator exited non-zero"
                );
                "failed".to_owned()
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(agent = %ctx.agent_name, "curator spawn failed: {e:#}");
            "failed".to_owned()
        }
        Err(_) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "curator timed out after {}s",
                CURATOR_TIMEOUT.as_secs()
            );
            "failed".to_owned()
        }
    };

    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some(run_status.clone());
    if run_status == "success" {
        state.consecutive_failures = 0;
        state.circuit_open_until = None;
    } else {
        // TODO(Phase-2): set circuit_open_until when consecutive_failures crosses a
        // threshold. Today the gate checks circuit_open_until but no runtime path
        // opens the circuit — it's only set by direct DB writes (tests).
        state.consecutive_failures += 1;
    }
    if let Err(e) = save_state_db(&conn, &state) {
        tracing::warn!(agent = %ctx.agent_name, "curator save state failed: {e:#}");
    }
}

fn serialize_evidence(trigger: &CuratorTrigger, now: DateTime<Utc>) -> String {
    let computed_at = now.to_rfc3339();
    match trigger {
        CuratorTrigger::CostSpike(ev) => serde_json::json!({
            "trigger": "cost_spike",
            "computed_at": computed_at,
            "details": {
                "today_cost_usd": ev.today_cost_usd,
                "baseline_p50_usd": ev.baseline_p50_usd,
                "k": ev.k,
                "min_floor_usd": ev.min_floor_usd
            }
        })
        .to_string(),
        CuratorTrigger::SkillChangeCount { count, threshold } => serde_json::json!({
            "trigger": "skill_change_count",
            "computed_at": computed_at,
            "details": { "count": count, "threshold": threshold }
        })
        .to_string(),
        CuratorTrigger::TimeFallback { interval_hours } => serde_json::json!({
            "trigger": "time_fallback",
            "computed_at": computed_at,
            "details": { "interval_hours": interval_hours }
        })
        .to_string(),
    }
}

fn build_curator_invocation(
    ctx: &CuratorContext,
    lifecycle_rows: &[right_lifecycle::SkillLifecycleRow],
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    let curator_session_id = uuid::Uuid::new_v4().to_string();
    let candidate_list = render_candidate_list(lifecycle_rows);
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

fn render_candidate_list(lifecycle_rows: &[right_lifecycle::SkillLifecycleRow]) -> String {
    use std::fmt::Write;
    let mut s = String::from("<inventory>\n");
    for r in lifecycle_rows {
        if matches!(
            r.created_by,
            right_lifecycle::CreatedBy::Foreground | right_lifecycle::CreatedBy::Bundled
        ) {
            continue;
        }
        if r.pinned {
            continue;
        }
        let _ = writeln!(
            s,
            "- {name}: state={state:?} use={used} patch={patched} created_by={by:?} pinned={pinned}",
            name = r.skill_name,
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
    use tempfile::tempdir;

    fn open_test_conn() -> rusqlite::Connection {
        let dir = tempdir().unwrap();
        right_db::open_connection(dir.path(), true).unwrap()
    }

    #[test]
    fn db_load_state_returns_default_when_empty() {
        let conn = open_test_conn();
        let s = load_state_db(&conn).unwrap();
        assert!(s.last_run_at.is_none());
        assert_eq!(s.consecutive_failures, 0);
    }

    #[test]
    fn db_save_then_load_round_trip() {
        let conn = open_test_conn();
        let s = CuratorState {
            last_run_at: Some("2026-05-22T00:00:00Z".to_owned()),
            last_run_status: Some("success".to_owned()),
            consecutive_failures: 2,
            circuit_open_until: None,
            last_spike_evidence_json: Some(r#"{"trigger":"cost_spike"}"#.to_owned()),
        };
        save_state_db(&conn, &s).unwrap();
        let loaded = load_state_db(&conn).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn db_save_replaces_existing_row() {
        let conn = open_test_conn();
        save_state_db(
            &conn,
            &CuratorState {
                last_run_at: Some("a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        save_state_db(
            &conn,
            &CuratorState {
                last_run_at: Some("b".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curator_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let loaded = load_state_db(&conn).unwrap();
        assert_eq!(loaded.last_run_at.as_deref(), Some("b"));
    }

    fn cfg() -> CuratorConfig {
        CuratorConfig {
            enabled: true,
            paused: false,
            interval_hours: 168,
            min_idle_hours: 2,
            min_cooldown_hours: 12,
            stale_after_days: 30,
            archive_after_days: 90,
            cost_spike_k: 3.0,
            cost_spike_baseline_days: 14,
            cost_spike_min_floor_usd: 0.05,
            skill_change_threshold: 3,
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
                None,
                None,
                0
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
                None,
                None,
                0
            ),
            CuratorGateDecision::SkipPaused
        );
    }

    #[test]
    fn circuit_open_in_future_skips() {
        let s = CuratorState {
            circuit_open_until: Some("2027-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(
            should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0),
            CuratorGateDecision::SkipCircuitOpen
        );
    }

    #[test]
    fn cooldown_blocks_all_triggers() {
        let s = CuratorState {
            last_run_at: Some("2026-05-21T18:00:00Z".into()),
            ..Default::default()
        };
        let ev = right_agent::usage::turn_baseline::CostSpikeEvidence {
            today_cost_usd: 1.0,
            baseline_p50_usd: 0.1,
            k: 3.0,
            min_floor_usd: 0.05,
        };
        assert_eq!(
            should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, Some(ev), 5),
            CuratorGateDecision::SkipCooldown
        );
    }

    #[test]
    fn cost_spike_fires_after_cooldown() {
        let s = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".into()),
            ..Default::default()
        };
        let ev = right_agent::usage::turn_baseline::CostSpikeEvidence {
            today_cost_usd: 1.0,
            baseline_p50_usd: 0.1,
            k: 3.0,
            min_floor_usd: 0.05,
        };
        let d = should_run_now(
            cfg(),
            &s,
            dt("2026-05-22T00:00:00Z"),
            None,
            Some(ev.clone()),
            0,
        );
        assert!(matches!(
            d,
            CuratorGateDecision::Run {
                trigger: CuratorTrigger::CostSpike(_)
            }
        ));
    }

    #[test]
    fn skill_change_count_fires_when_no_cost_spike() {
        let s = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".into()),
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 4);
        assert_eq!(
            d,
            CuratorGateDecision::Run {
                trigger: CuratorTrigger::SkillChangeCount {
                    count: 4,
                    threshold: 3
                }
            }
        );
    }

    #[test]
    fn time_fallback_fires_when_no_other_trigger() {
        let s = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".into()),
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
        assert_eq!(
            d,
            CuratorGateDecision::Run {
                trigger: CuratorTrigger::TimeFallback {
                    interval_hours: 168
                }
            }
        );
    }

    #[test]
    fn no_trigger_no_run() {
        let s = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".into()),
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
        // last_run_at 24h ago; cooldown 12h passed; no spike; no change-count; not 168h yet
        assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
    }

    #[test]
    fn first_ever_run_defers() {
        let s = CuratorState {
            last_run_at: None,
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
        assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
    }

    #[test]
    fn chat_active_within_min_idle_skips() {
        let s = CuratorState {
            last_run_at: Some("2026-05-01T00:00:00Z".into()),
            ..Default::default()
        };
        let now = dt("2026-05-22T00:00:00Z");
        // Chat activity 30 minutes before `now`, well within min_idle_hours=2.
        let just_now = now - Duration::minutes(30);
        assert_eq!(
            should_run_now(cfg(), &s, now, Some(just_now), None, 0),
            CuratorGateDecision::SkipChatNotIdle
        );
    }
}
