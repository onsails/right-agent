use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::RightBackend;

fn extract_error_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
        panic!("expected text content, got {:?}", result.content[0].raw);
    };
    serde_json::from_str(&t.text).expect("body must be valid JSON")
}

fn extract_json_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
        panic!("expected text content, got {:?}", result.content[0].raw);
    };
    serde_json::from_str(&t.text).expect("body must be valid JSON")
}

fn json_contains_forbidden_scope_name(value: &serde_json::Value) -> bool {
    const FORBIDDEN: [&str; 5] = ["chat_id", "thread_id", "scope", "user_id", "session_id"];

    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            FORBIDDEN.contains(&key.as_str()) || json_contains_forbidden_scope_name(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(json_contains_forbidden_scope_name),
        serde_json::Value::String(value) => FORBIDDEN.contains(&value.as_str()),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

fn json_contains_key(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == needle || json_contains_key(value, needle)),
        serde_json::Value::Array(values) => {
            values.iter().any(|value| json_contains_key(value, needle))
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
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
async fn create_agent_dir(agents_dir: &std::path::Path, name: &str) -> PathBuf {
    let agent_dir = agents_dir.join(name);
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    // open_connection will create the DB and run migrations
    let _conn = right_db::open_connection(&agent_dir, true)
        .await
        .expect("open memory db");
    agent_dir
}

fn create_host_skill_package(agent_dir: &Path, skill_name: &str) {
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");
    let skill_dir = agent_dir.join(".claude/skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Learned skill\n").expect("write skill");
}

async fn start_progress_sink(dir: &Path) -> PathBuf {
    let socket_path = dir.join("progress-sink.sock");
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).expect("remove stale progress sink socket");
    }
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind progress sink socket");
    let app = axum::Router::new().route(
        "/progress/send",
        axum::routing::post(|| async {
            axum::Json(right_mcp::internal_client::ProgressSendResponse {
                ok: true,
                message_id: Some(1),
            })
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    socket_path
}

async fn register_foreground_learning(
    backend: &RightBackend,
    invocation_id: &str,
    bot_socket_path: PathBuf,
) {
    register_learning_kind(
        backend,
        invocation_id,
        crate::progress::ProgressInvocationKind::Foreground,
        bot_socket_path,
    )
    .await;
}

async fn register_learning_kind(
    backend: &RightBackend,
    invocation_id: &str,
    kind: crate::progress::ProgressInvocationKind,
    bot_socket_path: PathBuf,
) {
    backend
        .progress_registry()
        .register(crate::progress::ProgressRegistration {
            invocation_id: invocation_id.to_owned(),
            kind,
            bot_socket_path,
            bot_send_token: "send-token".to_owned(),
            conversation_scope: None,
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_async_run(
    agent_dir: &std::path::Path,
    id: &str,
    kind: &str,
    producer_ref: Option<&str>,
    source_session_id: Option<&str>,
    run_session_id: &str,
    started_at: &str,
    status: &str,
) {
    let conn = right_db::open_connection(agent_dir, false)
        .await
        .expect("open db");
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, source_session_id, run_session_id, target_chat_id,
            started_at, status, log_path, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, -100, ?6, ?7, ?8, ?9, ?10, ?6, ?6)",
        right_db::params![
            id,
            kind,
            producer_ref,
            source_session_id,
            run_session_id,
            started_at,
            status,
            format!("/tmp/{id}.log"),
            i64::from(kind == "background"),
            if kind == "background" {
                "pending"
            } else {
                "none"
            },
        ],
    )
    .await
    .expect("insert async run");
}

#[test]
fn tools_list_returns_expected_count() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    // 9 cron + 1 mcp + 1 progress + 1 send_message + 2 learning + 3 conversation
    // + 3 channel + 5 forum + 1 conversation focus + 1 bootstrap
    // + 1 provider capabilities = 28
    assert_eq!(
        tools.len(),
        28,
        "expected 28 tools, got {}: {:?}",
        tools.len(),
        tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>()
    );
}

#[test]
fn tools_list_includes_learning_tools() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(
        names.contains(&"skill_learning_start"),
        "missing skill_learning_start: {names:?}"
    );
    assert!(
        names.contains(&"skill_learning_finish"),
        "missing skill_learning_finish: {names:?}"
    );
}

#[test]
fn tools_list_includes_channel_tools() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(names.contains(&"channel_list"), "missing channel_list");
    assert!(names.contains(&"channel_read"), "missing channel_read");
    assert!(names.contains(&"channel_post"), "missing channel_post");
}

#[tokio::test]
async fn channel_list_returns_only_opened_channels() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(
        &agent_dir,
        &[(-100, GroupKind::Group), (-200, GroupKind::Channel)],
    );

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_list",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_list should succeed");

    assert_ne!(result.is_error, Some(true));
    let channels = extract_json_body(&result);
    assert_eq!(channels.as_array().map(Vec::len), Some(1));
    assert_eq!(channels[0]["id"], -200);
    assert_eq!(channels[0]["label"], serde_json::Value::Null);
}

#[tokio::test]
async fn channel_list_rejects_unknown_params() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_list",
            json!({ "unexpected": true }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("invalid channel_list params should be a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn channel_read_rejects_channel_not_opened() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": -200 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_read should return a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "channel_not_opened");
}

#[tokio::test]
async fn channel_read_rejects_group_kind_allowlist_entry() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-100, GroupKind::Group)]);

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": -100 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_read should return a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "channel_not_opened");
}

#[tokio::test]
async fn channel_post_rejects_unopened_channel_before_uds() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    register_foreground_learning(
        &backend,
        "inv",
        tmp.path().join("channel-post-should-not-connect.sock"),
    )
    .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_post",
            json!({ "channel": -200, "text": "hello" }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv".to_owned()),
            },
        )
        .await
        .expect("unopened channel must return a tool-level error before UDS");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "channel_not_opened");
}

#[tokio::test]
async fn channel_post_requires_registered_invocation() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-200, GroupKind::Channel)]);

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_post",
            json!({ "channel": -200, "text": "hello" }),
            crate::progress::ToolCallContext {
                invocation_id: Some("unknown".to_owned()),
            },
        )
        .await
        .expect("unknown invocation must return a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "channel_post_unavailable");
}

#[tokio::test]
async fn channel_post_rejects_nonforeground_noncron_invocation() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-200, GroupKind::Channel)]);
    register_learning_kind(
        &backend,
        "background",
        crate::progress::ProgressInvocationKind::BackgroundReview,
        tmp.path().join("channel-post-should-not-connect.sock"),
    )
    .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_post",
            json!({ "channel": -200, "text": "hello" }),
            crate::progress::ToolCallContext {
                invocation_id: Some("background".to_owned()),
            },
        )
        .await
        .expect("forbidden invocation must return a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "channel_post_forbidden");
}

#[tokio::test]
async fn channel_read_returns_last_posts_newest_first() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-200, GroupKind::Channel)]);
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        for message_id in 1..=3 {
            right_db::conversation::archive_message(
                &conn,
                right_db::conversation::ConversationMessage {
                    platform: "telegram",
                    chat_id: -200,
                    thread_id: 0,
                    message_id: Some(message_id),
                    sender_user_id: Some(9001),
                    sender_name: Some("Channel"),
                    addressed_to_bot: false,
                    routed_to_agent: false,
                    root_session_id: None,
                    turn_id: None,
                    role: right_db::conversation::ConversationRole::User,
                    content: &format!("post {message_id}"),
                },
            )
            .await
            .expect("archive post");
        }
    }

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": -200, "limit": 2 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_read should succeed");

    assert_ne!(result.is_error, Some(true));
    let posts = extract_json_body(&result);
    assert_eq!(posts.as_array().map(Vec::len), Some(2));
    assert_eq!(posts[0]["message_id"], 3);
    assert_eq!(posts[1]["message_id"], 2);
    assert_eq!(posts[0]["snippet"], "post 3");
    assert_eq!(posts[1]["snippet"], "post 2");
}

#[tokio::test]
async fn channel_read_returns_posts_truncated_to_one_hundred_eighty_characters() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-200, GroupKind::Channel)]);
    let post = "x".repeat(500);
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        right_db::conversation::archive_message(
            &conn,
            right_db::conversation::ConversationMessage {
                platform: "telegram",
                chat_id: -200,
                thread_id: 0,
                message_id: Some(1),
                sender_user_id: Some(9001),
                sender_name: Some("Channel"),
                addressed_to_bot: false,
                routed_to_agent: false,
                root_session_id: None,
                turn_id: None,
                role: right_db::conversation::ConversationRole::User,
                content: &post,
            },
        )
        .await
        .expect("archive post");
    }

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": -200 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_read should succeed");

    let posts = extract_json_body(&result);
    let snippet = posts[0]["snippet"].as_str().expect("post snippet");
    let truncated = snippet
        .strip_suffix("...")
        .expect("long posts should carry a truncation marker");
    assert_eq!(truncated.len(), 180);
    assert_eq!(truncated, &post[..180]);
}

#[tokio::test]
async fn channel_read_rejects_invalid_params_as_tool_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": "not-a-chat-id" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("invalid channel_read params should be a tool-level error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn channel_read_caps_results_at_one_hundred_posts() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    write_allowlist_with_group_kinds(&agent_dir, &[(-200, GroupKind::Channel)]);
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        for message_id in 1..=101 {
            right_db::conversation::archive_message(
                &conn,
                right_db::conversation::ConversationMessage {
                    platform: "telegram",
                    chat_id: -200,
                    thread_id: 0,
                    message_id: Some(message_id),
                    sender_user_id: Some(9001),
                    sender_name: Some("Channel"),
                    addressed_to_bot: false,
                    routed_to_agent: false,
                    root_session_id: None,
                    turn_id: None,
                    role: right_db::conversation::ConversationRole::User,
                    content: &format!("post {message_id}"),
                },
            )
            .await
            .expect("archive post");
        }
    }

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "channel_read",
            json!({ "channel": -200, "limit": 101 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("channel_read should succeed");

    assert_ne!(result.is_error, Some(true));
    let posts = extract_json_body(&result);
    assert_eq!(posts.as_array().map(Vec::len), Some(100));
    assert_eq!(posts[0]["message_id"], 101);
    assert_eq!(posts[99]["message_id"], 2);
}

#[test]
fn tools_list_includes_conversation_search_tools_without_scope_params() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(names.contains(&"thread_search"), "missing thread_search");
    assert!(names.contains(&"chat_search"), "missing chat_search");
    assert!(
        names.contains(&"get_messages_by_id"),
        "missing get_messages_by_id"
    );

    for tool_name in ["thread_search", "chat_search", "get_messages_by_id"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name.as_ref() == tool_name)
            .expect("tool must exist");
        let schema = serde_json::Value::Object((*tool.input_schema).clone());
        assert!(
            !json_contains_forbidden_scope_name(&schema),
            "{tool_name} schema exposes forbidden scope fields: {schema}"
        );
        if tool_name == "get_messages_by_id" {
            let properties = schema
                .pointer("/properties")
                .and_then(serde_json::Value::as_object)
                .expect("get_messages_by_id schema must expose object properties");
            let property_names: Vec<&str> = properties.keys().map(String::as_str).collect();
            assert_eq!(
                property_names,
                vec!["message_ids"],
                "{tool_name} schema must expose only message_ids for agent input: {schema}"
            );
            assert!(
                properties
                    .get("message_ids")
                    .is_some_and(serde_json::Value::is_object),
                "{tool_name} schema must expose message_ids only for agent input: {schema}"
            );
        }
    }
}

#[test]
fn thread_focus_set_tool_is_registered() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let tool = tools
        .iter()
        .find(|tool| tool.name.as_ref() == "thread_focus_set")
        .expect("thread_focus_set tool must be registered");

    let description = tool
        .description
        .as_ref()
        .expect("thread_focus_set must describe safe usage");
    for required in [
        "CURRENT Telegram conversation",
        "shown to you on every future turn",
        "empty string clears it",
        "Scope is server-enforced",
        "not agent-controlled",
    ] {
        assert!(
            description.contains(required),
            "thread_focus_set description missing {required:?}: {description}"
        );
    }

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert!(
        !json_contains_forbidden_scope_name(&schema),
        "thread_focus_set schema exposes forbidden scope fields: {schema}"
    );
    let properties = schema
        .pointer("/properties")
        .and_then(serde_json::Value::as_object)
        .expect("thread_focus_set schema must expose object properties");
    let property_names: Vec<&str> = properties.keys().map(String::as_str).collect();
    assert_eq!(
        property_names,
        vec!["focus"],
        "thread_focus_set schema must expose only focus for agent input: {schema}"
    );
    assert!(
        properties
            .get("focus")
            .is_some_and(serde_json::Value::is_object),
        "thread_focus_set schema must expose focus only for agent input: {schema}"
    );
    assert_eq!(
        schema.pointer("/properties/focus/maxLength"),
        Some(&serde_json::json!(2000)),
        "thread_focus_set focus schema must cap persistent prompt text: {schema}"
    );
    assert_eq!(
        schema
            .pointer("/additionalProperties")
            .and_then(|v| v.as_bool()),
        Some(false),
        "thread_focus_set params must deny unknown fields: {schema}"
    );
}

#[test]
fn tools_list_includes_provider_capabilities_as_no_arg_scoped_tool() {
    let (backend, _, _tmp) = make_backend();
    let tools = backend.tools_list();
    let tool = tools
        .iter()
        .find(|tool| tool.name.as_ref() == "provider_capabilities")
        .expect("provider_capabilities tool must be registered");

    let description = tool
        .description
        .as_ref()
        .expect("provider_capabilities must describe safe usage");
    for required in [
        "providers attached to your own sandbox",
        "env-var placeholder names",
        "allowed binaries",
        "valid hosts",
        "server-enforced",
        "no arguments",
        "401/403",
        "specific binary/host may be required",
    ] {
        assert!(
            description.contains(required),
            "provider_capabilities description missing {required:?}: {description}"
        );
    }

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(
        schema.pointer("/type").and_then(|v| v.as_str()),
        Some("object")
    );
    assert!(
        schema
            .pointer("/properties")
            .and_then(|v| v.as_object())
            .is_none_or(serde_json::Map::is_empty),
        "provider_capabilities must not expose caller-controlled params: {schema}"
    );
    assert_eq!(
        schema
            .pointer("/additionalProperties")
            .and_then(|v| v.as_bool()),
        Some(false),
        "provider_capabilities params must deny unknown fields: {schema}"
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
async fn provider_capabilities_rejects_agent_supplied_scope_params() {
    let (backend, _agents_dir, _tmp) = make_backend();
    let result = backend
        .tools_call(
            "test-agent",
            Path::new("/tmp/unused"),
            "provider_capabilities",
            json!({ "sandbox_name": "other-sandbox" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn provider_capabilities_propagates_invalid_agent_config() {
    let mtls_dir = tempfile::tempdir().expect("mtls tempdir");
    let tmp = TempDir::new().expect("tempdir");
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join("test-agent");
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox: [").expect("write invalid agent yaml");
    let backend = RightBackend::new(agents_dir, Some(mtls_dir.path().to_path_buf()));

    let err = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "provider_capabilities",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect_err("invalid agent.yaml must propagate instead of falling back to another sandbox");

    let message = format!("{err:#}");
    assert!(
        message.contains("provider_capabilities: failed to parse agent config"),
        "error should identify provider_capabilities config parsing: {message}"
    );
    assert!(
        message.contains("agent.yaml"),
        "error should preserve agent.yaml parse context: {message}"
    );
}

#[tokio::test]
async fn cron_list_runs_reads_async_cron_rows_and_excludes_background() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    insert_async_run(
        &agent_dir,
        "cron-old",
        "cron",
        Some("job-a"),
        None,
        "cron-old",
        "2026-04-01T09:00:00Z",
        "success",
    )
    .await;
    insert_async_run(
        &agent_dir,
        "cron-new",
        "cron",
        Some("job-a"),
        None,
        "cron-new",
        "2026-04-01T10:00:00Z",
        "failed",
    )
    .await;
    insert_async_run(
        &agent_dir,
        "bg-newer",
        "background",
        Some("job-a"),
        Some("main"),
        "bg-session",
        "2026-04-01T11:00:00Z",
        "success",
    )
    .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "cron_list_runs",
            json!({ "job_name": "job-a", "limit": 10 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_list_runs should succeed");

    let body = extract_json_body(&result);
    let ids: Vec<&str> = body
        .as_array()
        .expect("cron_list_runs returns an array")
        .iter()
        .map(|row| row["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["cron-new", "cron-old"]);
}

#[tokio::test]
async fn cron_show_run_reads_async_cron_rows_and_excludes_background() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    insert_async_run(
        &agent_dir,
        "cron-show",
        "cron",
        Some("job-a"),
        None,
        "cron-show",
        "2026-04-01T10:00:00Z",
        "success",
    )
    .await;
    insert_async_run(
        &agent_dir,
        "bg-show",
        "background",
        Some("job-a"),
        Some("main"),
        "bg-session",
        "2026-04-01T11:00:00Z",
        "success",
    )
    .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "cron_show_run",
            json!({ "run_id": "cron-show" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_show_run should succeed");
    let body = extract_json_body(&result);
    assert_eq!(body["id"], "cron-show");
    assert_eq!(body["job_name"], "job-a");

    let background_result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "cron_show_run",
            json!({ "run_id": "bg-show" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_show_run should handle missing run");
    let rmcp::model::RawContent::Text(t) = &background_result.content[0].raw else {
        panic!(
            "expected text content, got {:?}",
            background_result.content[0].raw
        );
    };
    assert!(
        t.text.contains("not found"),
        "background row should not be exposed as cron history: {}",
        t.text
    );
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "nonexistent_tool",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await;

    assert!(result.is_err(), "unknown tool should return Err");
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(err_msg.contains("unknown tool"), "got: {err_msg}");
}

#[tokio::test]
async fn thread_search_without_invocation_scope_returns_tool_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_search",
            json!({ "query": "needle" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "conversation_scope_unavailable");
}

#[tokio::test]
async fn thread_search_rejects_agent_supplied_scope_params() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_search",
            json!({ "query": "needle", "chat_id": 100 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn get_messages_by_id_without_invocation_scope_returns_tool_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "get_messages_by_id",
            json!({ "message_ids": [1] }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "conversation_scope_unavailable");
}

#[tokio::test]
async fn get_messages_by_id_rejects_agent_supplied_scope_params() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    for (field_name, args) in [
        ("chat_id", json!({ "message_ids": [1], "chat_id": 100 })),
        ("thread_id", json!({ "message_ids": [1], "thread_id": 7 })),
    ] {
        let result = backend
            .tools_call(
                "test-agent",
                &agent_dir,
                "get_messages_by_id",
                args,
                crate::progress::ToolCallContext::default(),
            )
            .await
            .expect("tool errors should be returned as CallToolResult");

        assert_eq!(result.is_error, Some(true));
        let body = extract_error_body(&result);
        assert_eq!(body["error"]["code"], "invalid_argument");
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("error message")
                .contains(field_name),
            "error should identify rejected field {field_name}: {body}"
        );
    }
}

#[tokio::test]
async fn thread_focus_set_rejects_agent_supplied_scope_params() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    for (field_name, args) in [
        (
            "chat_id",
            json!({ "focus": "stay focused", "chat_id": 100 }),
        ),
        (
            "thread_id",
            json!({ "focus": "stay focused", "thread_id": 7 }),
        ),
    ] {
        let result = backend
            .tools_call(
                "test-agent",
                &agent_dir,
                "thread_focus_set",
                args,
                crate::progress::ToolCallContext::default(),
            )
            .await
            .expect("tool errors should be returned as CallToolResult");

        assert_eq!(result.is_error, Some(true));
        let body = extract_error_body(&result);
        assert_eq!(body["error"]["code"], "invalid_argument");
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("error message")
                .contains(field_name),
            "error should identify rejected field {field_name}: {body}"
        );
    }
}

#[tokio::test]
async fn thread_focus_set_without_invocation_scope_returns_tool_error() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_focus_set",
            json!({ "focus": "stay focused" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "conversation_scope_unavailable");
}

#[tokio::test]
async fn thread_focus_set_writes_and_clears_current_scope() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    register_foreground_scope(&backend, &agent_dir).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_focus_set",
            json!({ "focus": "  stay on invoices  " }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("thread_focus_set should succeed");

    assert_ne!(result.is_error, Some(true));
    let body = extract_json_body(&result);
    assert_eq!(body["status"].as_str(), Some("ok"));
    assert_eq!(body["cleared"].as_bool(), Some(false));
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let row = right_db::thread_focus::get(&conn, 100, 7)
        .await
        .expect("get thread focus")
        .expect("current scope focus row");
    assert_eq!(row.agent_focus.as_deref(), Some("stay on invoices"));
    assert!(
        right_db::thread_focus::get(&conn, 100, 0)
            .await
            .expect("get general scope")
            .is_none(),
        "thread_focus_set must not write outside the current thread"
    );

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_focus_set",
            json!({ "focus": "   " }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("thread_focus_set clear should succeed");

    assert_ne!(result.is_error, Some(true));
    let body = extract_json_body(&result);
    assert_eq!(body["status"].as_str(), Some("ok"));
    assert_eq!(body["cleared"].as_bool(), Some(true));
    let row = right_db::thread_focus::get(&conn, 100, 7)
        .await
        .expect("get thread focus")
        .expect("current scope focus row");
    assert_eq!(row.agent_focus, None);
}

#[tokio::test]
async fn thread_focus_set_rejects_overlong_focus_without_writing() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    register_foreground_scope(&backend, &agent_dir).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_focus_set",
            json!({ "focus": "x".repeat(2001) }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("at most 2000 characters"),
        "unexpected error body: {body}"
    );
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    assert!(
        right_db::thread_focus::get(&conn, 100, 7)
            .await
            .expect("get thread focus")
            .is_none(),
        "overlong thread_focus_set must not persist focus text"
    );
}

#[tokio::test]
async fn chat_search_rejects_query_without_searchable_terms() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "chat_search",
            json!({ "query": "!!!" }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

async fn archive_search_fixture(conn: &right_db::Connection) {
    for (chat_id, thread_id, message_id, content) in [
        (100, 7, 1, "needle in current thread"),
        (100, 8, 2, "needle in other thread"),
        (200, 7, 3, "needle in other chat"),
    ] {
        right_db::conversation::archive_message(
            conn,
            right_db::conversation::ConversationMessage {
                platform: "telegram",
                chat_id,
                thread_id,
                message_id: Some(message_id),
                sender_user_id: Some(9001),
                sender_name: Some("Ada"),
                addressed_to_bot: true,
                routed_to_agent: true,
                root_session_id: Some("session-1"),
                turn_id: None,
                role: right_db::conversation::ConversationRole::User,
                content,
            },
        )
        .await
        .expect("archive message");
    }
}

async fn register_foreground_scope(backend: &RightBackend, agent_dir: &std::path::Path) {
    backend
        .progress_registry()
        .register(crate::progress::ProgressRegistration {
            invocation_id: "inv-search".to_owned(),
            kind: crate::progress::ProgressInvocationKind::Foreground,
            bot_socket_path: agent_dir.join("missing-bot.sock"),
            bot_send_token: "send-token".to_owned(),
            conversation_scope: Some(crate::progress::ConversationScope {
                chat_id: 100,
                thread_id: 7,
            }),
        })
        .await;
}

#[tokio::test]
async fn get_messages_by_id_filters_current_thread_and_returns_messages() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        archive_search_fixture(&conn).await;
    }
    register_foreground_scope(&backend, &agent_dir).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "get_messages_by_id",
            json!({ "message_ids": [1, 2, 3, 999] }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("get_messages_by_id should succeed");

    assert_ne!(result.is_error, Some(true));
    let body = extract_json_body(&result);
    let messages = body["messages"]
        .as_array()
        .expect("messages must be an array");
    assert_eq!(messages.len(), 1, "unexpected messages: {body}");
    let row = messages.first().expect("one message");
    assert_eq!(row["message_id"].as_i64(), Some(1));
    assert_eq!(row["sender_name"].as_str(), Some("Ada"));
    assert_eq!(row["text"].as_str(), Some("needle in current thread"));
    assert_eq!(row["role"].as_str(), Some("user"));
    for field_name in ["message_id", "sender_name", "text", "role"] {
        assert!(
            row.get(field_name).is_some(),
            "message missing {field_name}: {row}"
        );
    }
    assert!(
        !json_contains_key(&body, "chat_id"),
        "leaked chat_id: {body}"
    );
}

#[tokio::test]
async fn get_messages_by_id_rejects_too_many_ids() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    register_foreground_scope(&backend, &agent_dir).await;
    let message_ids: Vec<i32> = (0..51).collect();

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "get_messages_by_id",
            json!({ "message_ids": message_ids }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("at most 50"),
        "unexpected error body: {body}"
    );
}

#[tokio::test]
async fn thread_search_filters_current_thread() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        archive_search_fixture(&conn).await;
    }
    register_foreground_scope(&backend, &agent_dir).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "thread_search",
            json!({ "query": "needle", "limit": 10 }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("thread_search should succeed");

    assert_ne!(result.is_error, Some(true));
    let body = extract_json_body(&result);
    let message_ids: Vec<i64> = body["results"]
        .as_array()
        .expect("results must be an array")
        .iter()
        .map(|row| row["message_id"].as_i64().expect("message_id"))
        .collect();
    assert_eq!(message_ids, vec![1]);
    assert!(
        !json_contains_key(&body, "chat_id"),
        "leaked chat_id: {body}"
    );
}

#[tokio::test]
async fn chat_search_includes_other_threads_in_same_chat() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        archive_search_fixture(&conn).await;
    }
    register_foreground_scope(&backend, &agent_dir).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "chat_search",
            json!({ "query": "needle", "limit": 10 }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-search".to_owned()),
            },
        )
        .await
        .expect("chat_search should succeed");

    assert_ne!(result.is_error, Some(true));
    let body = extract_json_body(&result);
    let message_ids: Vec<i64> = body["results"]
        .as_array()
        .expect("results must be an array")
        .iter()
        .map(|row| row["message_id"].as_i64().expect("message_id"))
        .collect();
    assert_eq!(message_ids, vec![2, 1]);
    assert!(
        !json_contains_key(&body, "chat_id"),
        "leaked chat_id: {body}"
    );
}

#[tokio::test]
async fn bootstrap_done_missing_files() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "bootstrap_done",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
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
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    // Create required files
    for name in ["IDENTITY.md", "SOUL.md", "USER.md"] {
        std::fs::write(agent_dir.join(name), "test").expect("write file");
    }
    // Create BOOTSTRAP.md to verify it gets removed
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "bootstrap").expect("write bootstrap");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "bootstrap_done",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("bootstrap_done should succeed");

    let text = format!("{:?}", result);
    assert!(text.contains("Bootstrap complete"), "got: {text}");
    assert!(
        !agent_dir.join("BOOTSTRAP.md").exists(),
        "BOOTSTRAP.md should be removed"
    );
}

#[tokio::test]
async fn skill_learning_start_rejects_create_without_learned_prefix() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "create",
                "skill_name": "custom-skill",
                "reason": "user requested a reusable workflow",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(right_mcp::LEARNED_SKILL_PREFIX),
        "error should mention learned-skill prefix: {body}"
    );
}

#[tokio::test]
async fn skill_learning_start_rejects_core_skill_update() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "right-cron",
                "reason": "try to patch a built-in skill",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_core_readonly");
}

#[tokio::test]
async fn skill_learning_start_rejects_non_learned_update() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");
    let skill_dir = agent_dir.join(".claude/skills/custom-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Custom skill").expect("write skill");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "custom-skill",
                "reason": "make the skill more precise",
                "message": "I am updating custom-skill with a narrower workflow.",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(right_mcp::LEARNED_SKILL_PREFIX),
        "error should mention learned-skill prefix: {body}"
    );
}

#[tokio::test]
async fn skill_learning_start_rejects_update_when_package_missing() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "rightx-custom-skill",
                "reason": "update a missing package",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_package_missing");
}

#[tokio::test]
async fn skill_learning_finish_requires_receipt_message_for_success() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-user-workflow",
                "status": "created",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("message"),
        "successful finish without receipt should fail on message: {body}"
    );
}

#[tokio::test]
async fn skill_learning_finish_rejects_success_when_package_missing() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-user-workflow",
                "status": "created",
                "message": "I learned rightx-user-workflow and will use it when this pattern appears again.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-1".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_package_missing");
}

#[tokio::test]
async fn skill_learning_finish_created_updates_lifecycle_with_foreground_provenance() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    let skill_name = "rightx-user-workflow";
    create_host_skill_package(&agent_dir, skill_name);
    let socket_path = start_progress_sink(tmp.path()).await;
    register_foreground_learning(&backend, "inv-created", socket_path).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": skill_name,
                "status": "created",
                "message": "I learned rightx-user-workflow and will use it when this pattern appears again.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-created".to_owned()),
            },
        )
        .await
        .expect("tool call should complete");

    assert_eq!(result.is_error, Some(false));
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let row = right_lifecycle::get(&conn, skill_name)
        .await
        .expect("read lifecycle")
        .expect("lifecycle row");
    assert_eq!(row.state, right_lifecycle::LifecycleState::Active);
    assert_eq!(row.created_by, right_lifecycle::CreatedBy::Foreground);
    assert_eq!(row.patch_count, 0);
    assert!(row.created_at.is_some(), "created_at must be set");
    assert_eq!(row.last_patched_at, None);

    let audit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id = 'inv-created'",
            [],
            |row| row.get(0),
        )
        .await
        .expect("count learning events");
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn skill_learning_finish_updated_bumps_patch_and_preserves_created_by() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    let skill_name = "rightx-user-workflow";
    create_host_skill_package(&agent_dir, skill_name);
    let created_at = chrono::DateTime::parse_from_rfc3339("2026-05-24T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        right_lifecycle::mark_created(
            &conn,
            skill_name,
            right_lifecycle::CreatedBy::Curator,
            created_at,
        )
        .await
        .expect("seed lifecycle");
    }
    let socket_path = start_progress_sink(tmp.path()).await;
    register_foreground_learning(&backend, "inv-updated", socket_path).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "update",
                "skill_name": skill_name,
                "status": "updated",
                "message": "I updated rightx-user-workflow with the latest workflow details.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-updated".to_owned()),
            },
        )
        .await
        .expect("tool call should complete");

    assert_eq!(result.is_error, Some(false));
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let row = right_lifecycle::get(&conn, skill_name)
        .await
        .expect("read lifecycle")
        .expect("lifecycle row");
    assert_eq!(row.state, right_lifecycle::LifecycleState::Active);
    assert_eq!(row.created_by, right_lifecycle::CreatedBy::Curator);
    assert_eq!(row.patch_count, 1);
    assert_eq!(row.created_at, Some(created_at));
    assert!(row.last_patched_at.is_some(), "last_patched_at must be set");
}

#[tokio::test]
async fn skill_learning_start_background_kinds_insert_events_without_telegram_delivery() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");
    let kinds = [
        (
            "inv-probe-start",
            crate::progress::ProgressInvocationKind::ProbeWriter,
        ),
        (
            "inv-curator-start",
            crate::progress::ProgressInvocationKind::Curator,
        ),
    ];

    for (invocation_id, kind) in kinds {
        register_learning_kind(
            &backend,
            invocation_id,
            kind,
            tmp.path().join(format!("{invocation_id}-missing.sock")),
        )
        .await;

        let result = backend
            .tools_call(
                "test-agent",
                &agent_dir,
                "skill_learning_start",
                json!({
                    "action": "create",
                    "skill_name": format!("rightx-{invocation_id}"),
                    "reason": "background learning should be recorded without Telegram delivery",
                }),
                crate::progress::ToolCallContext {
                    invocation_id: Some(invocation_id.to_owned()),
                },
            )
            .await
            .expect("tool call should complete");

        assert_eq!(result.is_error, Some(false));
    }

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id IN ('inv-probe-start', 'inv-curator-start')",
            [],
            |row| row.get(0),
        )
        .await
        .expect("count learning events");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn skill_learning_finish_background_kinds_update_lifecycle_without_telegram_delivery() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    let cases = [
        (
            "inv-probe-finish",
            "rightx-probe-finish",
            crate::progress::ProgressInvocationKind::ProbeWriter,
            right_lifecycle::CreatedBy::ProbeWriter,
        ),
        (
            "inv-curator-finish",
            "rightx-curator-finish",
            crate::progress::ProgressInvocationKind::Curator,
            right_lifecycle::CreatedBy::Curator,
        ),
    ];

    for (invocation_id, skill_name, kind, expected_created_by) in cases {
        create_host_skill_package(&agent_dir, skill_name);
        register_learning_kind(
            &backend,
            invocation_id,
            kind,
            tmp.path().join(format!("{invocation_id}-missing.sock")),
        )
        .await;

        let result = backend
            .tools_call(
                "test-agent",
                &agent_dir,
                "skill_learning_finish",
                json!({
                    "action": "create",
                    "skill_name": skill_name,
                    "status": "created",
                    "message": "Background learning recorded this skill.",
                }),
                crate::progress::ToolCallContext {
                    invocation_id: Some(invocation_id.to_owned()),
                },
            )
            .await
            .expect("tool call should complete");

        assert_eq!(result.is_error, Some(false));
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        let row = right_lifecycle::get(&conn, skill_name)
            .await
            .expect("read lifecycle")
            .expect("lifecycle row");
        assert_eq!(row.created_by, expected_created_by);
    }
}

#[tokio::test]
async fn skill_learning_finish_returns_error_when_lifecycle_write_fails() {
    let (backend, agents_dir, tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    let skill_name = "rightx-user-workflow";
    create_host_skill_package(&agent_dir, skill_name);
    {
        let conn = right_db::open_connection(&agent_dir, false)
            .await
            .expect("open db");
        conn.execute("DROP TABLE skill_lifecycle", [])
            .await
            .expect("drop lifecycle table");
    }
    let socket_path = start_progress_sink(tmp.path()).await;
    register_foreground_learning(&backend, "inv-lifecycle-fails", socket_path).await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": skill_name,
                "status": "created",
                "message": "I learned rightx-user-workflow and will use it when this pattern appears again.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-lifecycle-fails".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_lifecycle_write_failed");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let audit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id = 'inv-lifecycle-fails'",
            [],
            |row| row.get(0),
        )
        .await
        .expect("count learning events");
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn skill_learning_finish_persists_hint_outcome_for_aborted_finish() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-user-workflow",
                "status": "aborted",
                "hint_outcome": "refused",
                "message": "Refused because there was not enough evidence.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-hint-outcome".to_owned()),
            },
        )
        .await
        .expect("tool call should complete");

    assert_eq!(result.is_error, Some(false));
    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let hint_outcome: Option<String> = conn
        .query_row(
            "SELECT hint_outcome FROM skill_learning_events WHERE invocation_id = 'inv-hint-outcome'",
            [],
            |row| row.get(0),
        )
        .await
        .expect("hint outcome row");
    assert_eq!(hint_outcome.as_deref(), Some("refused"));
}

#[tokio::test]
async fn skill_learning_finish_rejects_invalid_hint_outcome_before_insert() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-user-workflow",
                "status": "aborted",
                "hint_outcome": "bogus",
                "message": "Invalid hint outcome should be rejected.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-invalid-hint".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id = 'inv-invalid-hint'",
            [],
            |row| row.get(0),
        )
        .await
        .expect("count learning events");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn skill_learning_finish_rejects_sandboxed_host_fallback_without_mtls() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  mode: openshell\n",
    )
    .expect("write agent config");
    let skill_dir = agent_dir.join(".claude/skills/rightx-demo");
    std::fs::create_dir_all(&skill_dir).expect("create host skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), "# RightX demo").expect("write host skill");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_finish",
            json!({
                "action": "create",
                "skill_name": "rightx-demo",
                "status": "created",
                "message": "I learned rightx-demo and will use it for this workflow.",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-1".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_package_check_failed");
}

#[tokio::test]
async fn skill_learning_start_rejects_malformed_installed_json() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    let skills_dir = agent_dir.join(".claude/skills");
    std::fs::create_dir_all(skills_dir.join("rightx-custom-skill")).expect("create skill dir");
    std::fs::write(
        skills_dir.join("rightx-custom-skill/SKILL.md"),
        "# Custom skill",
    )
    .expect("write skill");
    std::fs::write(skills_dir.join("installed.json"), "{not json").expect("write registry");

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "rightx-custom-skill",
                "reason": "try to update while registry is malformed",
                "message": "I am checking the registry before updating rightx-custom-skill.",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "skill_registry_invalid");
}

#[tokio::test]
async fn skill_learning_start_rejects_bundled_or_codegen_owned_update() {
    for (skill_name, source) in [
        ("bundled-skill", "bundled"),
        ("codegen-owned-skill", "codegen-owned"),
    ] {
        let (backend, agents_dir, _tmp) = make_backend();
        let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
        let skills_dir = agent_dir.join(".claude/skills");
        std::fs::create_dir_all(skills_dir.join(skill_name)).expect("create skill dir");
        std::fs::write(
            skills_dir.join(skill_name).join("SKILL.md"),
            "# Owned skill",
        )
        .expect("write skill");
        std::fs::write(
            skills_dir.join("installed.json"),
            serde_json::json!({ skill_name: { "source": source } }).to_string(),
        )
        .expect("write registry");

        let result = backend
            .tools_call(
                "test-agent",
                &agent_dir,
                "skill_learning_start",
                json!({
                    "action": "update",
                    "skill_name": skill_name,
                    "reason": "try to update an owned skill",
                    "message": "I am checking whether this skill is mutable.",
                }),
                crate::progress::ToolCallContext::default(),
            )
            .await
            .expect("tool errors should be returned as CallToolResult");

        assert_eq!(result.is_error, Some(true), "{skill_name}");
        let body = extract_error_body(&result);
        assert_eq!(body["error"]["code"], "skill_core_readonly", "{skill_name}");
    }
}

#[tokio::test]
async fn skill_learning_start_rejects_empty_message_before_insert() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  mode: none\n")
        .expect("write agent config");
    let skill_dir = agent_dir.join(".claude/skills/rightx-custom-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Custom skill").expect("write skill");

    backend
        .progress_registry()
        .register(crate::progress::ProgressRegistration {
            invocation_id: "inv-empty".to_owned(),
            kind: crate::progress::ProgressInvocationKind::Foreground,
            bot_socket_path: agent_dir.join("missing-bot.sock"),
            bot_send_token: "send-token".to_owned(),
            conversation_scope: None,
        })
        .await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "skill_learning_start",
            json!({
                "action": "update",
                "skill_name": "rightx-custom-skill",
                "reason": "try empty start message",
                "message": "   ",
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-empty".to_owned()),
            },
        )
        .await
        .expect("tool errors should be returned as CallToolResult");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id='inv-empty'",
            [],
            |row| row.get(0),
        )
        .await
        .expect("count learning events");
    assert_eq!(count, 0, "empty start message must not insert event");
}

// ---------------------------------------------------------------------------
// Integration tests — sandbox-aware bootstrap_done
// ---------------------------------------------------------------------------

/// Helper: spin up an ephemeral sandbox for testing.
/// Caller must delete sandbox after use.
async fn create_test_sandbox(
    mtls_dir: &std::path::Path,
    sandbox_name: &str,
) -> right_openshell::sandbox_exec::SandboxExec {
    right_openshell::test_cleanup::pkill_test_orphans(sandbox_name);
    right_openshell::test_cleanup::register_test_sandbox(sandbox_name);

    let mut grpc_client = right_openshell::openshell::connect_grpc(mtls_dir)
        .await
        .expect("gRPC connect");

    // Clean up leftover from a previous failed run.
    if right_openshell::openshell::sandbox_exists(&mut grpc_client, sandbox_name)
        .await
        .unwrap()
    {
        right_openshell::openshell::delete_sandbox(sandbox_name).await;
        right_openshell::openshell::wait_for_deleted(&mut grpc_client, sandbox_name, 60, 2)
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
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
",
    )
    .unwrap();

    let mut child =
        right_openshell::openshell::spawn_sandbox(sandbox_name, &policy_path, None, &[])
            .expect("failed to spawn sandbox");
    right_openshell::openshell::wait_for_ready(
        &mut grpc_client,
        sandbox_name,
        right_openshell::test_support::sandbox_ready_timeout_secs(120),
        2,
    )
    .await
    .expect("sandbox did not become READY");
    let _ = child.kill().await;

    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut grpc_client, sandbox_name)
        .await
        .expect("resolve sandbox_id");

    let sbox = right_openshell::sandbox_exec::SandboxExec::new(
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

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_bootstrap_done_sandbox_files_present() {
    let _slot = right_openshell::openshell::acquire_sandbox_slot();
    // Sandbox name must match the production derivation `sandbox_name(agent)`
    // (fitted to the 19-char upstream cap).
    let agent_name = "test-bootstrap-present";
    let sandbox_name = right_openshell::openshell::sandbox_name(agent_name);

    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let sbox = create_test_sandbox(&mtls_dir, &sandbox_name).await;

    // Create identity files inside sandbox.
    for name in ["IDENTITY.md", "SOUL.md", "USER.md"] {
        let (_, code) = sbox
            .exec(&["sh", "-c", &format!("echo '# test' > /sandbox/{name}")])
            .await
            .unwrap();
        assert_eq!(code, 0, "failed to create {name} in sandbox");
    }

    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join(agent_name);
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("BOOTSTRAP.md"), "bootstrap").unwrap();
    let _conn = right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir, Some(mtls_dir.clone()));
    let result = backend
        .tools_call(
            agent_name,
            &agent_dir,
            "bootstrap_done",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
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

    right_openshell::openshell::delete_sandbox(&sandbox_name).await;
    right_openshell::test_cleanup::unregister_test_sandbox(&sandbox_name);
}

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_bootstrap_done_sandbox_files_missing() {
    let _slot = right_openshell::openshell::acquire_sandbox_slot();
    let agent_name = "test-bootstrap-missing";
    let sandbox_name = right_openshell::openshell::sandbox_name(agent_name);

    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let sbox = create_test_sandbox(&mtls_dir, &sandbox_name).await;

    // Create only IDENTITY.md — SOUL.md and USER.md are missing.
    let (_, code) = sbox
        .exec(&["sh", "-c", "echo '# test' > /sandbox/IDENTITY.md"])
        .await
        .unwrap();
    assert_eq!(code, 0);

    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join(agent_name);
    std::fs::create_dir_all(&agent_dir).unwrap();
    let _conn = right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir, Some(mtls_dir.clone()));
    let result = backend
        .tools_call(
            agent_name,
            &agent_dir,
            "bootstrap_done",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
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

    right_openshell::openshell::delete_sandbox(&sandbox_name).await;
    right_openshell::test_cleanup::unregister_test_sandbox(&sandbox_name);
}

// ---------------------------------------------------------------------------
// Allowlist validation tests for cron_create
// ---------------------------------------------------------------------------

use right_agent::agent::allowlist::{AllowedUser, AllowlistFile, GroupKind, ResponseMode};

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
        file.groups
            .push(right_agent::agent::allowlist::AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
                kind: GroupKind::Group,
            });
    }
    right_agent::agent::allowlist::write_file(agent_dir, &file).unwrap();
}

fn write_allowlist_with_group_kinds(agent_dir: &std::path::Path, groups: &[(i64, GroupKind)]) {
    let now = chrono::Utc::now();
    let mut file = AllowlistFile::default();
    for &(id, kind) in groups {
        file.groups
            .push(right_agent::agent::allowlist::AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
                kind,
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
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -999_i64,
    });
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            args,
            crate::progress::ToolCallContext::default(),
        )
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
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -200_i64,
        "target_thread_id": 7_i64,
    });
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            args,
            crate::progress::ToolCallContext::default(),
        )
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
async fn cron_create_persists_model_and_update_clears_it() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[7], &[]);
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);

    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            serde_json::json!({
                "job_name": "j1",
                "schedule": "17 9 * * *",
                "prompt": "p",
                "target_chat_id": 7_i64,
                "model": "haiku",
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_create ok");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let m: Option<String> = conn
        .query_row(
            "SELECT model FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(m.as_deref(), Some("haiku"));

    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_update",
            serde_json::json!({
                "job_name": "j1",
                "model": null,
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_update ok");

    let m2: Option<String> = conn
        .query_row(
            "SELECT model FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(
        m2, None,
        "explicit null clears model back to inherit-global"
    );
}

/// Seed an active `skill_lifecycle` row so cron→skill links validate.
async fn seed_active_skill(conn: &right_db::Connection, skill_name: &str) {
    conn.execute(
        "INSERT INTO skill_lifecycle (skill_name, state) VALUES (?1, 'active')",
        right_db::params![skill_name],
    )
    .await
    .expect("seed skill_lifecycle row");
}

#[tokio::test]
async fn cron_create_links_skill_names() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[7], &[]);
    let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
    seed_active_skill(&conn, "rightx-a").await;

    let backend = RightBackend::new(agents_dir.clone(), None);
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            json!({
                "job_name": "j1",
                "schedule": "17 9 * * *",
                "prompt": "p",
                "target_chat_id": 7_i64,
                "skill_names": ["rightx-a"],
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_create ok");
    assert_ne!(result.is_error, Some(true), "cron_create should succeed");

    let linked = right_agent::cron_skill_link::list_for_job(&conn, "j1")
        .await
        .expect("list links");
    assert_eq!(linked, vec!["rightx-a".to_string()]);
}

#[tokio::test]
async fn cron_link_skill_links_and_validates() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[7], &[]);
    let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
    seed_active_skill(&conn, "rightx-a").await;

    let backend = RightBackend::new(agents_dir.clone(), None);
    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            json!({
                "job_name": "j1",
                "schedule": "17 9 * * *",
                "prompt": "p",
                "target_chat_id": 7_i64,
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_create ok");

    // Link an existing skill — appears in the link list.
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_link_skill",
            json!({ "job_name": "j1", "skill_names": ["rightx-a"] }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_link_skill ok");
    assert_ne!(result.is_error, Some(true), "link of existing skill ok");
    let linked = right_agent::cron_skill_link::list_for_job(&conn, "j1")
        .await
        .expect("list links");
    assert_eq!(linked, vec!["rightx-a".to_string()]);

    // Linking a missing skill is a validated tool_error.
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_link_skill",
            json!({ "job_name": "j1", "skill_names": ["rightx-missing"] }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("tool errors should be returned as CallToolResult");
    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "cron_link_failed");

    // Unlink removes the live link.
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_unlink_skill",
            json!({ "job_name": "j1", "skill_names": ["rightx-a"] }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_unlink_skill ok");
    assert_ne!(result.is_error, Some(true), "unlink ok");
    let linked = right_agent::cron_skill_link::list_for_job(&conn, "j1")
        .await
        .expect("list links");
    assert!(linked.is_empty(), "link removed after unlink: {linked:?}");
}

#[tokio::test]
async fn cron_trigger_with_then_persists_then_json_and_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[7], &[]);
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);

    // Standing job to trigger.
    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            serde_json::json!({
                "job_name": "j",
                "schedule": "17 9 * * *",
                "prompt": "p",
                "target_chat_id": 7_i64,
            }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("cron_create ok");

    // Register the foreground invocation that "issues" the trigger.
    backend
        .progress_registry()
        .register(crate::progress::ProgressRegistration {
            invocation_id: "inv-1".to_owned(),
            kind: crate::progress::ProgressInvocationKind::Foreground,
            bot_socket_path: "/tmp/x.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(crate::progress::ConversationScope {
                chat_id: 77,
                thread_id: 8,
            }),
        })
        .await;

    // Sibling foreground invocation from a thread-0 ("no topic") scope. Used
    // below to prove thread 0 normalizes to None (not Some(0)).
    backend
        .progress_registry()
        .register(crate::progress::ProgressRegistration {
            invocation_id: "inv-2".to_owned(),
            kind: crate::progress::ProgressInvocationKind::Foreground,
            bot_socket_path: "/tmp/x.sock".into(),
            bot_send_token: "tok".to_owned(),
            conversation_scope: Some(crate::progress::ConversationScope {
                chat_id: 77,
                thread_id: 0,
            }),
        })
        .await;

    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_trigger",
            serde_json::json!({
                "job_name": "j",
                "then": { "instruction": "go", "run_on": "success" }
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-1".to_owned()),
            },
        )
        .await
        .expect("cron_trigger ok");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("open db");
    let specs = right_agent::cron_spec::load_specs_from_db(&conn)
        .await
        .expect("load specs");
    let s = specs.get("j").expect("spec j present");
    let then = s.then.as_ref().expect("then persisted");
    assert_eq!(then.instruction, "go");
    assert_eq!(then.run_on, right_agent::cron_spec::RunOn::Success);
    assert_eq!(s.trigger_origin_chat_id, Some(77));
    // Non-zero origin thread is captured verbatim.
    assert_eq!(s.trigger_origin_thread_id, Some(8));

    // Re-trigger from the thread-0 scope: thread 0 ("no topic") normalizes to
    // None, never Some(0).
    backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_trigger",
            serde_json::json!({
                "job_name": "j",
                "then": { "instruction": "go", "run_on": "success" }
            }),
            crate::progress::ToolCallContext {
                invocation_id: Some("inv-2".to_owned()),
            },
        )
        .await
        .expect("cron_trigger (thread-0) ok");

    let conn = right_db::open_connection(&agent_dir, false)
        .await
        .expect("reopen db");
    let specs = right_agent::cron_spec::load_specs_from_db(&conn)
        .await
        .expect("reload specs");
    let s = specs.get("j").expect("spec j present");
    assert_eq!(s.trigger_origin_chat_id, Some(77));
    assert_eq!(s.trigger_origin_thread_id, None);
}

#[tokio::test]
async fn cron_create_rejects_missing_target_chat_id() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("a1");
    std::fs::create_dir_all(&agent_dir).unwrap();
    write_allowlist(&agent_dir, &[100], &[]);
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        // target_chat_id deliberately omitted
    });
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            args,
            crate::progress::ToolCallContext::default(),
        )
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
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir.clone(), None);
    let args = serde_json::json!({
        "job_name": "j1",
        "schedule": "*/5 * * * *",
        "prompt": "p",
        "target_chat_id": -200_i64,
    });
    let result = backend
        .tools_call(
            "a1",
            &agent_dir,
            "cron_create",
            args,
            crate::progress::ToolCallContext::default(),
        )
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
// Forum topic validation tests (validation runs before invocation lookup)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn forum_topic_create_rejects_bad_icon_color() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "forum_topic_create",
            serde_json::json!({ "name": "Bugs", "icon_color": 123 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.is_error,
        Some(true),
        "bad icon_color must be a tool error"
    );
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn forum_topic_create_rejects_negative_icon_color() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    // A negative icon_color must be rejected at validation, not silently
    // dropped (it can no longer slip past as an unset color).
    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "forum_topic_create",
            serde_json::json!({ "name": "Bugs", "icon_color": -1 }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.is_error,
        Some(true),
        "negative icon_color must be a tool error"
    );
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn forum_topic_create_rejects_empty_name() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "forum_topic_create",
            serde_json::json!({ "name": "   " }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
}

#[tokio::test]
async fn forum_topic_edit_rejects_blank_name() {
    let (backend, agents_dir, _tmp) = make_backend();
    let agent_dir = create_agent_dir(&agents_dir, "test-agent").await;

    let result = backend
        .tools_call(
            "test-agent",
            &agent_dir,
            "forum_topic_edit",
            serde_json::json!({ "message_thread_id": 5, "name": "   " }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.is_error,
        Some(true),
        "blank edit name must be a tool error"
    );
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "invalid_argument");
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
    right_db::open_connection(&agent_dir, true).await.unwrap();

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
            crate::progress::ToolCallContext::default(),
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
            crate::progress::ToolCallContext::default(),
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
            crate::progress::ToolCallContext::default(),
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
    right_db::open_connection(&agent_dir, true).await.unwrap();

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
            crate::progress::ToolCallContext::default(),
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
            crate::progress::ToolCallContext::default(),
        )
        .await
        .unwrap();

    let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
    let thread: Option<i64> = conn
        .query_row(
            "SELECT target_thread_id FROM cron_specs WHERE job_name='j1'",
            [],
            |r| r.get(0),
        )
        .await
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
        .tools_call(
            "test-agent",
            &agent_dir,
            "bootstrap_done",
            serde_json::json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "bootstrap_files_missing");
    let missing = body["error"]["details"]["missing"]
        .as_array()
        .expect("details.missing must be an array");
    let names: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"IDENTITY.md"),
        "missing IDENTITY.md: {names:?}"
    );
    assert!(names.contains(&"SOUL.md"));
    assert!(names.contains(&"USER.md"));
}

#[tokio::test]
async fn send_message_rejects_empty() {
    let (backend, _agents_dir, _tmp) = make_backend();
    let result = backend
        .tools_call(
            "test-agent",
            Path::new("/tmp/unused"),
            "send_message",
            json!({}),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "send_message_empty");
}

#[tokio::test]
async fn send_message_rejects_bad_path() {
    let (backend, _agents_dir, _tmp) = make_backend();
    let result = backend
        .tools_call(
            "test-agent",
            Path::new("/tmp/unused"),
            "send_message",
            json!({ "attachments": [{ "type": "photo", "path": "/etc/passwd" }] }),
            crate::progress::ToolCallContext::default(),
        )
        .await
        .expect("dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "send_message_bad_path");
}

#[tokio::test]
async fn get_conn_opens_per_operation_without_caching() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().to_path_buf();
    let agent_dir = agents_dir.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    right_db::open_connection(&agent_dir, true).await.unwrap();

    let backend = RightBackend::new(agents_dir, None);
    let a = backend.get_conn("agent").await.unwrap();
    let b = backend.get_conn("agent").await.unwrap();

    assert!(
        !std::sync::Arc::ptr_eq(&a, &b),
        "get_conn must open per operation, not return a cached handle",
    );
}
