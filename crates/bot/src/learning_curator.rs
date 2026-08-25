//! Periodic skill curator: backup + automatic transitions + LLM consolidation pass.
//!
//! Spec: docs/superpowers/specs/2026-05-22-skill-learning-writer-curator-design.md

use chrono::{DateTime, Duration, Utc};

/// Return the newest archived conversation message timestamp. An empty
/// conversation is idle enough for the curator gate.
#[cfg(test)]
async fn latest_chat_activity(
    conn: &right_db::Connection,
) -> Result<Option<DateTime<Utc>>, right_db::DbError> {
    let latest: Option<String> = conn
        .query_row(
            "SELECT MAX(created_at) FROM conversation_messages",
            (),
            |r| r.get(0),
        )
        .await?;
    match latest {
        None => Ok(None),
        Some(timestamp) => DateTime::parse_from_rfc3339(&timestamp)
            .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
            .map_err(|error| {
                right_db::DbError::InvalidParameter(format!(
                    "invalid conversation message timestamp {timestamp:?}: {error}"
                ))
            }),
    }
}

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
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown_hours: u32,
    pub mode: right_agent_config::CuratorMode,
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

/// Circuit-breaker decision: once `consecutive_failures >= threshold`, the
/// circuit opens for a FIXED `cooldown_hours` (not exponential). Failures
/// persist across opens, so a permanently-broken curator re-opens at this
/// cadence rather than hammering every cooldown. Returns the new
/// `circuit_open_until`, or `None` to leave the circuit closed.
pub(crate) fn next_circuit_open_until(
    consecutive_failures: u32,
    threshold: u32,
    cooldown_hours: u32,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if consecutive_failures >= threshold {
        Some(now + Duration::hours(cooldown_hours as i64))
    } else {
        None
    }
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

/// Record per-skill `maintain` spend for the skills a curator pass archived.
/// The pass cost/cache is split evenly across the archived skills (cost/N
/// each, cache integer-divided), written in one transaction, so summing the
/// `maintain` rows recovers the exact pass cost. Empty `mutated` writes
/// nothing. Best-effort observability at the fire-and-forget learning
/// boundary: a failure logs and swallows.
#[cfg(test)]
async fn record_curator_maintain_spend(
    conn: &right_db::Connection,
    mutated: &[String],
    b: &right_agent::usage::UsageBreakdown,
    invocation_id: Option<&str>,
) {
    if mutated.is_empty() {
        return;
    }
    let n = mutated.len();
    let per_cost = b.total_cost_usd / n as f64;
    let per_cache_read = b.cache_read_tokens as i64 / n as i64;
    let per_cache_creation = b.cache_creation_tokens as i64 / n as i64;
    if let Err(e) = right_agent::usage::insert::insert_skill_spend_many(
        conn,
        mutated,
        "maintain",
        per_cost,
        per_cache_read,
        per_cache_creation,
        invocation_id,
    )
    .await
    {
        tracing::warn!("curator maintain spend insert failed: {e:#}");
    }
}

#[cfg(test)]
pub(crate) async fn load_state_db(
    conn: &right_db::Connection,
) -> Result<CuratorState, right_db::DbError> {
    let row = conn
        .query_row(
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
        )
        .await;
    match row {
        Ok(s) => Ok(s),
        Err(right_db::DbError::NotFound) => Ok(CuratorState::default()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
pub(crate) async fn save_state_db(
    conn: &right_db::Connection,
    state: &CuratorState,
) -> Result<(), right_db::DbError> {
    conn.execute(
        "INSERT OR REPLACE INTO curator_state \
            (agent_singleton_id, last_run_at, last_run_status, \
             consecutive_failures, circuit_open_until, last_spike_evidence_json) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        right_db::params![
            state.last_run_at.as_deref(),
            state.last_run_status.as_deref(),
            state.consecutive_failures as i64,
            state.circuit_open_until.as_deref(),
            state.last_spike_evidence_json.as_deref(),
        ],
    )
    .await?;
    Ok(())
}

/// One append-only `curator_runs` history row (A1 observability).
#[derive(Debug, Clone)]
pub(crate) struct CuratorRunRecord {
    pub run_at: String,
    pub trigger: String,
    pub trigger_evidence_json: Option<String>,
    pub mode: String,
    pub status: String,
    pub cost_usd: f64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub consolidations: i64,
    pub archives: i64,
    pub summary: Option<String>,
    pub actions_json: String,
    pub invocation_id: Option<String>,
}

/// Append a curator run-history row. Best-effort at the learning boundary —
/// callers log-and-continue on error; never abort a pass over telemetry.
#[cfg(test)]
pub(crate) async fn insert_curator_run(
    conn: &right_db::Connection,
    rec: &CuratorRunRecord,
) -> Result<(), right_db::DbError> {
    conn.execute(
        "INSERT INTO curator_runs \
            (run_at, trigger, trigger_evidence_json, mode, status, cost_usd, \
             cache_read, cache_creation, consolidations, archives, summary, \
             actions_json, invocation_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        right_db::params![
            rec.run_at.as_str(),
            rec.trigger.as_str(),
            rec.trigger_evidence_json.as_deref(),
            rec.mode.as_str(),
            rec.status.as_str(),
            rec.cost_usd,
            rec.cache_read,
            rec.cache_creation,
            rec.consolidations,
            rec.archives,
            rec.summary.as_deref(),
            rec.actions_json.as_str(),
            rec.invocation_id.as_deref(),
        ],
    )
    .await?;
    Ok(())
}

/// Map a `CuratorTrigger` to its `curator_runs.trigger` string.
fn trigger_label(trigger: &CuratorTrigger) -> &'static str {
    match trigger {
        CuratorTrigger::CostSpike(_) => "cost_spike",
        CuratorTrigger::SkillChangeCount { .. } => "skill_change",
        CuratorTrigger::TimeFallback { .. } => "time_fallback",
    }
}

#[derive(Clone)]
pub(crate) struct CuratorContext {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub sandbox: Option<crate::sandbox::Sandbox>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub model: String,
    pub debug_flag: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub session_locks: SessionLocks,
    pub config: CuratorConfig,
}

/// Apply a failed-pass state transition: bump `consecutive_failures`, stamp
/// `last_run_at`/`last_run_status`, and open the circuit when the threshold is
/// reached (B1). Mutates `state` in place; the caller persists it.
fn mark_failed_run(state: &mut CuratorState, config: CuratorConfig, now: DateTime<Utc>) {
    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some("failed".to_owned());
    state.consecutive_failures += 1;
    if let Some(open_until) = next_circuit_open_until(
        state.consecutive_failures,
        config.circuit_failure_threshold,
        config.circuit_cooldown_hours,
        now,
    ) {
        state.circuit_open_until = Some(open_until.to_rfc3339());
    }
}

fn state_from_dto(dto: right_mcp::internal_db::CuratorStateDto) -> CuratorState {
    CuratorState {
        last_run_at: dto.last_run_at,
        last_run_status: dto.last_run_status,
        consecutive_failures: dto.consecutive_failures,
        circuit_open_until: dto.circuit_open_until,
        last_spike_evidence_json: dto.last_spike_evidence_json,
    }
}

fn state_to_dto(state: &CuratorState) -> right_mcp::internal_db::CuratorStateDto {
    right_mcp::internal_db::CuratorStateDto {
        last_run_at: state.last_run_at.clone(),
        last_run_status: state.last_run_status.clone(),
        consecutive_failures: state.consecutive_failures,
        circuit_open_until: state.circuit_open_until.clone(),
        last_spike_evidence_json: state.last_spike_evidence_json.clone(),
    }
}

async fn save_state_ipc(ctx: &CuratorContext, state: &CuratorState) {
    if let Err(error) = ctx
        .internal_client
        .curator_save_state(&right_mcp::internal_db::CuratorSaveStateRequest {
            agent: ctx.agent_name.clone(),
            request_id: crate::db::request_id(),
            state: state_to_dto(state),
        })
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "curator owner state save failed: {error:#}");
    }
}

fn lifecycle_from_dto(
    dto: right_mcp::internal_db::SkillLifecycleDto,
) -> Result<right_lifecycle::SkillLifecycleRow, String> {
    let parse = |value: Option<String>| {
        value
            .map(|v| {
                DateTime::parse_from_rfc3339(&v)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| e.to_string())
            })
            .transpose()
    };
    Ok(right_lifecycle::SkillLifecycleRow {
        skill_name: dto.skill_name,
        state: right_lifecycle::LifecycleState::from_db_str(&dto.state)
            .map_err(|e| e.to_string())?,
        pinned: dto.pinned,
        created_by: right_lifecycle::CreatedBy::from_db_str(&dto.created_by)
            .map_err(|e| e.to_string())?,
        use_count: i64::from(dto.use_count),
        patch_count: i64::from(dto.patch_count),
        created_at: parse(dto.created_at)?,
        last_used_at: parse(dto.last_used_at)?,
        last_patched_at: parse(dto.last_patched_at)?,
        archived_at: parse(dto.archived_at)?,
        absorbed_into: dto.absorbed_into,
    })
}

fn run_record_to_dto(record: CuratorRunRecord) -> right_mcp::internal_db::CuratorRunRecordDto {
    right_mcp::internal_db::CuratorRunRecordDto {
        run_at: record.run_at,
        trigger: record.trigger,
        trigger_evidence_json: record.trigger_evidence_json,
        mode: record.mode,
        status: record.status,
        cost_usd: record.cost_usd,
        cache_read: record.cache_read,
        cache_creation: record.cache_creation,
        consolidations: record.consolidations,
        archives: record.archives,
        summary: record.summary,
        actions_json: record.actions_json,
        invocation_id: record.invocation_id,
    }
}

/// Gate, snapshot, transitions, and LLM fork. Best-effort: every failure path
/// logs a warn and continues. Updates state after a Run-gated invocation.
pub(crate) async fn run_if_due(ctx: CuratorContext) {
    let mut state = match ctx
        .internal_client
        .curator_load_state(&right_mcp::internal_db::CuratorLoadStateRequest {
            agent: ctx.agent_name.clone(),
        })
        .await
    {
        Ok(response) => state_from_dto(response.state),
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator owner state load failed: {error:#}");
            return;
        }
    };
    let now = Utc::now();

    if state.last_run_at.is_none() {
        state.last_run_at = Some(now.to_rfc3339());
        save_state_ipc(&ctx, &state).await;
        return;
    }

    let latest_user_activity_at = match ctx
        .internal_client
        .curator_latest_chat_activity(&right_mcp::internal_db::CuratorLatestChatActivityRequest {
            agent: ctx.agent_name.clone(),
        })
        .await
    {
        Ok(response) => response
            .at
            .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
            .map(|at| at.with_timezone(&Utc)),
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator owner activity read failed: {error:#}");
            return;
        }
    };
    if let Some(skip) = cheap_skip(ctx.config, &state, now, latest_user_activity_at) {
        tracing::debug!(agent = %ctx.agent_name, "curator gate: {:?}", skip);
        return;
    }
    let cost_spike_evidence = match ctx
        .internal_client
        .learning_probe_cost_spike(&right_mcp::internal_db::LearningProbeCostSpikeRequest {
            agent: ctx.agent_name.clone(),
            now_rfc3339: now.to_rfc3339(),
            baseline_days: ctx.config.cost_spike_baseline_days,
            k: ctx.config.cost_spike_k,
            min_floor_usd: ctx.config.cost_spike_min_floor_usd,
        })
        .await
    {
        Ok(response) => {
            response
                .evidence
                .map(|e| right_agent::usage::turn_baseline::CostSpikeEvidence {
                    today_cost_usd: e.today_cost_usd,
                    baseline_p50_usd: e.baseline_p50_usd,
                    k: e.k,
                    min_floor_usd: e.min_floor_usd,
                })
        }
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator owner cost spike failed: {error:#}");
            None
        }
    };
    // last_run_at is always Some(_) here — the first-run defer above writes
    // it before any gate runs, so unwrap is structural, not assumed.
    let last_run_at = state
        .last_run_at
        .as_deref()
        .expect("last_run_at seeded by first-run defer");
    let since = match DateTime::parse_from_rfc3339(last_run_at) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(e) => {
            tracing::warn!(
                agent = %ctx.agent_name,
                "curator: unparseable last_run_at {last_run_at:?}: {e:#}"
            );
            return;
        }
    };
    let change_count = match ctx
        .internal_client
        .curator_change_count(&right_mcp::internal_db::CuratorChangeCountRequest {
            agent: ctx.agent_name.clone(),
            since_rfc3339: since.to_rfc3339(),
        })
        .await
    {
        Ok(response) => response.count,
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator owner change-count failed: {error:#}");
            return;
        }
    };
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
    state.last_spike_evidence_json = Some(serialize_evidence(&trigger, now));
    if ctx.config.mode == right_agent_config::CuratorMode::ReportOnly {
        run_report_only_pass(&ctx, &mut state, &trigger, now).await;
        return;
    }

    let skills_dir = ctx.agent_dir.join(".claude/skills");
    let backups_dir = ctx.agent_dir.join("curator_backups");
    let now_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    if let Err(e) = crate::lifecycle::snapshot::snapshot_skills(&skills_dir, &backups_dir, &now_str)
    {
        tracing::warn!(agent = %ctx.agent_name, "curator snapshot failed: {e:#}");
    }

    // A1: snapshot which skills are already archived BEFORE this pass mutates
    // anything, so we can attribute exactly what THIS pass archived/merged.
    // (Auto-transitions stamp archived_at = now; the fork stamps its own time,
    let pre_pass_archived: std::collections::HashSet<String> = ctx
        .internal_client
        .curator_archived_snapshot(&right_mcp::internal_db::CuratorArchivedSnapshotRequest {
            agent: ctx.agent_name.clone(),
        })
        .await
        .map(|response| {
            response
                .skills
                .into_iter()
                .map(|skill| skill.skill_name)
                .collect()
        })
        .unwrap_or_default();
    // so a timestamp-equality query would miss fork consolidations.)
    let transitions = match ctx
        .internal_client
        .curator_apply_transitions(&right_mcp::internal_db::CuratorApplyTransitionsRequest {
            agent: ctx.agent_name.clone(),
            now_rfc3339: now.to_rfc3339(),
            stale_after_days: ctx.config.stale_after_days,
            archive_after_days: ctx.config.archive_after_days,
        })
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator owner transitions failed: {error:#}");
            return;
        }
    };
    let transition_changes = transitions.transition_changes;
    let archived_skill_names = transitions.archived_this_pass;
    let lifecycle_rows = match transitions
        .candidates
        .into_iter()
        .map(lifecycle_from_dto)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "curator candidate decode failed: {error}");
            return;
        }
    };
    tracing::info!(agent = %ctx.agent_name, transitions = transition_changes, trigger = ?trigger, "curator auto-transitions applied");
    tracing::info!(
        agent = %ctx.agent_name,
        transitions = transition_changes,
        trigger = ?trigger,
        "curator auto-transitions applied"
    );

    // LLM consolidation fork.
    let sandbox = match crate::cc::invocation::guard_no_sandboxed_host_exec(
        &ctx.agent_name,
        ctx.sandbox.as_ref(),
    ) {
        Ok(sandbox) => std::sync::Arc::clone(sandbox),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "skipping curator: {e:#}");
            return;
        }
    };
    let active_invocation = match crate::cc::invocation::register_non_foreground_invocation(
        crate::cc::invocation::NonForegroundInvocationRegistration {
            agent_name: ctx.agent_name.clone(),
            agent_dir: ctx.agent_dir.clone(),
            sandbox: std::sync::Arc::clone(&sandbox),
            internal_client: Arc::clone(&ctx.internal_client),
            kind: right_mcp::internal_client::ProgressInvocationKindDto::Curator,
            chat_id: None,
            thread_id: None,
            progress_state: None,
        },
    )
    .await
    {
        Ok(active) => active,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator invocation registration failed: {e:#}");
            mark_failed_run(&mut state, ctx.config, now);
            save_state_ipc(&ctx, &state).await;
            return;
        }
    };
    let invocation = build_curator_invocation(
        &ctx,
        &lifecycle_rows,
        active_invocation.mcp_config_path().to_owned(),
    );
    let args = invocation.into_args();

    let command =
        match crate::cc::invocation::build_claude_command(&args, &ctx.agent_dir, &sandbox).await {
            Ok(command) => command
                .stdout(crate::cc::sandbox_process::Capture::Pipe)
                .stderr(crate::cc::sandbox_process::Capture::Pipe),
            Err(e) => {
                tracing::warn!(agent = %ctx.agent_name, "curator command build failed: {e:#}");
                mark_failed_run(&mut state, ctx.config, now);
                save_state_ipc(&ctx, &state).await;
                active_invocation.cleanup().await;
                return;
            }
        };

    let mut usage_for_run: Option<right_agent::usage::UsageBreakdown> = None;

    let run_status = match command.spawn().await {
        Ok(child) => {
            match crate::cc::invocation::wait_with_output_or_kill(child, CURATOR_TIMEOUT).await {
                Ok(crate::cc::invocation::ChildOutput::Completed(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                    if let Some(b) = curator_usage_from_stdout(&stdout) {
                        if let Err(error) = ctx
                            .internal_client
                            .usage_insert_event(&right_mcp::internal_db::UsageInsertEventRequest {
                                agent: ctx.agent_name.clone(),
                                request_id: crate::db::request_id(),
                                source: right_mcp::internal_db::UsageSourceDto::LearningCurator,
                                event: crate::db::usage_dto(&b),
                            })
                            .await
                        {
                            tracing::warn!(agent = %ctx.agent_name, "curator owner usage insert failed: {error:#}");
                        }
                        usage_for_run = Some(b);
                    }
                    if output.success() {
                        "success".to_owned()
                    } else {
                        tracing::warn!(
                            agent = %ctx.agent_name,
                            exit_code = output.code,
                            "curator exited non-zero"
                        );
                        "failed".to_owned()
                    }
                }
                Ok(crate::cc::invocation::ChildOutput::TimedOut) => {
                    tracing::warn!(
                        agent = %ctx.agent_name,
                        "curator timed out after {}s",
                        CURATOR_TIMEOUT.as_secs()
                    );
                    "failed".to_owned()
                }
                Err(e) => {
                    tracing::warn!(agent = %ctx.agent_name, "curator wait failed: {e:#}");
                    "failed".to_owned()
                }
            }
        }
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "curator spawn failed: {e:#}");
            "failed".to_owned()
        }
    };
    active_invocation.cleanup().await;

    if run_status == "success" {
        state.last_run_at = Some(now.to_rfc3339());
        state.last_run_status = Some(run_status.clone());
        state.consecutive_failures = 0;
        state.circuit_open_until = None;
    } else {
        mark_failed_run(&mut state, ctx.config, now);
    }
    // Retry once on transient DbError (BUSY/BUSY_SNAPSHOT). The save is a
    // cheap UPSERT on a singleton row and idempotent, so re-running it is
    // safe. Silently dropping this write would lose circuit-breaker
    // accounting, which ARCHITECTURE.md documents as a load-bearing gate.
    let post_pass_archived = ctx
        .internal_client
        .curator_archived_snapshot(&right_mcp::internal_db::CuratorArchivedSnapshotRequest {
            agent: ctx.agent_name.clone(),
        })
        .await
        .map(|response| response.skills)
        .unwrap_or_default();
    let this_pass: Vec<_> = post_pass_archived
        .into_iter()
        .filter(|skill| !pre_pass_archived.contains(&skill.skill_name))
        .collect();
    let consolidations = this_pass
        .iter()
        .filter(|skill| skill.absorbed_into.is_some())
        .count() as i64;
    let archives = this_pass.len() as i64;
    let actions_json = serde_json::to_string(
        &this_pass
            .iter()
            .map(|skill| {
                serde_json::json!({
                    "kind": if skill.absorbed_into.is_some() { "merge" } else { "archive" },
                    "skills": [skill.skill_name.clone()], "target": skill.absorbed_into,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_owned());
    let (cost_usd, cache_read, cache_creation) = usage_triple(usage_for_run.as_ref());
    let record = CuratorRunRecord {
        run_at: now.to_rfc3339(),
        trigger: trigger_label(&trigger).to_owned(),
        trigger_evidence_json: state.last_spike_evidence_json.clone(),
        mode: "apply".to_owned(),
        status: run_status,
        cost_usd,
        cache_read,
        cache_creation,
        consolidations,
        archives,
        summary: Some(format!("merged {consolidations}, archived {archives}")),
        actions_json,
        invocation_id: None,
    };
    let maintain_spend_entries = usage_for_run
        .as_ref()
        .map(|usage| {
            archived_skill_names
                .iter()
                .map(|skill_name| right_mcp::internal_db::SkillSpendDto {
                    skill_name: skill_name.clone(),
                    kind: "maintain".to_owned(),
                    cost_usd: if archived_skill_names.is_empty() {
                        0.0
                    } else {
                        usage.total_cost_usd / archived_skill_names.len() as f64
                    },
                    cache_read: if archived_skill_names.is_empty() {
                        0
                    } else {
                        usage.cache_read_tokens as i64 / archived_skill_names.len() as i64
                    },
                    cache_creation: if archived_skill_names.is_empty() {
                        0
                    } else {
                        usage.cache_creation_tokens as i64 / archived_skill_names.len() as i64
                    },
                    invocation_id: None,
                })
                .collect()
        })
        .unwrap_or_default();
    if let Err(error) = ctx
        .internal_client
        .curator_finalize(&right_mcp::internal_db::CuratorFinalizeRequest {
            agent: ctx.agent_name.clone(),
            request_id: crate::db::request_id(),
            state: state_to_dto(&state),
            run_record: run_record_to_dto(record),
            maintain_spend_entries,
        })
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "curator owner finalize failed: {error:#}");
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

async fn run_report_only_pass(
    ctx: &CuratorContext,
    state: &mut CuratorState,
    trigger: &CuratorTrigger,
    now: DateTime<Utc>,
) {
    let lifecycle_rows = match ctx
        .internal_client
        .skill_lifecycle_list(&right_mcp::internal_db::SkillLifecycleListRequest {
            agent: ctx.agent_name.clone(),
        })
        .await
    {
        Ok(response) => response
            .rows
            .into_iter()
            .filter_map(|dto| lifecycle_from_dto(dto).ok())
            .collect::<Vec<_>>(),
        Err(error) => {
            tracing::warn!(agent = %ctx.agent_name, "report-only owner candidates failed: {error:#}");
            return;
        }
    };
    let actions_json = "[]".to_owned();
    let record = CuratorRunRecord {
        run_at: now.to_rfc3339(),
        trigger: trigger_label(trigger).to_owned(),
        trigger_evidence_json: state.last_spike_evidence_json.clone(),
        mode: "report_only".to_owned(),
        status: "proposed".to_owned(),
        cost_usd: 0.0,
        cache_read: 0,
        cache_creation: 0,
        consolidations: 0,
        archives: 0,
        summary: Some(format!("{} candidates", lifecycle_rows.len())),
        actions_json,
        invocation_id: None,
    };
    state.last_run_at = Some(now.to_rfc3339());
    state.last_run_status = Some("proposed".to_owned());
    if let Err(error) = ctx
        .internal_client
        .curator_finalize(&right_mcp::internal_db::CuratorFinalizeRequest {
            agent: ctx.agent_name.clone(),
            request_id: crate::db::request_id(),
            state: state_to_dto(state),
            run_record: run_record_to_dto(record),
            maintain_spend_entries: Vec::new(),
        })
        .await
    {
        tracing::warn!(agent = %ctx.agent_name, "report-only owner finalize failed: {error:#}");
    }
}

fn build_report_only_invocation(
    ctx: &CuratorContext,
    lifecycle_rows: &[right_lifecycle::SkillLifecycleRow],
    mcp_config_path: String,
) -> crate::cc::invocation::ClaudeInvocation {
    build_curator_pass_invocation(
        ctx,
        lifecycle_rows,
        mcp_config_path,
        right_codegen::CURATOR_REPORT_PROMPT,
        Some(right_codegen::CURATOR_PLAN_SCHEMA.to_owned()),
        vec!["Read".into()],
    )
}

/// Shared `ClaudeInvocation` builder for both curator passes. The apply pass and
/// the report-only pass differ only in system prompt, structured-output schema,
/// and the tool allowlist; session id, model, turn cap, and stream-json format
/// are identical, so they live here rather than being duplicated per pass.
fn build_curator_pass_invocation(
    ctx: &CuratorContext,
    lifecycle_rows: &[right_lifecycle::SkillLifecycleRow],
    mcp_config_path: String,
    system_prompt: &str,
    json_schema: Option<String>,
    allowed_tools: Vec<String>,
) -> crate::cc::invocation::ClaudeInvocation {
    use crate::cc::invocation::{ClaudeInvocation, OutputFormat};
    let session_id = uuid::Uuid::new_v4().to_string();
    let user_prompt = format!(
        "{system_prompt}\n\n{candidates}",
        candidates = render_candidate_list(lifecycle_rows),
    );
    ClaudeInvocation {
        mcp_config_path: Some(mcp_config_path),
        json_schema,
        output_format: OutputFormat::StreamJson,
        model: Some(ctx.model.clone()),
        max_budget_usd: None,
        max_turns: Some(CURATOR_MAX_TURNS),
        resume_session_id: None,
        new_session_id: Some(session_id),
        fork_session: false,
        allowed_tools,
        disallowed_tools: vec![],
        extra_args: vec![],
        prompt: Some(user_prompt),
        debug_flag: Some(Arc::clone(&ctx.debug_flag)),
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct CuratorPlanAction {
    pub kind: String,
    pub skills: Vec<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct CuratorPlan {
    pub actions: Vec<CuratorPlanAction>,
}

/// Parse the usage breakdown from a curator fork's multi-line stream-json
/// stdout. `parse_usage_full` expects a single result-line object, so extract
/// the terminal result line first. (Passing full NDJSON directly always fails.)
fn curator_usage_from_stdout(stdout: &str) -> Option<right_agent::usage::UsageBreakdown> {
    let line = crate::cc::stream::last_result_line(stdout)?;
    crate::cc::stream::parse_usage_full(&line)
}

/// Flatten an optional usage breakdown into the `(cost_usd, cache_read,
/// cache_creation)` triple persisted in a `curator_runs` row. Absent usage maps
/// to zeros.
fn usage_triple(usage: Option<&right_agent::usage::UsageBreakdown>) -> (f64, i64, i64) {
    usage
        .map(|b| {
            (
                b.total_cost_usd,
                b.cache_read_tokens as i64,
                b.cache_creation_tokens as i64,
            )
        })
        .unwrap_or((0.0, 0, 0))
}

/// Parse the structured curator plan from a report-only fork's stdout. Reads the
/// terminal result line and prefers `structured_output`, falling back to
/// `result` (same convention as `worker_reply::parse_reply_output`).
pub(crate) fn parse_curator_plan(stdout: &str) -> Option<CuratorPlan> {
    let line = crate::cc::stream::last_result_line(stdout)?;
    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    let plan_val = v
        .get("structured_output")
        .filter(|x| !x.is_null())
        .or_else(|| v.get("result"))?;
    serde_json::from_value::<CuratorPlan>(plan_val.clone()).ok()
}

fn build_curator_invocation(
    ctx: &CuratorContext,
    lifecycle_rows: &[right_lifecycle::SkillLifecycleRow],
    mcp_config_path: String,
) -> crate::cc::invocation::ClaudeInvocation {
    build_curator_pass_invocation(
        ctx,
        lifecycle_rows,
        mcp_config_path,
        right_codegen::CURATOR_SYSTEM_PROMPT,
        None,
        vec![
            "Read".into(),
            "Bash".into(),
            "mcp__right__skill_learning_start".into(),
            "mcp__right__skill_learning_finish".into(),
        ],
    )
}

fn render_candidate_list(lifecycle_rows: &[right_lifecycle::SkillLifecycleRow]) -> String {
    use std::fmt::Write;
    let mut s = String::from("<inventory>\n");
    for r in lifecycle_rows {
        let latest_activity = right_lifecycle::latest_activity_at(r)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_else(|| "none".to_owned());
        let _ = writeln!(
            s,
            "- {name}: state={state:?} pinned={pinned} use_count={used} patch_count={patched} latest_activity={latest_activity} created_by={by:?}",
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

/// Maintain cron→skill links for skills the curator just archived: redirect to
/// the successor when absorbed, otherwise drop. Best-effort cleanup — the
/// runtime read (`list_live_for_job`) filters archived skills at read time, so
/// a failure here leaves no correctness gap; we warn and continue rather than
/// aborting the curator pass over link bookkeeping.
#[cfg(test)]
async fn maintain_cron_links_for_archived(
    conn: &right_db::Connection,
    archived_skill_names: &[String],
) {
    for skill in archived_skill_names {
        let absorbed_into: Option<String> = match conn
            .query_row(
                "SELECT absorbed_into FROM skill_lifecycle WHERE skill_name = ?1",
                right_db::params![skill.as_str()],
                |r| r.get::<_, Option<String>>(0),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(skill = %skill, "absorbed_into lookup failed: {e:#}");
                continue;
            }
        };
        let res = match absorbed_into {
            Some(ref target) => {
                right_agent::cron_skill_link::redirect_skill(conn, skill, target).await
            }
            None => right_agent::cron_skill_link::drop_skill(conn, skill).await,
        };
        if let Err(e) = res {
            tracing::warn!(skill = %skill, "cron link maintenance failed: {e:#}");
        }
    }
}

#[cfg(test)]
#[path = "learning_curator_curator_lifecycle_tests.rs"]
mod curator_lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_test_conn() -> right_db::Connection {
        let conn = right_db::Connection::open_in_memory().await.unwrap();
        right_db::MIGRATIONS.to_latest(&conn).await.unwrap();
        conn
    }

    #[tokio::test]
    async fn insert_curator_run_round_trips() {
        let conn = open_test_conn().await;
        let rec = CuratorRunRecord {
            run_at: "2026-06-15T00:00:00Z".into(),
            trigger: "time_fallback".into(),
            trigger_evidence_json: Some("{\"trigger\":\"time_fallback\"}".into()),
            mode: "apply".into(),
            status: "success".into(),
            cost_usd: 0.42,
            cache_read: 10,
            cache_creation: 5,
            consolidations: 1,
            archives: 2,
            summary: Some("merged 1, archived 2".into()),
            actions_json: "[]".into(),
            invocation_id: Some("inv-1".into()),
        };
        insert_curator_run(&conn, &rec).await.unwrap();
        let (trigger, status, archives): (String, String, i64) = conn
            .query_row(
                "SELECT trigger, status, archives FROM curator_runs WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .await
            .unwrap();
        assert_eq!(
            (trigger.as_str(), status.as_str(), archives),
            ("time_fallback", "success", 2)
        );
    }

    #[tokio::test]
    async fn db_load_state_returns_default_when_empty() {
        let conn = open_test_conn().await;
        let s = load_state_db(&conn).await.unwrap();
        assert!(s.last_run_at.is_none());
        assert_eq!(s.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn db_save_then_load_round_trip() {
        let conn = open_test_conn().await;
        let s = CuratorState {
            last_run_at: Some("2026-05-22T00:00:00Z".to_owned()),
            last_run_status: Some("success".to_owned()),
            consecutive_failures: 2,
            circuit_open_until: None,
            last_spike_evidence_json: Some(r#"{"trigger":"cost_spike"}"#.to_owned()),
        };
        save_state_db(&conn, &s).await.unwrap();
        let loaded = load_state_db(&conn).await.unwrap();
        assert_eq!(loaded, s);
    }

    #[tokio::test]
    async fn db_save_replaces_existing_row() {
        let conn = open_test_conn().await;
        save_state_db(
            &conn,
            &CuratorState {
                last_run_at: Some("a".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        save_state_db(
            &conn,
            &CuratorState {
                last_run_at: Some("b".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curator_state", [], |r| r.get(0))
            .await
            .unwrap();
        assert_eq!(count, 1);
        let loaded = load_state_db(&conn).await.unwrap();
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
            circuit_failure_threshold: 3,
            circuit_cooldown_hours: 24,
            mode: right_agent_config::CuratorMode::Apply,
        }
    }

    fn context(agent_dir: PathBuf) -> CuratorContext {
        CuratorContext {
            agent_db_dir: agent_dir.clone(),
            agent_dir,
            agent_name: "agent-1".into(),
            sandbox: None,
            internal_client: Arc::new(right_mcp::internal_client::InternalClient::new(
                "/tmp/fake.sock",
            )),
            model: "claude-sonnet-4-5".into(),
            debug_flag: Arc::new(AtomicBool::new(false)),
            session_locks: SessionLocks::default(),
            config: cfg(),
        }
    }

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[tokio::test]
    async fn background_invocation_curator_uses_invocation_scoped_mcp_config() {
        let dir = tempdir().unwrap();
        let ctx = context(dir.path().to_path_buf());
        let mcp_path = dir
            .path()
            .join(".claude")
            .join("mcp-inv-1.json")
            .to_string_lossy()
            .into_owned();

        let invocation = build_curator_invocation(&ctx, &[], mcp_path);

        let path = invocation
            .mcp_config_path
            .expect("curator should pass an MCP config path");
        assert!(
            path.contains("/.claude/mcp-"),
            "curator path should include a generated invocation id: {path}"
        );
        assert!(
            !path.ends_with("/mcp.json"),
            "curator must not use the static agent mcp.json: {path}"
        );
    }

    #[tokio::test]
    async fn disabled_skips() {
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

    #[tokio::test]
    async fn paused_skips() {
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

    #[tokio::test]
    async fn circuit_open_in_future_skips() {
        let s = CuratorState {
            circuit_open_until: Some("2027-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(
            should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0),
            CuratorGateDecision::SkipCircuitOpen
        );
    }

    #[tokio::test]
    async fn cooldown_blocks_all_triggers() {
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

    #[tokio::test]
    async fn cost_spike_fires_after_cooldown() {
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

    #[tokio::test]
    async fn skill_change_count_fires_when_no_cost_spike() {
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

    #[tokio::test]
    async fn time_fallback_fires_when_no_other_trigger() {
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

    #[tokio::test]
    async fn no_trigger_no_run() {
        let s = CuratorState {
            last_run_at: Some("2026-05-21T00:00:00Z".into()),
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
        // last_run_at 24h ago; cooldown 12h passed; no spike; no change-count; not 168h yet
        assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
    }

    #[tokio::test]
    async fn first_ever_run_defers() {
        let s = CuratorState {
            last_run_at: None,
            ..Default::default()
        };
        let d = should_run_now(cfg(), &s, dt("2026-05-22T00:00:00Z"), None, None, 0);
        assert_eq!(d, CuratorGateDecision::SkipNoTrigger);
    }

    #[tokio::test]
    async fn chat_active_within_min_idle_skips() {
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

    #[tokio::test]
    async fn maintain_links_redirects_absorbed_and_drops_retired() {
        let conn = open_test_conn().await;
        // Insert a cron job and two archived skills: one absorbed, one plain-retired.
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('j','17 9 * * *','x',2.0,'t','t')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at, absorbed_into) \
             VALUES ('rightx-absorbed','archived','cron','t','rightx-successor')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at) \
             VALUES ('rightx-successor','active','cron','t')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at) \
             VALUES ('rightx-retired','archived','cron','t')",
            (),
        )
        .await
        .unwrap();
        right_agent::cron_skill_link::link_auto(
            &conn,
            "j",
            &["rightx-absorbed".to_string(), "rightx-retired".to_string()],
        )
        .await
        .unwrap();

        maintain_cron_links_for_archived(
            &conn,
            &["rightx-absorbed".to_string(), "rightx-retired".to_string()],
        )
        .await;

        let links = right_agent::cron_skill_link::list_for_job(&conn, "j")
            .await
            .unwrap();
        assert_eq!(
            links,
            vec!["rightx-successor".to_string()],
            "absorbed skill redirected to successor, retired skill dropped"
        );
    }

    #[test]
    fn mark_failed_run_opens_circuit_at_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let mut s = CuratorState {
            consecutive_failures: 2,
            ..Default::default()
        };
        mark_failed_run(&mut s, cfg(), now);
        assert_eq!(s.consecutive_failures, 3);
        assert_eq!(
            s.circuit_open_until.as_deref(),
            Some((now + Duration::hours(24)).to_rfc3339().as_str())
        );
        assert_eq!(s.last_run_status.as_deref(), Some("failed"));
    }

    #[test]
    fn mark_failed_run_keeps_circuit_closed_below_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let mut s = CuratorState::default();
        mark_failed_run(&mut s, cfg(), now);
        assert_eq!(s.consecutive_failures, 1);
        assert_eq!(s.circuit_open_until, None);
    }

    #[test]
    fn circuit_stays_closed_below_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        assert_eq!(next_circuit_open_until(2, 3, 24, now), None);
    }

    #[test]
    fn circuit_opens_at_threshold() {
        let now = dt("2026-05-22T00:00:00Z");
        let got = next_circuit_open_until(3, 3, 24, now).unwrap();
        assert_eq!(got, now + Duration::hours(24));
    }

    #[test]
    fn circuit_stays_open_above_threshold_fixed_cooldown() {
        let now = dt("2026-05-22T00:00:00Z");
        let got = next_circuit_open_until(5, 3, 24, now).unwrap();
        assert_eq!(got, now + Duration::hours(24));
    }

    #[cfg(any())]
    #[tokio::test]
    async fn idle_gate_uses_archived_message_activity_across_restart() {
        // Regression for #135: startup seeds the delivery idle timestamp to
        // now, but the curator must derive its gate from persistent archived
        // messages. A recent archived message blocks an otherwise-due curator
        // pass; an old archived message allows its lifecycle transition.
        let dir = tempdir().unwrap();
        let conn = right_db::open_connection(dir.path(), true).await.unwrap();
        let last_run_at = (Utc::now() - Duration::days(8)).to_rfc3339();
        save_state_db(
            &conn,
            &CuratorState {
                last_run_at: Some(last_run_at),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(latest_chat_activity(&conn).await.unwrap(), None);
        conn.execute(
            "INSERT INTO skill_lifecycle (skill_name, state, created_by, created_at, last_used_at) \
             VALUES ('rightx-stale', 'active', 'foreground', '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO conversation_messages (chat_id, role, content, created_at) \
             VALUES (1, 'user', 'recent', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        run_if_due(context(dir.path().to_path_buf())).await;
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        let state_after_recent: String = conn
            .query_row(
                "SELECT state FROM skill_lifecycle WHERE skill_name = 'rightx-stale'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(state_after_recent, "active");

        conn.execute(
            "UPDATE conversation_messages SET created_at = '2020-01-01T00:00:00Z'",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        run_if_due(context(dir.path().to_path_buf())).await;
        let conn = right_db::open_connection(dir.path(), false).await.unwrap();
        let state_after_old: String = conn
            .query_row(
                "SELECT state FROM skill_lifecycle WHERE skill_name = 'rightx-stale'",
                (),
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(state_after_old, "archived");
    }

    #[test]
    fn curator_usage_from_multiline_stdout_extracts_cost() {
        let stdout = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n",
            "{\"type\":\"result\",\"total_cost_usd\":0.123,\"num_turns\":2,\"session_id\":\"s1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":7,\"cache_creation_input_tokens\":3}}\n",
        );

        let b = curator_usage_from_stdout(stdout).expect("usage parsed from result line");
        assert!((b.total_cost_usd - 0.123).abs() < 1e-9);
        assert_eq!(b.cache_read_tokens, 7);
        assert_eq!(b.cache_creation_tokens, 3);
    }
    #[tokio::test]
    async fn malformed_archived_activity_timestamp_fails_closed() {
        let conn = open_test_conn().await;
        conn.execute(
            "INSERT INTO conversation_messages (chat_id, role, content, created_at) \
             VALUES (1, 'user', 'bad timestamp', 'not-a-timestamp')",
            (),
        )
        .await
        .unwrap();

        assert!(latest_chat_activity(&conn).await.is_err());
    }

    #[test]
    fn curator_usage_from_stdout_none_without_result_line() {
        assert!(curator_usage_from_stdout("{\"type\":\"system\"}\n").is_none());
    }

    #[test]
    fn parse_curator_plan_extracts_actions_from_result_line() {
        let stdout = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
            "{\"type\":\"result\",\"structured_output\":{\"actions\":[{\"kind\":\"merge\",\"skills\":[\"rightx-a\",\"rightx-b\"],\"target\":\"rightx-u\",\"rationale\":\"dupes\"}]}}\n",
        );
        let plan = parse_curator_plan(stdout).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].kind, "merge");
        assert_eq!(plan.actions[0].target.as_deref(), Some("rightx-u"));
    }

    #[test]
    fn parse_curator_plan_none_when_no_result() {
        assert!(parse_curator_plan("{\"type\":\"system\"}\n").is_none());
    }
}
