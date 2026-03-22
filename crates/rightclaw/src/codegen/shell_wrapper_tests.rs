use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::codegen::generate_wrapper;

fn make_agent(name: &str, start_prompt: Option<&str>) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        start_prompt: start_prompt.map(String::from),
    });
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        policy_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/policy.yaml"
        )),
        config,
        mcp_config_path: None,
        soul_path: None,
        user_path: None,
        memory_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

fn make_agent_with_mcp(name: &str, start_prompt: Option<&str>) -> AgentDef {
    let mut agent = make_agent(name, start_prompt);
    agent.mcp_config_path = Some(PathBuf::from(format!(
        "/home/user/.rightclaw/agents/{name}/.mcp.json"
    )));
    agent
}

fn make_agent_no_config(name: &str) -> AgentDef {
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        policy_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/policy.yaml"
        )),
        config: None,
        mcp_config_path: None,
        soul_path: None,
        user_path: None,
        memory_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

const DUMMY_PROMPT_PATH: &str = "/tmp/run/testbot-prompt.md";

#[test]
fn wrapper_with_sandbox_contains_openshell() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("openshell sandbox create"),
        "expected openshell invocation in:\n{output}"
    );
    assert!(
        output.contains("--policy"),
        "expected --policy flag in:\n{output}"
    );
    assert!(
        output.contains("--name \"rightclaw-testbot\""),
        "expected sandbox name in:\n{output}"
    );
}

#[test]
fn wrapper_with_sandbox_contains_combined_prompt_and_permissions() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("--append-system-prompt-file"),
        "expected --append-system-prompt-file in:\n{output}"
    );
    assert!(
        output.contains("--dangerously-skip-permissions"),
        "expected --dangerously-skip-permissions in:\n{output}"
    );
    assert!(
        output.contains(DUMMY_PROMPT_PATH),
        "expected combined prompt path in:\n{output}"
    );
}

#[test]
fn wrapper_no_sandbox_runs_claude_directly() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, true, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains(r#"exec "$CLAUDE_BIN""#),
        "expected 'exec \"$CLAUDE_BIN\"' in no-sandbox mode:\n{output}"
    );
    assert!(
        !output.contains("openshell"),
        "should NOT contain openshell in no-sandbox mode:\n{output}"
    );
}

#[test]
fn wrapper_starts_with_shebang() {
    let agent = make_agent("testbot", Some("Hello"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.starts_with("#!/usr/bin/env bash"),
        "expected shebang at start of:\n{output}"
    );
}

#[test]
fn wrapper_no_config_agent_still_renders() {
    let agent = make_agent_no_config("testbot");
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("--append-system-prompt-file"),
        "expected --append-system-prompt-file in:\n{output}"
    );
    assert!(
        output.contains(DUMMY_PROMPT_PATH),
        "expected combined prompt path in:\n{output}"
    );
}

#[test]
fn wrapper_with_mcp_includes_channels_flag_sandbox() {
    let agent = make_agent_with_mcp("testbot", Some("Go"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("--channels plugin:telegram@claude-plugins-official"),
        "expected --channels flag in sandbox mode:\n{output}"
    );
}

#[test]
fn wrapper_with_mcp_includes_channels_flag_no_sandbox() {
    let agent = make_agent_with_mcp("testbot", Some("Go"));
    let output = generate_wrapper(&agent, true, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("--channels plugin:telegram@claude-plugins-official"),
        "expected --channels flag in no-sandbox mode:\n{output}"
    );
}

#[test]
fn wrapper_without_mcp_omits_channels_flag() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        !output.contains("--channels"),
        "should NOT contain --channels without mcp_config_path:\n{output}"
    );
}

#[test]
fn wrapper_without_mcp_omits_channels_flag_no_sandbox() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, true, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        !output.contains("--channels"),
        "should NOT contain --channels in no-sandbox without mcp:\n{output}"
    );
}

#[test]
fn wrapper_has_exactly_one_append_system_prompt_file() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, false, DUMMY_PROMPT_PATH, None).unwrap();

    let count = output.matches("--append-system-prompt-file").count();
    assert_eq!(
        count, 1,
        "expected exactly 1 --append-system-prompt-file (combined prompt), got {count}:\n{output}"
    );
}

#[test]
fn wrapper_has_exactly_one_append_system_prompt_file_no_sandbox() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, true, DUMMY_PROMPT_PATH, None).unwrap();

    let count = output.matches("--append-system-prompt-file").count();
    assert_eq!(
        count, 1,
        "expected exactly 1 --append-system-prompt-file in no-sandbox, got {count}:\n{output}"
    );
}
