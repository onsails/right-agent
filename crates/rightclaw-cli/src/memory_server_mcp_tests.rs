use super::*;
use rmcp::handler::server::ServerHandler;
use tempfile::tempdir;

fn setup_server() -> (MemoryServer, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let conn = rightclaw::memory::open_connection(dir.path()).expect("open_connection");
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

// --- MCP tool tests ---

#[tokio::test]
async fn test_mcp_add_creates_entry() {
    let (server, dir) = setup_server_with_dir();
    let result = server
        .mcp_add(Parameters(McpAddParams {
            name: "notion".to_string(),
            url: "https://mcp.notion.com/mcp".to_string(),
        }))
        .await
        .expect("mcp_add ok");
    let text = call_result_text(result);
    assert!(text.contains("notion"), "response should mention server name");
    let mcp_json = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
    assert!(mcp_json.contains("\"notion\""), ".mcp.json should contain notion");
    assert!(mcp_json.contains("\"http\""), ".mcp.json entry should have type:http");
}

#[tokio::test]
async fn test_mcp_remove_rightmemory_rejected() {
    let (server, _dir) = setup_server_with_dir();
    let err = server
        .mcp_remove(Parameters(McpRemoveParams {
            name: "rightmemory".to_string(),
        }))
        .await
        .expect_err("should return error for protected server");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("Cannot remove") || msg.contains("rightmemory"),
        "error should mention protected server: {msg}"
    );
}

#[tokio::test]
async fn test_mcp_remove_existing_server() {
    let (server, dir) = setup_server_with_dir();
    server
        .mcp_add(Parameters(McpAddParams {
            name: "linear".to_string(),
            url: "https://mcp.linear.app/mcp".to_string(),
        }))
        .await
        .expect("mcp_add ok");
    let result = server
        .mcp_remove(Parameters(McpRemoveParams {
            name: "linear".to_string(),
        }))
        .await
        .expect("mcp_remove ok");
    let text = call_result_text(result);
    assert!(text.contains("linear"), "response should mention server name");
    let mcp_json = std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
    assert!(
        !mcp_json.contains("\"linear\""),
        "linear should be removed from .mcp.json"
    );
}

#[tokio::test]
async fn test_mcp_remove_nonexistent_returns_error() {
    let (server, _dir) = setup_server_with_dir();
    let err = server
        .mcp_remove(Parameters(McpRemoveParams {
            name: "ghost".to_string(),
        }))
        .await
        .expect_err("should return error for missing server");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("ghost") || msg.contains("not found"),
        "error should mention server: {msg}"
    );
}

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

#[tokio::test]
async fn test_mcp_list_shows_server_metadata() {
    let (server, _dir) = setup_server_with_dir();
    server
        .mcp_add(Parameters(McpAddParams {
            name: "notion".to_string(),
            url: "https://mcp.notion.com/mcp".to_string(),
        }))
        .await
        .expect("mcp_add ok");
    let result = server
        .mcp_list(Parameters(McpListParams {}))
        .await
        .expect("mcp_list ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json array");
    assert_eq!(parsed.len(), 1);
    let entry = &parsed[0];
    assert_eq!(entry["name"], "notion");
    assert_eq!(entry["url"], "https://mcp.notion.com/mcp");
    assert!(entry.get("auth").is_some(), "auth field must be present");
    assert!(entry.get("source").is_some(), "source field must be present");
    assert!(entry.get("kind").is_some(), "kind field must be present");
    // MCP-NF-01: no token/secret fields
    assert!(entry.get("token").is_none(), "token field must NOT be present");
    assert!(entry.get("secret").is_none(), "secret field must NOT be present");
    assert!(entry.get("access_token").is_none(), "access_token must NOT be present");
}

#[tokio::test]
async fn test_mcp_auth_server_not_found() {
    let (server, _dir) = setup_server_with_dir();
    let err = server
        .mcp_auth(Parameters(McpAuthParams {
            server_name: "ghost".to_string(),
        }))
        .await
        .expect_err("should return error for missing server");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("ghost") || msg.contains("not found"),
        "error should mention missing server: {msg}"
    );
}

#[test]
fn test_get_info_mentions_mcp_tools() {
    let (server, _dir) = setup_server_with_dir();
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    assert!(
        instructions.contains("mcp_add"),
        "instructions should mention mcp_add: {instructions}"
    );
    assert!(
        instructions.contains("mcp_remove"),
        "instructions should mention mcp_remove: {instructions}"
    );
    assert!(
        instructions.contains("mcp_list"),
        "instructions should mention mcp_list: {instructions}"
    );
    assert!(
        instructions.contains("mcp_auth"),
        "instructions should mention mcp_auth: {instructions}"
    );
}
