use std::collections::HashMap;
use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::codegen::generate_wrapper;

fn make_agent(name: &str, start_prompt: Option<&str>) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        start_prompt: start_prompt.map(String::from),
        model: None,
        sandbox: None,
        telegram_token_file: None,
        telegram_token: None,
        telegram_user_id: None,
        env: HashMap::new(),
    });
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        config,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

fn make_agent_with_mcp(name: &str, start_prompt: Option<&str>) -> AgentDef {
    // Creates agent with .mcp.json present but NO telegram config — used to test
    // that mcp.json existence alone does NOT trigger --channels.
    make_agent(name, start_prompt)
}

fn make_agent_no_config(name: &str) -> AgentDef {
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        config: None,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

const DUMMY_PROMPT_PATH: &str = "/tmp/run/testbot-prompt.md";

#[test]
fn wrapper_runs_claude_directly() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains(r#"exec "$CLAUDE_BIN""#),
        "expected 'exec \"$CLAUDE_BIN\"' in wrapper:\n{output}"
    );
    assert!(
        !output.contains("openshell"),
        "should NOT contain openshell:\n{output}"
    );
}

#[test]
fn wrapper_contains_combined_prompt_and_permissions() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

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
fn wrapper_starts_with_shebang() {
    let agent = make_agent("testbot", Some("Hello"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.starts_with("#!/usr/bin/env bash"),
        "expected shebang at start of:\n{output}"
    );
}

#[test]
fn wrapper_no_config_agent_still_renders() {
    let agent = make_agent_no_config("testbot");
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

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
fn wrapper_with_telegram_config_includes_channels_flag() {
    let config = Some(AgentConfig {
        telegram_token: Some("123:abc".to_string()),
        ..make_agent("x", None).config.unwrap()
    });
    let mut agent = make_agent("testbot", Some("Go"));
    agent.config = config;
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("--channels plugin:telegram@claude-plugins-official"),
        "expected --channels flag when telegram_token configured:\n{output}"
    );
}

#[test]
fn wrapper_without_mcp_omits_channels_flag() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        !output.contains("--channels"),
        "should NOT contain --channels without telegram config:\n{output}"
    );
}

#[test]
fn wrapper_without_telegram_omits_channels_when_mcp_json_exists() {
    let agent = make_agent_with_mcp("testbot", Some("Go"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        !output.contains("--channels"),
        "should NOT contain --channels when no telegram config, even if .mcp.json exists:\n{output}"
    );
}

#[test]
fn wrapper_has_exactly_one_append_system_prompt_file() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    let count = output.matches("--append-system-prompt-file").count();
    assert_eq!(
        count, 1,
        "expected exactly 1 --append-system-prompt-file, got {count}:\n{output}"
    );
}

// Phase 8: HOME override and env var forwarding tests

#[test]
fn wrapper_contains_home_override() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("export HOME=\"/home/user/.rightclaw/agents/testbot\""),
        "expected HOME override in:\n{output}"
    );
}

#[test]
fn wrapper_contains_git_env_forwarding() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    let expected = [
        "export GIT_CONFIG_GLOBAL=\"${GIT_CONFIG_GLOBAL:-}\"",
        "export GIT_AUTHOR_NAME=\"${GIT_AUTHOR_NAME:-}\"",
        "export GIT_AUTHOR_EMAIL=\"${GIT_AUTHOR_EMAIL:-}\"",
        "export SSH_AUTH_SOCK=\"${SSH_AUTH_SOCK:-}\"",
        "export GIT_SSH_COMMAND=\"${GIT_SSH_COMMAND:-}\"",
    ];
    for line in &expected {
        assert!(output.contains(line), "expected '{line}' in:\n{output}");
    }
}

#[test]
fn wrapper_contains_anthropic_key_forwarding() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("export ANTHROPIC_API_KEY=\"${ANTHROPIC_API_KEY:-}\""),
        "expected ANTHROPIC_API_KEY forwarding in:\n{output}"
    );
}

#[test]
fn wrapper_home_override_after_env_capture() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    let home_idx = output
        .lines()
        .position(|l| l.contains("export HOME="))
        .expect("HOME export line not found");

    let env_vars = [
        "GIT_CONFIG_GLOBAL",
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "SSH_AUTH_SOCK",
        "GIT_SSH_COMMAND",
        "ANTHROPIC_API_KEY",
    ];
    for var in &env_vars {
        let var_idx = output
            .lines()
            .position(|l| l.contains(&format!("export {var}=")))
            .unwrap_or_else(|| panic!("{var} export line not found"));
        assert!(
            home_idx > var_idx,
            "HOME export (line {home_idx}) must come AFTER {var} export (line {var_idx})"
        );
    }
}

#[test]
fn wrapper_home_override_before_exec() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    let home_idx = output
        .lines()
        .position(|l| l.contains("export HOME="))
        .expect("HOME export line not found");

    let exec_idx = output
        .lines()
        .position(|l| l.contains("exec "))
        .expect("exec line not found");

    assert!(
        home_idx < exec_idx,
        "HOME export (line {home_idx}) must come BEFORE exec (line {exec_idx})"
    );
}

#[test]
fn wrapper_retains_dangerously_skip_permissions() {
    let agent = make_agent("testbot", Some("Do the thing"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    {
        assert!(
            output.contains("--dangerously-skip-permissions"),
            "expected --dangerously-skip-permissions in:\n{output}"
        );
    }
}

// Phase 11: env var injection tests

fn make_agent_with_env(name: &str, env: HashMap<String, String>) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        start_prompt: None,
        model: None,
        sandbox: None,
        telegram_token_file: None,
        telegram_token: None,
        telegram_user_id: None,
        env,
    });
    AgentDef {
        name: name.to_owned(),
        path: PathBuf::from(format!("/home/user/.rightclaw/agents/{name}")),
        identity_path: PathBuf::from(format!(
            "/home/user/.rightclaw/agents/{name}/IDENTITY.md"
        )),
        config,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

#[test]
fn wrapper_env_basic() {
    let mut env = HashMap::new();
    env.insert("MY_VAR".to_owned(), "hello world".to_owned());
    let agent = make_agent_with_env("testbot", env);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("export MY_VAR='hello world'"),
        "expected export MY_VAR='hello world' in:\n{output}"
    );
}

#[test]
fn wrapper_env_single_quote_escape() {
    let mut env = HashMap::new();
    env.insert("MSG".to_owned(), "it's alive".to_owned());
    let agent = make_agent_with_env("testbot", env);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains(r"export MSG='it'\''s alive'"),
        "expected single-quote escaped export in:\n{output}"
    );
}

#[test]
fn wrapper_env_special_chars() {
    let mut env = HashMap::new();
    env.insert("TOKEN".to_owned(), "$secret`hack`".to_owned());
    let agent = make_agent_with_env("testbot", env);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("export TOKEN='$secret`hack`'"),
        "expected literal single-quoted TOKEN with special chars in:\n{output}"
    );
}

#[test]
fn wrapper_env_before_home() {
    let mut env = HashMap::new();
    env.insert("MYKEY".to_owned(), "myval".to_owned());
    let agent = make_agent_with_env("testbot", env);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    // Collect lines to vec once — used for both position lookups.
    let lines: Vec<&str> = output.lines().collect();
    let home_idx = lines
        .iter()
        .position(|l| l.contains("export HOME="))
        .expect("HOME export line not found");

    // Find the last env: export line.
    let env_idx = lines
        .iter()
        .rposition(|l| l.contains("export MYKEY="))
        .expect("MYKEY export line not found");

    assert!(
        env_idx < home_idx,
        "env var export (line {env_idx}) must appear BEFORE HOME override (line {home_idx})"
    );
}

#[test]
fn wrapper_no_env_no_exports() {
    let agent = make_agent("testbot", Some("Go"));
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        !output.contains("export MY_VAR="),
        "should NOT contain extra export MY_VAR= in:\n{output}"
    );
}

#[test]
fn wrapper_env_empty_value() {
    let mut env = HashMap::new();
    env.insert("EMPTY".to_owned(), String::new());
    let agent = make_agent_with_env("testbot", env);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();

    assert!(
        output.contains("export EMPTY=''"),
        "expected export EMPTY='' in:\n{output}"
    );
}

// Phase 21: startup_prompt regression tests (D-01)

#[test]
fn startup_prompt_does_not_use_agent_tool() {
    let agent = make_agent("testbot", None);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        !output.contains("Agent tool"),
        "startup_prompt must NOT delegate to Agent tool:\n{output}"
    );
}

#[test]
fn startup_prompt_invokes_rightcron() {
    let agent = make_agent("testbot", None);
    let output = generate_wrapper(&agent, DUMMY_PROMPT_PATH, None).unwrap();
    assert!(
        output.contains("/rightcron"),
        "startup_prompt must invoke /rightcron:\n{output}"
    );
}
