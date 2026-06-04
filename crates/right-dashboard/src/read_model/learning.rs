use crate::api_types::{
    DashboardDataWarning, LearningCapabilities, LearningEventSummary, LearningFlowEdge,
    LearningFlowNode, LearningLifecycle, LearningOverviewResponse, LearningSignalPoint,
};
use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use right_db::{Connection, OptionalExtension, params};

use super::{
    ReadModelError, coarse_timestamp_bounds, count_parsed_window_rows,
    learning_outcomes::{learning_outcome_kind, learning_outcome_severity},
    parse_utc,
};

const RECENT_EVENT_LIMIT: i64 = 10;
const RECENT_LEARNING_SIGNAL_LIMIT: usize = 30;
const CANDIDATE_NAME_LIMIT: i64 = 20;

pub struct LearningOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
}

fn learning_capabilities() -> LearningCapabilities {
    LearningCapabilities {
        learning_metrics: true,
        learning_evidence_snippets: false,
        learning_commands: false,
    }
}

pub async fn learning_overview(
    conn: &Connection,
    input: LearningOverviewInput,
) -> Result<LearningOverviewResponse, ReadModelError> {
    let generated_at_utc = parse_utc(&input.generated_at)?;
    let since_7d_utc = generated_at_utc - Duration::days(7);
    let agent_name = input.agent;
    let generated_at = input.generated_at;
    let agent = agent_name.as_str();

    let lifecycle = learning_lifecycle(conn, agent, &since_7d_utc, &generated_at_utc).await?;
    let (flow_counts, mut warnings) =
        learning_flow_counts(conn, agent, &since_7d_utc, &generated_at_utc).await?;
    let flow_nodes = learning_flow_nodes(&flow_counts);
    let (flow_edges, partial_flow) = learning_flow_edges(&flow_nodes, &flow_counts);
    let recent_learning_signals =
        recent_learning_signals(conn, agent, &since_7d_utc, &generated_at_utc).await?;
    if partial_flow {
        warnings.push(DashboardDataWarning {
            source: "learning_flow".to_owned(),
            kind: "partial_data".to_owned(),
            message: "writer transitions are aggregate-only and partially inferred".to_owned(),
        });
    }
    Ok(LearningOverviewResponse {
        agent: agent_name,
        generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
        capabilities: learning_capabilities(),
        lifecycle,
        flow_nodes,
        flow_edges,
        recent_learning_signals,
        warnings,
    })
}

struct LearningFlowCounts {
    signals: i64,
    curator_triggered: i64,
    writer_applied_as_hinted: i64,
    writer_applied_differently: i64,
    writer_refused: i64,
    writer_failed: i64,
    skill_created: i64,
    skill_updated: i64,
    applied_as_hinted_created: i64,
    applied_as_hinted_updated: i64,
    applied_differently_created: i64,
    applied_differently_updated: i64,
}

async fn learning_flow_counts(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(LearningFlowCounts, Vec<DashboardDataWarning>), ReadModelError> {
    let (curator_triggered, curator_warning) =
        curator_trigger_count_in_window(conn, since, now).await?;
    let mut warnings = Vec::new();
    if let Some(warning) = curator_warning {
        warnings.push(warning);
    }

    Ok((
        LearningFlowCounts {
            signals: writer_finish_count_in_window(conn, agent, since, now).await?,
            curator_triggered,
            writer_applied_as_hinted: writer_hint_outcome_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                since,
                now,
            )
            .await?,
            writer_applied_differently: writer_hint_outcome_count_in_window(
                conn,
                agent,
                "applied_differently",
                since,
                now,
            )
            .await?,
            writer_refused: writer_hint_outcome_count_in_window(conn, agent, "refused", since, now)
                .await?,
            writer_failed: failed_writer_flow_count_in_window(conn, agent, since, now).await?,
            skill_created: writer_status_count_in_window(conn, agent, "created", since, now)
                .await?,
            skill_updated: writer_status_count_in_window(conn, agent, "updated", since, now)
                .await?,
            applied_as_hinted_created: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                "created",
                since,
                now,
            )
            .await?,
            applied_as_hinted_updated: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                "updated",
                since,
                now,
            )
            .await?,
            applied_differently_created: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_differently",
                "created",
                since,
                now,
            )
            .await?,
            applied_differently_updated: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_differently",
                "updated",
                since,
                now,
            )
            .await?,
        },
        warnings,
    ))
}

fn learning_flow_nodes(counts: &LearningFlowCounts) -> Vec<LearningFlowNode> {
    vec![
        LearningFlowNode {
            id: "signals".to_owned(),
            label: "Learning events".to_owned(),
            kind: "signal".to_owned(),
            count: counts.signals,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "curator_triggered".to_owned(),
            label: "Curator triggered".to_owned(),
            kind: "curator".to_owned(),
            count: counts.curator_triggered,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_applied_as_hinted".to_owned(),
            label: "Applied as hinted".to_owned(),
            kind: "writer".to_owned(),
            count: counts.writer_applied_as_hinted,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_applied_differently".to_owned(),
            label: "Applied differently".to_owned(),
            kind: "writer".to_owned(),
            count: counts.writer_applied_differently,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_refused".to_owned(),
            label: "Refused".to_owned(),
            kind: "writer".to_owned(),
            count: counts.writer_refused,
            severity: "warn".to_owned(),
        },
        LearningFlowNode {
            id: "writer_failed".to_owned(),
            label: "Failed or aborted".to_owned(),
            kind: "writer".to_owned(),
            count: counts.writer_failed,
            severity: "bad".to_owned(),
        },
        LearningFlowNode {
            id: "skill_created".to_owned(),
            label: "Skills created".to_owned(),
            kind: "skill".to_owned(),
            count: counts.skill_created,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "skill_updated".to_owned(),
            label: "Skills patched".to_owned(),
            kind: "skill".to_owned(),
            count: counts.skill_updated,
            severity: "info".to_owned(),
        },
    ]
}

fn learning_flow_edges(
    nodes: &[LearningFlowNode],
    counts: &LearningFlowCounts,
) -> (Vec<LearningFlowEdge>, bool) {
    let count = |id: &str| -> i64 {
        nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.count)
            .unwrap_or(0)
    };
    let mut edges = Vec::new();
    let mut signal_budget = count("signals");
    let mut curator_budget = count("curator_triggered");
    let mut writer_remaining = BTreeMap::<&str, i64>::from([
        (
            "writer_applied_as_hinted",
            count("writer_applied_as_hinted"),
        ),
        (
            "writer_applied_differently",
            count("writer_applied_differently"),
        ),
        ("writer_refused", count("writer_refused")),
        ("writer_failed", count("writer_failed")),
    ]);
    let mut represented_writers = 0;

    for target in [
        "writer_applied_as_hinted",
        "writer_applied_differently",
        "writer_refused",
        "writer_failed",
    ] {
        attach_flow_edge(
            &mut edges,
            "curator_triggered",
            target,
            &mut curator_budget,
            &mut writer_remaining,
            &mut represented_writers,
        );
    }

    for target in [
        "writer_applied_as_hinted",
        "writer_applied_differently",
        "writer_refused",
        "writer_failed",
    ] {
        attach_flow_edge(
            &mut edges,
            "signals",
            target,
            &mut signal_budget,
            &mut writer_remaining,
            &mut represented_writers,
        );
    }

    let mut skill_remaining = BTreeMap::<&str, i64>::from([
        ("skill_created", count("skill_created")),
        ("skill_updated", count("skill_updated")),
    ]);
    let mut represented_skills = 0;
    let mut hinted_budget = count("writer_applied_as_hinted");
    attach_flow_edge_with_cap(
        &mut edges,
        "writer_applied_as_hinted",
        "skill_created",
        &mut hinted_budget,
        &mut skill_remaining,
        counts.applied_as_hinted_created,
        &mut represented_skills,
    );
    attach_flow_edge_with_cap(
        &mut edges,
        "writer_applied_as_hinted",
        "skill_updated",
        &mut hinted_budget,
        &mut skill_remaining,
        counts.applied_as_hinted_updated,
        &mut represented_skills,
    );
    let mut different_budget = count("writer_applied_differently");
    attach_flow_edge_with_cap(
        &mut edges,
        "writer_applied_differently",
        "skill_created",
        &mut different_budget,
        &mut skill_remaining,
        counts.applied_differently_created,
        &mut represented_skills,
    );
    attach_flow_edge_with_cap(
        &mut edges,
        "writer_applied_differently",
        "skill_updated",
        &mut different_budget,
        &mut skill_remaining,
        counts.applied_differently_updated,
        &mut represented_skills,
    );

    let writer_total = count("writer_applied_as_hinted")
        + count("writer_applied_differently")
        + count("writer_refused")
        + count("writer_failed");
    let skill_total = count("skill_created") + count("skill_updated");
    (
        edges,
        writer_total > represented_writers || skill_total > represented_skills,
    )
}

fn attach_flow_edge(
    edges: &mut Vec<LearningFlowEdge>,
    source: &str,
    target: &'static str,
    source_budget: &mut i64,
    target_remaining: &mut BTreeMap<&'static str, i64>,
    represented: &mut i64,
) {
    attach_flow_edge_with_cap(
        edges,
        source,
        target,
        source_budget,
        target_remaining,
        i64::MAX,
        represented,
    );
}

fn attach_flow_edge_with_cap(
    edges: &mut Vec<LearningFlowEdge>,
    source: &str,
    target: &'static str,
    source_budget: &mut i64,
    target_remaining: &mut BTreeMap<&'static str, i64>,
    cap: i64,
    represented: &mut i64,
) {
    let remaining = target_remaining.get(target).copied().unwrap_or(0);
    let value = (*source_budget).min(remaining).min(cap);
    if value <= 0 {
        return;
    }
    edges.push(LearningFlowEdge {
        source: source.to_owned(),
        target: target.to_owned(),
        count: value,
    });
    *source_budget -= value;
    target_remaining.insert(target, remaining - value);
    *represented += value;
}

async fn writer_finish_count_in_window(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            row.get::<_, String>(0)
        })
        .await?;
    count_parsed_window_rows(rows, since, now)
}

async fn writer_status_count_in_window(
    conn: &Connection,
    agent: &str,
    status: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status=?2
           AND created_at >= ?3
           AND created_at <= ?4",
    )?;
    let rows = stmt
        .query_map(params![agent, status, coarse_since, coarse_until], |row| {
            row.get::<_, String>(0)
        })
        .await?;
    count_parsed_window_rows(rows, since, now)
}

async fn writer_hint_outcome_count_in_window(
    conn: &Connection,
    agent: &str,
    hint_outcome: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND hint_outcome=?2
           AND created_at >= ?3
           AND created_at <= ?4",
    )?;
    let rows = stmt
        .query_map(
            params![agent, hint_outcome, coarse_since, coarse_until],
            |row| row.get::<_, String>(0),
        )
        .await?;
    count_parsed_window_rows(rows, since, now)
}

async fn writer_hint_status_count_in_window(
    conn: &Connection,
    agent: &str,
    hint_outcome: &str,
    status: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND hint_outcome=?2
           AND status=?3
           AND created_at >= ?4
           AND created_at <= ?5",
    )?;
    let rows = stmt
        .query_map(
            params![agent, hint_outcome, status, coarse_since, coarse_until],
            |row| row.get::<_, String>(0),
        )
        .await?;
    count_parsed_window_rows(rows, since, now)
}

async fn failed_writer_flow_count_in_window(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('failed','aborted')
           AND COALESCE(hint_outcome, '') <> 'refused'
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            row.get::<_, String>(0)
        })
        .await?;
    count_parsed_window_rows(rows, since, now)
}

async fn curator_trigger_count_in_window(
    conn: &Connection,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(i64, Option<DashboardDataWarning>), ReadModelError> {
    let raw = conn
        .query_row(
            "SELECT last_spike_evidence_json
             FROM curator_state
             WHERE agent_singleton_id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .await
        .optional()?
        .flatten();

    let Some(raw) = raw else {
        return Ok((0, None));
    };
    match curator_trigger_evidence_timestamp(&raw) {
        Ok(Some(occurred_at)) if occurred_at >= *since && occurred_at <= *now => Ok((1, None)),
        Ok(_) => Ok((0, None)),
        Err(()) => {
            tracing::warn!(
                source = "curator_state.last_spike_evidence_json",
                "dashboard: skipped malformed curator trigger evidence JSON"
            );
            Ok((
                0,
                Some(DashboardDataWarning {
                    source: "curator_state.last_spike_evidence_json".to_owned(),
                    kind: "malformed_json".to_owned(),
                    message: "skipped malformed curator spike evidence JSON".to_owned(),
                }),
            ))
        }
    }
}

fn curator_trigger_evidence_timestamp(raw: &str) -> Result<Option<DateTime<Utc>>, ()> {
    let value = serde_json::from_str::<serde_json::Value>(raw).map_err(|_| ())?;
    if value
        .get("trigger")
        .and_then(|field| field.as_str())
        .is_none()
    {
        return Ok(None);
    }
    let Some(computed_at) = value.get("computed_at").and_then(|field| field.as_str()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(computed_at)
        .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
        .map_err(|_| ())
}

async fn recent_learning_signals(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningSignalPoint>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT id, action, skill_name, status, hint_outcome, COALESCE(summary, message), created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .await?;
    let mut signals = Vec::<(DateTime<Utc>, i64, LearningSignalPoint)>::new();
    for row in rows {
        let (id, action, skill_name, status, hint_outcome, detail, occurred_at) = row?;
        let occurred_at_utc = parse_utc(&occurred_at)?;
        if occurred_at_utc < *since || occurred_at_utc > *now {
            continue;
        }
        signals.push((
            occurred_at_utc,
            id,
            LearningSignalPoint {
                id: format!("learning:{id}"),
                occurred_at: occurred_at_utc.to_rfc3339(),
                kind: learning_outcome_kind(&action, status.as_deref(), hint_outcome.as_deref())
                    .to_owned(),
                label: skill_name.clone(),
                severity: learning_outcome_severity(status.as_deref(), hint_outcome.as_deref())
                    .to_owned(),
                detail,
                skill_name: Some(skill_name),
                count: 1,
            },
        ));
    }
    signals.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    signals.truncate(RECENT_LEARNING_SIGNAL_LIMIT);
    Ok(signals.into_iter().map(|(_, _, signal)| signal).collect())
}

pub async fn skill_spend_by_skill(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, crate::api_types::SkillSpendAgg>, ReadModelError> {
    let rows = conn
        .query_all(
            "SELECT skill_name, \
               COALESCE(SUM(CASE WHEN kind='create' THEN cost_usd END),0), \
               COALESCE(SUM(CASE WHEN kind IN ('patch','maintain') THEN cost_usd END),0), \
               COALESCE(SUM(CASE WHEN kind='usage' THEN cost_usd END),0), \
               COALESCE(SUM(CASE WHEN kind IN ('create','patch','maintain') THEN cache_read END),0), \
               COALESCE(SUM(CASE WHEN kind IN ('create','patch','maintain') THEN cache_creation END),0) \
             FROM skill_spend GROUP BY skill_name",
            (),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    crate::api_types::SkillSpendAgg {
                        learn_cost_usd: r.get(1)?,
                        fix_cost_usd: r.get(2)?,
                        usage_cost_usd: r.get(3)?,
                        cache_read_tokens: r.get(4)?,
                        cache_creation_tokens: r.get(5)?,
                    },
                ))
            },
        )
        .await?;
    Ok(rows.into_iter().collect())
}

pub async fn skill_lifecycle_overview(
    conn: &Connection,
    agent: &str,
) -> Result<crate::api_types::SkillLifecycleOverviewResponse, ReadModelError> {
    use right_lifecycle::{CreatedBy, LifecycleState};

    let mut total_active = 0;
    let mut total_stale = 0;
    let mut total_archived = 0;
    let mut pinned_count = 0;
    let mut probe_writer_active = 0;
    let mut curator_active = 0;
    let mut foreground_active = 0;
    let mut bundled_active = 0;
    let mut recently_used: Vec<crate::api_types::RecentSkill> = Vec::new();

    let rows = conn
        .query_all(
            "SELECT skill_name, state, pinned, created_by, use_count, last_used_at
         FROM skill_lifecycle",
            (),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .await?;
    for (skill_name, state_raw, pinned, created_by_raw, use_count, last_used_at) in rows {
        let state = LifecycleState::from_db_str(&state_raw).map_err(|_| {
            ReadModelError::InvalidLifecycle(format!(
                "skill {skill_name}: invalid state {state_raw:?}"
            ))
        })?;
        let created_by = CreatedBy::from_db_str(&created_by_raw).map_err(|_| {
            ReadModelError::InvalidLifecycle(format!(
                "skill {skill_name}: invalid created_by {created_by_raw:?}"
            ))
        })?;

        match state {
            LifecycleState::Active => total_active += 1,
            LifecycleState::Stale => total_stale += 1,
            LifecycleState::Archived => total_archived += 1,
        }
        if pinned != 0 {
            pinned_count += 1;
        }
        if state == LifecycleState::Active {
            match created_by {
                CreatedBy::ProbeWriter => probe_writer_active += 1,
                CreatedBy::Curator => curator_active += 1,
                CreatedBy::Foreground => foreground_active += 1,
                CreatedBy::Bundled => bundled_active += 1,
            }
        }
        if use_count > 0 {
            recently_used.push(crate::api_types::RecentSkill {
                package_name: skill_name,
                use_count: use_count as u64,
                last_used_at,
            });
        }
    }
    recently_used.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then_with(|| b.use_count.cmp(&a.use_count))
            .then_with(|| a.package_name.cmp(&b.package_name))
    });
    recently_used.truncate(20);
    let agent_created_active = probe_writer_active + curator_active;

    Ok(crate::api_types::SkillLifecycleOverviewResponse {
        agent: agent.to_owned(),
        total_active,
        total_stale,
        total_archived,
        pinned_count,
        agent_created_active,
        probe_writer_active,
        curator_active,
        foreground_active,
        bundled_active,
        recently_used,
    })
}

async fn learning_lifecycle(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<LearningLifecycle, ReadModelError> {
    let (failed_7d, recent_failed_events) =
        recent_failed_events(conn, agent, since_7d, now).await?;
    let (refused_7d, recent_refused_events) =
        recent_refused_events(conn, agent, since_7d, now).await?;
    Ok(LearningLifecycle {
        created_7d: writer_status_count_in_window(conn, agent, "created", since_7d, now).await?,
        updated_7d: writer_status_count_in_window(conn, agent, "updated", since_7d, now).await?,
        failed_7d: failed_7d as i64,
        refused_7d: refused_7d as i64,
        recent_successful_events: recent_successful_events(conn, agent, since_7d, now).await?,
        recent_failed_events,
        recent_refused_events,
        candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d, now).await?,
    })
}

/// Learning-event rows for `agent` whose `created_at` falls in `[since_7d, now]`,
/// newest first, restricted to the static internal status predicate. Returns
/// the exact count of precise-window matches and the list, optionally truncated
/// to `limit` (applied AFTER counting, so the count is unaffected by the cap).
async fn learning_events_in_window(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
    status_predicate: &str,
    limit: Option<usize>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let sql = format!(
        "SELECT id, skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND ({status_predicate})
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .await?;
    let mut events = Vec::<(DateTime<Utc>, i64, LearningEventSummary)>::new();
    for row in rows {
        let (id, skill_name, action, status, message, summary, created_at) = row?;
        let created_at_utc = parse_utc(&created_at)?;
        if created_at_utc < *since_7d || created_at_utc > *now {
            continue;
        }
        events.push((
            created_at_utc,
            id,
            LearningEventSummary {
                skill_name,
                action,
                status,
                message,
                summary,
                created_at: created_at_utc.to_rfc3339(),
            },
        ));
    }
    events.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let total = events.len();
    if let Some(limit) = limit {
        events.truncate(limit);
    }
    Ok((
        total,
        events.into_iter().map(|(_, _, event)| event).collect(),
    ))
}

async fn recent_successful_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let (_total, events) = learning_events_in_window(
        conn,
        agent,
        since_7d,
        now,
        "status IN ('created','updated')",
        Some(RECENT_EVENT_LIMIT as usize),
    )
    .await?;
    Ok(events)
}

async fn recent_failed_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    // Capped at FAILURE_SAMPLE_LIMIT (newest-first); the returned total is the
    // exact windowed count, which may exceed the list length and feeds
    // `failed_7d`.
    learning_events_in_window(
        conn,
        agent,
        since_7d,
        now,
        "status='failed' OR (status='aborted' AND COALESCE(hint_outcome,'') <> 'refused')",
        Some(super::FAILURE_SAMPLE_LIMIT),
    )
    .await
}

async fn recent_refused_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    learning_events_in_window(
        conn,
        agent,
        since_7d,
        now,
        "status='aborted' AND hint_outcome='refused'",
        Some(super::FAILURE_SAMPLE_LIMIT),
    )
    .await
}

async fn candidate_skill_names(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<String>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let mut stmt = conn.prepare(
        "SELECT skill_name, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .await?;
    let mut newest_by_name = BTreeMap::<String, DateTime<Utc>>::new();
    for row in rows {
        let (name, created_at) = row?;
        let created_at_utc = parse_utc(&created_at)?;
        if created_at_utc < *since_7d || created_at_utc > *now {
            continue;
        }
        newest_by_name
            .entry(name)
            .and_modify(|current| {
                if created_at_utc > *current {
                    *current = created_at_utc;
                }
            })
            .or_insert(created_at_utc);
    }
    let mut rows = newest_by_name.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.truncate(CANDIDATE_NAME_LIMIT as usize);
    Ok(rows.into_iter().map(|(name, _)| name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true)
            .await
            .expect("open db");
        (dir, conn)
    }

    fn input() -> LearningOverviewInput {
        LearningOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            refresh_interval_secs: 5,
        }
    }

    fn flow_node_count(response: &LearningOverviewResponse, id: &str) -> i64 {
        response
            .flow_nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.count)
            .unwrap_or(0)
    }

    fn flow_edge_count(response: &LearningOverviewResponse, source: &str, target: &str) -> i64 {
        response
            .flow_edges
            .iter()
            .find(|edge| edge.source == source && edge.target == target)
            .map(|edge| edge.count)
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn learning_overview_reads_current_sources_without_legacy_tables() {
        let (_dir, conn) = fixture().await;

        for table in [
            "learning_episodes",
            "skill_nudge_signals",
            "skill_nudge_state",
            "skill_review_reports",
            "execution_events",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM sqlite_master
                     WHERE type=\"table\" AND name=?1",
                    params![table],
                    |row| row.get(0),
                )
                .await
                .unwrap();
            assert_eq!(count, 0, "{table} should not exist after v35");
        }

        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES (
                \"inv-current\", \"right\", \"create\", \"rightx-current\", \"finish\",
                \"created\", \"Learned current workflow.\", \"Current workflow.\",
                \"[]\", \"applied_as_hinted\", \"2026-05-20T11:00:00Z\"
             )",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(&conn, input()).await.unwrap();

        assert!(response.recent_learning_signals.iter().any(|signal| {
            signal.skill_name.as_deref() == Some("rightx-current")
                && signal.label == "rightx-current"
        }));
    }

    #[tokio::test]
    async fn learning_overview_builds_current_lifecycle_flow_and_recent_signals() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (
                1, NULL, NULL, 0, NULL,
                '{\"trigger\":\"skill_change_count\",\"computed_at\":\"2026-05-20T09:30:00Z\"}'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                (\"inv-created\", \"right\", \"create\", \"rightx-created\", \"finish\",
                 \"created\", \"created\", \"created\", \"[]\", \"applied_as_hinted\",
                 \"2026-05-20T10:10:00Z\"),
                (\"inv-updated\", \"right\", \"update\", \"rightx-updated\", \"finish\",
                 \"updated\", \"updated\", \"updated\", \"[]\", \"applied_differently\",
                 \"2026-05-20T10:11:00Z\"),
                (\"inv-refused\", \"right\", \"create\", \"rightx-refused\", \"finish\",
                 \"aborted\", \"refused\", \"refused\", \"[]\", \"refused\",
                 \"2026-05-20T10:12:00Z\"),
                (\"inv-failed\", \"right\", \"update\", \"rightx-failed\", \"finish\",
                 \"failed\", \"failed\", \"failed\", \"[]\", NULL,
                 \"2026-05-20T10:13:00Z\")",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(&conn, input()).await.unwrap();

        assert_eq!(response.lifecycle.created_7d, 1);
        assert_eq!(response.lifecycle.updated_7d, 1);
        assert_eq!(response.lifecycle.failed_7d, 1);
        assert_eq!(response.lifecycle.refused_7d, 1);
        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-updated", "rightx-created"]
        );
        assert_eq!(flow_node_count(&response, "signals"), 4);
        assert_eq!(flow_node_count(&response, "curator_triggered"), 1);
        assert_eq!(flow_node_count(&response, "writer_applied_as_hinted"), 1);
        assert_eq!(flow_node_count(&response, "writer_applied_differently"), 1);
        assert_eq!(flow_node_count(&response, "writer_refused"), 1);
        assert_eq!(flow_node_count(&response, "writer_failed"), 1);
        assert_eq!(flow_node_count(&response, "skill_created"), 1);
        assert_eq!(flow_node_count(&response, "skill_updated"), 1);
        assert_eq!(
            flow_edge_count(&response, "writer_applied_as_hinted", "skill_created"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "writer_applied_differently", "skill_updated"),
            1
        );
        assert_eq!(response.recent_learning_signals[0].label, "rightx-failed");
    }

    #[tokio::test]
    async fn learning_lifecycle_excludes_refusals_from_failed_and_counts_them_separately() {
        let (_dir, conn) = fixture().await;
        // 1 genuine failure, 1 genuine abort (not refused), 2 refusals.
        conn.execute(
            "INSERT INTO skill_learning_events
                (invocation_id, agent_name, action, skill_name, phase, status,
                 hint_outcome, message, summary, event_refs_json, created_at)
             VALUES
                ('i1','alpha','update','rightx-a','finish','failed',
                 NULL,'boom',NULL,'[]','2026-06-03T09:00:00Z'),
                ('i2','alpha','update','rightx-b','finish','aborted',
                 NULL,'crash',NULL,'[]','2026-06-03T09:05:00Z'),
                ('i3','alpha','update','rightx-c','finish','aborted',
                 'refused','already covered',NULL,'[]','2026-06-03T09:10:00Z'),
                ('i4','alpha','update','rightx-c','finish','aborted',
                 'refused','already covered','dup','[]','2026-06-03T09:15:00Z')",
            [],
        )
        .await
        .unwrap();

        let now = parse_utc("2026-06-04T10:00:00Z").unwrap();
        let since = now - Duration::days(7);
        let lifecycle = learning_lifecycle(&conn, "alpha", &since, &now)
            .await
            .unwrap();

        assert_eq!(lifecycle.failed_7d, 2);
        assert_eq!(lifecycle.refused_7d, 2);
        assert_eq!(lifecycle.recent_failed_events.len(), 2);
        assert_eq!(
            lifecycle
                .recent_failed_events
                .iter()
                .map(|e| (e.skill_name.as_str(), e.status.as_str()))
                .collect::<Vec<_>>(),
            vec![("rightx-b", "aborted"), ("rightx-a", "failed")]
        );
        assert_eq!(lifecycle.recent_refused_events.len(), 2);
        assert_eq!(
            lifecycle
                .recent_refused_events
                .iter()
                .map(|e| (e.skill_name.as_str(), e.status.as_str()))
                .collect::<Vec<_>>(),
            vec![("rightx-c", "aborted"), ("rightx-c", "aborted")]
        );
    }

    #[tokio::test]
    async fn learning_overview_recent_signals_parse_utc_bounds_and_sort() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                (\"normal-created\", \"right\", \"create\", \"rightx-normal\", \"finish\", \"created\", \"created\", \"created\", \"[]\", NULL, \"2026-05-20T10:00:00Z\"),
                (\"offset-updated\", \"right\", \"update\", \"rightx-offset\", \"finish\", \"updated\", \"updated\", \"updated\", \"[]\", NULL, \"2026-05-20T15:30:00.500+04:00\"),
                (\"future-failed\", \"right\", \"update\", \"rightx-future\", \"finish\", \"failed\", \"future\", \"future\", \"[]\", NULL, \"2026-05-20T11:30:00.000-02:00\"),
                (\"old-created\", \"right\", \"create\", \"rightx-old\", \"finish\", \"created\", \"old\", \"old\", \"[]\", NULL, \"2026-05-13T11:59:59Z\")",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(&conn, input()).await.unwrap();
        let labels = response
            .recent_learning_signals
            .iter()
            .map(|signal| signal.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["rightx-offset", "rightx-normal"]);
        assert_eq!(response.recent_learning_signals[0].kind, "skill_updated");
        assert_eq!(response.recent_learning_signals[0].severity, "ok");
    }

    #[tokio::test]
    async fn recent_learning_signals_include_detail_summary_then_message() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('s1', 'right', 'create', 'rightx-sum', 'finish', 'aborted',
                 'msg-a', 'summary-a', '[]', 'refused', '2026-05-20T10:00:00Z'),
                ('s2', 'right', 'create', 'rightx-msg', 'finish', 'aborted',
                 'msg-b', NULL, '[]', 'refused', '2026-05-20T09:00:00Z')",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(&conn, input()).await.unwrap();

        let sum = response
            .recent_learning_signals
            .iter()
            .find(|s| s.label == "rightx-sum")
            .expect("summary signal present");
        assert_eq!(sum.detail.as_deref(), Some("summary-a"));

        let msg = response
            .recent_learning_signals
            .iter()
            .find(|s| s.label == "rightx-msg")
            .expect("message-fallback signal present");
        assert_eq!(msg.detail.as_deref(), Some("msg-b"));
    }

    #[tokio::test]
    async fn recent_failed_events_includes_failed_and_aborted() {
        let (_dir, conn) = fixture().await;

        // Seed 12 "failed" + 1 "aborted" finish events (13 total, below the
        // FAILURE_SAMPLE_LIMIT of 50, so the full set is returned).
        // All timestamps are within the 7d window of 2026-05-31T12:00:00Z.
        for i in 0..12 {
            let day = if i % 2 == 0 { "30" } else { "31" };
            conn.execute(
                "INSERT INTO skill_learning_events (
                    invocation_id, agent_name, action, skill_name, phase, status,
                    message, summary, event_refs_json, hint_outcome, created_at
                 ) VALUES (?1, 'agent', 'create', ?2, 'finish', 'failed',
                    'fail msg', 'fail summary', '[]', NULL, ?3)",
                right_db::params![
                    format!("inv-fail-{i}"),
                    format!("rightx-skill-{i}"),
                    format!("2026-05-{day}T10:00:00Z"),
                ],
            )
            .await
            .unwrap();
        }
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES ('inv-abort', 'agent', 'create', 'rightx-skill-ab', 'finish', 'aborted',
                'abort msg', 'abort summary', '[]', NULL, '2026-05-31T10:00:00Z')",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(
            &conn,
            LearningOverviewInput {
                agent: "agent".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 5,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            response.lifecycle.recent_failed_events.len() as i64,
            response.lifecycle.failed_7d
        );
        assert!(
            response.lifecycle.recent_failed_events.len() >= 13,
            "expected >= 13 (full set below the 50 cap), got {}",
            response.lifecycle.recent_failed_events.len()
        );
        assert!(
            response
                .lifecycle
                .recent_failed_events
                .iter()
                .any(|e| e.status == "aborted"),
            "expected at least one aborted event"
        );
        assert!(
            response
                .lifecycle
                .recent_failed_events
                .iter()
                .all(|e| e.status == "failed" || e.status == "aborted"),
            "all events must be failed or aborted"
        );
    }

    #[tokio::test]
    async fn skill_spend_agg_buckets_by_kind() {
        let (_dir, conn) = fixture().await;
        for (k, c) in [
            ("create", 0.5),
            ("patch", 0.1),
            ("patch", 0.2),
            ("maintain", 0.4),
            ("usage", 0.9),
        ] {
            conn.execute(
                "INSERT INTO skill_spend (skill_name, kind, cost_usd, cache_read, cache_creation) \
                 VALUES ('rightx-a', ?1, ?2, 5, 7)",
                right_db::params![k, c],
            )
            .await
            .unwrap();
        }
        let map = skill_spend_by_skill(&conn).await.unwrap();
        let a = map.get("rightx-a").unwrap();
        assert!((a.learn_cost_usd - 0.5).abs() < 1e-9);
        assert!((a.fix_cost_usd - 0.7).abs() < 1e-9); // patch + maintain summed
        assert!((a.usage_cost_usd - 0.9).abs() < 1e-9);
        assert_eq!(a.cache_read_tokens, 20); // 4 learning rows * 5 (usage excluded)
        assert_eq!(a.cache_creation_tokens, 28); // 4 learning rows * 7 (usage excluded)
    }

    #[tokio::test]
    async fn recent_failed_events_caps_at_sample_limit_with_true_count() {
        let (_dir, conn) = fixture().await;
        // 51 failed finish events in the 7d window (> FAILURE_SAMPLE_LIMIT = 50).
        for i in 0..51 {
            conn.execute(
                "INSERT INTO skill_learning_events (
                    invocation_id, agent_name, action, skill_name, phase, status,
                    message, summary, event_refs_json, hint_outcome, created_at
                 ) VALUES (?1, 'agent', 'create', ?2, 'finish', 'failed',
                    'fail msg', 'fail summary', '[]', NULL, '2026-05-31T10:00:00Z')",
                right_db::params![format!("inv-{i:03}"), format!("rightx-skill-{i:03}")],
            )
            .await
            .unwrap();
        }

        let response = learning_overview(
            &conn,
            LearningOverviewInput {
                agent: "agent".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 5,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.lifecycle.failed_7d, 51);
        assert_eq!(response.lifecycle.recent_failed_events.len(), 50);
    }
}
