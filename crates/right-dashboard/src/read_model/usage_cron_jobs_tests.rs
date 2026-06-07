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
    insert_usage_with_tokens(
        conn,
        ts,
        source,
        job_name,
        cost,
        model,
        (10, 20, 5, 40),
        (10, 20, 5, 40),
        "none",
    )
    .await;
}

async fn insert_usage_with_tokens(
    conn: &right_db::Connection,
    ts: &str,
    source: &str,
    job_name: Option<&str>,
    cost: f64,
    model: &str,
    row_tokens: (i64, i64, i64, i64),
    model_tokens: (u64, u64, u64, u64),
    api_key_source: &str,
) {
    let (row_input_tokens, row_output_tokens, row_cache_creation_tokens, row_cache_read_tokens) =
        row_tokens;
    let (
        model_input_tokens,
        model_output_tokens,
        model_cache_creation_tokens,
        model_cache_read_tokens,
    ) = model_tokens;
    let model_json = format!(
        r#"{{"{model}":{{"costUSD":{cost},"inputTokens":{model_input_tokens},"outputTokens":{model_output_tokens},"cacheCreationInputTokens":{model_cache_creation_tokens},"cacheReadInputTokens":{model_cache_read_tokens}}}}}"#
    );
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, 1, 2, ?10, ?11)",
        params![
            ts,
            source,
            job_name,
            format!("{source}-{model}-{ts}"),
            cost,
            row_input_tokens,
            row_output_tokens,
            row_cache_creation_tokens,
            row_cache_read_tokens,
            model_json,
            api_key_source
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
        "2026-06-02T03:59:59+04:00",
        "cron",
        Some("daily"),
        7.00,
        "sonnet",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-02T03:59:59+04:00",
        "cron",
        Some("out_of_range"),
        6.00,
        "opus",
    )
    .await;
    insert_usage_with_tokens(
        &conn,
        "2026-06-02T08:00:00Z",
        "cron",
        Some("daily"),
        0.10,
        "sonnet",
        (10, 20, 5, 40),
        (1, 2, 3, 4),
        "none",
    )
    .await;
    insert_usage_with_tokens(
        &conn,
        "2026-06-03T08:00:00Z",
        "cron",
        Some("daily"),
        0.40,
        "sonnet",
        (10, 20, 5, 40),
        (11, 22, 33, 44),
        "workspace_api_key",
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
    insert_usage(
        &conn,
        "2026-06-04T12:00:00.001Z",
        "cron",
        Some("after_range"),
        5.00,
        "opus",
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
    let cron_cost: f64 = response.cron_jobs.iter().map(|job| job.cost_usd).sum();
    assert!((cron_cost - 0.70).abs() < 1e-9);
    let daily = &response.cron_jobs[0];
    assert!((daily.cost_usd - 0.50).abs() < 1e-9);
    assert!((daily.subscription_cost_usd - 0.10).abs() < 1e-9);
    assert!((daily.api_cost_usd - 0.40).abs() < 1e-9);
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
    assert_eq!(daily.per_model[0].input_tokens, 12);
    assert_eq!(daily.per_model[0].output_tokens, 24);
    assert_eq!(daily.per_model[0].cache_creation_tokens, 36);
    assert_eq!(daily.per_model[0].cache_read_tokens, 48);
}

#[tokio::test]
async fn usage_overview_sorts_equal_cost_cron_jobs_by_name() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(
        &conn,
        "2026-06-04T08:00:00Z",
        "cron",
        Some("beta"),
        0.10,
        "sonnet",
    )
    .await;
    insert_usage(
        &conn,
        "2026-06-04T09:00:00Z",
        "cron",
        Some("alpha"),
        0.10,
        "opus",
    )
    .await;

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

    assert_eq!(
        response
            .cron_jobs
            .iter()
            .map(|job| job.job_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
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
