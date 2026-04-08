use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef};
use crate::codegen::{generate_agent_definition, REPLY_SCHEMA_JSON};

/// Builder for test `AgentDef` instances. All optional files default to absent.
struct TestAgent {
    path: PathBuf,
    model: Option<String>,
    soul: bool,
    user: bool,
    agents: bool,
    bootstrap: bool,
}

impl TestAgent {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            model: None,
            soul: false,
            user: false,
            agents: false,
            bootstrap: false,
        }
    }

    fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }

    fn soul(mut self) -> Self {
        self.soul = true;
        self
    }

    fn user(mut self) -> Self {
        self.user = true;
        self
    }

    fn agents(mut self) -> Self {
        self.agents = true;
        self
    }

    fn bootstrap(mut self) -> Self {
        self.bootstrap = true;
        self
    }

    fn build(self) -> AgentDef {
        let config = self.model.map(|m| AgentConfig {
            model: Some(m),
            restart: Default::default(),
            max_restarts: 3,
            backoff_seconds: 5,
            network_policy: Default::default(),
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: Default::default(),
            secret: None,
            attachments: Default::default(),
        });
        let p = &self.path;
        AgentDef {
            name: "testbot".to_owned(),
            path: p.clone(),
            identity_path: p.join("IDENTITY.md"),
            config,
            soul_path: if self.soul { Some(p.join("SOUL.md")) } else { None },
            user_path: if self.user { Some(p.join("USER.md")) } else { None },
            agents_path: if self.agents { Some(p.join("AGENTS.md")) } else { None },
            tools_path: None,
            bootstrap_path: if self.bootstrap { Some(p.join("BOOTSTRAP.md")) } else { None },
            heartbeat_path: None,
        }
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

    let agent = TestAgent::new(tmp.path().to_path_buf())
        .model("claude-sonnet-4-5")
        .soul()
        .user()
        .agents()
        .build();
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

    let agent = TestAgent::new(tmp.path().to_path_buf()).build();
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

    let agent = TestAgent::new(tmp.path().to_path_buf()).build();
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
    let agent = TestAgent::new(tmp.path().to_path_buf()).build();
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

    let agent = TestAgent::new(tmp.path().to_path_buf()).soul().build();
    let result = generate_agent_definition(&agent).unwrap();

    assert!(result.contains("soul-text"), "must contain soul section");
    assert!(result.contains("## Message Input Format"), "must contain attachment format docs");
    let soul_pos = result.find("soul-text").unwrap();
    let format_pos = result.find("## Message Input Format").unwrap();
    assert!(format_pos > soul_pos, "attachment format docs must come after body sections");
}

/// Test 6: REPLY_SCHEMA_JSON has typed attachments (not media_paths)
#[test]
fn reply_schema_json_is_valid_and_has_attachments() {
    let value: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");

    assert_eq!(value["type"].as_str(), Some("object"));

    let props = &value["properties"];
    assert!(!props["content"].is_null(), "must have 'content' property");
    assert!(
        !props["reply_to_message_id"].is_null(),
        "must have 'reply_to_message_id' property"
    );

    // media_paths must NOT exist (replaced by attachments)
    assert!(
        props["media_paths"].is_null(),
        "media_paths must be removed from schema"
    );

    // attachments must exist with correct structure
    let atts = &props["attachments"];
    assert!(!atts.is_null(), "must have 'attachments' property");
    let items = &atts["items"];
    assert!(!items.is_null(), "attachments must have 'items'");
    let item_props = &items["properties"];
    assert!(!item_props["type"].is_null(), "attachment items must have 'type'");
    assert!(!item_props["path"].is_null(), "attachment items must have 'path'");
    assert!(!item_props["filename"].is_null(), "attachment items must have 'filename'");
    assert!(!item_props["caption"].is_null(), "attachment items must have 'caption'");

    // type must be an enum with expected variants
    let type_enum = items["properties"]["type"]["enum"]
        .as_array()
        .expect("type must be an enum");
    assert!(type_enum.iter().any(|v| v.as_str() == Some("photo")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("document")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("video")));
    assert!(type_enum.iter().any(|v| v.as_str() == Some("voice")));

    // required must include "content"
    let required = value["required"].as_array().expect("required must be an array");
    assert!(required.iter().any(|v| v.as_str() == Some("content")));
}

/// Test: generated agent definition includes message input/output format documentation
#[test]
fn agent_definition_includes_attachment_format_docs() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");

    let agent = TestAgent::new(tmp.path().to_path_buf()).build();
    let result = generate_agent_definition(&agent).unwrap();

    assert!(
        result.contains("## Message Input Format"),
        "must contain input format section"
    );
    assert!(
        result.contains("## Sending Attachments"),
        "must contain output format section"
    );
    assert!(
        result.contains("/sandbox/outbox/"),
        "must mention outbox directory"
    );
    assert!(
        result.contains("Photos: max 10MB"),
        "must mention photo size limit"
    );
}

/// Test that frontmatter does NOT contain a tools: field (per D-05)
#[test]
fn no_tools_field_in_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");

    let agent = TestAgent::new(tmp.path().to_path_buf()).build();
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

#[test]
fn bootstrap_present_appears_between_identity_and_soul() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "BOOTSTRAP.md", "bootstrap-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = TestAgent::new(tmp.path().to_path_buf())
        .soul()
        .bootstrap()
        .build();
    let result = generate_agent_definition(&agent).unwrap();

    assert!(result.contains("bootstrap-text"), "must contain bootstrap section");
    let identity_pos = result.find("identity-text").unwrap();
    let bootstrap_pos = result.find("bootstrap-text").unwrap();
    let soul_pos = result.find("soul-text").unwrap();
    assert!(
        bootstrap_pos > identity_pos,
        "bootstrap must come after identity"
    );
    assert!(
        bootstrap_pos < soul_pos,
        "bootstrap must come before soul"
    );
}

#[test]
fn bootstrap_none_excluded_from_output() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = TestAgent::new(tmp.path().to_path_buf())
        .soul()
        .build();
    let result = generate_agent_definition(&agent).unwrap();

    assert!(!result.contains("bootstrap"), "must not contain bootstrap when path is None");
    assert!(result.contains("identity-text"), "must still contain identity");
    assert!(result.contains("soul-text"), "must still contain soul");
}
