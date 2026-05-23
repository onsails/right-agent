use crate::api_types::{
    DashboardDataWarning, LearningCapabilities, LearningEpisodeDetail, LearningEventSummary,
    LearningEvidenceSnippet, LearningFlowEdge, LearningFlowNode, LearningFunnel, LearningHealth,
    LearningLifecycle, LearningOverviewResponse, LearningQuality, LearningReportDetailResponse,
    LearningReportSummary, LearningReviewerDetail, LearningSelectorDetail, LearningSignalPoint,
};
use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    ReadModelError, coarse_timestamp_bounds, count_parsed_window_rows,
    learning_outcomes::{learning_outcome_kind, learning_outcome_severity},
    parse_utc,
};

pub const LEARNING_REVIEW_DAILY_LIMIT: i64 = 12;
const RECENT_REPORT_LIMIT: i64 = 20;
const RECENT_EVENT_LIMIT: i64 = 10;
const RECENT_LEARNING_SIGNAL_LIMIT: usize = 30;
const CANDIDATE_NAME_LIMIT: i64 = 20;
const EVIDENCE_SNIPPET_TEXT_MAX_CHARS: usize = 320;
const EVIDENCE_SNIPPET_LIMIT: usize = 24;

pub struct LearningOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
}

fn rate(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(numerator as f64 / denominator as f64)
    }
}

fn learning_capabilities() -> LearningCapabilities {
    LearningCapabilities {
        learning_metrics: true,
        learning_evidence_snippets: true,
        learning_commands: false,
    }
}

pub fn learning_overview(
    conn: &Connection,
    input: LearningOverviewInput,
) -> Result<LearningOverviewResponse, ReadModelError> {
    let generated_at_utc = parse_utc(&input.generated_at)?;
    let since_24h_utc = generated_at_utc - Duration::hours(24);
    let since_7d_utc = generated_at_utc - Duration::days(7);
    let agent_name = input.agent;
    let generated_at = input.generated_at;
    let agent = agent_name.as_str();

    let signals_accepted_24h =
        signal_count_in_window(conn, agent, &since_24h_utc, &generated_at_utc)?;

    let episode_count = |status: &str| -> Result<i64, ReadModelError> {
        episode_count_in_window(conn, agent, status, &since_24h_utc, &generated_at_utc)
    };

    let report_count = |status: &str| -> Result<i64, ReadModelError> {
        report_count_in_window(conn, agent, status, &since_24h_utc, &generated_at_utc)
    };

    let reports_total_24h =
        report_total_count_in_window(conn, agent, &since_24h_utc, &generated_at_utc)?;
    let create_candidates_24h = report_count("create_candidate")?;
    let update_candidates_24h = report_count("update_candidate")?;
    let nothing_to_learn_24h = report_count("nothing_to_learn")?;
    let failed_reviews_24h = report_count("failed")?;
    let non_failed_reports = create_candidates_24h + update_candidates_24h + nothing_to_learn_24h;
    let foreground_created_or_updated_7d =
        successful_writer_count_in_window(conn, agent, &since_7d_utc, &generated_at_utc)?;

    let quality = LearningQuality {
        candidate_rate: rate(
            create_candidates_24h + update_candidates_24h,
            non_failed_reports,
        ),
        nothing_to_learn_rate: rate(nothing_to_learn_24h, non_failed_reports),
        create_count_24h: create_candidates_24h,
        update_count_24h: update_candidates_24h,
        high_confidence_count_24h: confidence_count(
            conn,
            agent,
            &since_24h_utc,
            &generated_at_utc,
            "high",
        )?,
        medium_confidence_count_24h: confidence_count(
            conn,
            agent,
            &since_24h_utc,
            &generated_at_utc,
            "medium",
        )?,
        low_confidence_count_24h: confidence_count(
            conn,
            agent,
            &since_24h_utc,
            &generated_at_utc,
            "low",
        )?,
        failed_count_24h: failed_reviews_24h,
    };

    let health = learning_health(conn, agent, &generated_at)?;
    let lifecycle = learning_lifecycle(conn, agent, &since_7d_utc, &generated_at_utc)?;
    let recent_reports = recent_reports(conn, agent)?;
    let (flow_counts, mut warnings) =
        learning_flow_counts(conn, agent, &since_7d_utc, &generated_at_utc)?;
    let flow_nodes = learning_flow_nodes(&flow_counts);
    let (flow_edges, partial_flow) = learning_flow_edges(&flow_nodes, &flow_counts);
    let recent_learning_signals =
        recent_learning_signals(conn, agent, &since_7d_utc, &generated_at_utc)?;
    if partial_flow {
        warnings.push(DashboardDataWarning {
            source: "learning_flow".to_owned(),
            kind: "partial_data".to_owned(),
            message: "writer transitions are aggregate-only and partially inferred".to_owned(),
        });
    }
    let episodes_pending_24h = episode_count("pending")?;
    let episodes_selecting_24h = episode_count("selecting")?;
    let episodes_selected_24h = episode_count("selected")?;
    let episodes_reviewing_24h = episode_count("reviewing")?;
    let episodes_reviewed_24h = episode_count("reviewed")?;
    let episodes_no_episode_24h = episode_count("no_episode")?;
    let episodes_insufficient_context_24h = episode_count("insufficient_context")?;
    let episodes_failed_24h = episode_count("failed")?;

    Ok(LearningOverviewResponse {
        agent: agent_name,
        generated_at,
        refresh_interval_secs: input.refresh_interval_secs,
        capabilities: learning_capabilities(),
        funnel: LearningFunnel {
            signals_accepted_24h,
            episodes_pending_24h,
            episodes_selecting_24h,
            episodes_selected_24h,
            episodes_reviewing_24h,
            episodes_reviewed_24h,
            episodes_no_episode_24h,
            episodes_insufficient_context_24h,
            episodes_failed_24h,
            reports_total_24h,
            create_candidates_24h,
            update_candidates_24h,
            nothing_to_learn_24h,
            failed_reviews_24h,
            foreground_created_or_updated_7d,
        },
        quality,
        health,
        lifecycle,
        recent_reports,
        flow_nodes,
        flow_edges,
        recent_learning_signals,
        warnings,
    })
}

struct LearningFlowCounts {
    signals: i64,
    create_candidates: i64,
    update_candidates: i64,
    nothing_to_learn: i64,
    failed_reviews: i64,
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

fn learning_flow_counts(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(LearningFlowCounts, Vec<DashboardDataWarning>), ReadModelError> {
    let (curator_triggered, curator_warning) = curator_trigger_count_in_window(conn, since, now)?;
    let mut warnings = Vec::new();
    if let Some(warning) = curator_warning {
        warnings.push(warning);
    }

    Ok((
        LearningFlowCounts {
            signals: signal_count_in_window(conn, agent, since, now)?,
            create_candidates: report_count_in_window(conn, agent, "create_candidate", since, now)?,
            update_candidates: report_count_in_window(conn, agent, "update_candidate", since, now)?,
            nothing_to_learn: report_count_in_window(conn, agent, "nothing_to_learn", since, now)?,
            failed_reviews: report_count_in_window(conn, agent, "failed", since, now)?,
            curator_triggered,
            writer_applied_as_hinted: writer_hint_outcome_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                since,
                now,
            )?,
            writer_applied_differently: writer_hint_outcome_count_in_window(
                conn,
                agent,
                "applied_differently",
                since,
                now,
            )?,
            writer_refused: writer_hint_outcome_count_in_window(
                conn, agent, "refused", since, now,
            )?,
            writer_failed: failed_writer_flow_count_in_window(conn, agent, since, now)?,
            skill_created: writer_status_count_in_window(conn, agent, "created", since, now)?,
            skill_updated: writer_status_count_in_window(conn, agent, "updated", since, now)?,
            applied_as_hinted_created: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                "created",
                since,
                now,
            )?,
            applied_as_hinted_updated: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_as_hinted",
                "updated",
                since,
                now,
            )?,
            applied_differently_created: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_differently",
                "created",
                since,
                now,
            )?,
            applied_differently_updated: writer_hint_status_count_in_window(
                conn,
                agent,
                "applied_differently",
                "updated",
                since,
                now,
            )?,
        },
        warnings,
    ))
}

fn learning_flow_nodes(counts: &LearningFlowCounts) -> Vec<LearningFlowNode> {
    vec![
        LearningFlowNode {
            id: "signals".to_owned(),
            label: "Signals".to_owned(),
            kind: "signal".to_owned(),
            count: counts.signals,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_create".to_owned(),
            label: "Create candidates".to_owned(),
            kind: "prefilter".to_owned(),
            count: counts.create_candidates,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_patch".to_owned(),
            label: "Patch candidates".to_owned(),
            kind: "prefilter".to_owned(),
            count: counts.update_candidates,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_skip".to_owned(),
            label: "Nothing to learn".to_owned(),
            kind: "prefilter".to_owned(),
            count: counts.nothing_to_learn,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_failed".to_owned(),
            label: "Failed reviews".to_owned(),
            kind: "prefilter".to_owned(),
            count: counts.failed_reviews,
            severity: "bad".to_owned(),
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
    let mut prefilter_incoming = BTreeMap::<&str, i64>::new();
    let mut signal_budget = count("signals");
    for target in [
        "prefilter_create",
        "prefilter_patch",
        "prefilter_skip",
        "prefilter_failed",
    ] {
        let value = count(target).min(signal_budget);
        if value > 0 {
            edges.push(LearningFlowEdge {
                source: "signals".to_owned(),
                target: target.to_owned(),
                count: value,
            });
            *prefilter_incoming.entry(target).or_default() += value;
            signal_budget -= value;
        }
    }

    let mut curator_budget = count("curator_triggered");
    for target in ["prefilter_create", "prefilter_patch"] {
        let already_attached = prefilter_incoming.get(target).copied().unwrap_or(0);
        let value = (count(target) - already_attached).min(curator_budget);
        if value > 0 {
            edges.push(LearningFlowEdge {
                source: "curator_triggered".to_owned(),
                target: target.to_owned(),
                count: value,
            });
            *prefilter_incoming.entry(target).or_default() += value;
            curator_budget -= value;
        }
    }

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

    let mut create_budget = count("prefilter_create");
    attach_flow_edge_with_cap(
        &mut edges,
        "prefilter_create",
        "writer_applied_as_hinted",
        &mut create_budget,
        &mut writer_remaining,
        counts.applied_as_hinted_created,
        &mut represented_writers,
    );
    attach_flow_edge_with_cap(
        &mut edges,
        "prefilter_create",
        "writer_applied_differently",
        &mut create_budget,
        &mut writer_remaining,
        counts.applied_differently_created,
        &mut represented_writers,
    );
    attach_flow_edge(
        &mut edges,
        "prefilter_create",
        "writer_refused",
        &mut create_budget,
        &mut writer_remaining,
        &mut represented_writers,
    );
    attach_flow_edge(
        &mut edges,
        "prefilter_create",
        "writer_failed",
        &mut create_budget,
        &mut writer_remaining,
        &mut represented_writers,
    );

    let mut patch_budget = count("prefilter_patch");
    attach_flow_edge_with_cap(
        &mut edges,
        "prefilter_patch",
        "writer_applied_as_hinted",
        &mut patch_budget,
        &mut writer_remaining,
        counts.applied_as_hinted_updated,
        &mut represented_writers,
    );
    attach_flow_edge_with_cap(
        &mut edges,
        "prefilter_patch",
        "writer_applied_differently",
        &mut patch_budget,
        &mut writer_remaining,
        counts.applied_differently_updated,
        &mut represented_writers,
    );
    attach_flow_edge(
        &mut edges,
        "prefilter_patch",
        "writer_refused",
        &mut patch_budget,
        &mut writer_remaining,
        &mut represented_writers,
    );
    attach_flow_edge(
        &mut edges,
        "prefilter_patch",
        "writer_failed",
        &mut patch_budget,
        &mut writer_remaining,
        &mut represented_writers,
    );

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

fn signal_count_in_window(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT accepted_at
         FROM skill_nudge_signals
         WHERE agent_name=?1
           AND accepted_at >= ?2
           AND accepted_at <= ?3",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn episode_count_in_window(
    conn: &Connection,
    agent: &str,
    status: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM learning_episodes
         WHERE agent_name=?1
           AND status=?2
           AND created_at >= ?3
           AND created_at <= ?4",
    )?;
    let rows = stmt.query_map(params![agent, status, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn report_total_count_in_window(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_review_reports
         WHERE agent_name=?1
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn report_count_in_window(
    conn: &Connection,
    agent: &str,
    status: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_review_reports
         WHERE agent_name=?1
           AND status=?2
           AND created_at >= ?3
           AND created_at <= ?4",
    )?;
    let rows = stmt.query_map(params![agent, status, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn confidence_count(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
    confidence: &str,
) -> Result<i64, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_review_reports
         WHERE agent_name=?1
           AND confidence=?2
           AND created_at >= ?3
           AND created_at <= ?4",
    )?;
    let rows = stmt.query_map(
        params![agent, confidence, coarse_since, coarse_until],
        |row| row.get::<_, String>(0),
    )?;
    count_parsed_window_rows(rows, since, now)
}

fn writer_status_count_in_window(
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
    let rows = stmt.query_map(params![agent, status, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn writer_hint_outcome_count_in_window(
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
    let rows = stmt.query_map(
        params![agent, hint_outcome, coarse_since, coarse_until],
        |row| row.get::<_, String>(0),
    )?;
    count_parsed_window_rows(rows, since, now)
}

fn writer_hint_status_count_in_window(
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
    let rows = stmt.query_map(
        params![agent, hint_outcome, status, coarse_since, coarse_until],
        |row| row.get::<_, String>(0),
    )?;
    count_parsed_window_rows(rows, since, now)
}

fn successful_writer_count_in_window(
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
           AND status IN ('created','updated')
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn failed_writer_flow_count_in_window(
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
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn failed_writer_count_in_window(
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
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        row.get::<_, String>(0)
    })?;
    count_parsed_window_rows(rows, since, now)
}

fn curator_trigger_count_in_window(
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

fn recent_learning_signals(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningSignalPoint>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT id, action, skill_name, status, hint_outcome, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut signals = Vec::<(DateTime<Utc>, i64, LearningSignalPoint)>::new();
    for row in rows {
        let (id, action, skill_name, status, hint_outcome, occurred_at) = row?;
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
                skill_name: Some(skill_name),
                count: 1,
            },
        ));
    }
    signals.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    signals.truncate(RECENT_LEARNING_SIGNAL_LIMIT);
    Ok(signals.into_iter().map(|(_, _, signal)| signal).collect())
}

/// Compute the skill-lifecycle overview from the host-side `.usage.json` file
/// produced by the lifecycle subsystem. Returns empty counts when the file is
/// absent or empty. Other I/O errors (permission denied, etc.) propagate per
/// FAIL FAST.
pub fn skill_lifecycle_overview(
    agent: &str,
    usage_path: &std::path::Path,
) -> Result<crate::api_types::SkillLifecycleOverviewResponse, ReadModelError> {
    let content = match std::fs::read_to_string(usage_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let parsed: serde_json::Value = if content.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(&content)?
    };

    let mut total_active = 0;
    let mut total_stale = 0;
    let mut total_archived = 0;
    let mut agent_created_active = 0;
    let mut foreground_active = 0;
    let mut bundled_active = 0;
    let mut recently_used: Vec<crate::api_types::RecentSkill> = Vec::new();

    if let Some(map) = parsed.as_object() {
        for (name, record) in map {
            let state = record
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("active");
            let created_by = record
                .get("created_by")
                .and_then(|v| v.as_str())
                .unwrap_or("foreground");
            match state {
                "active" => total_active += 1,
                "stale" => total_stale += 1,
                "archived" => total_archived += 1,
                _ => {}
            }
            if state == "active" {
                match created_by {
                    "probe_writer" | "curator" => agent_created_active += 1,
                    "foreground" => foreground_active += 1,
                    "bundled" => bundled_active += 1,
                    _ => {}
                }
            }
            let use_count = record
                .get("use_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let last_used_at = record
                .get("last_used_at")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            if use_count > 0 {
                recently_used.push(crate::api_types::RecentSkill {
                    package_name: name.clone(),
                    use_count,
                    last_used_at,
                });
            }
        }
    }
    recently_used.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    recently_used.truncate(20);

    Ok(crate::api_types::SkillLifecycleOverviewResponse {
        agent: agent.to_owned(),
        total_active,
        total_stale,
        total_archived,
        agent_created_active,
        foreground_active,
        bundled_active,
        recently_used,
    })
}

fn learning_health(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<LearningHealth, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT review_running, daily_review_count, creation_review_interval,
                    tool_iters_since_review, turns_since_review,
                    skill_issue_hints_since_review, last_review_status, last_review_at
             FROM skill_nudge_state WHERE agent_name=?1",
            params![agent],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let (
        review_running,
        daily_review_count,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
    ) = row.unwrap_or((0, 0, 15, 0, 0, 0, None, None));

    Ok(LearningHealth {
        review_running: review_running != 0,
        daily_review_count,
        daily_limit: LEARNING_REVIEW_DAILY_LIMIT,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
        possibly_stuck: possibly_stuck(conn, agent, generated_at)?,
    })
}

fn possibly_stuck(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<bool, ReadModelError> {
    let cutoff = (parse_utc(generated_at)? - Duration::minutes(10)).to_rfc3339();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM learning_episodes
         WHERE agent_name=?1 AND status='reviewing' AND updated_at < ?2
           AND COALESCE(
                 (SELECT review_running FROM skill_nudge_state WHERE agent_name=?1),
                 0
               ) = 0",
        params![agent, cutoff],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn learning_lifecycle(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<LearningLifecycle, ReadModelError> {
    Ok(LearningLifecycle {
        created_7d: writer_status_count_in_window(conn, agent, "created", since_7d, now)?,
        updated_7d: writer_status_count_in_window(conn, agent, "updated", since_7d, now)?,
        failed_or_aborted_7d: failed_writer_count_in_window(conn, agent, since_7d, now)?,
        recent_successful_events: recent_successful_events(conn, agent, since_7d, now)?,
        candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d, now)?,
    })
}

fn recent_successful_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let mut stmt = conn.prepare(
        "SELECT id, skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
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
    events.truncate(RECENT_EVENT_LIMIT as usize);
    Ok(events.into_iter().map(|(_, _, event)| event).collect())
}

fn candidate_skill_names(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<String>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let mut stmt = conn.prepare(
        "SELECT candidate_skill_name, created_at
         FROM skill_review_reports
         WHERE agent_name=?1
           AND candidate_skill_name IS NOT NULL
           AND status IN ('create_candidate','update_candidate')
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt.query_map(params![agent, coarse_since, coarse_until], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
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

fn recent_reports(
    conn: &Connection,
    agent: &str,
) -> Result<Vec<LearningReportSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                candidate_summary, telegram_notified, created_at
         FROM skill_review_reports
         WHERE agent_name=?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![agent, RECENT_REPORT_LIMIT], report_summary_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(super) fn report_summary_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LearningReportSummary> {
    Ok(LearningReportSummary {
        id: row.get(0)?,
        status: row.get(1)?,
        confidence: row.get(2)?,
        trigger_kind: row.get(3)?,
        candidate_skill_name: row.get(4)?,
        candidate_summary: row.get(5)?,
        telegram_notified: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}

struct ReportDetailRow {
    report: LearningReportSummary,
    learning_episode_id: Option<i64>,
    evidence_refs: Vec<String>,
    review_output_json: serde_json::Value,
}

struct EpisodeDetailRow {
    episode: LearningEpisodeDetail,
    selector: LearningSelectorDetail,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
}

pub fn learning_report_detail(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<LearningReportDetailResponse>, ReadModelError> {
    let Some(report_row) = load_report_detail_row(conn, agent, report_id)? else {
        return Ok(None);
    };
    let episode_row = match report_row.learning_episode_id {
        Some(episode_id) => load_episode_detail_row(conn, agent, episode_id)?,
        None => None,
    };
    let allowed_message_refs = episode_row
        .as_ref()
        .map(|row| row.message_refs.as_slice())
        .unwrap_or(&[]);
    let allowed_execution_refs = episode_row
        .as_ref()
        .map(|row| row.execution_event_refs.as_slice())
        .unwrap_or(&[]);
    let evidence_refs = if report_row.evidence_refs.is_empty() {
        episode_row
            .as_ref()
            .map(|row| {
                row.message_refs
                    .iter()
                    .chain(row.execution_event_refs.iter())
                    .take(EVIDENCE_SNIPPET_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        report_row
            .evidence_refs
            .iter()
            .take(EVIDENCE_SNIPPET_LIMIT)
            .cloned()
            .collect()
    };
    let evidence = load_evidence_snippets(
        conn,
        agent,
        &evidence_refs,
        allowed_message_refs,
        allowed_execution_refs,
    )?;
    let reviewer = reviewer_detail(&report_row);

    Ok(Some(LearningReportDetailResponse {
        report: report_row.report,
        episode: episode_row.as_ref().map(|row| row.episode.clone()),
        selector: episode_row.map(|row| row.selector),
        evidence,
        reviewer,
    }))
}

fn load_report_detail_row(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<ReportDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                    candidate_summary, telegram_notified, created_at,
                    learning_episode_id, evidence_refs_json, review_output_json
             FROM skill_review_reports
             WHERE agent_name=?1 AND id=?2",
            params![agent, report_id],
            |row| {
                Ok((
                    report_summary_from_row(row)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(report, learning_episode_id, evidence_refs_json, review_output_json)| {
            Ok(ReportDetailRow {
                report,
                learning_episode_id,
                evidence_refs: parse_string_array(&evidence_refs_json)?,
                review_output_json: serde_json::from_str(&review_output_json)?,
            })
        },
    )
    .transpose()
}

fn load_episode_detail_row(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Option<EpisodeDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, kind, seed_trigger_kind, status, start_ref, end_ref,
                    message_refs_json, execution_event_refs_json, selector_model,
                    selector_output_json, boundary_rationale, confidence,
                    context_incomplete
             FROM learning_episodes
             WHERE agent_name=?1 AND id=?2",
            params![agent, episode_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            id,
            kind,
            seed_trigger_kind,
            status,
            start_ref,
            end_ref,
            message_refs_json,
            execution_event_refs_json,
            selector_model,
            _selector_output_json,
            boundary_rationale,
            confidence,
            context_incomplete,
        )| {
            let message_refs = parse_string_array(&message_refs_json)?;
            let execution_event_refs = parse_string_array(&execution_event_refs_json)?;
            Ok(EpisodeDetailRow {
                episode: LearningEpisodeDetail {
                    id,
                    kind,
                    seed_trigger_kind,
                    status,
                    start_ref,
                    end_ref,
                    boundary_rationale: boundary_rationale.clone(),
                    confidence: confidence.clone(),
                    context_incomplete: context_incomplete != 0,
                },
                selector: LearningSelectorDetail {
                    model: selector_model,
                    boundary_rationale,
                    selected_message_refs: message_refs.clone(),
                    selected_execution_event_refs: execution_event_refs.clone(),
                },
                message_refs,
                execution_event_refs,
            })
        },
    )
    .transpose()
}

fn reviewer_detail(row: &ReportDetailRow) -> LearningReviewerDetail {
    let user_notice_present = row
        .review_output_json
        .get("user_notice")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    LearningReviewerDetail {
        status: row.report.status.clone(),
        confidence: row.report.confidence.clone(),
        candidate_skill_name: row.report.candidate_skill_name.clone(),
        candidate_summary: row.report.candidate_summary.clone(),
        evidence_refs: row.evidence_refs.clone(),
        user_notice_present,
    }
}

fn load_evidence_snippets(
    conn: &Connection,
    agent: &str,
    refs: &[String],
    allowed_message_refs: &[String],
    allowed_execution_refs: &[String],
) -> Result<Vec<LearningEvidenceSnippet>, ReadModelError> {
    let allowed_messages: HashSet<&str> = allowed_message_refs.iter().map(String::as_str).collect();
    let allowed_executions: HashSet<&str> =
        allowed_execution_refs.iter().map(String::as_str).collect();
    let mut snippets = Vec::with_capacity(refs.len());
    for ref_id in refs {
        if ref_id.starts_with("msg:") {
            if !allowed_messages.contains(ref_id.as_str()) {
                snippets.push(unavailable_snippet(ref_id.clone(), "message"));
                continue;
            }
            snippets.push(load_message_snippet(conn, ref_id)?);
        } else if ref_id.starts_with("exec:") {
            if !allowed_executions.contains(ref_id.as_str()) {
                snippets.push(unavailable_snippet(ref_id.clone(), "execution_event"));
                continue;
            }
            snippets.push(load_execution_snippet(conn, agent, ref_id)?);
        } else {
            snippets.push(unavailable_snippet(ref_id.clone(), "unknown"));
        }
    }
    Ok(snippets)
}

fn load_message_snippet(
    conn: &Connection,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "msg:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    let row = conn
        .query_row(
            "SELECT role, content, created_at, addressed_to_bot, routed_to_agent
             FROM conversation_messages
             WHERE id=?1 AND role IN ('user','assistant')",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((role, content, created_at, addressed_to_bot, routed_to_agent)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    if addressed_to_bot == 0 && routed_to_agent == 0 {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    }
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "message".to_owned(),
        available: true,
        trust_label: Some("primary".to_owned()),
        role: Some(role),
        event_kind: None,
        tool_name: None,
        created_at: Some(created_at),
        text: Some(bounded_text(content)),
    })
}

fn load_execution_snippet(
    conn: &Connection,
    agent: &str,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "exec:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    let row = conn
        .query_row(
            "SELECT event_kind, tool_name, content_text, trust_label, created_at
             FROM execution_events
             WHERE agent_name=?1 AND id=?2",
            params![agent, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((event_kind, tool_name, content_text, trust_label, created_at)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    if event_kind == "thinking" || trust_label == "low_trust" {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    }
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "execution_event".to_owned(),
        available: true,
        trust_label: Some(trust_label),
        role: None,
        event_kind: Some(event_kind),
        tool_name,
        created_at: Some(created_at),
        text: Some(bounded_text(content_text)),
    })
}

pub(super) fn parse_string_array(raw: &str) -> Result<Vec<String>, ReadModelError> {
    Ok(serde_json::from_str(raw)?)
}

fn bounded_text(value: String) -> String {
    let mut chars = value.chars();
    let mut out = chars
        .by_ref()
        .take(EVIDENCE_SNIPPET_TEXT_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        out.push_str("... [truncated]");
    }
    out
}

fn parse_ref_id(reference: &str, prefix: &str) -> Option<i64> {
    reference.strip_prefix(prefix)?.parse::<i64>().ok()
}

fn unavailable_snippet(ref_id: String, source: &str) -> LearningEvidenceSnippet {
    LearningEvidenceSnippet {
        ref_id,
        source: source.to_owned(),
        available: false,
        trust_label: None,
        role: None,
        event_kind: None,
        tool_name: None,
        created_at: None,
        text: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");
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

    #[test]
    fn learning_overview_builds_funnel_quality_health_and_lifecycle() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, tool_iters_since_review, turns_since_review,
                skill_issue_hints_since_review, last_review_at, review_running,
                creation_review_interval, daily_review_count, daily_review_date,
                last_review_status
             ) VALUES (
                'right', 6, 2, 1, '2026-05-20T11:00:00Z', 0,
                15, 4, '2026-05-20', 'nothing_to_learn'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, message_refs_json,
                execution_event_refs_json, selector_output_json, ready_after,
                created_at, updated_at
             ) VALUES (
                1, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, '[]', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                root_session_id, chat_id, thread_id, trigger_kind, status,
                confidence, candidate_skill_name, candidate_summary,
                evidence_refs_json, review_output_json, telegram_notified,
                created_at
             ) VALUES (
                7, 'right', 'inv-1', 1, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:1\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"evidence_refs\":[\"msg:1\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                'created', 'Learned OAuth callback verification.',
                'Reusable OAuth setup workflow.', '[]',
                '2026-05-20T11:10:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.signals_accepted_24h, 1);
        assert_eq!(response.funnel.episodes_reviewed_24h, 1);
        assert_eq!(response.funnel.create_candidates_24h, 1);
        assert_eq!(response.funnel.foreground_created_or_updated_7d, 1);
        assert_eq!(response.quality.candidate_rate, Some(1.0));
        assert_eq!(response.quality.nothing_to_learn_rate, Some(0.0));
        assert!(!response.health.review_running);
        assert_eq!(response.health.daily_review_count, 4);
        assert_eq!(response.lifecycle.created_7d, 1);
        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-oauth-debugging"]
        );
        assert_eq!(response.recent_reports[0].id, 7);
    }

    #[test]
    fn learning_overview_builds_flow_nodes_edges_and_recent_signals() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES (
                'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                'created', 'Learned OAuth callback verification.',
                'Reusable OAuth setup workflow.', '[]', 'applied_as_hinted',
                '2026-05-20T11:10:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert!(
            response
                .flow_nodes
                .iter()
                .any(|node| node.id == "signals" && node.count == 1)
        );
        assert!(
            response
                .flow_nodes
                .iter()
                .any(|node| node.id == "writer_applied_as_hinted" && node.count == 1)
        );
        assert!(
            response
                .flow_nodes
                .iter()
                .any(|node| node.id == "skill_created" && node.count == 1)
        );
        assert!(
            response
                .flow_edges
                .iter()
                .any(|edge| edge.source == "signals" && edge.target == "writer_applied_as_hinted")
        );
        assert!(response.flow_edges.iter().any(
            |edge| edge.source == "writer_applied_as_hinted" && edge.target == "skill_created"
        ));
        assert_eq!(response.recent_learning_signals[0].kind, "skill_created");
    }

    #[test]
    fn learning_overview_flow_projects_hint_outcomes_skills_and_curator_trigger() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (
                1, NULL, NULL, 0, NULL,
                '{\"trigger\":\"skill_change_count\",\"computed_at\":\"2026-05-20T09:30:00Z\",\"details\":{\"count\":3,\"threshold\":2}}'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES
                ('right', 'inv-create-1', 'learning_signal', 'create_candidate',
                 'high', '[]', '{}', '2026-05-20T10:00:00Z'),
                ('right', 'inv-patch-1', 'learning_signal', 'update_candidate',
                 'high', '[]', '{}', '2026-05-20T10:01:00Z'),
                ('right', 'inv-create-2', 'learning_signal', 'create_candidate',
                 'medium', '[]', '{}', '2026-05-20T10:02:00Z'),
                ('right', 'inv-patch-2', 'learning_signal', 'update_candidate',
                 'medium', '[]', '{}', '2026-05-20T10:03:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('inv-created', 'right', 'create', 'rightx-created', 'finish',
                 'created', 'created', 'created', '[]', 'applied_as_hinted',
                 '2026-05-20T10:10:00Z'),
                ('inv-updated', 'right', 'update', 'rightx-updated', 'finish',
                 'updated', 'updated', 'updated', '[]', 'applied_differently',
                 '2026-05-20T10:11:00Z'),
                ('inv-refused', 'right', 'create', 'rightx-refused', 'finish',
                 'aborted', 'refused', 'refused', '[]', 'refused',
                 '2026-05-20T10:12:00Z'),
                ('inv-failed', 'right', 'update', 'rightx-failed', 'finish',
                 'failed', 'failed', 'failed', '[]', NULL,
                 '2026-05-20T10:13:00Z')",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(flow_node_count(&response, "curator_triggered"), 1);
        assert_eq!(flow_node_count(&response, "writer_applied_as_hinted"), 1);
        assert_eq!(flow_node_count(&response, "writer_applied_differently"), 1);
        assert_eq!(flow_node_count(&response, "writer_refused"), 1);
        assert_eq!(flow_node_count(&response, "writer_failed"), 1);
        assert_eq!(flow_node_count(&response, "skill_created"), 1);
        assert_eq!(flow_node_count(&response, "skill_updated"), 1);
        assert_eq!(
            flow_edge_count(&response, "curator_triggered", "prefilter_create"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "prefilter_create", "writer_applied_as_hinted"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "prefilter_patch", "writer_applied_differently"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "writer_applied_as_hinted", "skill_created"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "writer_applied_differently", "skill_updated"),
            1
        );

        conn.execute(
            "UPDATE curator_state
             SET last_spike_evidence_json = '{not json'",
            [],
        )
        .unwrap();
        let response = learning_overview(&conn, input()).unwrap();
        assert_eq!(flow_node_count(&response, "curator_triggered"), 0);
        assert!(response.warnings.iter().any(|warning| {
            warning.source == "curator_state.last_spike_evidence_json"
                && warning.kind == "malformed_json"
        }));
    }

    #[test]
    fn learning_overview_flow_prefers_prefilter_pipeline_when_reports_exist() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'learning_signal', 'create_candidate',
                'high', '[]', '{}', '2026-05-20T10:30:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES (
                'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                'created', 'Learned OAuth callback verification.',
                'Reusable OAuth setup workflow.', '[]', 'applied_as_hinted',
                '2026-05-20T11:10:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(flow_edge_count(&response, "signals", "prefilter_create"), 1);
        assert_eq!(
            flow_edge_count(&response, "prefilter_create", "writer_applied_as_hinted"),
            1
        );
        assert_eq!(
            flow_edge_count(&response, "signals", "writer_applied_as_hinted"),
            0
        );
        assert_eq!(
            flow_edge_count(&response, "writer_applied_as_hinted", "skill_created"),
            1
        );
        assert!(response.warnings.is_empty());
    }

    #[test]
    fn learning_overview_warns_when_writer_flow_is_partially_inferred() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'learning_signal', 'create_candidate',
                'high', '[]', '{}', '2026-05-20T10:30:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                 'created', 'Learned OAuth callback verification.',
                 'Reusable OAuth setup workflow.', '[]', 'applied_as_hinted',
                 '2026-05-20T11:10:00Z'),
                ('inv-3', 'right', 'create', 'rightx-extra', 'finish',
                 'created', 'Learned another workflow.',
                 'Another workflow.', '[]', 'applied_as_hinted',
                 '2026-05-20T11:20:00Z')",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(
            flow_edge_count(&response, "prefilter_create", "writer_applied_as_hinted"),
            1
        );
        assert!(response.warnings.iter().any(|warning| {
            warning.source == "learning_flow" && warning.kind == "partial_data"
        }));
    }

    #[test]
    fn learning_overview_flow_uses_bounded_7d_window() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES
                ('old-signal-1', 'right', 'session-1', 10, 20, 'learning', '{}', '2026-05-16T10:00:00Z'),
                ('old-signal-2', 'right', 'session-1', 10, 20, 'learning', '{}', '2026-05-17T10:00:00Z'),
                ('future-signal', 'right', 'session-1', 10, 20, 'learning', '{}', '2026-05-21T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES
                ('right', 'old-create', 'learning_signal', 'create_candidate', 'high', '[]', '{}', '2026-05-16T10:10:00Z'),
                ('right', 'old-update', 'learning_signal', 'update_candidate', 'medium', '[]', '{}', '2026-05-16T10:20:00Z'),
                ('right', 'old-skip', 'learning_signal', 'nothing_to_learn', 'low', '[]', '{}', '2026-05-16T10:30:00Z'),
                ('right', 'future-create', 'learning_signal', 'create_candidate', 'high', '[]', '{}', '2026-05-21T10:10:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('old-created', 'right', 'create', 'rightx-created', 'finish', 'created', 'created', 'created', '[]', NULL, '2026-05-16T11:00:00Z'),
                ('future-created', 'right', 'create', 'rightx-future', 'finish', 'created', 'future', 'future', '[]', NULL, '2026-05-21T11:00:00Z')",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(flow_node_count(&response, "signals"), 2);
        assert_eq!(flow_node_count(&response, "prefilter_create"), 1);
        assert_eq!(flow_node_count(&response, "prefilter_patch"), 1);
        assert_eq!(flow_node_count(&response, "prefilter_skip"), 1);
        assert_eq!(flow_node_count(&response, "skill_created"), 1);
        assert_eq!(flow_edge_count(&response, "signals", "prefilter_create"), 1);
        assert_eq!(
            flow_edge_count(&response, "prefilter_create", "writer_applied_as_hinted"),
            0
        );
        assert_eq!(
            flow_edge_count(&response, "signals", "writer_applied_as_hinted"),
            0
        );
    }

    #[test]
    fn learning_overview_flow_keeps_failed_reviews_out_of_writer_failed() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'failed-review', 'learning_signal', 'failed',
                'low', '[]', '{}', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(flow_node_count(&response, "prefilter_failed"), 1);
        assert_eq!(flow_node_count(&response, "writer_failed"), 0);
    }

    #[test]
    fn learning_overview_recent_signals_parse_utc_bounds_and_sort() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('normal-created', 'right', 'create', 'rightx-normal', 'finish', 'created', 'created', 'created', '[]', NULL, '2026-05-20T10:00:00Z'),
                ('offset-updated', 'right', 'update', 'rightx-offset', 'finish', 'updated', 'updated', 'updated', '[]', NULL, '2026-05-20T15:30:00.500+04:00'),
                ('future-failed', 'right', 'update', 'rightx-future', 'finish', 'failed', 'future', 'future', '[]', NULL, '2026-05-20T11:30:00.000-02:00'),
                ('old-created', 'right', 'create', 'rightx-old', 'finish', 'created', 'old', 'old', '[]', NULL, '2026-05-13T11:59:59Z')",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();
        let labels = response
            .recent_learning_signals
            .iter()
            .map(|signal| signal.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["rightx-offset", "rightx-normal"]);
        assert_eq!(response.recent_learning_signals[0].kind, "skill_updated");
        assert_eq!(response.recent_learning_signals[0].severity, "info");
    }

    #[test]
    fn learning_overview_counts_parse_utc_bounds() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES
                ('valid-signal', 'right', 'session-1', 10, 20, 'learning', '{}', '2026-05-20T15:30:00.500+04:00'),
                ('future-signal', 'right', 'session-1', 10, 20, 'learning', '{}', '2026-05-20T11:30:00.000-02:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES
                ('right', 'foreground_thread', 'learning_signal', 'inv:valid', 'selected',
                 '[]', '[]', '2026-05-20T15:30:00.500+04:00',
                 '2026-05-20T15:30:00.500+04:00', '2026-05-20T15:31:00.500+04:00'),
                ('right', 'foreground_thread', 'learning_signal', 'inv:future', 'selected',
                 '[]', '[]', '2026-05-20T11:30:00.000-02:00',
                 '2026-05-20T11:30:00.000-02:00', '2026-05-20T11:31:00.000-02:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES
                ('right', 'valid-report', 'learning_signal', 'create_candidate',
                 'high', 'rightx-valid-candidate', '[]', '{}',
                 '2026-05-20T15:30:00.500+04:00'),
                ('right', 'future-report', 'learning_signal', 'update_candidate',
                 'high', 'rightx-future-candidate', '[]', '{}',
                 '2026-05-20T11:30:00.000-02:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('valid-created', 'right', 'create', 'rightx-valid', 'finish',
                 'created', 'created', 'created', '[]', NULL,
                 '2026-05-20T15:30:00.500+04:00'),
                ('future-updated', 'right', 'update', 'rightx-future', 'finish',
                 'updated', 'future', 'future', '[]', NULL,
                 '2026-05-20T11:30:00.000-02:00')",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.signals_accepted_24h, 1);
        assert_eq!(response.funnel.episodes_selected_24h, 1);
        assert_eq!(response.funnel.reports_total_24h, 1);
        assert_eq!(response.funnel.create_candidates_24h, 1);
        assert_eq!(response.funnel.update_candidates_24h, 0);
        assert_eq!(response.quality.high_confidence_count_24h, 1);
        assert_eq!(response.funnel.foreground_created_or_updated_7d, 1);
        assert_eq!(response.lifecycle.created_7d, 1);
        assert_eq!(response.lifecycle.updated_7d, 0);
        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-valid-candidate"]
        );
        assert_eq!(response.lifecycle.recent_successful_events.len(), 1);
        assert_eq!(
            response.lifecycle.recent_successful_events[0].skill_name,
            "rightx-valid"
        );
    }

    #[test]
    fn learning_overview_rates_are_null_without_non_failed_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'effort_threshold', 'failed',
                'low', '[]', '{}', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.quality.candidate_rate, None);
        assert_eq!(response.quality.nothing_to_learn_rate, None);
        assert_eq!(response.quality.failed_count_24h, 1);
    }

    #[test]
    fn learning_overview_detects_old_reviewing_episode_as_possibly_stuck() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, review_running, creation_review_interval,
                daily_review_count
             ) VALUES ('right', 0, 15, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:stuck',
                'reviewing', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert!(!response.health.review_running);
        assert!(response.health.possibly_stuck);
    }

    #[test]
    fn learning_overview_does_not_flag_stuck_while_reviewer_running() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, review_running, creation_review_interval,
                daily_review_count
             ) VALUES ('right', 1, 15, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:stuck',
                'reviewing', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert!(response.health.review_running);
        assert!(!response.health.possibly_stuck);
    }

    #[test]
    fn learning_overview_candidate_names_include_only_candidate_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'learning_signal', 'create_candidate',
                'high', 'rightx-valid-candidate', '[]', '{}',
                '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, candidate_skill_name, evidence_refs_json,
                review_output_json, created_at
             ) VALUES (
                'right', 'inv-2', 'effort_threshold', 'nothing_to_learn',
                'medium', 'rightx-not-a-candidate', '[]', '{}',
                '2026-05-20T11:30:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(
            response.lifecycle.candidate_skill_names_7d,
            vec!["rightx-valid-candidate"]
        );
    }

    #[test]
    fn learning_overview_counts_insufficient_context_episodes() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:context',
                'insufficient_context', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.episodes_insufficient_context_24h, 1);
    }

    #[test]
    fn learning_report_detail_returns_message_and_execution_snippets() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO conversation_messages (
                id, platform, chat_id, thread_id, message_id, role, content,
                root_session_id, turn_id, routed_to_agent, created_at
             ) VALUES (
                101, 'telegram', 10, 20, 77, 'user',
                'Verify the OAuth callback URL before retrying auth.',
                'session-1', 3, 1, '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, tool_name, content_json, content_text, trust_label,
                created_at
             ) VALUES (
                202, 'right', 'session-1', 'inv-1', 3, 9,
                'tool_result', 'shell', '{}', 'callback verified', 'primary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, start_ref, end_ref,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, boundary_rationale, confidence,
                context_incomplete, ready_after, created_at, updated_at
             ) VALUES (
                4, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, 'msg:101', 'exec:202',
                '[\"msg:101\"]', '[\"exec:202\"]', 'claude-sonnet-4-6',
                '{\"status\":\"selected\"}', 'Selected OAuth setup correction.',
                'high', 0, '2026-05-20T10:01:30Z',
                '2026-05-20T10:00:00Z', '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                root_session_id, chat_id, thread_id, trigger_kind, status,
                confidence, candidate_skill_name, candidate_summary,
                evidence_refs_json, review_output_json, telegram_notified,
                created_at
             ) VALUES (
                9, 'right', 'inv-1', 4, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:101\",\"exec:202\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"candidate_skill_name\":\"rightx-oauth-debugging\",\"candidate_summary\":\"Verify OAuth callback setup.\",\"evidence_refs\":[\"msg:101\",\"exec:202\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 9).unwrap().unwrap();

        assert_eq!(detail.report.id, 9);
        assert_eq!(detail.episode.as_ref().unwrap().id, 4);
        assert_eq!(
            detail.selector.as_ref().unwrap().selected_message_refs,
            vec!["msg:101"]
        );
        assert_eq!(detail.evidence.len(), 2);
        assert_eq!(detail.evidence[0].source, "message");
        assert_eq!(
            detail.evidence[0].text.as_deref(),
            Some("Verify the OAuth callback URL before retrying auth.")
        );
        assert_eq!(detail.evidence[1].source, "execution_event");
        assert_eq!(
            detail.evidence[1].event_kind.as_deref(),
            Some("tool_result")
        );
        assert!(detail.reviewer.user_notice_present);
    }

    #[test]
    fn learning_report_detail_marks_missing_refs_unavailable_and_hides_thinking() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, content_json, content_text, trust_label, created_at
             ) VALUES (
                303, 'right', 'session-1', 'inv-2', 5, 1,
                'thinking', '{}', 'private reasoning', 'secondary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, created_at, updated_at
             ) VALUES (
                5, 'right', 'foreground_thread', 'effort_threshold', 'inv:inv-2',
                'reviewed', '[\"msg:404\"]', '[\"exec:303\"]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                trigger_kind, status, confidence, evidence_refs_json,
                review_output_json, telegram_notified, created_at
             ) VALUES (
                10, 'right', 'inv-2', 5, 'effort_threshold',
                'nothing_to_learn', 'medium', '[\"msg:404\",\"exec:303\"]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"medium\",\"evidence_refs\":[\"msg:404\",\"exec:303\"],\"user_notice\":null}',
                0, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 10).unwrap().unwrap();

        assert_eq!(detail.evidence.len(), 2);
        assert!(!detail.evidence[0].available);
        assert_eq!(detail.evidence[0].ref_id, "msg:404");
        assert!(!detail.evidence[1].available);
        assert_eq!(detail.evidence[1].ref_id, "exec:303");
        assert_eq!(detail.evidence[1].text, None);
    }

    #[test]
    fn learning_report_detail_hides_low_trust_messages() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO conversation_messages (
                id, platform, chat_id, thread_id, message_id, role, content,
                addressed_to_bot, routed_to_agent, created_at
             ) VALUES (
                102, 'telegram', 10, 20, 78, 'user',
                'Ambient chat that was not routed to the agent.',
                0, 0, '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, created_at, updated_at
             ) VALUES (
                6, 'right', 'foreground_thread', 'effort_threshold', 'inv:inv-4',
                'reviewed', '[\"msg:102\"]', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                trigger_kind, status, confidence, evidence_refs_json,
                review_output_json, telegram_notified, created_at
             ) VALUES (
                12, 'right', 'inv-4', 6, 'effort_threshold',
                'nothing_to_learn', 'low', '[\"msg:102\"]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"low\",\"evidence_refs\":[\"msg:102\"],\"user_notice\":null}',
                0, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 12).unwrap().unwrap();

        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(detail.evidence[0].ref_id, "msg:102");
        assert_eq!(detail.evidence[0].source, "message");
        assert!(!detail.evidence[0].available);
        assert_eq!(detail.evidence[0].text, None);
    }

    #[test]
    fn learning_report_detail_errors_on_malformed_report_json() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                11, 'right', 'inv-3', 'effort_threshold', 'failed',
                'low', '[]', '{malformed', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        assert!(learning_report_detail(&conn, "right", 11).is_err());
    }

    #[test]
    fn skill_lifecycle_overview_counts_by_state_and_provenance() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let usage_path = dir.path().join(".usage.json");
        let json = r#"{
            "rightx-foo": {"state": "active", "created_by": "probe_writer", "use_count": 5, "last_used_at": "2026-05-22T10:00:00Z"},
            "rightx-bar": {"state": "stale", "created_by": "probe_writer", "use_count": 0},
            "rightx-baz": {"state": "archived", "created_by": "probe_writer", "use_count": 1, "last_used_at": "2026-05-15T10:00:00Z"},
            "rightx-explicit": {"state": "active", "created_by": "foreground"},
            "rightx-bundled": {"state": "active", "created_by": "bundled"}
        }"#;
        std::fs::write(&usage_path, json).unwrap();

        let resp = skill_lifecycle_overview("right", &usage_path).unwrap();
        assert_eq!(resp.total_active, 3); // foo + explicit + bundled
        assert_eq!(resp.total_stale, 1);
        assert_eq!(resp.total_archived, 1);
        assert_eq!(resp.agent_created_active, 1); // foo only
        assert_eq!(resp.foreground_active, 1); // explicit
        assert_eq!(resp.bundled_active, 1);
        // recently_used includes only entries with use_count > 0; sorted by last_used_at desc.
        assert_eq!(resp.recently_used.len(), 2);
        assert_eq!(resp.recently_used[0].package_name, "rightx-foo");
    }

    #[test]
    fn skill_lifecycle_overview_empty_when_file_missing() {
        let resp =
            skill_lifecycle_overview("right", std::path::Path::new("/nonexistent/.usage.json"))
                .unwrap();
        assert_eq!(resp.total_active, 0);
        assert_eq!(resp.total_stale, 0);
        assert_eq!(resp.total_archived, 0);
        assert!(resp.recently_used.is_empty());
    }
}
