use super::*;
use rmcp::handler::server::ServerHandler;
use tempfile::tempdir;

async fn setup_server() -> (MemoryServer, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let conn = right_db::open_connection(dir.path(), true)
        .await
        .expect("open_connection");
    let server = MemoryServer::new(
        conn,
        "test-agent".to_string(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    (server, dir)
}

async fn setup_server_with_dir() -> (MemoryServer, tempfile::TempDir) {
    setup_server().await
}

async fn insert_cron_run(
    server: &MemoryServer,
    id: &str,
    job_name: &str,
    started_at: &str,
    status: &str,
) {
    let conn = server.conn.lock().await;
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, status, log_path, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (?1, 'cron', ?2, ?1, -100, ?3, ?4, ?5, 0, 'none', ?3, ?3)",
        right_db::params![id, job_name, started_at, status, format!("/tmp/{id}.log")],
    )
    .await
    .expect("insert async cron run");
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

#[tokio::test]
async fn test_get_info_server_name() {
    let (server, _dir) = setup_server().await;
    let info = server.get_info();
    assert_eq!(info.server_info.name, "right");
}

#[tokio::test]
async fn test_cron_list_runs_empty() {
    let (server, _dir) = setup_server().await;
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
    let (server, _dir) = setup_server().await;
    insert_cron_run(
        &server,
        "run-001",
        "deploy-check",
        "2026-04-01T10:00:00Z",
        "success",
    )
    .await;
    insert_cron_run(
        &server,
        "run-002",
        "health-ping",
        "2026-04-01T11:00:00Z",
        "success",
    )
    .await;

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
    let (server, _dir) = setup_server().await;
    insert_cron_run(
        &server,
        "run-a1",
        "job-a",
        "2026-04-01T10:00:00Z",
        "success",
    )
    .await;
    insert_cron_run(
        &server,
        "run-b1",
        "job-b",
        "2026-04-01T10:01:00Z",
        "success",
    )
    .await;

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
async fn test_cron_list_runs_excludes_background_rows() {
    let (server, _dir) = setup_server().await;
    insert_cron_run(
        &server,
        "cron-001",
        "job-a",
        "2026-04-01T10:00:00Z",
        "success",
    )
    .await;
    {
        let conn = server.conn.lock().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, source_session_id, run_session_id, target_chat_id,
                started_at, status, delivery_required, delivery_status, created_at, updated_at
             ) VALUES (
                'bg-001', 'background', 'main', 'bg-session', -100,
                '2026-04-01T11:00:00Z', 'success', 1, 'pending',
                '2026-04-01T11:00:00Z', '2026-04-01T11:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
    }

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["id"], "cron-001");
}

#[tokio::test]
async fn test_cron_list_runs_propagates_malformed_row_error() {
    let (server, _dir) = setup_server().await;
    {
        let conn = server.conn.lock().await;
        conn.execute(
            "INSERT INTO async_runs (
                id, kind, producer_ref, run_session_id, target_chat_id,
                started_at, status, delivery_required, delivery_status, created_at, updated_at
             ) VALUES (
                'bad-cron-001', 'cron', NULL, 'bad-cron-001', -100,
                '2026-04-01T10:00:00Z', 'success', 0, 'none',
                '2026-04-01T10:00:00Z', '2026-04-01T10:00:00Z'
             )",
            [],
        )
        .await
        .unwrap();
    }

    let err = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: None,
            limit: None,
        }))
        .await
        .expect_err("malformed cron row should be returned as an MCP internal error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("row read failed"),
        "expected row read failure, got: {msg}"
    );
}

#[tokio::test]
async fn test_cron_list_runs_limit() {
    let (server, _dir) = setup_server().await;
    for i in 0..5 {
        insert_cron_run(
            &server,
            &format!("run-{i:03}"),
            "batch-job",
            &format!("2026-04-01T{i:02}:00:00Z"),
            "success",
        )
        .await;
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
    let (server, _dir) = setup_server().await;
    insert_cron_run(
        &server,
        "run-xyz",
        "nightly-report",
        "2026-04-01T02:00:00Z",
        "success",
    )
    .await;

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
    let (server, _dir) = setup_server().await;

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

// Guard is dropped before the .await below, so it is not held across it.
// Clippy's analysis is not smart enough to see the explicit drop.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_cron_list_runs_includes_diagnostics_fields() {
    let (server, _dir) = setup_server().await;
    let conn = server.conn.lock().await;
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, status, log_path, run_note, delivery_json, delivery_required,
            delivery_status, created_at, updated_at
         ) VALUES (
            'diag-1', 'cron', 'tracker', 'diag-1', -100,
            '2026-04-01T10:00:00Z', 'success', '/log', 'quiet',
            '{\"kind\":\"silent\",\"reason\":\"No changes since last run\"}', 0,
            'none', '2026-04-01T10:00:00Z', '2026-04-01T10:00:00Z'
         )",
        [],
    )
    .await
    .expect("insert");
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            started_at, status, log_path, run_note, delivery_json,
            delivery_required, delivery_status, delivered_at, created_at, updated_at
         ) VALUES (
            'diag-2', 'cron', 'tracker', 'diag-2', -100,
            '2026-04-01T11:00:00Z', 'success', '/log', 'found stuff',
            '{\"kind\":\"notify\",\"content\":\"new release\"}',
            1, 'delivered', '2026-04-01T11:05:00Z', '2026-04-01T11:00:00Z', '2026-04-01T11:00:00Z'
         )",
        [],
    )
    .await
    .expect("insert");
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
    assert_eq!(parsed[0]["run_note"], "found stuff");
    assert_eq!(parsed[0]["delivery"]["kind"], "notify");
    assert_eq!(parsed[0]["delivery"]["content"], "new release");

    // diag-1 is second
    assert_eq!(parsed[1]["delivery_status"], "none");
    assert_eq!(parsed[1]["run_note"], "quiet");
    assert_eq!(parsed[1]["delivery"]["kind"], "silent");
    assert_eq!(parsed[1]["delivery"]["reason"], "No changes since last run");
    assert!(parsed[1]["delivered_at"].is_null());
}

// --- MCP tool tests ---

#[tokio::test]
async fn test_mcp_list_empty() {
    let (server, _dir) = setup_server_with_dir().await;
    let result = server
        .mcp_list(Parameters(McpListParams {}))
        .await
        .expect("mcp_list ok");
    let text = call_result_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed, serde_json::json!([]), "empty list should return []");
}

#[tokio::test]
async fn stdio_send_progress_returns_progress_unavailable() {
    let (server, _dir) = setup_server().await;
    let result = server
        .send_progress(Parameters(crate::progress::SendProgressParams {
            message: "hello".to_string(),
        }))
        .await
        .expect("send_progress dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let text = call_result_text(result);
    let body: serde_json::Value = serde_json::from_str(&text).expect("body must be valid JSON");
    assert_eq!(body["error"]["code"], "progress_unavailable");
}

#[tokio::test]
async fn stdio_conversation_search_returns_scope_unavailable() {
    let (server, _dir) = setup_server().await;

    let thread_result = server
        .thread_search(Parameters(crate::right_backend::ConversationSearchParams {
            query: "needle".to_string(),
            limit: None,
        }))
        .await
        .expect("thread_search dispatch should be Ok with operation error");
    assert_eq!(thread_result.is_error, Some(true));
    let thread_text = call_result_text(thread_result);
    let thread_body: serde_json::Value =
        serde_json::from_str(&thread_text).expect("body must be valid JSON");
    assert_eq!(
        thread_body["error"]["code"],
        "conversation_scope_unavailable"
    );

    let chat_result = server
        .chat_search(Parameters(crate::right_backend::ConversationSearchParams {
            query: "needle".to_string(),
            limit: None,
        }))
        .await
        .expect("chat_search dispatch should be Ok with operation error");
    assert_eq!(chat_result.is_error, Some(true));
    let chat_text = call_result_text(chat_result);
    let chat_body: serde_json::Value =
        serde_json::from_str(&chat_text).expect("body must be valid JSON");
    assert_eq!(chat_body["error"]["code"], "conversation_scope_unavailable");

    let get_result = server
        .get_messages_by_id(Parameters(crate::right_backend::GetMessagesByIdParams {
            message_ids: vec![1],
        }))
        .await
        .expect("get_messages_by_id dispatch should be Ok with operation error");
    assert_eq!(get_result.is_error, Some(true));
    let get_text = call_result_text(get_result);
    let get_body: serde_json::Value =
        serde_json::from_str(&get_text).expect("body must be valid JSON");
    assert_eq!(get_body["error"]["code"], "conversation_scope_unavailable");

    let focus_result = server
        .thread_focus_set(Parameters(crate::right_backend::ThreadFocusSetParams {
            focus: "stay focused".to_string(),
        }))
        .await
        .expect("thread_focus_set dispatch should be Ok with operation error");
    assert_eq!(focus_result.is_error, Some(true));
    let focus_text = call_result_text(focus_result);
    let focus_body: serde_json::Value =
        serde_json::from_str(&focus_text).expect("body must be valid JSON");
    assert_eq!(
        focus_body["error"]["code"],
        "conversation_scope_unavailable"
    );
}

#[tokio::test]
async fn stdio_channel_tools_return_unavailable() {
    let (server, _dir) = setup_server().await;

    let list_result = server
        .channel_list(Parameters(crate::right_backend::ChannelListParams {}))
        .await
        .expect("channel_list dispatch should be Ok with operation error");
    assert_eq!(list_result.is_error, Some(true));
    let list_body: serde_json::Value =
        serde_json::from_str(&call_result_text(list_result)).expect("body must be valid JSON");
    assert_eq!(list_body["error"]["code"], "channel_tools_unavailable");

    let read_result = server
        .channel_read(Parameters(crate::right_backend::ChannelReadParams {
            channel: -200,
            limit: Some(2),
        }))
        .await
        .expect("channel_read dispatch should be Ok with operation error");
    assert_eq!(read_result.is_error, Some(true));
    let read_body: serde_json::Value =
        serde_json::from_str(&call_result_text(read_result)).expect("body must be valid JSON");
    assert_eq!(read_body["error"]["code"], "channel_tools_unavailable");
}

#[tokio::test]
async fn test_get_info_mentions_cron_and_mcp_tools() {
    let (server, _dir) = setup_server_with_dir().await;
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    assert!(
        !instructions.contains("store_record"),
        "instructions should NOT mention removed store_record: {instructions}"
    );
    assert!(
        instructions.contains("cron_create"),
        "instructions should mention cron_create: {instructions}"
    );
    assert!(
        instructions.contains("mcp_list"),
        "instructions should mention mcp_list: {instructions}"
    );
}

#[tokio::test]
async fn test_get_info_delegates_memory_routing_to_right_memory() {
    let (server, _dir) = setup_server_with_dir().await;
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    for needle in [
        "remember",
        "save this",
        "don't forget",
        "/right-memory",
        "mcp__right__memory_retain",
        "residual durable context",
        "fallback target",
    ] {
        assert!(
            instructions.contains(needle),
            "instructions should delegate memory routing to /right-memory: missing {needle:?}; {instructions}"
        );
    }

    for forbidden in [
        "Route tool/API/environment rules",
        "stable user facts/preferences to USER.md",
        "agent voice or escalation boundaries to SOUL.md",
        "core identity/security posture to IDENTITY.md",
    ] {
        assert!(
            !instructions.contains(forbidden),
            "instructions must not duplicate detailed /right-memory routing: found {forbidden:?}"
        );
    }
}
