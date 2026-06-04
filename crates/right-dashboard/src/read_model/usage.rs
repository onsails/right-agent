use std::collections::{BTreeMap, BTreeSet};

use crate::api_types::{
    CostSeriesPoint, DashboardDataWarning, UsageDailyPoint, UsageModelSummary,
    UsageOverviewResponse, UsageSourcePoint, UsageSourceSeries, UsageSourceSummary, UsageWindow,
};
use chrono::{DateTime, Duration, Utc};
use right_db::{Connection, params};

use super::ReadModelError;

#[path = "usage_time.rs"]
mod usage_time;

const SOURCES: [&str; 7] = [
    "interactive",
    "cron",
    "reflection",
    "idle_compaction",
    "learning_prefilter",
    "learning_probe_writer",
    "learning_curator",
];
const DEFAULT_CHART_WINDOW: &str = "last_30_days";
const DAILY_SERIES_DAYS: i64 = 30;

pub struct UsageOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub timezone: Option<String>,
}

pub async fn usage_overview(
    conn: &Connection,
    input: UsageOverviewInput,
) -> Result<UsageOverviewResponse, ReadModelError> {
    let clock = usage_time::resolve_usage_clock(&input.generated_at, input.timezone.as_deref())?;
    let unknown_sources = unknown_usage_sources(conn, &clock.now_utc).await?;

    let mut windows = Vec::new();
    for range in usage_time::usage_window_ranges(&clock)? {
        windows.push(build_window(conn, &range, &unknown_sources).await?);
    }

    let (daily_series, mut warnings) = build_daily_series(conn, &clock).await?;
    let source_series = build_source_series(&daily_series, &unknown_sources);
    warnings.extend(clock.warnings);
    warnings.extend(unknown_source_warnings(&unknown_sources));

    Ok(UsageOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        timezone: clock.timezone,
        windows,
        selected_window: DEFAULT_CHART_WINDOW.to_owned(),
        daily_series,
        source_series,
        warnings,
    })
}

async fn build_window(
    conn: &Connection,
    range: &usage_time::UsageWindowRange,
    unknown_sources: &[String],
) -> Result<UsageWindow, ReadModelError> {
    let since = range.since_utc.as_ref();
    let until = &range.until_utc;
    let mut sources = Vec::with_capacity(SOURCES.len() + unknown_sources.len());
    for source in SOURCES {
        sources.push(aggregate_source(conn, source, since, until).await?);
    }
    for source in unknown_sources {
        let summary = aggregate_source(conn, source, since, until).await?;
        if summary.invocations > 0 {
            sources.push(summary);
        }
    }
    let per_model = aggregate_window_models(&sources);

    Ok(UsageWindow {
        key: range.key.to_owned(),
        label: range.label.to_owned(),
        range_start: range.range_start.clone(),
        range_end: range.range_end.clone(),
        range_label: range.range_label.clone(),
        total_cost_usd: sources.iter().map(|source| source.cost_usd).sum(),
        subscription_cost_usd: sources
            .iter()
            .map(|source| source.subscription_cost_usd)
            .sum(),
        api_cost_usd: sources.iter().map(|source| source.api_cost_usd).sum(),
        turns: sources.iter().map(|source| source.turns).sum(),
        invocations: sources.iter().map(|source| source.invocations).sum(),
        input_tokens: sources.iter().map(|source| source.input_tokens).sum(),
        output_tokens: sources.iter().map(|source| source.output_tokens).sum(),
        cache_creation_tokens: sources
            .iter()
            .map(|source| source.cache_creation_tokens)
            .sum(),
        cache_read_tokens: sources.iter().map(|source| source.cache_read_tokens).sum(),
        web_search_requests: sources
            .iter()
            .map(|source| source.web_search_requests)
            .sum(),
        web_fetch_requests: sources.iter().map(|source| source.web_fetch_requests).sum(),
        per_model,
        sources,
        budget_skip_count: budget_skip_count(conn, since, until).await?,
    })
}

async fn budget_skip_count(
    conn: &Connection,
    since: Option<&DateTime<Utc>>,
    until: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let until_str = until.to_rfc3339();
    let count = if let Some(since) = since {
        let since_str = since.to_rfc3339();
        conn.query_row(
            "SELECT COUNT(*) FROM learning_skip WHERE reason='budget' AND ts >= ?1 AND ts <= ?2",
            right_db::params![since_str, until_str],
            |r| r.get(0),
        )
        .await?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM learning_skip WHERE reason='budget' AND ts <= ?1",
            right_db::params![until_str],
            |r| r.get(0),
        )
        .await?
    };
    Ok(count)
}

async fn build_daily_series(
    conn: &Connection,
    clock: &usage_time::UsageClock,
) -> Result<(Vec<UsageDailyPoint>, Vec<DashboardDataWarning>), ReadModelError> {
    let mut warnings = Vec::new();
    let chart_start_utc = usage_time::chart_start_utc(clock, DAILY_SERIES_DAYS)?;
    let coarse_since = (chart_start_utc - Duration::days(1)).to_rfc3339();
    let coarse_until = (clock.now_utc + Duration::days(1)).to_rfc3339();

    let mut points = usage_time::local_chart_dates(clock, DAILY_SERIES_DAYS)?
        .into_iter()
        .map(|date| UsageDailyPoint {
            date,
            total_cost_usd: 0.0,
            subscription_cost_usd: 0.0,
            api_cost_usd: 0.0,
            turns: 0,
            invocations: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            sources: Vec::new(),
            models: Vec::new(),
        })
        .collect::<Vec<_>>();

    let mut by_date = BTreeMap::<String, usize>::new();
    for (idx, point) in points.iter().enumerate() {
        by_date.insert(point.date.clone(), idx);
    }

    let mut stmt = conn.prepare(
        "SELECT ts, source, total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
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
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .await?;

    let mut source_totals = BTreeMap::<(String, String), UsageSourcePoint>::new();
    let mut model_totals = BTreeMap::<String, BTreeMap<String, UsageModelSummary>>::new();

    for row in rows {
        let (
            ts,
            source,
            cost,
            turns,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            web_search_requests,
            web_fetch_requests,
            model_usage_json,
            api_key_source,
        ) = row?;
        let event_at = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
        if event_at < chart_start_utc || event_at > clock.now_utc {
            continue;
        }

        let date = usage_time::local_date_label(&event_at, &clock.tz);
        let Some(idx) = by_date.get(&date).copied() else {
            continue;
        };

        let point = &mut points[idx];
        point.total_cost_usd += cost;
        if api_key_source == "none" {
            point.subscription_cost_usd += cost;
        } else {
            point.api_cost_usd += cost;
        }
        point.turns += turns.max(0) as u64;
        point.invocations += 1;
        point.input_tokens += input_tokens.max(0) as u64;
        point.output_tokens += output_tokens.max(0) as u64;
        point.cache_creation_tokens += cache_creation_tokens.max(0) as u64;
        point.cache_read_tokens += cache_read_tokens.max(0) as u64;
        point.web_search_requests += web_search_requests.max(0) as u64;
        point.web_fetch_requests += web_fetch_requests.max(0) as u64;

        let source_entry = source_totals
            .entry((date.clone(), source.clone()))
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            });
        source_entry.cost_usd += cost;
        if api_key_source == "none" {
            source_entry.subscription_cost_usd += cost;
        } else {
            source_entry.api_cost_usd += cost;
        }
        source_entry.turns += turns.max(0) as u64;
        source_entry.invocations += 1;
        source_entry.input_tokens += input_tokens.max(0) as u64;
        source_entry.output_tokens += output_tokens.max(0) as u64;
        source_entry.cache_creation_tokens += cache_creation_tokens.max(0) as u64;
        source_entry.cache_read_tokens += cache_read_tokens.max(0) as u64;

        match parse_model_usage_for_daily(&model_usage_json) {
            Ok(models) => {
                let date_models = model_totals.entry(date).or_default();
                for model in models {
                    let entry = date_models.entry(model.model.clone()).or_insert_with(|| {
                        UsageModelSummary {
                            model: model.model.clone(),
                            cost_usd: 0.0,
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_creation_tokens: 0,
                            cache_read_tokens: 0,
                        }
                    });
                    entry.cost_usd += model.cost_usd;
                    entry.input_tokens += model.input_tokens;
                    entry.output_tokens += model.output_tokens;
                    entry.cache_creation_tokens += model.cache_creation_tokens;
                    entry.cache_read_tokens += model.cache_read_tokens;
                }
            }
            Err(_) => warnings.push(DashboardDataWarning {
                source: "usage_events.model_usage_json".to_owned(),
                kind: "malformed_json".to_owned(),
                message: format!("skipped malformed model usage JSON for usage event at {ts}"),
            }),
        }
    }

    for point in &mut points {
        point.sources = source_totals
            .iter()
            .filter_map(|((date, _source), value)| (date == &point.date).then_some(value.clone()))
            .collect();
        sort_source_points(&mut point.sources);
        point.models = model_totals
            .remove(&point.date)
            .map(|models| {
                let mut rows = models.into_values().collect::<Vec<_>>();
                sort_models(&mut rows);
                rows
            })
            .unwrap_or_default();
    }

    Ok((points, warnings))
}

fn build_source_series(
    points: &[UsageDailyPoint],
    unknown_sources: &[String],
) -> Vec<UsageSourceSeries> {
    let mut totals = BTreeMap::<(&str, &str), f64>::new();
    for point in points {
        for source in &point.sources {
            totals.insert(
                (source.source.as_str(), point.date.as_str()),
                source.cost_usd,
            );
        }
    }

    let mut source_names = SOURCES
        .iter()
        .map(|source| (*source).to_owned())
        .collect::<Vec<_>>();
    for source in unknown_sources {
        if !source_names.iter().any(|existing| existing == source) {
            source_names.push(source.clone());
        }
    }
    for point in points {
        for source in &point.sources {
            if !source_names
                .iter()
                .any(|existing| existing == &source.source)
            {
                source_names.push(source.source.clone());
            }
        }
    }
    sort_source_names(&mut source_names);

    source_names
        .iter()
        .map(|source| UsageSourceSeries {
            source: source.clone(),
            points: points
                .iter()
                .map(|point| CostSeriesPoint {
                    bucket: point.date.clone(),
                    cost_usd: totals
                        .get(&(source.as_str(), point.date.as_str()))
                        .copied()
                        .unwrap_or(0.0),
                })
                .collect(),
        })
        .collect()
}

fn parse_model_usage_for_daily(raw: &str) -> Result<Vec<UsageModelSummary>, ()> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| ())?;
    let Some(models) = value.as_object() else {
        return Err(());
    };
    Ok(models
        .iter()
        .map(|(model, fields)| UsageModelSummary {
            model: model.clone(),
            cost_usd: field_f64(fields, "costUSD"),
            input_tokens: field_u64(fields, "inputTokens"),
            output_tokens: field_u64(fields, "outputTokens"),
            cache_creation_tokens: field_u64(fields, "cacheCreationInputTokens"),
            cache_read_tokens: field_u64(fields, "cacheReadInputTokens"),
        })
        .collect())
}

fn aggregate_window_models(sources: &[UsageSourceSummary]) -> Vec<UsageModelSummary> {
    let mut totals: BTreeMap<String, UsageModelSummary> = BTreeMap::new();
    for source in sources {
        for model in &source.per_model {
            let entry = totals
                .entry(model.model.clone())
                .or_insert_with(|| UsageModelSummary {
                    model: model.model.clone(),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                });
            entry.cost_usd += model.cost_usd;
            entry.input_tokens += model.input_tokens;
            entry.output_tokens += model.output_tokens;
            entry.cache_creation_tokens += model.cache_creation_tokens;
            entry.cache_read_tokens += model.cache_read_tokens;
        }
    }
    let mut rows = totals.into_values().collect::<Vec<_>>();
    sort_models(&mut rows);
    rows
}

fn sort_source_points(rows: &mut [UsageSourcePoint]) {
    rows.sort_by(
        |left, right| match (source_rank(&left.source), source_rank(&right.source)) {
            (Some(left_rank), Some(right_rank)) => left_rank.cmp(&right_rank),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.source.cmp(&right.source),
        },
    );
}

fn sort_source_names(rows: &mut [String]) {
    rows.sort_by(
        |left, right| match (source_rank(left), source_rank(right)) {
            (Some(left_rank), Some(right_rank)) => left_rank.cmp(&right_rank),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    );
}

fn source_rank(source: &str) -> Option<usize> {
    SOURCES
        .iter()
        .position(|known_source| *known_source == source)
}

async fn unknown_usage_sources(
    conn: &Connection,
    generated_at: &DateTime<Utc>,
) -> Result<Vec<String>, ReadModelError> {
    let coarse_until = (*generated_at + Duration::days(1)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT source, ts
         FROM usage_events
         WHERE ts <= ?1
         ORDER BY source ASC, ts ASC",
    )?;
    let rows = stmt
        .query_map(params![coarse_until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .await?;
    let mut sources = BTreeSet::new();
    for row in rows {
        let (source, ts) = row?;
        if source_rank(&source).is_none() {
            let event_at = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
            if event_at <= *generated_at {
                sources.insert(source);
            }
        }
    }
    Ok(sources.into_iter().collect())
}

fn unknown_source_warnings(unknown_sources: &[String]) -> Vec<DashboardDataWarning> {
    unknown_sources
        .iter()
        .map(|source| DashboardDataWarning {
            source: "usage_events.source".to_owned(),
            kind: "unknown_source".to_owned(),
            message: format!("included unknown usage source `{source}` in usage totals"),
        })
        .collect()
}

async fn aggregate_source(
    conn: &Connection,
    source: &str,
    since: Option<&DateTime<Utc>>,
    until: &DateTime<Utc>,
) -> Result<UsageSourceSummary, ReadModelError> {
    let coarse_since = since.map(|since| (*since - Duration::days(1)).to_rfc3339());
    let coarse_until = (*until + Duration::days(1)).to_rfc3339();
    let rows = aggregate_source_rows(conn, source, coarse_since.as_deref(), &coarse_until).await?;

    let mut summary = UsageSourceSummary {
        source: source.to_owned(),
        cost_usd: 0.0,
        subscription_cost_usd: 0.0,
        api_cost_usd: 0.0,
        turns: 0,
        invocations: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        web_search_requests: 0,
        web_fetch_requests: 0,
        per_model: Vec::new(),
    };
    let mut totals: BTreeMap<String, UsageModelSummary> = BTreeMap::new();

    for row in rows {
        let SourceAggregateRow {
            ts,
            cost,
            turns,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            web_search_requests,
            web_fetch_requests,
            model_usage_json,
            api_key_source,
        } = row;
        let event_at = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
        if since.is_some_and(|since| event_at < *since) || event_at > *until {
            continue;
        }

        summary.cost_usd += cost;
        if api_key_source == "none" {
            summary.subscription_cost_usd += cost;
        } else {
            summary.api_cost_usd += cost;
        }
        summary.turns += turns.max(0) as u64;
        summary.invocations += 1;
        summary.input_tokens += input_tokens.max(0) as u64;
        summary.output_tokens += output_tokens.max(0) as u64;
        summary.cache_creation_tokens += cache_creation_tokens.max(0) as u64;
        summary.cache_read_tokens += cache_read_tokens.max(0) as u64;
        summary.web_search_requests += web_search_requests.max(0) as u64;
        summary.web_fetch_requests += web_fetch_requests.max(0) as u64;

        aggregate_model_usage_for_window(&mut totals, source, &model_usage_json);
    }

    summary.per_model = totals.into_values().collect::<Vec<_>>();
    sort_models(&mut summary.per_model);
    Ok(summary)
}

struct SourceAggregateRow {
    ts: String,
    cost: f64,
    turns: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    web_search_requests: i64,
    web_fetch_requests: i64,
    model_usage_json: String,
    api_key_source: String,
}

async fn aggregate_source_rows(
    conn: &Connection,
    source: &str,
    coarse_since: Option<&str>,
    coarse_until: &str,
) -> Result<Vec<SourceAggregateRow>, ReadModelError> {
    if let Some(coarse_since) = coarse_since {
        let mut stmt = conn.prepare(
            "SELECT ts, total_cost_usd, num_turns, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, web_search_requests,
                    web_fetch_requests, model_usage_json, api_key_source
             FROM usage_events
             WHERE source = ?1 AND ts >= ?2 AND ts <= ?3
             ORDER BY ts ASC",
        )?;
        return stmt
            .query_map(
                params![source, coarse_since, coarse_until],
                source_aggregate_row,
            )
            .await?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
    }

    let mut stmt = conn.prepare(
        "SELECT ts, total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
         FROM usage_events
         WHERE source = ?1 AND ts <= ?2
         ORDER BY ts ASC",
    )?;
    stmt.query_map(params![source, coarse_until], source_aggregate_row)
        .await?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn source_aggregate_row(
    row: &right_db::row::Row<'_>,
) -> Result<SourceAggregateRow, right_db::DbError> {
    Ok(SourceAggregateRow {
        ts: row.get(0)?,
        cost: row.get(1)?,
        turns: row.get(2)?,
        input_tokens: row.get(3)?,
        output_tokens: row.get(4)?,
        cache_creation_tokens: row.get(5)?,
        cache_read_tokens: row.get(6)?,
        web_search_requests: row.get(7)?,
        web_fetch_requests: row.get(8)?,
        model_usage_json: row.get(9)?,
        api_key_source: row.get(10)?,
    })
}

fn aggregate_model_usage_for_window(
    totals: &mut BTreeMap<String, UsageModelSummary>,
    source: &str,
    json: &str,
) {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                source,
                "skipping row with malformed model_usage_json: {error}"
            );
            return;
        }
    };
    let Some(models) = value.as_object() else {
        tracing::warn!(source, "skipping row: model_usage_json is not an object");
        return;
    };

    for (model, fields) in models {
        let entry = totals
            .entry(model.clone())
            .or_insert_with(|| UsageModelSummary {
                model: model.clone(),
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            });
        entry.cost_usd += field_f64(fields, "costUSD");
        entry.input_tokens += field_u64(fields, "inputTokens");
        entry.output_tokens += field_u64(fields, "outputTokens");
        entry.cache_creation_tokens += field_u64(fields, "cacheCreationInputTokens");
        entry.cache_read_tokens += field_u64(fields, "cacheReadInputTokens");
    }
}

fn sort_models(rows: &mut [UsageModelSummary]) {
    rows.sort_by(|left, right| {
        right
            .cost_usd
            .partial_cmp(&left.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.model.cmp(&right.model))
    });
}

fn field_f64(value: &serde_json::Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0)
}

fn field_u64(value: &serde_json::Value, key: &str) -> u64 {
    value.get(key).and_then(|value| value.as_u64()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_db::open_connection;
    use tempfile::tempdir;

    async fn insert_usage(
        conn: &right_db::Connection,
        ts: &str,
        source: &str,
        cost: f64,
        model: &str,
    ) {
        let model_json = format!(
            r#"{{"{model}":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":5,"cacheReadInputTokens":40}}}}"#
        );
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (?1, ?2, 1, 0, NULL, ?3, ?4, 1, 10, 20, 5, 40, 1, 2, ?5, 'none')",
            params![ts, source, format!("{source}-{model}"), cost, model_json],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn usage_overview_includes_learning_sources() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(
            &conn,
            "2026-05-21T01:00:00Z",
            "learning_prefilter",
            0.10,
            "sonnet",
        )
        .await;
        insert_usage(
            &conn,
            "2026-05-21T02:00:00Z",
            "learning_probe_writer",
            0.20,
            "sonnet",
        )
        .await;
        insert_usage(
            &conn,
            "2026-05-21T03:00:00Z",
            "learning_curator",
            0.30,
            "sonnet",
        )
        .await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "agent-b".to_owned(),
                generated_at: "2026-05-21T05:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();
        let today = response.windows.iter().find(|w| w.key == "today").unwrap();
        let names: Vec<&str> = today.sources.iter().map(|s| s.source.as_str()).collect();
        assert!(names.contains(&"learning_prefilter"));
        assert!(names.contains(&"learning_probe_writer"));
        assert!(names.contains(&"learning_curator"));

        let prefilter = today
            .sources
            .iter()
            .find(|s| s.source == "learning_prefilter")
            .unwrap();
        assert!((prefilter.cost_usd - 0.10).abs() < 1e-9);
    }

    #[tokio::test]
    async fn usage_overview_sources_match_learning_sources_constant() {
        for source in right_agent::usage::LEARNING_SOURCES {
            assert!(
                SOURCES.contains(source),
                "dashboard SOURCES is missing learning source `{source}`"
            );
        }
    }

    #[tokio::test]
    async fn usage_overview_recognizes_idle_compaction_source() {
        // Regression: idle-compaction spend is recorded with source='idle_compaction'
        // (crates/bot/src/idle_compaction.rs). It must be a first-class dashboard source,
        // not fall through the unknown-source path (which warns and bottom-sorts it).
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(
            &conn,
            "2026-05-21T01:00:00Z",
            "idle_compaction",
            0.05,
            "opus",
        )
        .await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "agent-b".to_owned(),
                generated_at: "2026-05-21T05:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        assert!(
            !response.warnings.iter().any(|w| w.kind == "unknown_source"),
            "idle_compaction must not be reported as an unknown usage source"
        );

        let today = response.windows.iter().find(|w| w.key == "today").unwrap();
        let compaction = today
            .sources
            .iter()
            .find(|s| s.source == "idle_compaction")
            .expect("idle_compaction must appear as a recognized source");
        assert_eq!(compaction.invocations, 1);
        assert!((compaction.cost_usd - 0.05).abs() < 1e-9);
    }

    #[tokio::test]
    async fn usage_overview_builds_windows_and_sources() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-20T08:00:00Z", "interactive", 0.10, "sonnet").await;
        insert_usage(&conn, "2026-05-19T08:00:00Z", "cron", 0.20, "sonnet").await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-20T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            response
                .windows
                .iter()
                .map(|window| window.key.as_str())
                .collect::<Vec<_>>(),
            vec!["today", "last_7_days", "last_30_days", "all_time"]
        );

        let today = response.windows.iter().find(|w| w.key == "today").unwrap();
        let today_interactive = today
            .sources
            .iter()
            .find(|s| s.source == "interactive")
            .unwrap();
        assert_eq!(today_interactive.invocations, 1);
        assert!((today_interactive.cost_usd - 0.10).abs() < 1e-9);
        assert_eq!(today_interactive.per_model[0].model, "sonnet");

        let all_time = response
            .windows
            .iter()
            .find(|w| w.key == "all_time")
            .unwrap();
        let all_cron = all_time
            .sources
            .iter()
            .find(|s| s.source == "cron")
            .unwrap();
        assert_eq!(all_cron.invocations, 1);
        assert!((all_cron.cost_usd - 0.20).abs() < 1e-9);
        assert!((all_time.total_cost_usd - 0.30).abs() < 1e-9);
        assert_eq!(all_time.turns, 2);
        assert_eq!(all_time.invocations, 2);
        assert_eq!(all_time.input_tokens, 20);
        assert_eq!(all_time.output_tokens, 40);
        assert_eq!(all_time.cache_creation_tokens, 10);
        assert_eq!(all_time.cache_read_tokens, 80);
        assert_eq!(all_time.web_search_requests, 2);
        assert_eq!(all_time.web_fetch_requests, 4);
        assert_eq!(all_time.per_model[0].model, "sonnet");
        assert!((all_time.per_model[0].cost_usd - 0.30).abs() < 1e-9);
    }

    #[tokio::test]
    async fn usage_overview_builds_daily_series_for_last_30_days() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-01T08:00:00Z", "interactive", 0.10, "sonnet").await;
        insert_usage(&conn, "2026-05-01T09:00:00Z", "cron", 0.20, "opus").await;
        insert_usage(&conn, "2026-04-01T08:00:00Z", "interactive", 9.99, "old").await;
        insert_usage(&conn, "2026-05-01T10:00:00Z", "new_source", 4.44, "unknown").await;
        insert_usage(&conn, "2026-05-23T23:00:00Z", "interactive", 7.77, "future").await;
        insert_usage(
            &conn,
            "2026-05-23T23:00:00Z",
            "future_source",
            8.88,
            "future_unknown",
        )
        .await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.selected_window, "last_30_days");
        assert_eq!(response.daily_series.len(), 30);
        assert_eq!(response.daily_series.first().unwrap().date, "2026-04-24");
        assert_eq!(response.daily_series.last().unwrap().date, "2026-05-23");
        let may_1 = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-01")
            .unwrap();
        assert!((may_1.total_cost_usd - 4.74).abs() < 1e-9);
        assert_eq!(may_1.invocations, 3);
        assert_eq!(may_1.sources.len(), 3);
        let new_source = may_1
            .sources
            .iter()
            .find(|source| source.source == "new_source")
            .unwrap();
        assert!((new_source.cost_usd - 4.44).abs() < 1e-9);
        assert_eq!(new_source.invocations, 1);
        assert_eq!(may_1.models[0].model, "unknown");
        assert_eq!(may_1.models[1].model, "opus");
        assert_eq!(may_1.models[2].model, "sonnet");
        assert!(
            response
                .daily_series
                .iter()
                .all(|point| point.date != "2026-04-01")
        );
        let may_23 = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-23")
            .unwrap();
        assert_eq!(may_23.invocations, 0);
        assert!((may_23.total_cost_usd - 0.0).abs() < 1e-9);
        let today = response
            .windows
            .iter()
            .find(|window| window.key == "today")
            .unwrap();
        assert!((today.total_cost_usd - 0.0).abs() < 1e-9);
        let last_30_days = response
            .windows
            .iter()
            .find(|window| window.key == "last_30_days")
            .unwrap();
        assert!((last_30_days.total_cost_usd - 4.74).abs() < 1e-9);
        let last_30_days_new_source = last_30_days
            .sources
            .iter()
            .find(|source| source.source == "new_source")
            .unwrap();
        assert!((last_30_days_new_source.cost_usd - 4.44).abs() < 1e-9);

        let mut expected_sources = SOURCES.to_vec();
        expected_sources.push("new_source");
        assert_eq!(
            response
                .source_series
                .iter()
                .map(|series| series.source.as_str())
                .collect::<Vec<_>>(),
            expected_sources
        );
        let interactive = response
            .source_series
            .iter()
            .find(|series| series.source == "interactive")
            .unwrap();
        let cron = response
            .source_series
            .iter()
            .find(|series| series.source == "cron")
            .unwrap();
        let new_source_series = response
            .source_series
            .iter()
            .find(|series| series.source == "new_source")
            .unwrap();
        assert!(
            response
                .source_series
                .iter()
                .all(|series| series.source != "future_source")
        );
        assert_eq!(interactive.points.len(), 30);
        assert_eq!(cron.points.len(), 30);
        assert_eq!(new_source_series.points.len(), 30);
        let interactive_may_1 = interactive
            .points
            .iter()
            .find(|point| point.bucket == "2026-05-01")
            .unwrap();
        assert!((interactive_may_1.cost_usd - 0.10).abs() < 1e-9);
        let interactive_apr_24 = interactive
            .points
            .iter()
            .find(|point| point.bucket == "2026-04-24")
            .unwrap();
        assert!((interactive_apr_24.cost_usd - 0.0).abs() < 1e-9);
        let new_source_may_1 = new_source_series
            .points
            .iter()
            .find(|point| point.bucket == "2026-05-01")
            .unwrap();
        assert!((new_source_may_1.cost_usd - 4.44).abs() < 1e-9);
        let new_source_apr_24 = new_source_series
            .points
            .iter()
            .find(|point| point.bucket == "2026-04-24")
            .unwrap();
        assert!((new_source_apr_24.cost_usd - 0.0).abs() < 1e-9);
        assert_eq!(response.warnings.len(), 1);
        assert_eq!(response.warnings[0].source, "usage_events.source");
        assert_eq!(response.warnings[0].kind, "unknown_source");
        assert!(response.warnings[0].message.contains("new_source"));
    }

    #[tokio::test]
    async fn usage_overview_warns_and_skips_malformed_model_json_in_daily_series() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (
                '2026-05-23T08:00:00Z', 'interactive', 1, 0, NULL, 'bad-json',
                0.50, 1, 10, 20, 5, 40, 1, 2, '{not-json', 'none'
             )",
            [],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (
                ts, source, chat_id, thread_id, job_name, session_uuid,
                total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
             ) VALUES (
                '2026-05-23T09:00:00Z', 'interactive', 1, 0, NULL, 'not-object',
                0.25, 1, 10, 20, 5, 40, 1, 2, '[]', 'none'
             )",
            [],
        )
        .await
        .unwrap();

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        let today = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-23")
            .unwrap();
        assert!((today.total_cost_usd - 0.75).abs() < 1e-9);
        assert!(today.models.is_empty());
        assert_eq!(response.warnings.len(), 2);
        assert!(
            response
                .warnings
                .iter()
                .all(|warning| warning.source == "usage_events.model_usage_json")
        );
        assert!(
            response
                .warnings
                .iter()
                .all(|warning| warning.kind == "malformed_json")
        );
    }

    #[tokio::test]
    async fn usage_overview_filters_and_buckets_daily_series_by_parsed_utc_timestamps() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-23T12:00:00Z", "interactive", 0.10, "exact").await;
        insert_usage(
            &conn,
            "2026-05-23T11:59:59.999Z",
            "interactive",
            0.20,
            "fractional",
        )
        .await;
        insert_usage(
            &conn,
            "2026-05-23T13:00:00+02:00",
            "interactive",
            0.30,
            "offset-included",
        )
        .await;
        insert_usage(
            &conn,
            "2026-05-23T15:00:00+02:00",
            "interactive",
            9.99,
            "offset-excluded",
        )
        .await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        let today = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-23")
            .unwrap();
        assert_eq!(today.invocations, 3);
        assert!((today.total_cost_usd - 0.60).abs() < 1e-9);
        assert_eq!(
            today
                .models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec!["offset-included", "fractional", "exact"]
        );
        let today_window = response
            .windows
            .iter()
            .find(|window| window.key == "today")
            .unwrap();
        assert!((today_window.total_cost_usd - 0.60).abs() < 1e-9);
    }

    #[tokio::test]
    async fn usage_overview_aggregates_cache_tokens_per_source_in_daily_series() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-20T08:00:00Z", "interactive", 0.10, "sonnet").await;
        insert_usage(&conn, "2026-05-20T09:00:00Z", "interactive", 0.10, "sonnet").await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-20T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        let day = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-20")
            .unwrap();
        let interactive = day
            .sources
            .iter()
            .find(|source| source.source == "interactive")
            .unwrap();
        // Two events × (input 10, cache_creation 5, cache_read 40).
        assert_eq!(interactive.input_tokens, 20);
        assert_eq!(interactive.cache_creation_tokens, 10);
        assert_eq!(interactive.cache_read_tokens, 80);
    }

    #[tokio::test]
    async fn usage_overview_aggregates_output_tokens_per_source_in_daily_series() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_usage(&conn, "2026-05-20T08:00:00Z", "interactive", 0.10, "sonnet").await;
        insert_usage(&conn, "2026-05-20T09:00:00Z", "interactive", 0.10, "sonnet").await;

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-20T12:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        let day = response
            .daily_series
            .iter()
            .find(|point| point.date == "2026-05-20")
            .unwrap();
        let interactive = day
            .sources
            .iter()
            .find(|source| source.source == "interactive")
            .unwrap();
        // Two events × output 20 (see insert_usage VALUES).
        assert_eq!(interactive.output_tokens, 40);
    }

    #[tokio::test]
    async fn budget_skip_count_appears_in_window() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        // Two rows inside the today window, one outside (old)
        for ts in [
            "2026-05-21T01:00:00Z",
            "2026-05-21T02:00:00Z",
            "2026-05-19T00:00:00Z", // outside today
        ] {
            conn.execute(
                "INSERT INTO learning_skip (reason, ts) VALUES ('budget', ?1)",
                right_db::params![ts],
            )
            .await
            .unwrap();
        }

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "agent-b".to_owned(),
                generated_at: "2026-05-21T05:00:00Z".to_owned(),
                timezone: Some("UTC".to_owned()),
            },
        )
        .await
        .unwrap();

        let today = response.windows.iter().find(|w| w.key == "today").unwrap();
        assert_eq!(today.budget_skip_count, 2);

        let all_time = response
            .windows
            .iter()
            .find(|w| w.key == "all_time")
            .unwrap();
        assert_eq!(all_time.budget_skip_count, 3);
    }
}

#[cfg(test)]
#[path = "usage_local_time_tests.rs"]
mod usage_local_time_tests;
