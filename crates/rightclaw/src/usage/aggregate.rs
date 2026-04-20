//! Read path — used by `/usage` Telegram handler.

use crate::usage::{ModelTotals, UsageError, WindowSummary};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Aggregate rows for one (source, window) pair. `since=None` → all-time.
pub fn aggregate(
    conn: &Connection,
    since: Option<DateTime<Utc>>,
    source: &str,
) -> Result<WindowSummary, UsageError> {
    let since_str = since.map(|t| t.to_rfc3339());

    let (cost_usd, turns, invocations, input, output, cache_c, cache_r, web_s, web_f): (
        f64, i64, i64, i64, i64, i64, i64, i64, i64,
    ) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(total_cost_usd), 0.0),
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
            rusqlite::params![source, since_str],
            |r| Ok((
                r.get(0)?, r.get(1)?, r.get(2)?,
                r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                r.get(7)?, r.get(8)?,
            )),
        )?;

    let per_model = aggregate_per_model(conn, &since_str, source)?;

    Ok(WindowSummary {
        source: source.to_string(),
        cost_usd,
        turns: turns as u64,
        invocations: invocations as u64,
        input_tokens: input as u64,
        output_tokens: output as u64,
        cache_creation_tokens: cache_c as u64,
        cache_read_tokens: cache_r as u64,
        web_search_requests: web_s as u64,
        web_fetch_requests: web_f as u64,
        per_model,
    })
}

fn aggregate_per_model(
    conn: &Connection,
    since_str: &Option<String>,
    source: &str,
) -> Result<BTreeMap<String, ModelTotals>, UsageError> {
    let mut stmt = conn.prepare(
        "SELECT model_usage_json FROM usage_events
         WHERE source = ?1 AND (?2 IS NULL OR ts >= ?2)",
    )?;
    let rows = stmt.query_map(rusqlite::params![source, since_str], |r| {
        r.get::<_, String>(0)
    })?;

    let mut out: BTreeMap<String, ModelTotals> = BTreeMap::new();
    for row in rows {
        let json = row?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue; // skip malformed rows rather than failing the whole query
        };
        let Some(obj) = v.as_object() else { continue };
        for (model_name, fields) in obj {
            let cost = fields.get("costUSD").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let input = fields.get("inputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let output = fields.get("outputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let cache_c = fields.get("cacheCreationInputTokens").and_then(|n| n.as_u64()).unwrap_or(0);
            let cache_r = fields.get("cacheReadInputTokens").and_then(|n| n.as_u64()).unwrap_or(0);

            let entry = out.entry(model_name.clone()).or_default();
            entry.cost_usd += cost;
            entry.input_tokens += input;
            entry.output_tokens += output;
            entry.cache_creation_tokens += cache_c;
            entry.cache_read_tokens += cache_r;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::open_connection;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::{insert_cron, insert_interactive};
    use tempfile::tempdir;

    fn breakdown(cost: f64, model: &str) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: 1,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 30,
            cache_read_tokens: 40,
            web_search_requests: 1,
            web_fetch_requests: 1,
            model_usage_json: format!(
                r#"{{"{model}":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":30,"cacheReadInputTokens":40}}}}"#
            ),
        }
    }

    #[test]
    fn aggregate_empty_table_returns_zeros() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.invocations, 0);
        assert_eq!(s.cost_usd, 0.0);
        assert!(s.per_model.is_empty());
    }

    #[test]
    fn aggregate_sums_across_invocations() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.invocations, 2);
        assert!((s.cost_usd - 0.3).abs() < 1e-9);
        assert_eq!(s.turns, 2);
        assert_eq!(s.input_tokens, 20);
    }

    #[test]
    fn aggregate_per_model_reduces_across_rows_and_models() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();
        insert_interactive(&conn, &breakdown(0.05, "haiku"), 1, 0).unwrap();
        let s = aggregate(&conn, None, "interactive").unwrap();
        assert_eq!(s.per_model.len(), 2);
        assert!((s.per_model["sonnet"].cost_usd - 0.3).abs() < 1e-9);
        assert!((s.per_model["haiku"].cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(s.per_model["sonnet"].input_tokens, 20);
    }

    #[test]
    fn aggregate_filters_by_source() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        insert_cron(&conn, &breakdown(0.2, "sonnet"), "job1").unwrap();
        let i = aggregate(&conn, None, "interactive").unwrap();
        let c = aggregate(&conn, None, "cron").unwrap();
        assert_eq!(i.invocations, 1);
        assert!((i.cost_usd - 0.1).abs() < 1e-9);
        assert_eq!(c.invocations, 1);
        assert!((c.cost_usd - 0.2).abs() < 1e-9);
    }

    #[test]
    fn aggregate_filters_by_since() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        insert_interactive(&conn, &breakdown(0.1, "sonnet"), 1, 0).unwrap();
        // Backdate this row so the filter excludes it.
        conn.execute(
            "UPDATE usage_events SET ts = '2020-01-01T00:00:00Z' WHERE id = 1",
            [],
        ).unwrap();
        insert_interactive(&conn, &breakdown(0.2, "sonnet"), 1, 0).unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let s = aggregate(&conn, Some(since), "interactive").unwrap();
        assert_eq!(s.invocations, 1);
        assert!((s.cost_usd - 0.2).abs() < 1e-9);
    }
}
