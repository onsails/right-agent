use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;

use super::RightBackend;

fn extract_error_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
        panic!("expected text content, got {:?}", result.content[0].raw);
    };
    serde_json::from_str(&t.text).expect("body must be valid JSON")
}

/// Create a [`RightBackend`] with a temp dir as agents_dir and right_home.
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
    let _conn = right_db::open_connection(&agent_dir, true).expect("open memory db");
    agent_dir
}

#[test]
fn tools_list_returns_expected_count() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    // 7 cron + 1 mcp + 1 bootstrap = 9
    assert_eq!(
        tools.len(),
        9,
        "expected 9 tools, got {}: {:?}",
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
async fn unknown_tool_returns_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent");

    let result = backend
        .tools_call("test-agent", &agent_dir, "nonexistent_tool", json!({}))
        .await;

    assert!(result.is_err(), "unknown tool should return Err");
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(err_msg.contains("unknown tool"), "got: {err_msg}");
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

// ---------------------------------------------------------------------------
// Integration tests — sandbox-aware bootstrap_done
// ---------------------------------------------------------------------------

/// Helper: spin up an ephemeral sandbox for testing.
/// Caller must delete sandbox after use.
async fn create_test_sandbox(
    mtls_dir: &std::path::Path,
    sandbox_name: &str,
) -> right_core::sandbox_exec::SandboxExec {
    right_core::test_cleanup::pkill_test_orphans(sandbox_name);
    right_core::test_cleanup::register_test_sandbox(sandbox_name);

    let mut grpc_client = right_agent::openshell::connect_grpc(mtls_dir)
        .await
        .expect("gRPC connect");

    // Clean up leftover from a previous failed run.
    if right_agent::openshell::sandbox_exists(&mut grpc_client, sandbox_name)
        .await
        .unwrap()
    {
        right_agent::openshell::delete_sandbox(sandbox_name).await;
        right_agent::openshell::wait_for_deleted(&mut grpc_client, sandbox_name, 60, 2)
            .await
            .expect("cleanup of leftover sandbox failed");
    }

    // Create sandbox with minimal policy.
    let policy_dir = tempfile::tempdir().unwrap();
    let policy_path = policy_dir.path().join("policy.yaml");
    std::fs::write(
        &policy_path,
        "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  outbound:
    endpoints:
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
",
    )
    .unwrap();

    let mut child = right_agent::openshell::spawn_sandbox(sandbox_name, &policy_path, None)
        .expect("failed to spawn sandbox");
    right_agent::openshell::wait_for_ready(&mut grpc_client, sandbox_name, 120, 2)
        .await
        .expect("sandbox did not become READY");
    let _ = child.kill().await;

    let sandbox_id = right_agent::openshell::resolve_sandbox_id(&mut grpc_client, sandbox_name)
        .await
        .expect("resolve sandbox_id");

    let sbox = right_core::sandbox_exec::SandboxExec::new(
        mtls_dir.to_path_buf(),
        sandbox_name.to_owned(),
        sandbox_id,
    );

    // Poll exec until ready — OpenShell reports READY before exec transport is available.
    for attempt in 1..=20 {
        match sbox.exec(&["echo", "ready"]).await {
            Ok((out, 0)) if out.trim() == "ready" => break,
            _ if attempt == 20 => panic!("exec not ready after 20 attempts"),
            _ => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
        }
    }

    sbox
}

#[tokio::test]
async fn bootstrap_done_sandbox_files_present() {
    let _slot = right_agent::openshell::acquire_sandbox_slot();
    let sandbox_name = "rightclaw-test-bootstrap-present";

    let mtls_dir = match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let sbox = create_test_sandbox(&mtls_dir, sandbox_name).await;

    // Create identity files inside sandbox.
    for name in ["IDENTITY.md", "SOUL.md", "USER.md"] {
        let (_, code) = sbox
            .exec(&["sh", "-c", &format!("echo '# test' > /sandbox/{name}")])
            .await
            .unwrap();
        assert_eq!(code, 0, "failed to create {name} in sandbox");
    }

    // Agent name must match: sandbox_name = "rightclaw-{agent_name}" (compat shim)
    let agent_name = "test-bootstrap-present";
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join(agent_name);
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "bootstrap").unwrap();
    let _conn = right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir, Some(mtls_dir.clone()));
    let result = backend
        .tools_call(agent_name, &agent_dir, "bootstrap_done", json!({}))
        .await
        .expect("bootstrap_done should succeed");

    let text = format!("{:?}", result);
    assert!(
        text.contains("Bootstrap complete"),
        "expected success, got: {text}"
    );
    assert!(
        !agent_dir.join("BOOTSTRAP.md").exists(),
        "BOOTSTRAP.md should be removed from host"
    );

    right_agent::openshell::delete_sandbox(sandbox_name).await;
    right_core::test_cleanup::unregister_test_sandbox(sandbox_name);
}

#[tokio::test]
async fn bootstrap_done_sandbox_files_missing() {
    let _slot = right_agent::openshell::acquire_sandbox_slot();
    let sandbox_name = "rightclaw-test-bootstrap-missing";

    let mtls_dir = match right_agent::openshell::preflight_check() {
        right_agent::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let sbox = create_test_sandbox(&mtls_dir, sandbox_name).await;

    // Create only IDENTITY.md — SOUL.md and USER.md are missing.
    let (_, code) = sbox
        .exec(&["sh", "-c", "echo '# test' > /sandbox/IDENTITY.md"])
        .await
        .unwrap();
    assert_eq!(code, 0);

    let agent_name = "test-bootstrap-missing";
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join(agent_name);
    std::fs::create_dir_all(&agent_dir).unwrap();
    let _conn = right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir, Some(mtls_dir.clone()));
    let result = backend
        .tools_call(agent_name, &agent_dir, "bootstrap_done", json!({}))
        .await
        .expect("bootstrap_done should return Ok (tool-level error)");

    let text = format!("{:?}", result);
    assert!(
        text.contains("missing files"),
        "expected missing files error, got: {text}"
    );
    assert!(
        text.contains("SOUL.md"),
        "should mention SOUL.md as missing, got: {text}"
    );
    assert!(
        text.contains("USER.md"),
        "should mention USER.md as missing, got: {text}"
    );

    right_agent::openshell::delete_sandbox(sandbox_name).await;
    right_core::test_cleanup::unregister_test_sandbox(sandbox_name);
}

// ---------------------------------------------------------------------------
// Allowlist validation tests for cron_create
// ---------------------------------------------------------------------------

use right_agent::agent::allowlist::{AllowedUser, AllowlistFile};

fn write_allowlist(agent_dir: &std::path::Path, users: &[i64], groups: &[i64]) {
    let now = chrono::Utc::now();
    let mut file = AllowlistFile::default();
    for &id in users {
        file.users.push(AllowedUser {
            id,
            label: None,
            added_by: None,
            added_at: now,
        });
    }
    for &id in groups {
        file.groups.push(right_agent::agent::allowlist::AllowedGroup {
            id,
            label: None,
            opened_by: None,
            opened_at: now,
        });
    }
    right_agent::agent::allowlist::write_file(agent_dir, &file).unwrap();
}

#[tokio::test]
async fn cron_create_rejects_target_not_in_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[100], &[]);
    // Initialize the agent's data.db so get_conn succeeds.
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -999_i64,
    });
    let result = backend
        .tools_call("a1", &agent_dir, "cron_create", args)
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        text.contains("not in allowlist") || text.contains("-999"),
        "expected allowlist rejection, got: {text}"
    );
}

#[tokio::test]
async fn cron_create_accepts_target_in_allowlist_group() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[], &[-200]);
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -200_i64,
        "target_thread_id": 7_i64,
    });
    let result = backend
        .tools_call("a1", &agent_dir, "cron_create", args)
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(text.contains("Created"), "got: {text}");
}

#[tokio::test]
async fn cron_create_rejects_missing_target_chat_id() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[100], &[]);
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        // target_chat_id deliberately omitted
    });
    let result = backend
        .tools_call("a1", &agent_dir, "cron_create", args)
        .await;
    assert!(
        result.is_err(),
        "missing required field must surface as error"
    );
}

#[tokio::test]
async fn cron_create_rejects_when_allowlist_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Note: NOT calling write_allowlist — file does not exist.
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -200_i64,
    });
    let result = backend
        .tools_call("a1", &agent_dir, "cron_create", args)
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        text.contains("does not exist") || text.contains("cannot be validated"),
        "expected missing-allowlist error, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// cron_update — target_chat_id + target_thread_id (Task 7)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cron_update_changes_target_chat_id_with_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[100], &[-200, -300]);
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            serde_json::json!({
                "job_name": "j1",
                "schedule": "*/5 * * * *",
                "prompt": "p",
                "target_chat_id": -200,
            }),
        )
        .await
        .unwrap();

    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_update",
            serde_json::json!({
                "job_name": "j1",
                "target_chat_id": -300,
            }),
        )
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(text.contains("Updated"), "got: {text}");

    // Reject change to non-allowlisted chat
    let denied = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_update",
            serde_json::json!({
                "job_name": "j1",
                "target_chat_id": -999,
            }),
        )
        .await
        .unwrap();
    let denied_text = denied
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        denied_text.contains("not in allowlist"),
        "got: {denied_text}"
    );
}

#[tokio::test]
async fn cron_update_clears_target_thread_id_with_explicit_null() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[], &[-200]);
    right_db::open_connection(&agent_dir, true).unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            serde_json::json!({
                "job_name": "j1",
                "schedule": "*/5 * * * *",
                "prompt": "p",
                "target_chat_id": -200,
                "target_thread_id": 7,
            }),
        )
        .await
        .unwrap();

    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_update",
            serde_json::json!({
                "job_name": "j1",
                "target_thread_id": null,
            }),
        )
        .await
        .unwrap();

    let conn = right_db::open_connection(&agent_dir, false).unwrap();
    let thread: Option<i64> = conn
        .query_row(
            "SELECT target_thread_id FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(thread.is_none(), "explicit null must clear the column");
}

#[tokio::test]
async fn bootstrap_done_returns_tool_error_when_files_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join("test-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let backend = RightBackend::new(agents_dir, None);
    let result = backend
        .tools_call("test-agent", &agent_dir, "bootstrap_done", serde_json::json!({}))
        .await
        .expect("dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "bootstrap_files_missing");
    let missing = body["error"]["details"]["missing"]
        .as_array()
        .expect("details.missing must be an array");
    let names: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"IDENTITY.md"), "missing IDENTITY.md: {names:?}");
    assert!(names.contains(&"SOUL.md"));
    assert!(names.contains(&"USER.md"));
}
