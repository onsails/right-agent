use super::*;
use right_db::{open_connection, params};
use tempfile::tempdir;

async fn insert_usage(
    conn: &right_db::Connection,
    ts: &str,
    source: &str,
    job_name: Option<&str>,
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
         ) VALUES (?1, ?2, 1, 0, ?3, ?4, ?5, 1, 10, 20, 5, 40, 1, 2, ?6, 'none')",
        params![
            ts,
            source,
            job_name,
            format!("{source}-{model}-{ts}"),
            cost,
            model_json
        ],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn usage_overview_groups_cron_jobs_for_selected_range() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(
        &conn,
        "2026-06-02T08:00:00Z",
        "cron",
        Some("daily"),
        0.10,
        "sonnet",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-03T08:00:00Z",
        "cron",
        Some("daily"),
        0.40,
        "sonnet",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-04T08:00:00Z",
        "cron",
        Some("weekly"),
        0.20,
        "opus",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-04T09:00:00Z",
        "reflection",
        Some("daily"),
        9.99,
        "opus",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-04T10:00:00Z",
        "interactive",
        None,
        8.88,
        "sonnet",
    )
    .await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("last_3_days".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        response
            .cron_jobs
            .iter()
            .map(|job| job.job_name.as_str())
            .collect::<Vec<_>>(),
        vec!["daily", "weekly"]
    );
    let daily = &response.cron_jobs[0];
    assert!((daily.cost_usd - 0.50).abs() < 1e-9);
    assert_eq!(daily.invocations, 2);
    assert_eq!(daily.turns, 2);
    assert_eq!(daily.input_tokens, 20);
    assert_eq!(daily.output_tokens, 40);
    assert_eq!(daily.cache_creation_tokens, 10);
    assert_eq!(daily.cache_read_tokens, 80);
    assert_eq!(daily.web_search_requests, 2);
    assert_eq!(daily.web_fetch_requests, 4);
    assert_eq!(daily.per_model.len(), 1);
    assert_eq!(daily.per_model[0].model, "sonnet");
    assert!((daily.per_model[0].cost_usd - 0.50).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_labels_null_cron_job_name_as_unknown() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-04T08:00:00Z", "cron", None, 0.10, "sonnet").await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("today".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.cron_jobs.len(), 1);
    assert_eq!(response.cron_jobs[0].job_name, "(unknown job)");
    assert!((response.cron_jobs[0].cost_usd - 0.10).abs() < 1e-9);
}
