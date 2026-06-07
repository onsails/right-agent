use super::*;
use right_db::{open_connection, params};
use tempfile::tempdir;

async fn insert_usage(conn: &right_db::Connection, ts: &str, source: &str, cost: f64) {
    let model_json = format!(
        r#"{{"sonnet":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":5,"cacheReadInputTokens":40}}}}"#
    );
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, NULL, ?3, ?4, 1, 10, 20, 5, 40, 0, 0, ?5, 'none')",
        params![ts, source, format!("{source}-{ts}"), cost, model_json],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn usage_overview_defaults_to_last_7_days() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-05-28T00:00:00Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-05-29T00:00:00Z", "interactive", 1.00).await;
    insert_usage(&conn, "2026-06-04T00:00:00Z", "interactive", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_7_days");
    assert_eq!(response.selected_window, "last_7_days");
    assert_eq!(response.window.key, "last_7_days");
    assert_eq!(response.windows.len(), 1);
    assert_eq!(response.windows[0].key, "last_7_days");
    assert_eq!(response.daily_series.len(), 7);
    assert_eq!(response.daily_series.first().unwrap().date, "2026-05-29");
    assert_eq!(response.daily_series.last().unwrap().date, "2026-06-04");
    assert!((response.window.total_cost_usd - 3.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_last_3_days_uses_local_calendar_window() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-01T19:59:59Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-06-01T20:00:00Z", "interactive", 1.00).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "interactive", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
            range: Some("last_3_days".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_3_days");
    assert_eq!(response.window.key, "last_3_days");
    assert_eq!(
        response.window.range_start.as_deref(),
        Some("2026-06-02T00:00:00+04:00")
    );
    assert_eq!(
        response
            .daily_series
            .iter()
            .map(|point| point.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-06-02", "2026-06-03", "2026-06-04"]
    );
    assert!((response.window.total_cost_usd - 3.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_invalid_range_falls_back_to_last_7_days_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("forever".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_7_days");
    assert_eq!(response.window.key, "last_7_days");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.range"
            && warning.kind == "invalid_range"
            && warning.message.contains("forever")
    }));
}

#[tokio::test]
async fn usage_overview_all_time_buckets_from_first_recorded_usage() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-05-01T08:00:00Z", "interactive", 0.10).await;
    insert_usage(&conn, "2026-05-03T08:00:00Z", "cron", 0.20).await;
    insert_usage(&conn, "2026-05-07T08:00:00Z", "interactive", 9.99).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-05T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("all_time".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "all_time");
    assert_eq!(response.window.key, "all_time");
    assert_eq!(response.window.range_start, None);
    assert_eq!(
        response
            .daily_series
            .iter()
            .map(|point| point.date.as_str())
            .collect::<Vec<_>>(),
        vec![
            "2026-05-01",
            "2026-05-02",
            "2026-05-03",
            "2026-05-04",
            "2026-05-05"
        ]
    );
    assert!((response.window.total_cost_usd - 0.30).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_all_time_start_uses_earliest_parsed_instant_not_raw_order() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-05-01T00:30:00Z", "interactive", 1.00).await;
    insert_usage(&conn, "2026-05-01T01:00:00+03:00", "cron", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-03T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("all_time".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.daily_series.first().unwrap().date, "2026-04-30");
    assert!((response.window.total_cost_usd - 3.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_today_ignores_unknown_sources_outside_selected_range() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-03T12:00:00Z", "external_tool", 9.99).await;
    insert_usage(&conn, "2026-06-04T12:00:00Z", "interactive", 1.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T18:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("today".to_owned()),
        },
    )
    .await
    .unwrap();

    assert!(!response.warnings.iter().any(|warning| {
        warning.source == "usage_events.source" && warning.kind == "unknown_source"
    }));
    assert!(
        response
            .source_series
            .iter()
            .all(|series| series.source != "external_tool")
    );
    assert!((response.window.total_cost_usd - 1.00).abs() < 1e-9);
}
