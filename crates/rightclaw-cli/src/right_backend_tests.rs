use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;

use super::RightBackend;

/// Create a [`RightBackend`] with a temp dir as agents_dir and rightclaw_home.
/// Returns `(backend, agents_dir_path, _temp_dir_guard)`.
fn make_backend() -> (RightBackend, PathBuf, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir_all(&agents_dir).expect("create agents dir");
    let backend = RightBackend::new(agents_dir.clone(), None);
    (backend, agents_dir, tmp)
}

/// Create an agent directory with a valid data.db inside it.
fn create_agent_dir(agents_dir: &std::path::Path, name: &str) -> PathBuf {
    let agent_dir = agents_dir.join(name);
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    // open_connection will create the DB and run migrations
    let _conn = rightclaw::memory::open_connection(&agent_dir).expect("open memory db");
    agent_dir
}

#[test]
fn tools_list_returns_expected_count() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    // 4 memory + 7 cron + 1 mcp + 1 bootstrap = 13
    assert_eq!(
        tools.len(),
        13,
        "expected 13 tools, got {}: {:?}",
        tools.len(),
        tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>()
    );
}

#[test]
fn tools_list_has_unique_names() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "tool names must be unique");
}

#[test]
fn tools_list_all_have_descriptions() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    for tool in &tools {
        assert!(
            tool.description.is_some(),
            "tool '{}' is missing description",
            tool.name
        );
    }
}

#[tokio::test]
async fn store_and_query_roundtrip() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    // Store a record
    let store_result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "store_record",
            json!({"content": "the sky is blue", "tags": "facts,sky"}),
        )
        .await
        .expect("store_record should succeed");

    let text = format!("{:?}", store_result);
    assert!(text.contains("stored record id="), "got: {text}");

    // Query it back
    let query_result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "query_records",
            json!({"query": "sky"}),
        )
        .await
        .expect("query_records should succeed");

    let text = format!("{:?}", query_result);
    assert!(text.contains("the sky is blue"), "got: {text}");
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "nonexistent_tool",
            json!({}),
        )
        .await;

    assert!(result.is_err(), "unknown tool should return Err");
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(err_msg.contains("unknown tool"), "got: {err_msg}");
}

#[tokio::test]
async fn store_and_delete_roundtrip() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    // Store
    let store_result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "store_record",
            json!({"content": "temporary note"}),
        )
        .await
        .expect("store should succeed");

    // Extract ID from "stored record id=N"
    let text = format!("{:?}", store_result);
    let id_str = text
        .split("stored record id=")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("should contain id");
    let id: i64 = id_str.parse().expect("id should be numeric");

    // Delete
    let delete_result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "delete_record",
            json!({"id": id}),
        )
        .await
        .expect("delete should succeed");

    let text = format!("{:?}", delete_result);
    assert!(text.contains("deleted record"), "got: {text}");
}

#[tokio::test]
async fn bootstrap_done_missing_files() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call("test-agent", &agent_dir, "bootstrap_done", json!({}))
        .await
        .expect("bootstrap_done should return Ok");

    let text = format!("{:?}", result);
    assert!(
        text.contains("missing files"),
        "should report missing files, got: {text}"
    );
}

#[tokio::test]
async fn bootstrap_done_with_files() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    // Create required files
    for name in ["IDENTITY.md", "SOUL.md", "USER.md"] {
        std::fs::write(agent_dir.join(name), "test").expect("write file");
    }
    // Create BOOTSTRAP.md to verify it gets removed
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "bootstrap").expect("write bootstrap");

    let result = backend
        .tools_call("test-agent", &agent_dir, "bootstrap_done", json!({}))
        .await
        .expect("bootstrap_done should succeed");

    let text = format!("{:?}", result);
    assert!(text.contains("Bootstrap complete"), "got: {text}");
    assert!(
        !agent_dir.join("BOOTSTRAP.md").exists(),
        "BOOTSTRAP.md should be removed"
    );
}
