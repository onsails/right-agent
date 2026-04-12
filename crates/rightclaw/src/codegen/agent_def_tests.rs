use crate::codegen::{generate_agent_definition, generate_bootstrap_definition, generate_system_prompt, BOOTSTRAP_SCHEMA_JSON, REPLY_SCHEMA_JSON};

#[test]
fn agent_definition_has_at_references_in_cache_order() {
    let result = generate_agent_definition("myagent", Some("sonnet"));
    assert!(result.contains("name: myagent"));
    assert!(result.contains("model: sonnet"));
    assert!(result.contains("description: \"RightClaw agent: myagent\""));

    // Verify order: AGENTS → SOUL → IDENTITY → USER → TOOLS
    let agents_pos = result.find("@./AGENTS.md").expect("missing @./AGENTS.md");
    let soul_pos = result.find("@./SOUL.md").expect("missing @./SOUL.md");
    let identity_pos = result.find("@./IDENTITY.md").expect("missing @./IDENTITY.md");
    let user_pos = result.find("@./USER.md").expect("missing @./USER.md");
    let tools_pos = result.find("@./TOOLS.md").expect("missing @./TOOLS.md");

    assert!(agents_pos < soul_pos, "AGENTS must come before SOUL");
    assert!(soul_pos < identity_pos, "SOUL must come before IDENTITY");
    assert!(identity_pos < user_pos, "IDENTITY must come before USER");
    assert!(user_pos < tools_pos, "USER must come before TOOLS");
}

#[test]
fn agent_definition_model_none_produces_inherit() {
    let result = generate_agent_definition("test", None);
    assert!(result.contains("model: inherit"));
}

#[test]
fn agent_definition_no_embedded_file_content() {
    let result = generate_agent_definition("test", Some("opus"));
    // Must NOT contain any raw file content — only @ references
    assert!(!result.contains("Agent Instructions"), "should not embed AGENTS.md content");
    assert!(!result.contains("Core Values"), "should not embed SOUL.md content");
}

#[test]
fn bootstrap_definition_has_only_bootstrap_reference() {
    let result = generate_bootstrap_definition("myagent", Some("sonnet"));
    assert!(result.contains("name: myagent-bootstrap"));
    assert!(result.contains("@./BOOTSTRAP.md"));
    assert!(!result.contains("@./AGENTS.md"), "bootstrap must not include AGENTS");
    assert!(!result.contains("@./SOUL.md"), "bootstrap must not include SOUL");
    assert!(!result.contains("@./IDENTITY.md"), "bootstrap must not include IDENTITY");
}

#[test]
fn bootstrap_definition_model_none_produces_inherit() {
    let result = generate_bootstrap_definition("test", None);
    assert!(result.contains("model: inherit"));
}

#[test]
fn reply_schema_json_is_valid() {
    let parsed: serde_json::Value = serde_json::from_str(REPLY_SCHEMA_JSON)
        .expect("REPLY_SCHEMA_JSON must be valid JSON");
    assert!(parsed.get("required").is_some());
}

#[test]
fn bootstrap_schema_json_is_valid() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON)
        .expect("BOOTSTRAP_SCHEMA_JSON must be valid JSON");
    let required = parsed.get("required").unwrap().as_array().unwrap();
    let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(required_strs.contains(&"content"), "must require content");
    assert!(required_strs.contains(&"bootstrap_complete"), "must require bootstrap_complete");
}

#[test]
fn bootstrap_schema_has_bootstrap_complete_field() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON).unwrap();
    let props = parsed.get("properties").unwrap();
    assert!(props.get("bootstrap_complete").is_some(), "must have bootstrap_complete property");
}

#[test]
fn system_prompt_contains_agent_name() {
    let result = generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell);
    assert!(result.contains("mybot"));
}

#[test]
fn system_prompt_contains_rightclaw_description() {
    let result = generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell);
    assert!(result.contains("RightClaw"));
    assert!(result.contains("multi-agent runtime"));
}

#[test]
fn system_prompt_contains_sandbox_mode() {
    let openshell = generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell);
    assert!(openshell.contains("OpenShell"));

    let none = generate_system_prompt("test", &crate::agent::types::SandboxMode::None);
    assert!(none.contains("no sandbox"));
}

#[test]
fn system_prompt_mentions_right_mcp() {
    let result = generate_system_prompt("test", &crate::agent::types::SandboxMode::Openshell);
    assert!(result.contains("right"));
    assert!(result.contains("MCP"));
}

#[test]
fn system_prompt_contains_ssh_block_for_openshell() {
    let result = generate_system_prompt("mybot", &crate::agent::types::SandboxMode::Openshell);
    assert!(result.contains("rightclaw agent ssh mybot"), "openshell prompt must include SSH command");
    assert!(result.contains("interactive terminal"), "openshell prompt must explain when to use SSH");
}

#[test]
fn system_prompt_no_ssh_block_for_no_sandbox() {
    let result = generate_system_prompt("mybot", &crate::agent::types::SandboxMode::None);
    assert!(!result.contains("rightclaw agent ssh"), "no-sandbox prompt must NOT include SSH command");
}
