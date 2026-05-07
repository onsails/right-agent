use crate::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON, REPLY_SCHEMA_JSON,
    generate_system_prompt,
};

#[test]
fn reply_schema_json_is_valid() {
    let parsed: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");
    assert!(parsed.get("required").is_some());
}

#[test]
fn bootstrap_schema_json_is_valid() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON)
        .expect("BOOTSTRAP_SCHEMA_JSON must be valid JSON");
    let required = parsed.get("required").unwrap().as_array().unwrap();
    let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(required_strs.contains(&"content"), "must require content");
    assert!(
        required_strs.contains(&"bootstrap_complete"),
        "must require bootstrap_complete"
    );
}

#[test]
fn bootstrap_schema_has_bootstrap_complete_field() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON).unwrap();
    let props = parsed.get("properties").unwrap();
    assert!(
        props.get("bootstrap_complete").is_some(),
        "must have bootstrap_complete property"
    );
}

#[test]
fn system_prompt_contains_agent_name() {
    let result = generate_system_prompt(
        "mybot",
        &right_core::agent_types::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("mybot"));
}

#[test]
fn system_prompt_contains_right_description() {
    let result = generate_system_prompt(
        "test",
        &right_core::agent_types::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("Right Agent"));
    assert!(result.contains("multi-agent runtime"));
}

#[test]
fn system_prompt_contains_sandbox_mode() {
    let openshell = generate_system_prompt(
        "test",
        &right_core::agent_types::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(openshell.contains("OpenShell"));

    let none = generate_system_prompt(
        "test",
        &right_core::agent_types::SandboxMode::None,
        "/test/agent/home",
    );
    assert!(none.contains("no sandbox"));
}

#[test]
fn system_prompt_mentions_right_mcp() {
    let result = generate_system_prompt(
        "test",
        &right_core::agent_types::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("right"));
    assert!(result.contains("MCP"));
}

#[test]
fn system_prompt_contains_ssh_block_for_openshell() {
    let result = generate_system_prompt(
        "mybot",
        &right_core::agent_types::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(
        result.contains("right agent ssh mybot"),
        "openshell prompt must include SSH command"
    );
    assert!(
        result.contains("interactive terminal"),
        "openshell prompt must explain when to use SSH"
    );
}

#[test]
fn system_prompt_no_ssh_block_for_no_sandbox() {
    let result = generate_system_prompt(
        "mybot",
        &right_core::agent_types::SandboxMode::None,
        "/test/agent/home",
    );
    assert!(
        !result.contains("right agent ssh"),
        "no-sandbox prompt must NOT include SSH command"
    );
}

#[test]
fn operating_instructions_constant_is_non_empty() {
    assert!(
        !crate::OPERATING_INSTRUCTIONS.is_empty(),
        "OPERATING_INSTRUCTIONS must not be empty"
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains("## Your Files"),
        "OPERATING_INSTRUCTIONS must contain Your Files section"
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains("## MCP Management"),
        "OPERATING_INSTRUCTIONS must contain MCP Management section"
    );
}

#[test]
fn bootstrap_instructions_constant_is_non_empty() {
    assert!(
        !crate::BOOTSTRAP_INSTRUCTIONS.is_empty(),
        "BOOTSTRAP_INSTRUCTIONS must not be empty"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("First-Time Setup"),
        "BOOTSTRAP_INSTRUCTIONS must contain bootstrap header"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("### IDENTITY.md"),
        "BOOTSTRAP_INSTRUCTIONS must contain IDENTITY.md structure"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("### SOUL.md"),
        "BOOTSTRAP_INSTRUCTIONS must contain SOUL.md structure"
    );
}

#[test]
fn system_prompt_contains_home_dir() {
    let result = generate_system_prompt(
        "test",
        &right_core::agent_types::SandboxMode::Openshell,
        "/my/custom/home",
    );
    assert!(
        result.contains("/my/custom/home"),
        "system prompt must contain the passed home_dir"
    );
}

fn attachments_item_schema(schema_json: &str, path: &[&str]) -> serde_json::Value {
    let mut node: serde_json::Value = serde_json::from_str(schema_json).unwrap();
    for key in path {
        node = node
            .get(*key)
            .unwrap_or_else(|| panic!("missing key {key}"))
            .clone();
    }
    node
}

fn assert_has_nullable_media_group_id(items: &serde_json::Value) {
    let props = items.get("properties").expect("items.properties");
    let field = props
        .get("media_group_id")
        .expect("media_group_id property missing");
    let ty = field.get("type").expect("media_group_id.type missing");
    let arr = ty
        .as_array()
        .expect("media_group_id.type must be an array for nullable");
    let kinds: Vec<&str> = arr
        .iter()
        .map(|v| {
            v.as_str()
                .expect("type array element must be a string JSON value")
        })
        .collect();
    assert!(
        kinds.contains(&"string"),
        "must allow string, got {kinds:?}"
    );
    assert!(kinds.contains(&"null"), "must allow null, got {kinds:?}");
}

#[test]
fn reply_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(REPLY_SCHEMA_JSON, &["properties", "attachments", "items"]);
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn bootstrap_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        BOOTSTRAP_SCHEMA_JSON,
        &["properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn cron_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        CRON_SCHEMA_JSON,
        &["properties", "notify", "properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn bg_continuation_schema_requires_notify() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"notify"), "notify must be required");
    assert!(names.contains(&"summary"), "summary must be required");
}

#[test]
fn bg_continuation_schema_notify_is_non_nullable_object() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let notify_type = &v["properties"]["notify"]["type"];
    assert_eq!(
        notify_type.as_str(),
        Some("object"),
        "notify must be non-nullable; got {:?}",
        notify_type
    );
}

#[test]
fn bg_continuation_schema_content_min_length_one() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let min_len = &v["properties"]["notify"]["properties"]["content"]["minLength"];
    assert_eq!(min_len.as_i64(), Some(1));
}

#[test]
fn bg_continuation_schema_no_notify_reason_field_absent() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let props = v["properties"].as_object().unwrap();
    assert!(
        !props.contains_key("no_notify_reason"),
        "no_notify_reason must not be in bg schema"
    );
}

#[test]
fn bg_continuation_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        BG_CONTINUATION_SCHEMA_JSON,
        &["properties", "notify", "properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn operating_instructions_documents_media_groups() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    assert!(ops.contains("Media Groups"), "missing media-group docs");
    assert!(
        ops.contains("media_group_id"),
        "missing media_group_id mention"
    );
    assert!(
        ops.contains("2–10") || ops.contains("2-10"),
        "must mention the 2–10 item limit"
    );
}
