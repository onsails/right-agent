use super::*;
use rmcp::handler::server::ServerHandler;
use tempfile::tempdir;

fn setup_server() -> (MemoryServer, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let conn = rightclaw::memory::open_connection(dir.path(), true).expect("open_connection");
    let server = MemoryServer::new(
        conn,
        "test-agent".to_string(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    (server, dir)
}

fn setup_server_with_dir() -> (MemoryServer, tempfile::TempDir) {
    setup_server()
}

fn insert_cron_run(
    server: &MemoryServer,
    id: &str,
    job_name: &str,
    started_at: &str,
    status: &str,
) {
    let conn = server.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, job_name, started_at, status, format!("/tmp/{id}.log")],
    )
    .expect("insert cron_run");
}

fn call_result_text(result: CallToolResult) -> String {
    result
        .content
        .into_iter()
        .filter_map(|c| {
            if let rmcp::model::RawContent::Text(t) = c.raw {
                Some(t.text)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn test_get_info_server_name() {
    let (server, _dir) = setup_server();
    let info = server.get_info();
    assert_eq!(info.server_info.name, "rightclaw");
}

#[tokio::test]
async fn test_cron_list_runs_empty() {
    let (server, _dir) = setup_server();
    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed, serde_json::json!([]));
}

#[tokio::test]
async fn test_cron_list_runs_two_rows() {
    let (server, _dir) = setup_server();
    insert_cron_run(&server, "run-001", "deploy-check", "2026-04-01T10:00:00Z", "success");
    insert_cron_run(&server, "run-002", "health-ping", "2026-04-01T11:00:00Z", "success");

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 2);
    // Ordered by started_at DESC — run-002 first
    assert_eq!(parsed[0]["id"], "run-002");
    assert_eq!(parsed[1]["id"], "run-001");
}

#[tokio::test]
async fn test_cron_list_runs_filter_job_name() {
    let (server, _dir) = setup_server();
    insert_cron_run(&server, "run-a1", "job-a", "2026-04-01T10:00:00Z", "success");
    insert_cron_run(&server, "run-b1", "job-b", "2026-04-01T10:01:00Z", "success");

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: Some("job-a".to_string()),
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["job_name"], "job-a");
    assert_eq!(parsed[0]["id"], "run-a1");
}

#[tokio::test]
async fn test_cron_list_runs_limit() {
    let (server, _dir) = setup_server();
    for i in 0..5 {
        insert_cron_run(
            &server,
            &format!("run-{i:03}"),
            "batch-job",
            &format!("2026-04-01T{i:02}:00:00Z"),
            "success",
        );
    }
    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: Some(2),
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 2);
}

#[tokio::test]
async fn test_cron_show_run_found() {
    let (server, _dir) = setup_server();
    insert_cron_run(&server, "run-xyz", "nightly-report", "2026-04-01T02:00:00Z", "success");

    let result = server
        .cron_show_run(Parameters(CronShowRunParams {
            run_id: "run-xyz".to_string(),
        }))
        .await
        .expect("cron_show_run ok");
    let text = call_result_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed["id"], "run-xyz");
    assert_eq!(parsed["job_name"], "nightly-report");
    assert!(parsed["log_path"].as_str().unwrap().contains("run-xyz"));
}

#[tokio::test]
async fn test_cron_show_run_not_found() {
    let (server, _dir) = setup_server();

    let result = server
        .cron_show_run(Parameters(CronShowRunParams {
            run_id: "nonexistent-id".to_string(),
        }))
        .await
        .expect("cron_show_run returns Ok (not error) for missing");
    let text = call_result_text(result);
    assert!(
        text.contains("not found"),
        "Expected 'not found' in output, got: {text}"
    );
}

#[tokio::test]
async fn test_cron_list_runs_includes_diagnostics_fields() {
    let (server, _dir) = setup_server();
    let conn = server.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary, delivery_status, no_notify_reason) \
         VALUES ('diag-1', 'tracker', '2026-04-01T10:00:00Z', 'success', '/log', 'quiet', 'silent', 'No changes since last run')",
        [],
    ).expect("insert");
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary, notify_json, delivery_status, delivered_at) \
         VALUES ('diag-2', 'tracker', '2026-04-01T11:00:00Z', 'success', '/log', 'found stuff', '{\"content\":\"new release\"}', 'delivered', '2026-04-01T11:05:00Z')",
        [],
    ).expect("insert");
    drop(conn);

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: Some("tracker".to_string()),
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 2);

    // diag-2 is first (DESC order)
    assert_eq!(parsed[0]["delivery_status"], "delivered");
    assert_eq!(parsed[0]["delivered_at"], "2026-04-01T11:05:00Z");
    assert!(parsed[0]["no_notify_reason"].is_null());

    // diag-1 is second
    assert_eq!(parsed[1]["delivery_status"], "silent");
    assert_eq!(parsed[1]["no_notify_reason"], "No changes since last run");
    assert!(parsed[1]["delivered_at"].is_null());
}

// --- MCP tool tests ---

#[tokio::test]
async fn test_mcp_list_empty() {
    let (server, _dir) = setup_server_with_dir();
    let result = server
        .mcp_list(Parameters(McpListParams {}))
        .await
        .expect("mcp_list ok");
    let text = call_result_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed, serde_json::json!([]), "empty list should return []");
}

#[test]
fn test_get_info_mentions_record_tools() {
    let (server, _dir) = setup_server_with_dir();
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    assert!(
        instructions.contains("store_record"),
        "instructions should mention store_record: {instructions}"
    );
    assert!(
        instructions.contains("query_records"),
        "instructions should mention query_records: {instructions}"
    );
    assert!(
        instructions.contains("search_records"),
        "instructions should mention search_records: {instructions}"
    );
    assert!(
        instructions.contains("delete_record"),
        "instructions should mention delete_record: {instructions}"
    );
    assert!(
        instructions.contains("mcp_list"),
        "instructions should mention mcp_list: {instructions}"
    );
}
