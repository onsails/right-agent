use crate::api_types::{
    CostLearningPoint, CostLearningRiver, CostLearningSeries, CostSeriesPoint,
    DashboardDataWarning, DashboardOverviewResponse, DashboardSignal, LearningMarker,
    OverviewDoctorStatus, OverviewSandboxStatus, RunSummary, UsageSourcePoint,
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use right_db::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;

use super::{
    ReadModelError, coarse_timestamp_bounds, count_parsed_window_rows,
    learning_outcomes::{learning_outcome_kind, learning_outcome_severity, learning_outcome_title},
    parse_utc,
};

const SIGNAL_LIMIT: usize = 30;
const RIVER_DAYS: i64 = 30;
const RIVER_WINDOW: &str = "last_30_days";

pub struct DashboardOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub foreground_active_count: i64,
    pub sandbox: OverviewSandboxStatus,
}

pub async fn dashboard_overview(
    conn: &Connection,
    input: DashboardOverviewInput,
) -> Result<DashboardOverviewResponse, ReadModelError> {
    let active_runs =
        active_async_run_count(conn, &input.generated_at).await? + input.foreground_active_count;
    let recent_failed_runs = recent_failed_runs(conn, &input.generated_at).await?;
    // Same predicate and 24h window as the list, so the count is exactly the
    // number of rows — derive it instead of issuing a second identical query.
    // Keeps the card count and the revealed list structurally consistent.
    let recent_failures = recent_failed_runs.len() as i64;
    let today_cost_usd = today_cost_usd(conn, &input.generated_at).await?;
    let learning_candidates_24h =
        learning_candidate_count(conn, &input.agent, &input.generated_at).await?;
    let generated_at_utc = parse_utc(&input.generated_at)?;
    let (mut cost_learning_river, mut warnings) =
        cost_learning_river(conn, &input.generated_at, &input.agent).await?;
    let (curator_signals, curator_markers, curator_warnings) =
        curator_projection(conn, &generated_at_utc).await?;
    cost_learning_river.markers.extend(curator_markers);
    warnings.extend(curator_warnings);
    let mut signals = overview_signals(
        conn,
        &input.agent,
        &input.generated_at,
        input.foreground_active_count,
        &input.sandbox,
        &cost_learning_river,
        curator_signals,
    )
    .await?;
    signals.truncate(SIGNAL_LIMIT);

    Ok(DashboardOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        active_runs,
        recent_failures,
        recent_failed_runs,
        today_cost_usd,
        learning_candidates_24h,
        doctor: OverviewDoctorStatus {
            state: "not_loaded".to_string(),
            pass_count: 0,
            warn_count: 0,
            fail_count: 0,
            generated_at: None,
        },
        sandbox: input.sandbox,
        signals,
        cost_learning_river,
        warnings,
    })
}

async fn active_async_run_count(
    conn: &Connection,
    generated_at: &str,
) -> Result<i64, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let coarse_until = (now + Duration::days(1)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(started_at, updated_at, created_at)
         FROM async_runs
         WHERE status IN ('queued', 'running')
           AND COALESCE(started_at, updated_at, created_at) <= ?1",
    )?;
    let rows = stmt
        .query_map(params![coarse_until], |row| row.get::<_, String>(0))
        .await?;
    let mut count = 0;
    for row in rows {
        let active_at = parse_utc(&row?)?;
        if active_at <= now {
            count += 1;
        }
    }
    Ok(count)
}

async fn today_cost_usd(conn: &Connection, generated_at: &str) -> Result<f64, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let start_naive = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(generated_at.to_owned()))?;
    let start = Utc.from_utc_datetime(&start_naive);
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(&start, &now);

    let mut stmt = conn.prepare(
        "SELECT ts, total_cost_usd
         FROM usage_events
         WHERE ts >= ?1 AND ts <= ?2",
    )?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })
        .await?;

    let mut cost = 0.0;
    for row in rows {
        let (ts, event_cost) = row?;
        let event_at = parse_utc(&ts)?;
        if event_at >= start && event_at <= now {
            cost += event_cost;
        }
    }
    Ok(cost)
}

async fn overview_signals(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
    foreground_active_count: i64,
    sandbox: &OverviewSandboxStatus,
    river: &CostLearningRiver,
    curator_signals: Vec<DashboardSignal>,
) -> Result<Vec<DashboardSignal>, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::hours(24);
    let mut signals = Vec::new();

    signals.extend(run_failure_signals(conn, &since, &now).await?);
    signals.extend(learning_outcome_signals(conn, agent, &since, &now).await?);
    signals.extend(cost_spike_signals(river));
    signals.extend(curator_signals);

    if foreground_active_count > 0 {
        signals.push(DashboardSignal {
            id: format!("active_work:foreground:{generated_at}"),
            kind: "active_work".to_owned(),
            severity: "info".to_owned(),
            occurred_at: generated_at.to_owned(),
            title: "Foreground session active".to_owned(),
            detail: Some(format!(
                "{foreground_active_count} foreground session(s) active"
            )),
            source: Some("foreground".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        });
    }

    if matches!(sandbox.state.as_str(), "warn" | "unavailable") {
        signals.push(DashboardSignal {
            id: format!("health:sandbox:{generated_at}"),
            kind: "health".to_owned(),
            severity: if sandbox.state == "unavailable" {
                "bad".to_owned()
            } else {
                "warn".to_owned()
            },
            occurred_at: generated_at.to_owned(),
            title: "Sandbox needs attention".to_owned(),
            detail: sandbox.detail.clone(),
            source: Some("sandbox".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        });
    }

    signals.sort_by(|left, right| {
        match (
            parse_utc(&left.occurred_at).ok(),
            parse_utc(&right.occurred_at).ok(),
        ) {
            (Some(left_at), Some(right_at)) => right_at.cmp(&left_at),
            _ => right.occurred_at.cmp(&left.occurred_at),
        }
    });
    Ok(signals)
}

async fn run_failure_signals(
    conn: &Connection,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<DashboardSignal>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT id, kind, producer_ref, COALESCE(finished_at, updated_at, created_at)
         FROM async_runs
         WHERE status = 'failed'
           AND COALESCE(finished_at, updated_at, created_at) >= ?1
           AND COALESCE(finished_at, updated_at, created_at) <= ?2
         ORDER BY COALESCE(finished_at, updated_at, created_at) DESC",
    )?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .await?;

    let mut signals = Vec::new();
    for row in rows {
        let (id, kind, producer_ref, occurred_at) = row?;
        let occurred_at_utc = parse_utc(&occurred_at)?;
        if occurred_at_utc < *since || occurred_at_utc > *now {
            continue;
        }
        let detail = producer_ref
            .map(|producer_ref| format!("{kind}:{producer_ref}"))
            .unwrap_or_else(|| kind.clone());
        signals.push(DashboardSignal {
            id: format!("run_failure:{id}"),
            kind: "run_failure".to_owned(),
            severity: "bad".to_owned(),
            occurred_at: occurred_at_utc.to_rfc3339(),
            title: "Async run failed".to_owned(),
            detail: Some(detail),
            source: Some(kind),
            cost_usd: None,
            related_run_id: Some(id),
            related_skill_name: None,
            related_report_id: None,
        });
    }
    Ok(signals)
}

async fn learning_outcome_signals(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<DashboardSignal>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT id, action, skill_name, status, hint_outcome, COALESCE(summary, message), created_at
         FROM skill_learning_events
         WHERE agent_name = ?1
           AND phase = 'finish'
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC",
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

    let mut signals = Vec::new();
    for row in rows {
        let (id, action, skill_name, status, hint_outcome, detail, occurred_at) = row?;
        let occurred_at_utc = parse_utc(&occurred_at)?;
        if occurred_at_utc < *since || occurred_at_utc > *now {
            continue;
        }
        signals.push(DashboardSignal {
            id: format!("learning:{id}:{skill_name}"),
            kind: "learning_outcome".to_owned(),
            severity: learning_outcome_severity(status.as_deref(), hint_outcome.as_deref())
                .to_owned(),
            occurred_at: occurred_at_utc.to_rfc3339(),
            title: learning_outcome_title(&action, status.as_deref(), hint_outcome.as_deref())
                .to_owned(),
            detail,
            source: Some("learning_probe_writer".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: Some(skill_name),
            related_report_id: None,
        });
    }
    Ok(signals)
}

async fn cost_learning_river(
    conn: &Connection,
    generated_at: &str,
    agent: &str,
) -> Result<(CostLearningRiver, Vec<DashboardDataWarning>), ReadModelError> {
    let now = parse_utc(generated_at)?;
    let start_naive = (now.date_naive() - Duration::days(RIVER_DAYS - 1))
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(generated_at.to_owned()))?;
    let start_utc = Utc.from_utc_datetime(&start_naive);
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(&start_utc, &now);

    let mut points = (0..RIVER_DAYS)
        .map(|offset| CostLearningPoint {
            bucket: (now.date_naive() - Duration::days(RIVER_DAYS - 1 - offset))
                .format("%Y-%m-%d")
                .to_string(),
            total_cost_usd: 0.0,
            sources: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut by_bucket = BTreeMap::<String, usize>::new();
    for (idx, point) in points.iter().enumerate() {
        by_bucket.insert(point.bucket.clone(), idx);
    }

    let mut stmt = conn.prepare(
        "SELECT ts, source, total_cost_usd, num_turns, api_key_source
         FROM usage_events
         WHERE ts >= ?1 AND ts <= ?2
         ORDER BY ts ASC",
    )?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .await?;

    let mut source_totals = BTreeMap::<(String, String), UsageSourcePoint>::new();
    for row in rows {
        let (ts, source, cost, turns, api_key_source) = row?;
        let event_at = parse_utc(&ts)?;
        if event_at < start_utc || event_at > now {
            continue;
        }
        let bucket = event_at.date_naive().format("%Y-%m-%d").to_string();
        let Some(idx) = by_bucket.get(&bucket).copied() else {
            continue;
        };
        points[idx].total_cost_usd += cost;
        let source_point = source_totals
            .entry((bucket, source.clone()))
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
                // Token fields intentionally zero: the cost-learning river query
                // selects no token columns and does not render cache.
                input_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            });
        source_point.cost_usd += cost;
        if api_key_source == "none" {
            source_point.subscription_cost_usd += cost;
        } else {
            source_point.api_cost_usd += cost;
        }
        source_point.turns += turns.max(0) as u64;
        source_point.invocations += 1;
    }

    for point in &mut points {
        point.sources = source_totals
            .iter()
            .filter_map(|((bucket, _), source_point)| {
                (bucket == &point.bucket).then_some(source_point.clone())
            })
            .collect();
        point
            .sources
            .sort_by(|left, right| match left.source.cmp(&right.source) {
                std::cmp::Ordering::Equal => left.cost_usd.total_cmp(&right.cost_usd),
                ordering => ordering,
            });
    }

    let series = build_cost_series(&points);
    let markers = learning_markers(conn, agent, &start_utc, &now).await?;
    Ok((
        CostLearningRiver {
            window: RIVER_WINDOW.to_owned(),
            points,
            series,
            markers,
        },
        Vec::new(),
    ))
}

fn build_cost_series(points: &[CostLearningPoint]) -> Vec<CostLearningSeries> {
    let mut sources = BTreeMap::<String, BTreeMap<String, f64>>::new();
    for point in points {
        for source in &point.sources {
            sources
                .entry(source.source.clone())
                .or_default()
                .insert(point.bucket.clone(), source.cost_usd);
        }
    }

    sources
        .into_iter()
        .map(|(source, costs)| CostLearningSeries {
            source,
            points: points
                .iter()
                .map(|point| CostSeriesPoint {
                    bucket: point.bucket.clone(),
                    cost_usd: costs.get(&point.bucket).copied().unwrap_or(0.0),
                })
                .collect(),
        })
        .collect()
}

async fn learning_markers(
    conn: &Connection,
    agent: &str,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningMarker>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    let mut stmt = conn.prepare(
        "SELECT id, action, skill_name, status, hint_outcome, created_at
         FROM skill_learning_events
         WHERE agent_name = ?1
           AND phase = 'finish'
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .await?;

    let mut markers = Vec::new();
    for row in rows {
        let (id, action, skill_name, status, hint_outcome, occurred_at) = row?;
        let occurred_at_utc = parse_utc(&occurred_at)?;
        if occurred_at_utc < *since || occurred_at_utc > *now {
            continue;
        }
        markers.push(LearningMarker {
            id: format!("marker:{id}:{skill_name}"),
            occurred_at: occurred_at_utc.to_rfc3339(),
            kind: learning_outcome_kind(&action, status.as_deref(), hint_outcome.as_deref())
                .to_owned(),
            label: skill_name.clone(),
            severity: learning_outcome_severity(status.as_deref(), hint_outcome.as_deref())
                .to_owned(),
            skill_name: Some(skill_name),
            source: Some("learning_probe_writer".to_owned()),
            cost_usd: None,
        });
    }
    Ok(markers)
}

fn cost_spike_signals(river: &CostLearningRiver) -> Vec<DashboardSignal> {
    let mut non_zero = river
        .points
        .iter()
        .filter_map(|point| (point.total_cost_usd > 0.0).then_some(point.total_cost_usd))
        .collect::<Vec<_>>();
    if non_zero.len() < 2 {
        return Vec::new();
    }
    non_zero.sort_by(f64::total_cmp);
    let median = median(&non_zero);
    if median <= 0.0 {
        return Vec::new();
    }

    river
        .points
        .iter()
        .filter(|point| point.total_cost_usd > 0.0 && point.total_cost_usd >= median * 2.0)
        .map(|point| DashboardSignal {
            id: format!("cost_spike:{}", point.bucket),
            kind: "cost_spike".to_owned(),
            severity: "warn".to_owned(),
            occurred_at: format!("{}T00:00:00Z", point.bucket),
            title: "Cost spike detected".to_owned(),
            detail: Some(format!("Daily cost reached ${:.2}", point.total_cost_usd)),
            source: Some("usage_events".to_owned()),
            cost_usd: Some(point.total_cost_usd),
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        })
        .collect()
}

fn median(sorted: &[f64]) -> f64 {
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

type CuratorProjection = (
    Vec<DashboardSignal>,
    Vec<LearningMarker>,
    Vec<DashboardDataWarning>,
);

async fn curator_projection(
    conn: &Connection,
    generated_at: &DateTime<Utc>,
) -> Result<CuratorProjection, ReadModelError> {
    let Some(state) = conn
        .query_row(
            "SELECT last_run_at, last_run_status, consecutive_failures,
                circuit_open_until, last_spike_evidence_json
         FROM curator_state WHERE agent_singleton_id = 1",
            [],
            |row| {
                Ok(CuratorStateRow {
                    last_run_at: row.get(0)?,
                    last_run_status: row.get(1)?,
                    consecutive_failures: row.get::<_, i64>(2)?,
                    circuit_open_until: row.get(3)?,
                    last_spike_evidence_json: row.get(4)?,
                })
            },
        )
        .await
        .optional()?
    else {
        return Ok((
            Vec::new(),
            Vec::new(),
            vec![DashboardDataWarning {
                source: "curator_state".to_owned(),
                kind: "unavailable".to_owned(),
                message: "curator state row is absent".to_owned(),
            }],
        ));
    };

    let mut signals = Vec::new();
    let mut markers = Vec::new();
    let mut warnings = Vec::new();

    if let (Some(last_run_at), Some(last_run_status)) = (
        state.last_run_at.as_deref(),
        state.last_run_status.as_deref(),
    ) {
        match parse_curator_timestamp("curator_state.last_run_at", last_run_at, &mut warnings) {
            Some(last_run_at_utc) if last_run_at_utc <= *generated_at => {
                signals.push(DashboardSignal {
                    id: format!("curator:last_run:{}", last_run_at_utc.to_rfc3339()),
                    kind: "curator".to_owned(),
                    severity: curator_status_severity(last_run_status).to_owned(),
                    occurred_at: last_run_at_utc.to_rfc3339(),
                    title: curator_status_title(last_run_status).to_owned(),
                    detail: Some(format!(
                        "status={last_run_status}; consecutive_failures={}",
                        state.consecutive_failures
                    )),
                    source: Some("curator_state".to_owned()),
                    cost_usd: None,
                    related_run_id: None,
                    related_skill_name: None,
                    related_report_id: None,
                });
            }
            _ => {}
        }
    }

    if let Some(circuit_open_until) = state.circuit_open_until.as_deref() {
        match parse_curator_timestamp(
            "curator_state.circuit_open_until",
            circuit_open_until,
            &mut warnings,
        ) {
            Some(circuit_open_until_utc) if circuit_open_until_utc > *generated_at => {
                signals.push(DashboardSignal {
                    id: format!("curator:circuit:{}", circuit_open_until_utc.to_rfc3339()),
                    kind: "curator".to_owned(),
                    severity: "warn".to_owned(),
                    occurred_at: generated_at.to_rfc3339(),
                    title: "Curator circuit open".to_owned(),
                    detail: Some(format!(
                        "open_until={}",
                        circuit_open_until_utc.to_rfc3339()
                    )),
                    source: Some("curator_state".to_owned()),
                    cost_usd: None,
                    related_run_id: None,
                    related_skill_name: None,
                    related_report_id: None,
                });
            }
            _ => {}
        }
    }

    if let Some(raw) = state.last_spike_evidence_json.as_deref() {
        match curator_trigger_evidence(raw) {
            Ok(Some(evidence)) if evidence.occurred_at <= *generated_at => {
                signals.push(curator_trigger_signal(&evidence));
                if evidence.trigger == "cost_spike" {
                    signals.push(DashboardSignal {
                        id: format!("cost_spike:curator:{}", evidence.occurred_at.to_rfc3339()),
                        kind: "cost_spike".to_owned(),
                        severity: "warn".to_owned(),
                        occurred_at: evidence.occurred_at.to_rfc3339(),
                        title: "Curator cost spike evidence".to_owned(),
                        detail: evidence.detail.clone(),
                        source: Some("curator_state".to_owned()),
                        cost_usd: evidence.cost_usd,
                        related_run_id: None,
                        related_skill_name: None,
                        related_report_id: None,
                    });
                    markers.push(LearningMarker {
                        id: format!("marker:curator:{}", evidence.occurred_at.to_rfc3339()),
                        occurred_at: evidence.occurred_at.to_rfc3339(),
                        kind: "cost_spike".to_owned(),
                        label: "Cost spike".to_owned(),
                        severity: "warn".to_owned(),
                        skill_name: None,
                        source: Some("curator_state".to_owned()),
                        cost_usd: evidence.cost_usd,
                    });
                }
            }
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    source = "curator_state.last_spike_evidence_json",
                    "dashboard: skipped malformed curator trigger evidence JSON"
                );
                warnings.push(DashboardDataWarning {
                    source: "curator_state.last_spike_evidence_json".to_owned(),
                    kind: "malformed_json".to_owned(),
                    message: "skipped malformed curator spike evidence JSON".to_owned(),
                });
            }
        }
    }

    Ok((signals, markers, warnings))
}

struct CuratorStateRow {
    last_run_at: Option<String>,
    last_run_status: Option<String>,
    consecutive_failures: i64,
    circuit_open_until: Option<String>,
    last_spike_evidence_json: Option<String>,
}

fn parse_curator_timestamp(
    source: &str,
    value: &str,
    warnings: &mut Vec<DashboardDataWarning>,
) -> Option<DateTime<Utc>> {
    match parse_utc(value) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            tracing::warn!(
                source,
                value,
                error = %error,
                "dashboard: skipped malformed curator timestamp"
            );
            warnings.push(DashboardDataWarning {
                source: source.to_owned(),
                kind: "partial_data".to_owned(),
                message: format!("skipped malformed curator timestamp at {source}"),
            });
            None
        }
    }
}

struct CuratorTriggerEvidence {
    trigger: String,
    occurred_at: DateTime<Utc>,
    cost_usd: Option<f64>,
    detail: Option<String>,
}

fn curator_trigger_evidence(raw: &str) -> Result<Option<CuratorTriggerEvidence>, ()> {
    let value = serde_json::from_str::<serde_json::Value>(raw).map_err(|_| ())?;
    let Some(trigger) = value.get("trigger").and_then(|field| field.as_str()) else {
        return Ok(None);
    };
    let computed_at = value
        .get("computed_at")
        .and_then(|field| field.as_str())
        .ok_or(())?;
    let occurred_at = DateTime::parse_from_rfc3339(computed_at)
        .map_err(|_| ())?
        .with_timezone(&Utc);
    let details = value.get("details");
    let cost_usd = (trigger == "cost_spike")
        .then(|| {
            details
                .and_then(|details| details.get("today_cost_usd"))
                .and_then(|field| field.as_f64())
        })
        .flatten();
    let detail = curator_trigger_detail(trigger, details);
    Ok(Some(CuratorTriggerEvidence {
        trigger: trigger.to_owned(),
        occurred_at,
        cost_usd,
        detail,
    }))
}

fn curator_trigger_signal(evidence: &CuratorTriggerEvidence) -> DashboardSignal {
    DashboardSignal {
        id: format!(
            "curator:trigger:{}:{}",
            evidence.trigger,
            evidence.occurred_at.to_rfc3339()
        ),
        kind: "curator".to_owned(),
        severity: "info".to_owned(),
        occurred_at: evidence.occurred_at.to_rfc3339(),
        title: "Curator triggered".to_owned(),
        detail: evidence.detail.clone(),
        source: Some("curator_state".to_owned()),
        cost_usd: evidence.cost_usd,
        related_run_id: None,
        related_skill_name: None,
        related_report_id: None,
    }
}

fn curator_trigger_detail(trigger: &str, details: Option<&serde_json::Value>) -> Option<String> {
    match trigger {
        "cost_spike" => {
            let cost = details
                .and_then(|details| details.get("today_cost_usd"))
                .and_then(|field| field.as_f64());
            let baseline = details
                .and_then(|details| details.get("baseline_p50_usd"))
                .and_then(|field| field.as_f64());
            match (cost, baseline) {
                (Some(cost), Some(baseline)) => Some(format!(
                    "cost_spike: today=${cost:.2}; baseline_p50=${baseline:.2}"
                )),
                (Some(cost), None) => Some(format!("cost_spike: today=${cost:.2}")),
                _ => Some("cost_spike".to_owned()),
            }
        }
        "skill_change_count" => {
            let count = details
                .and_then(|details| details.get("count"))
                .and_then(|field| field.as_i64());
            let threshold = details
                .and_then(|details| details.get("threshold"))
                .and_then(|field| field.as_i64());
            match (count, threshold) {
                (Some(count), Some(threshold)) => Some(format!(
                    "skill_change_count: count={count}; threshold={threshold}"
                )),
                _ => Some("skill_change_count".to_owned()),
            }
        }
        "time_fallback" => {
            let interval_hours = details
                .and_then(|details| details.get("interval_hours"))
                .and_then(|field| field.as_i64());
            interval_hours
                .map(|interval_hours| format!("time_fallback: interval_hours={interval_hours}"))
                .or_else(|| Some("time_fallback".to_owned()))
        }
        other => Some(other.to_owned()),
    }
}

fn curator_status_severity(status: &str) -> &'static str {
    let normalized = status.to_ascii_lowercase();
    if normalized.contains("fail") || normalized.contains("error") {
        "bad"
    } else if normalized.contains("circuit") {
        "warn"
    } else {
        "info"
    }
}

fn curator_status_title(status: &str) -> &'static str {
    let normalized = status.to_ascii_lowercase();
    if normalized.contains("fail") || normalized.contains("error") {
        "Curator failed"
    } else if normalized.contains("skip") {
        "Curator skipped"
    } else if normalized.contains("trigger") || normalized.contains("run") {
        "Curator triggered"
    } else {
        "Curator state updated"
    }
}

async fn recent_failed_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<Vec<RunSummary>, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::hours(24);
    super::run_summary::failed_runs_in_window(conn, &now, &since, None).await
}

async fn learning_candidate_count(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<i64, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::hours(24);
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(&since, &now);
    let mut stmt = conn.prepare(
        "SELECT created_at
         FROM skill_learning_events
         WHERE agent_name = ?1
           AND phase = 'finish'
           AND status IN ('created', 'updated')
           AND created_at >= ?2
           AND created_at <= ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            row.get::<_, String>(0)
        })
        .await?;
    count_parsed_window_rows(rows, &since, &now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    async fn fixture() -> (TempDir, right_db::Connection) {
        let dir = tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true)
            .await
            .expect("open db");
        (dir, conn)
    }

    #[tokio::test]
    async fn dashboard_overview_summarizes_activity_usage_and_learning() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, started_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-active', 'background', 'handoff', 'session-active', 123,
                'running', '2026-05-20T08:00:00Z', 1, 'pending',
                '2026-05-20T08:00:00Z', '2026-05-20T08:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-failed', 'cron', 'daily', 'session-failed', 123,
                'failed', '2026-05-20T07:00:00Z', 1, 1, 'pending',
                '2026-05-20T07:00:00Z', '2026-05-20T07:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json
             ) VALUES (
                '2026-05-20T08:30:00Z', 'cron', NULL, NULL, 'daily',
                'session-active', 1.25, 1, '[]'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                hint_outcome, reason, message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-1', 'alpha', 'create', 'rightx-debugging', 'finish', 'created',
                'applied_as_hinted', 'captured workflow', 'Created skill',
                'Reusable workflow', '[]', '2026-05-20T09:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-20T10:00:00Z".to_string(),
                foreground_active_count: 2,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: Some("sandbox alpha".to_string()),
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.active_runs, 3);
        assert_eq!(response.recent_failures, 1);
        assert_eq!(response.today_cost_usd, 1.25);
        assert_eq!(response.learning_candidates_24h, 1);
        assert_eq!(response.doctor.state, "not_loaded");
        assert_eq!(response.sandbox.state, "configured");
    }

    #[tokio::test]
    async fn dashboard_overview_builds_signal_timeline_and_cost_river() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-failed', 'cron', 'daily', 'session-failed', 123,
                'failed', '2026-05-23T07:00:00Z', 1, 1, 'pending',
                '2026-05-23T07:00:00Z', '2026-05-23T07:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json, api_key_source
             ) VALUES
                ('2026-05-21T08:00:00Z', 'interactive', 1, 0, NULL, 's1', 0.10, 1, '{}', 'none'),
                ('2026-05-22T08:00:00Z', 'interactive', 1, 0, NULL, 's2', 0.10, 1, '{}', 'none'),
                ('2026-05-23T08:00:00Z', 'learning_probe_writer', 1, 0, NULL, 's3', 1.00, 1, '{}', 'none')",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-2', 'alpha', 'create', 'rightx-debug', 'finish',
                'created', 'Learned debugging.', 'Reusable debugging workflow.',
                '[]', '2026-05-23T09:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 1,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: Some("sandbox alpha".to_string()),
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.len() >= 3);
        assert!(
            response
                .signals
                .iter()
                .any(|signal| signal.kind == "learning_outcome")
        );
        assert!(
            response
                .signals
                .iter()
                .any(|signal| signal.kind == "cost_spike")
        );
        assert!(
            response
                .signals
                .iter()
                .any(|signal| signal.kind == "run_failure")
        );
        assert_eq!(response.cost_learning_river.window, "last_30_days");
        assert!(
            response
                .cost_learning_river
                .series
                .iter()
                .any(|series| series.source == "learning_probe_writer")
        );
        assert_eq!(
            response.cost_learning_river.markers[0].kind,
            "skill_created"
        );
    }

    #[tokio::test]
    async fn dashboard_overview_uses_contract_signal_values() {
        let (_dir, conn) = fixture().await;

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 2,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "unavailable".to_string(),
                    detail: Some("missing sandbox".to_string()),
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.iter().all(|signal| matches!(
            signal.kind.as_str(),
            "cost_spike"
                | "learning_outcome"
                | "curator"
                | "run_failure"
                | "health"
                | "active_work"
        )));
        assert!(
            response
                .signals
                .iter()
                .all(|signal| matches!(signal.severity.as_str(), "info" | "warn" | "bad"))
        );
        let active = response
            .signals
            .iter()
            .find(|signal| signal.kind == "active_work")
            .expect("active work signal");
        assert_eq!(active.id, "active_work:foreground:2026-05-23T10:00:00Z");
        assert_eq!(active.occurred_at, "2026-05-23T10:00:00Z");
        assert_eq!(active.severity, "info");
        let health = response
            .signals
            .iter()
            .find(|signal| signal.kind == "health")
            .expect("health signal");
        assert_eq!(health.id, "health:sandbox:2026-05-23T10:00:00Z");
        assert_eq!(health.occurred_at, "2026-05-23T10:00:00Z");
        assert_eq!(health.severity, "bad");
    }

    #[tokio::test]
    async fn dashboard_overview_excludes_future_rows_from_snapshot_counts() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES
                ('run-current', 'cron', 'daily', 'session-current', 123,
                 'failed', '2026-05-23T09:00:00Z', 1, 1, 'pending',
                 '2026-05-23T09:00:00Z', '2026-05-23T09:00:00Z'),
                ('run-future', 'cron', 'daily', 'session-future', 123,
                 'failed', '2026-05-23T11:00:00Z', 1, 1, 'pending',
                 '2026-05-23T11:00:00Z', '2026-05-23T11:00:00Z')",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                hint_outcome, reason, message, summary, event_refs_json, created_at
             ) VALUES
                ('inv-current', 'alpha', 'create', 'rightx-current', 'finish', 'created',
                 'applied_as_hinted', 'captured workflow', 'Created skill',
                 'Reusable workflow', '[]', '2026-05-23T09:00:00Z'),
                ('inv-future', 'alpha', 'create', 'rightx-future', 'finish', 'created',
                 'applied_as_hinted', 'future workflow', 'Created skill',
                 'Future workflow', '[]', '2026-05-23T11:00:00Z')",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json, api_key_source
             ) VALUES
                ('2026-05-23T09:00:00Z', 'interactive', 1, 0, NULL, 'usage-current', 1.25, 1, '{}', 'none'),
                ('2026-05-23T11:00:00Z', 'interactive', 1, 0, NULL, 'usage-future', 9.75, 1, '{}', 'none')",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(response.recent_failures, 1);
        assert_eq!(response.learning_candidates_24h, 1);
        assert_eq!(response.today_cost_usd, 1.25);
    }

    #[tokio::test]
    async fn dashboard_overview_excludes_future_active_async_runs() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, started_at, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES
                ('run-current-active', 'background', 'handoff', 'session-current', 123,
                 'running', '2026-05-23T09:00:00Z', 1, 'pending',
                 '2026-05-23T09:00:00Z', '2026-05-23T09:00:00Z'),
                ('run-future-active', 'background', 'handoff', 'session-future', 123,
                 'queued', NULL, 1, 'pending',
                 '2026-05-23T11:00:00Z', '2026-05-23T11:00:00Z')",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 2,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(response.active_runs, 3);
    }

    #[tokio::test]
    async fn dashboard_overview_projects_refused_learning_outcomes() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                hint_outcome, message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-refused', 'alpha', 'create', 'rightx-refused', 'finish',
                'aborted', 'refused', 'Refused to create skill.', 'Insufficient evidence.',
                '[]', '2026-05-23T09:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.iter().any(|signal| {
            signal.kind == "learning_outcome"
                && signal.severity == "warn"
                && signal.title == "Learning refused"
                && signal.related_skill_name.as_deref() == Some("rightx-refused")
        }));
        assert!(response.cost_learning_river.markers.iter().any(|marker| {
            marker.kind == "skill_refused"
                && marker.severity == "warn"
                && marker.skill_name.as_deref() == Some("rightx-refused")
        }));
    }

    #[tokio::test]
    async fn dashboard_overview_cost_spikes_use_true_even_median() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, model_usage_json, api_key_source
             ) VALUES
                ('2026-05-20T08:00:00Z', 'interactive', 1, 0, NULL, 's1', 1.00, 1, '{}', 'none'),
                ('2026-05-21T08:00:00Z', 'interactive', 1, 0, NULL, 's2', 3.00, 1, '{}', 'none'),
                ('2026-05-22T08:00:00Z', 'interactive', 1, 0, NULL, 's3', 5.00, 1, '{}', 'none'),
                ('2026-05-23T08:00:00Z', 'interactive', 1, 0, NULL, 's4', 8.00, 1, '{}', 'none')",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(
            response
                .signals
                .iter()
                .any(|signal| signal.kind == "cost_spike"
                    && signal.cost_usd == Some(8.0)
                    && signal.severity == "warn")
        );
    }

    #[tokio::test]
    async fn dashboard_overview_projects_curator_state_and_warns_on_malformed_evidence() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (
                1, '2026-05-23T09:30:00Z', 'failure', 2,
                '2026-05-23T12:00:00Z',
                '{\"trigger\":\"cost_spike\",\"computed_at\":\"2026-05-23T09:00:00Z\",\"details\":{\"today_cost_usd\":2.5,\"baseline_p50_usd\":0.5,\"k\":3.0,\"min_floor_usd\":0.05}}'
             )",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.iter().any(|signal| {
            signal.kind == "curator"
                && signal.severity == "bad"
                && signal.occurred_at == "2026-05-23T09:30:00+00:00"
        }));
        assert!(response.signals.iter().any(|signal| {
            signal.kind == "curator"
                && signal.severity == "warn"
                && signal.id == "curator:circuit:2026-05-23T12:00:00+00:00"
        }));
        assert!(response.signals.iter().any(|signal| {
            signal.kind == "cost_spike"
                && signal.source.as_deref() == Some("curator_state")
                && signal.cost_usd == Some(2.5)
        }));
        assert!(response.cost_learning_river.markers.iter().any(|marker| {
            marker.kind == "cost_spike"
                && marker.source.as_deref() == Some("curator_state")
                && marker.cost_usd == Some(2.5)
        }));

        conn.execute(
            "UPDATE curator_state
             SET last_spike_evidence_json = '{not json'",
            [],
        )
        .await
        .unwrap();
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.warnings.iter().any(|warning| {
            warning.source == "curator_state.last_spike_evidence_json"
                && warning.kind == "malformed_json"
        }));
    }

    #[tokio::test]
    async fn dashboard_overview_projects_non_cost_curator_trigger_evidence() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (
                1, NULL, NULL, 0, NULL,
                '{\"trigger\":\"skill_change_count\",\"computed_at\":\"2026-05-23T09:00:00Z\",\"details\":{\"count\":7,\"threshold\":5}}'
             )",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.iter().any(|signal| {
            signal.kind == "curator"
                && signal.id == "curator:trigger:skill_change_count:2026-05-23T09:00:00+00:00"
                && signal.title == "Curator triggered"
                && signal.detail.as_deref() == Some("skill_change_count: count=7; threshold=5")
        }));
        assert!(!response.signals.iter().any(|signal| {
            signal.kind == "cost_spike" && signal.source.as_deref() == Some("curator_state")
        }));

        conn.execute(
            "UPDATE curator_state
             SET last_spike_evidence_json =
                '{\"trigger\":\"time_fallback\",\"computed_at\":\"2026-05-23T09:30:00Z\",\"details\":{\"interval_hours\":24}}'",
            [],
        )
        .await
        .unwrap();
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.iter().any(|signal| {
            signal.kind == "curator"
                && signal.id == "curator:trigger:time_fallback:2026-05-23T09:30:00+00:00"
                && signal.detail.as_deref() == Some("time_fallback: interval_hours=24")
        }));
    }

    #[tokio::test]
    async fn dashboard_overview_logs_malformed_curator_evidence() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (1, NULL, NULL, 0, NULL, '{not json')",
            [],
        )
        .await
        .unwrap();
        let subscriber = WarnCounter::default();
        let warn_count = subscriber.warn_count.clone();

        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.warnings.iter().any(|warning| {
            warning.source == "curator_state.last_spike_evidence_json"
                && warning.kind == "malformed_json"
        }));
        assert_eq!(warn_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dashboard_overview_warns_on_malformed_curator_timestamps() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT OR REPLACE INTO curator_state (
                agent_singleton_id, last_run_at, last_run_status,
                consecutive_failures, circuit_open_until, last_spike_evidence_json
             ) VALUES (
                1, 'not-a-time', 'failure', 0, 'also-not-a-time', NULL
             )",
            [],
        )
        .await
        .unwrap();
        let subscriber = WarnCounter::default();
        let warn_count = subscriber.warn_count.clone();

        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.warnings.iter().any(|warning| {
            warning.source == "curator_state.last_run_at" && warning.kind == "partial_data"
        }));
        assert!(response.warnings.iter().any(|warning| {
            warning.source == "curator_state.circuit_open_until" && warning.kind == "partial_data"
        }));
        assert_eq!(warn_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[derive(Default)]
    struct WarnCounter {
        warn_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.warn_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[tokio::test]
    async fn recent_failed_runs_matches_failure_count_and_lists_each_run() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-f1', 'cron', 'daily', 'session-f1', 123,
                'failed', '2026-05-31T11:00:00Z', 1, 1, 'pending',
                '2026-05-31T11:00:00Z', '2026-05-31T11:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                status, finished_at, exit_code, delivery_required, delivery_status,
                created_at, updated_at
             ) VALUES (
                'run-f2', 'background', 'handoff', 'session-f2', 123,
                'failed', '2026-05-31T11:30:00Z', 1, 1, 'pending',
                '2026-05-31T11:30:00Z', '2026-05-31T11:30:00Z'
             )",
            [],
        )
        .await
        .unwrap();

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-31T12:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(response.recent_failures, 2);
        assert_eq!(response.recent_failed_runs.len(), 2);
        assert_eq!(
            response.recent_failed_runs.len() as i64,
            response.recent_failures
        );
        // newest first
        assert_eq!(response.recent_failed_runs[0].id, "run-f2");
        assert_eq!(response.recent_failed_runs[1].id, "run-f1");
    }

    #[tokio::test]
    async fn dashboard_overview_warns_when_curator_state_row_is_absent() {
        let (_dir, conn) = fixture().await;

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-23T10:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert!(response.signals.is_empty());
        assert!(response.cost_learning_river.markers.is_empty());
        assert_eq!(
            response.warnings,
            vec![DashboardDataWarning {
                source: "curator_state".to_string(),
                kind: "unavailable".to_string(),
                message: "curator state row is absent".to_string(),
            }]
        );
    }
}
