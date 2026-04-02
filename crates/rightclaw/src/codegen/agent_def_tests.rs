use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef};
use crate::codegen::{generate_agent_definition, REPLY_SCHEMA_JSON};

/// Build a minimal AgentDef pointing at `path` as agent dir.
/// Optional files are set based on flags — paths are set but files only exist if you create them.
fn make_agent_at(
    path: PathBuf,
    model: Option<String>,
    with_soul: bool,
    with_user: bool,
    with_agents: bool,
) -> AgentDef {
    let config = model.map(|m| AgentConfig {
        model: Some(m),
        restart: Default::default(),
        max_restarts: 3,
        backoff_seconds: 5,
        sandbox: None,
        telegram_token_file: None,
        telegram_token: None,
        allowed_chat_ids: vec![],
        env: Default::default(),
    });
    AgentDef {
        name: "testbot".to_owned(),
        path: path.clone(),
        identity_path: path.join("IDENTITY.md"),
        config,
        soul_path: if with_soul { Some(path.join("SOUL.md")) } else { None },
        user_path: if with_user { Some(path.join("USER.md")) } else { None },
        agents_path: if with_agents { Some(path.join("AGENTS.md")) } else { None },
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

fn write_file(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// Test 1: generate_agent_definition with all files present returns correct YAML frontmatter
/// followed by body sections joined with `\n\n---\n\n`
#[test]
fn all_files_produces_frontmatter_and_body_sections() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");
    write_file(tmp.path(), "USER.md", "user-text");
    write_file(tmp.path(), "AGENTS.md", "agents-text");

    let agent = make_agent_at(
        tmp.path().to_path_buf(),
        Some("claude-sonnet-4-5".to_owned()),
        true,
        true,
        true,
    );
    let result = generate_agent_definition(&agent).unwrap();

    // Frontmatter at position 0
    assert!(result.starts_with("---\n"), "must start with ---");
    assert!(
        result.contains("name: testbot\n"),
        "must contain name field"
    );
    assert!(
        result.contains("model: claude-sonnet-4-5\n"),
        "must contain model field"
    );
    assert!(
        result.contains("description: \"RightClaw agent: testbot\"\n"),
        "must contain description field"
    );
    // Body sections joined with separator
    assert!(result.contains("identity-text"), "must contain identity");
    assert!(result.contains("soul-text"), "must contain soul");
    assert!(result.contains("user-text"), "must contain user");
    assert!(result.contains("agents-text"), "must contain agents");
}

/// Test 2: generate_agent_definition with config.model = None produces `model: inherit`
#[test]
fn model_none_produces_inherit() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");

    let agent = make_agent_at(tmp.path().to_path_buf(), None, false, false, false);
    let result = generate_agent_definition(&agent).unwrap();

    assert!(
        result.contains("model: inherit\n"),
        "expected 'model: inherit' when model is None, got:\n{result}"
    );
}

/// Test 3: generate_agent_definition with only IDENTITY.md produces frontmatter + identity only
#[test]
fn identity_only_produces_frontmatter_plus_identity() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "# My Identity\n");

    let agent = make_agent_at(tmp.path().to_path_buf(), None, false, false, false);
    let result = generate_agent_definition(&agent).unwrap();

    assert!(result.starts_with("---\n"), "must start with frontmatter");
    assert!(result.contains("# My Identity"), "must contain identity content");
    // No extra separator since only one body section
    let body_part = result.split("---\n\n").last().unwrap_or("");
    assert!(
        !body_part.contains("\n\n---\n\n"),
        "no body separator expected with only one section"
    );
}

/// Test 4: generate_agent_definition with missing IDENTITY.md returns Err
#[test]
fn missing_identity_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    // Intentionally do NOT create IDENTITY.md.
    let agent = make_agent_at(tmp.path().to_path_buf(), None, false, false, false);
    let result = generate_agent_definition(&agent);

    assert!(result.is_err(), "expected Err when IDENTITY.md is missing");
    let err = format!("{:?}", result.unwrap_err());
    assert!(
        err.contains("IDENTITY.md"),
        "error should mention IDENTITY.md, got: {err}"
    );
}

/// Test 5: generate_agent_definition with SOUL.md present but USER.md absent skips USER.md
#[test]
fn soul_present_user_absent_skips_user() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");
    // user_path is None (flag = false)

    let agent = make_agent_at(tmp.path().to_path_buf(), None, true, false, false);
    let result = generate_agent_definition(&agent).unwrap();

    assert!(result.contains("soul-text"), "must contain soul section");
    // Body should be identity + soul only — two sections
    let after_frontmatter_end = result.find("---\n\n").unwrap();
    let body = &result[after_frontmatter_end + 5..];
    assert_eq!(
        body,
        "identity-text\n\n---\n\nsoul-text",
        "body should be identity + soul only"
    );
}

/// Test 6: REPLY_SCHEMA_JSON const is valid JSON with required fields
#[test]
fn reply_schema_json_is_valid_and_has_required_fields() {
    // Must parse as valid JSON
    let value: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");

    // Must be an object type
    assert_eq!(
        value["type"].as_str(),
        Some("object"),
        "top-level type must be 'object'"
    );

    // Must have properties for content, reply_to_message_id, media_paths
    let props = &value["properties"];
    assert!(
        !props["content"].is_null(),
        "must have 'content' property"
    );
    assert!(
        !props["reply_to_message_id"].is_null(),
        "must have 'reply_to_message_id' property"
    );
    assert!(
        !props["media_paths"].is_null(),
        "must have 'media_paths' property"
    );

    // Must have required array containing "content"
    let required = value["required"]
        .as_array()
        .expect("required must be an array");
    assert!(
        required.iter().any(|v| v.as_str() == Some("content")),
        "required must include 'content'"
    );
}

/// Test that frontmatter does NOT contain a tools: field (per D-05)
#[test]
fn no_tools_field_in_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");

    let agent = make_agent_at(tmp.path().to_path_buf(), None, false, false, false);
    let result = generate_agent_definition(&agent).unwrap();

    // Extract frontmatter between first --- and second ---
    let after_first = &result[4..]; // skip opening "---\n"
    let end = after_first.find("\n---\n").unwrap();
    let frontmatter = &after_first[..end];
    assert!(
        !frontmatter.contains("tools:"),
        "frontmatter must not contain 'tools:' field, got:\n{frontmatter}"
    );
}
