use std::collections::BTreeMap;

use crate::api_types::{UsageModelSummary, UsageOverviewResponse, UsageSourceSummary, UsageWindow};
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

pub struct UsageOverviewInput {
    pub agent: String,
    pub generated_at: String,
}

pub fn usage_overview(
    conn: &Connection,
    input: UsageOverviewInput,
) -> Result<UsageOverviewResponse, ReadModelError> {
    let now = DateTime::parse_from_rfc3339(&input.generated_at)?.with_timezone(&Utc);
    let today_start = Utc
        .from_utc_datetime(
            &now.date_naive()
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| ReadModelError::InvalidStartOfDay(input.generated_at.clone()))?,
        )
        .to_rfc3339();
    let week_start = (now - Duration::days(7)).to_rfc3339();
    let month_start = (now - Duration::days(30)).to_rfc3339();

    let windows = vec![
        build_window(conn, "today", "Today", Some(today_start.as_str()))?,
        build_window(
            conn,
            "last_7_days",
            "Last 7 days",
            Some(week_start.as_str()),
        )?,
        build_window(
            conn,
            "last_30_days",
            "Last 30 days",
            Some(month_start.as_str()),
        )?,
        build_window(conn, "all_time", "All time", None)?,
    ];

    Ok(UsageOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        windows,
    })
}

fn build_window(
    conn: &Connection,
    key: &str,
    label: &str,
    since: Option<&str>,
) -> Result<UsageWindow, ReadModelError> {
    let mut sources = Vec::with_capacity(SOURCES.len());
    for source in SOURCES {
        sources.push(aggregate_source(conn, source, since)?);
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

fn aggregate_source(
    conn: &Connection,
    source: &str,
    since: Option<&str>,
) -> Result<UsageSourceSummary, ReadModelError> {
    let (
        cost_usd,
        subscription_cost_usd,
        api_cost_usd,
        turns,
        invocations,
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        web_search_requests,
        web_fetch_requests,
    ): (f64, f64, f64, i64, i64, i64, i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT
            COALESCE(SUM(total_cost_usd), 0.0),
            COALESCE(SUM(CASE WHEN api_key_source = 'none' THEN total_cost_usd ELSE 0.0 END), 0.0),
            COALESCE(SUM(CASE WHEN api_key_source != 'none' THEN total_cost_usd ELSE 0.0 END), 0.0),
            COALESCE(SUM(num_turns), 0),
            COUNT(*),
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_creation_tokens), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(web_search_requests), 0),
            COALESCE(SUM(web_fetch_requests), 0)
         FROM usage_events
         WHERE source = ?1
           AND (?2 IS NULL OR ts >= ?2)",
        params![source, since],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        },
    )?;

    Ok(UsageSourceSummary {
        source: source.to_owned(),
        cost_usd,
        subscription_cost_usd,
        api_cost_usd,
        turns: turns as u64,
        invocations: invocations as u64,
        input_tokens: input_tokens as u64,
        output_tokens: output_tokens as u64,
        cache_creation_tokens: cache_creation_tokens as u64,
        cache_read_tokens: cache_read_tokens as u64,
        web_search_requests: web_search_requests as u64,
        web_fetch_requests: web_fetch_requests as u64,
        per_model: aggregate_per_model(conn, source, since)?,
    })
}

fn aggregate_per_model(
    conn: &Connection,
    source: &str,
    since: Option<&str>,
) -> Result<Vec<UsageModelSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT model_usage_json FROM usage_events
         WHERE source = ?1 AND (?2 IS NULL OR ts >= ?2)",
    )?;
    let rows = stmt.query_map(params![source, since], |row| row.get::<_, String>(0))?;
    let mut totals: BTreeMap<String, UsageModelSummary> = BTreeMap::new();

    for row in rows {
        let json = row?;
        let value: serde_json::Value = match serde_json::from_str(&json) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    source,
                    "skipping row with malformed model_usage_json: {error}"
                );
                continue;
            }
        };
        let Some(models) = value.as_object() else {
            tracing::warn!(source, "skipping row: model_usage_json is not an object");
            continue;
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

    let mut rows = totals.into_values().collect::<Vec<_>>();
    sort_models(&mut rows);
    Ok(rows)
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
                agent: "him".to_owned(),
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
}
