use std::collections::BTreeMap;

use crate::api_types::{
    CostSeriesPoint, DashboardDataWarning, UsageDailyPoint, UsageModelSummary,
    UsageOverviewResponse, UsageSourcePoint, UsageSourceSeries, UsageSourceSummary, UsageWindow,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{Connection, params};

use super::ReadModelError;

const SOURCES: [&str; 9] = [
    "interactive",
    "cron",
    "reflection",
    "learning_selector",
    "learning_reviewer",
    "learning_skill_review",
    "learning_prefilter",
    "learning_probe_writer",
    "learning_curator",
];
const DEFAULT_CHART_WINDOW: &str = "last_30_days";
const DAILY_SERIES_DAYS: i64 = 30;

pub struct UsageOverviewInput {
    pub agent: String,
    pub generated_at: String,
}

pub fn usage_overview(
    conn: &Connection,
    input: UsageOverviewInput,
) -> Result<UsageOverviewResponse, ReadModelError> {
    let now = DateTime::parse_from_rfc3339(&input.generated_at)?.with_timezone(&Utc);
    let today_start = Utc.from_utc_datetime(
        &now.date_naive()
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ReadModelError::InvalidStartOfDay(input.generated_at.clone()))?,
    );
    let week_start = now - Duration::days(7);
    let month_start = now - Duration::days(30);

    let windows = vec![
        build_window(conn, "today", "Today", Some(&today_start), &now)?,
        build_window(conn, "last_7_days", "Last 7 days", Some(&week_start), &now)?,
        build_window(
            conn,
            "last_30_days",
            "Last 30 days",
            Some(&month_start),
            &now,
        )?,
        build_window(conn, "all_time", "All time", None, &now)?,
    ];

    let (daily_series, warnings) = build_daily_series(conn, &now)?;
    let source_series = build_source_series(&daily_series);

    Ok(UsageOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        windows,
        selected_window: DEFAULT_CHART_WINDOW.to_owned(),
        daily_series,
        source_series,
        warnings,
    })
}

fn build_window(
    conn: &Connection,
    key: &str,
    label: &str,
    since: Option<&DateTime<Utc>>,
    until: &DateTime<Utc>,
) -> Result<UsageWindow, ReadModelError> {
    let mut sources = Vec::with_capacity(SOURCES.len());
    for source in SOURCES {
        sources.push(aggregate_source(conn, source, since, until)?);
    }
    let per_model = aggregate_window_models(&sources);

    Ok(UsageWindow {
        key: key.to_owned(),
        label: label.to_owned(),
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
    })
}

fn build_daily_series(
    conn: &Connection,
    now: &DateTime<Utc>,
) -> Result<(Vec<UsageDailyPoint>, Vec<DashboardDataWarning>), ReadModelError> {
    let mut warnings = Vec::new();
    let chart_start_naive = (now.date_naive() - Duration::days(DAILY_SERIES_DAYS - 1))
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(now.to_rfc3339()))?;
    let chart_start_utc = Utc.from_utc_datetime(&chart_start_naive);
    let coarse_since = (chart_start_utc - Duration::days(1)).to_rfc3339();
    let coarse_until = (*now + Duration::days(1)).to_rfc3339();

    let mut points = (0..DAILY_SERIES_DAYS)
        .map(|offset| {
            let date = (now.date_naive() - Duration::days(DAILY_SERIES_DAYS - 1 - offset))
                .format("%Y-%m-%d")
                .to_string();
            UsageDailyPoint {
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
            }
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
    let rows = stmt.query_map(params![coarse_since, coarse_until], |row| {
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
    })?;

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
        if event_at < chart_start_utc || event_at > *now {
            continue;
        }
        if source_rank(&source).is_none() {
            continue;
        }

        let date = event_at.date_naive().format("%Y-%m-%d").to_string();
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
            });
        source_entry.cost_usd += cost;
        if api_key_source == "none" {
            source_entry.subscription_cost_usd += cost;
        } else {
            source_entry.api_cost_usd += cost;
        }
        source_entry.turns += turns.max(0) as u64;
        source_entry.invocations += 1;

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

fn build_source_series(points: &[UsageDailyPoint]) -> Vec<UsageSourceSeries> {
    let mut totals = BTreeMap::<(&str, &str), f64>::new();
    for point in points {
        for source in &point.sources {
            totals.insert(
                (source.source.as_str(), point.date.as_str()),
                source.cost_usd,
            );
        }
    }

    SOURCES
        .iter()
        .map(|source| UsageSourceSeries {
            source: (*source).to_owned(),
            points: points
                .iter()
                .map(|point| CostSeriesPoint {
                    bucket: point.date.clone(),
                    cost_usd: totals
                        .get(&(*source, point.date.as_str()))
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

fn source_rank(source: &str) -> Option<usize> {
    SOURCES
        .iter()
        .position(|known_source| *known_source == source)
}

fn aggregate_source(
    conn: &Connection,
    source: &str,
    since: Option<&DateTime<Utc>>,
    until: &DateTime<Utc>,
) -> Result<UsageSourceSummary, ReadModelError> {
    let coarse_since = since.map(|since| (*since - Duration::days(1)).to_rfc3339());
    let coarse_until = (*until + Duration::days(1)).to_rfc3339();
    let rows = aggregate_source_rows(conn, source, coarse_since.as_deref(), &coarse_until)?;

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

fn aggregate_source_rows(
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
            )?
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
    stmt.query_map(params![source, coarse_until], source_aggregate_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn source_aggregate_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceAggregateRow> {
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
    use rusqlite::params;
    use tempfile::tempdir;

    fn insert_usage(conn: &rusqlite::Connection, ts: &str, source: &str, cost: f64, model: &str) {
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
        .unwrap();
    }

    #[test]
    fn usage_overview_includes_learning_sources() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_usage(
            &conn,
            "2026-05-21T01:00:00Z",
            "learning_selector",
            0.10,
            "sonnet",
        );
        insert_usage(
            &conn,
            "2026-05-21T02:00:00Z",
            "learning_reviewer",
            0.20,
            "sonnet",
        );
        insert_usage(
            &conn,
            "2026-05-21T03:00:00Z",
            "learning_skill_review",
            0.30,
            "sonnet",
        );

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "agent-b".to_owned(),
                generated_at: "2026-05-21T05:00:00Z".to_owned(),
            },
        )
        .unwrap();
        let today = response.windows.iter().find(|w| w.key == "today").unwrap();
        let names: Vec<&str> = today.sources.iter().map(|s| s.source.as_str()).collect();
        assert!(names.contains(&"learning_selector"));
        assert!(names.contains(&"learning_reviewer"));
        assert!(names.contains(&"learning_skill_review"));

        let selector = today
            .sources
            .iter()
            .find(|s| s.source == "learning_selector")
            .unwrap();
        assert!((selector.cost_usd - 0.10).abs() < 1e-9);
    }

    #[test]
    fn usage_overview_sources_match_learning_sources_constant() {
        for source in right_agent::usage::LEARNING_SOURCES {
            assert!(
                SOURCES.contains(source),
                "dashboard SOURCES is missing learning source `{source}`"
            );
        }
    }

    #[test]
    fn usage_overview_builds_windows_and_sources() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_usage(&conn, "2026-05-20T08:00:00Z", "interactive", 0.10, "sonnet");
        insert_usage(&conn, "2026-05-19T08:00:00Z", "cron", 0.20, "sonnet");

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-20T12:00:00Z".to_owned(),
            },
        )
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

    #[test]
    fn usage_overview_builds_daily_series_for_last_30_days() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_usage(&conn, "2026-05-01T08:00:00Z", "interactive", 0.10, "sonnet");
        insert_usage(&conn, "2026-05-01T09:00:00Z", "cron", 0.20, "opus");
        insert_usage(&conn, "2026-04-01T08:00:00Z", "interactive", 9.99, "old");
        insert_usage(&conn, "2026-05-01T10:00:00Z", "new_source", 4.44, "unknown");
        insert_usage(&conn, "2026-05-23T23:00:00Z", "interactive", 7.77, "future");

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
            },
        )
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
        assert!((may_1.total_cost_usd - 0.30).abs() < 1e-9);
        assert_eq!(may_1.invocations, 2);
        assert_eq!(may_1.sources.len(), 2);
        assert!(
            may_1
                .sources
                .iter()
                .all(|source| source.source != "new_source")
        );
        assert_eq!(may_1.models[0].model, "opus");
        assert_eq!(may_1.models[1].model, "sonnet");
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
        assert!((last_30_days.total_cost_usd - 0.30).abs() < 1e-9);
        assert!(
            response
                .source_series
                .iter()
                .all(|series| series.source != "new_source")
        );

        assert_eq!(
            response
                .source_series
                .iter()
                .map(|series| series.source.as_str())
                .collect::<Vec<_>>(),
            SOURCES
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
        assert_eq!(interactive.points.len(), 30);
        assert_eq!(cron.points.len(), 30);
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
    }

    #[test]
    fn usage_overview_warns_and_skips_malformed_model_json_in_daily_series() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
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
        .unwrap();

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
            },
        )
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

    #[test]
    fn usage_overview_filters_and_buckets_daily_series_by_parsed_utc_timestamps() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_usage(&conn, "2026-05-23T12:00:00Z", "interactive", 0.10, "exact");
        insert_usage(
            &conn,
            "2026-05-23T11:59:59.999Z",
            "interactive",
            0.20,
            "fractional",
        );
        insert_usage(
            &conn,
            "2026-05-23T13:00:00+02:00",
            "interactive",
            0.30,
            "offset-included",
        );
        insert_usage(
            &conn,
            "2026-05-23T15:00:00+02:00",
            "interactive",
            9.99,
            "offset-excluded",
        );

        let response = usage_overview(
            &conn,
            UsageOverviewInput {
                agent: "alpha".to_owned(),
                generated_at: "2026-05-23T12:00:00Z".to_owned(),
            },
        )
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
}
