use super::*;
use right_db::{open_connection, params};
use tempfile::tempdir;

async fn insert_usage(conn: &right_db::Connection, ts: &str, source: &str, cost: f64) {
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, NULL, ?3, ?4, 1, 10, 20, 5, 40, 0, 0,
            '{\"sonnet\":{\"costUSD\":0.0,\"inputTokens\":10,\"outputTokens\":20,\"cacheCreationInputTokens\":5,\"cacheReadInputTokens\":40}}',
            'none')",
        params![ts, source, format!("{source}-{ts}"), cost],
    )
    .await
    .unwrap();
}

fn window<'a>(
    response: &'a crate::api_types::UsageOverviewResponse,
    key: &str,
) -> &'a crate::api_types::UsageWindow {
    response
        .windows
        .iter()
        .find(|window| window.key == key)
        .unwrap()
}

#[tokio::test]
async fn usage_overview_uses_requested_timezone_for_today() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    insert_usage(&conn, "2026-06-03T19:59:59Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-06-03T20:00:00Z", "interactive", 1.25).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "interactive", 2.50).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "Asia/Dubai");
    let today = window(&response, "today");
    let interactive = today
        .sources
        .iter()
        .find(|source| source.source == "interactive")
        .unwrap();
    assert_eq!(interactive.invocations, 2);
    assert!((interactive.cost_usd - 3.75).abs() < 1e-9);
    assert_eq!(
        today.range_start.as_deref(),
        Some("2026-06-04T00:00:00+04:00")
    );
    assert_eq!(today.range_end, "2026-06-04T20:47:36+04:00");
    assert_eq!(today.range_label, "Asia/Dubai · Jun 4 00:00-20:47");
}

#[tokio::test]
async fn usage_overview_uses_local_calendar_windows_not_rolling_hours() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    insert_usage(&conn, "2026-05-28T19:59:59Z", "cron", 9.99).await;
    insert_usage(&conn, "2026-05-28T20:00:00Z", "cron", 1.00).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "cron", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
        },
    )
    .await
    .unwrap();

    let last_7_days = window(&response, "last_7_days");
    let cron = last_7_days
        .sources
        .iter()
        .find(|source| source.source == "cron")
        .unwrap();
    assert_eq!(cron.invocations, 2);
    assert!((cron.cost_usd - 3.00).abs() < 1e-9);
    assert_eq!(
        last_7_days.range_start.as_deref(),
        Some("2026-05-29T00:00:00+04:00")
    );
    assert_eq!(
        last_7_days.range_label,
        "Asia/Dubai · May 29 00:00-Jun 4 20:47"
    );
}

#[tokio::test]
async fn usage_overview_invalid_timezone_falls_back_to_utc_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-04T01:00:00Z", "interactive", 0.10).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("Not/AZone".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "UTC");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.timezone"
            && warning.kind == "invalid_timezone"
            && warning.message.contains("Not/AZone")
    }));
    assert_eq!(
        window(&response, "today").range_start.as_deref(),
        Some("2026-06-04T00:00:00+00:00")
    );
}

#[tokio::test]
async fn usage_overview_missing_timezone_falls_back_to_utc_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "UTC");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.timezone" && warning.kind == "missing_timezone"
    }));
}
