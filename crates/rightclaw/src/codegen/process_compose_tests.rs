use std::path::{Path, PathBuf};

use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::codegen::generate_process_compose;

const EXE_PATH: &str = "/usr/bin/rightclaw";

fn make_bot_agent(name: &str, token: &str) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: None,
        telegram_token: Some(token.to_string()),
        telegram_token_file: None,
        telegram_user_id: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
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

fn make_agent_no_token(name: &str) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: None,
        telegram_token: None,
        telegram_token_file: None,
        telegram_user_id: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
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

fn make_agent_token_file(name: &str, file: &str) -> AgentDef {
    let config = Some(AgentConfig {
        restart: RestartPolicy::OnFailure,
        max_restarts: 3,
        backoff_seconds: 5,
        model: None,
        sandbox: None,
        telegram_token: None,
        telegram_token_file: Some(file.to_string()),
        telegram_user_id: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
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

fn make_agent_with_restart(name: &str, token: &str, restart: RestartPolicy) -> AgentDef {
    let config = Some(AgentConfig {
        restart,
        max_restarts: 5,
        backoff_seconds: 10,
        model: None,
        sandbox: None,
        telegram_token: Some(token.to_string()),
        telegram_token_file: None,
        telegram_user_id: None,
        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
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

// ── Bot process key ─────────────────────────────────────────────────────────

#[test]
fn bot_agent_process_key_contains_name_bot() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("myagent-bot:"),
        "expected '<name>-bot:' process key in:\n{output}"
    );
}

#[test]
fn bot_agent_command_contains_rightclaw_bot_agent() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("rightclaw bot --agent myagent"),
        "expected 'rightclaw bot --agent myagent' in:\n{output}"
    );
}

#[test]
fn bot_agent_env_contains_rc_agent_dir() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("RC_AGENT_DIR=/home/user/.rightclaw/agents/myagent"),
        "expected RC_AGENT_DIR in:\n{output}"
    );
}

#[test]
fn bot_agent_env_contains_rc_agent_name() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("RC_AGENT_NAME=myagent"),
        "expected RC_AGENT_NAME=myagent in:\n{output}"
    );
}

#[test]
fn inline_token_uses_rc_telegram_token() {
    let agents = vec![make_bot_agent("myagent", "999:mytoken")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("RC_TELEGRAM_TOKEN=999:mytoken"),
        "expected RC_TELEGRAM_TOKEN in:\n{output}"
    );
}

#[test]
fn token_file_uses_rc_telegram_token_file_with_abs_path() {
    let agents = vec![make_agent_token_file("myagent", ".telegram.env")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    // Should resolve relative path to absolute: agent.path + file
    let expected = "RC_TELEGRAM_TOKEN_FILE=/home/user/.rightclaw/agents/myagent/.telegram.env";
    assert!(
        output.contains(expected),
        "expected absolute RC_TELEGRAM_TOKEN_FILE in:\n{output}"
    );
}

// ── Non-telegram agents absent from output ──────────────────────────────────

#[test]
fn agent_without_telegram_token_absent_from_output() {
    let agents = vec![
        make_bot_agent("with-token", "123:tok"),
        make_agent_no_token("no-token"),
    ];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        !output.contains("no-token"),
        "agent without token must be absent from output:\n{output}"
    );
}

#[test]
fn agent_without_config_absent_from_output() {
    let agents = vec![
        make_bot_agent("with-token", "123:tok"),
        make_agent_no_config("no-config"),
    ];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        !output.contains("no-config"),
        "agent without config must be absent from output:\n{output}"
    );
}

// ── No is_interactive anywhere ───────────────────────────────────────────────

#[test]
fn output_does_not_contain_is_interactive() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        !output.contains("is_interactive"),
        "is_interactive must not appear in output:\n{output}"
    );
}

// ── Header and version ───────────────────────────────────────────────────────

#[test]
fn output_starts_with_generated_comment() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.starts_with("# Generated by rightclaw"),
        "expected '# Generated by rightclaw' at start of:\n{output}"
    );
}

#[test]
fn output_contains_is_strict_true() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("is_strict: true"),
        "expected is_strict: true in:\n{output}"
    );
}

// ── Restart policies ─────────────────────────────────────────────────────────

#[test]
fn restart_policy_on_failure_maps_correctly() {
    let agents = vec![make_agent_with_restart("bot", "123:tok", RestartPolicy::OnFailure)];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("restart: \"on_failure\""),
        "expected on_failure policy in:\n{output}"
    );
}

#[test]
fn restart_policy_always_maps_correctly() {
    let agents = vec![make_agent_with_restart("bot", "123:tok", RestartPolicy::Always)];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("restart: \"always\""),
        "expected always policy in:\n{output}"
    );
}

#[test]
fn restart_policy_never_maps_to_no() {
    let agents = vec![make_agent_with_restart("bot", "123:tok", RestartPolicy::Never)];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    assert!(
        output.contains("restart: \"no\""),
        "expected 'no' for Never policy in:\n{output}"
    );
}

#[test]
fn defaults_when_no_config_not_in_output() {
    // Agent with no config has no telegram token, so should not appear in output at all
    let agents = vec![make_agent_no_config("plain")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe).unwrap();
    // No bot agents => the processes section should be empty (no plain: entry)
    assert!(
        !output.contains("plain"),
        "agent without config (no token) must not appear in output:\n{output}"
    );
}
