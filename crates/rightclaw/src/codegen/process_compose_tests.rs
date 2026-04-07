use std::path::{Path, PathBuf};

use tempfile::tempdir;

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

        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
            secret: None,
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

        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
            secret: None,
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

fn make_agent_with_restart(name: &str, token: &str, restart: RestartPolicy) -> AgentDef {
    let config = Some(AgentConfig {
        restart,
        max_restarts: 5,
        backoff_seconds: 10,
        model: None,
        sandbox: None,
        telegram_token: Some(token.to_string()),

        allowed_chat_ids: vec![],
        env: std::collections::HashMap::new(),
            secret: None,
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("myagent-bot:"),
        "expected '<name>-bot:' process key in:\n{output}"
    );
}

#[test]
fn bot_agent_command_contains_rightclaw_bot_agent() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("rightclaw bot --agent myagent"),
        "expected 'rightclaw bot --agent myagent' in:\n{output}"
    );
}

#[test]
fn bot_agent_env_contains_rc_agent_dir() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("RC_AGENT_DIR=/home/user/.rightclaw/agents/myagent"),
        "expected RC_AGENT_DIR in:\n{output}"
    );
}

#[test]
fn bot_agent_env_contains_rc_agent_name() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("RC_AGENT_NAME=myagent"),
        "expected RC_AGENT_NAME=myagent in:\n{output}"
    );
}

#[test]
fn inline_token_uses_rc_telegram_token() {
    let agents = vec![make_bot_agent("myagent", "999:mytoken")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("RC_TELEGRAM_TOKEN=999:mytoken"),
        "expected RC_TELEGRAM_TOKEN in:\n{output}"
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        !output.contains("is_interactive"),
        "is_interactive must not appear in output:\n{output}"
    );
}

// ── MCP env vars ────────────────────────────────────────────────────────────

#[test]
fn env_contains_enable_claudeai_mcp_servers_false() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("ENABLE_CLAUDEAI_MCP_SERVERS=false"),
        "expected ENABLE_CLAUDEAI_MCP_SERVERS=false in:\n{output}"
    );
}

#[test]
fn env_contains_enable_tool_search_false() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("ENABLE_TOOL_SEARCH=false"),
        "expected ENABLE_TOOL_SEARCH=false in:\n{output}"
    );
}

#[test]
fn env_contains_mcp_connection_nonblocking() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("MCP_CONNECTION_NONBLOCKING=1"),
        "expected MCP_CONNECTION_NONBLOCKING=1 in:\n{output}"
    );
}

// ── Header and version ───────────────────────────────────────────────────────

#[test]
fn output_starts_with_generated_comment() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.starts_with("# Generated by rightclaw"),
        "expected '# Generated by rightclaw' at start of:\n{output}"
    );
}

#[test]
fn output_contains_is_strict_true() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("restart: \"on_failure\""),
        "expected on_failure policy in:\n{output}"
    );
}

#[test]
fn restart_policy_always_maps_correctly() {
    let agents = vec![make_agent_with_restart("bot", "123:tok", RestartPolicy::Always)];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("restart: \"always\""),
        "expected always policy in:\n{output}"
    );
}

#[test]
fn restart_policy_never_maps_to_no() {
    let agents = vec![make_agent_with_restart("bot", "123:tok", RestartPolicy::Never)];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
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
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    // No bot agents => the processes section should be empty (no plain: entry)
    assert!(
        !output.contains("plain"),
        "agent without config (no token) must not appear in output:\n{output}"
    );
}

// ── Cloudflared tunnel process ───────────────────────────────────────────────

#[test]
fn cloudflared_without_tunnel_absent_from_output() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        !output.contains("cloudflared:"),
        "cloudflared process must be absent when script is None:\n{output}"
    );
}

#[test]
fn cloudflared_with_script_produces_process_entry() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let script = Path::new("/home/user/.rightclaw/scripts/cloudflared-start.sh");
    let output = generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), Some(script), None).unwrap();
    assert!(
        output.contains("  cloudflared:"),
        "expected cloudflared process key in:\n{output}"
    );
    assert!(
        output.contains("command: \"/home/user/.rightclaw/scripts/cloudflared-start.sh\""),
        "expected absolute script path in cloudflared command:\n{output}"
    );
    assert!(
        output.contains("working_dir: \"/home/user/.rightclaw\""),
        "expected home dir as working_dir:\n{output}"
    );
    assert!(
        output.contains("restart: \"on_failure\""),
        "expected on_failure restart policy:\n{output}"
    );
    assert!(
        output.contains("backoff_seconds: 5"),
        "expected backoff_seconds: 5:\n{output}"
    );
    assert!(
        output.contains("max_restarts: 10"),
        "expected max_restarts: 10:\n{output}"
    );
    assert!(
        output.contains("signal: 15"),
        "expected signal: 15:\n{output}"
    );
    assert!(
        output.contains("timeout_seconds: 30"),
        "expected timeout_seconds: 30:\n{output}"
    );
}

// ── Sandbox mode env vars ───────────────────────────────────────────────────

#[test]
fn no_sandbox_true_emits_sandbox_mode_none() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output =
        generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("RC_SANDBOX_MODE=none"),
        "expected RC_SANDBOX_MODE=none when no_sandbox=true:\n{output}"
    );
    assert!(
        !output.contains("RC_SANDBOX_POLICY"),
        "RC_SANDBOX_POLICY must be absent when no_sandbox=true:\n{output}"
    );
}

#[test]
fn no_sandbox_false_emits_openshell_mode_and_policy_path() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let run_dir = Path::new("/tmp/run");
    let output =
        generate_process_compose(&agents, exe, false, false, run_dir, Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("RC_SANDBOX_MODE=openshell"),
        "expected RC_SANDBOX_MODE=openshell when no_sandbox=false:\n{output}"
    );
    assert!(
        output.contains("RC_SANDBOX_POLICY=/tmp/run/policies/myagent.yaml"),
        "expected RC_SANDBOX_POLICY with policy path:\n{output}"
    );
}

#[test]
fn no_sandbox_false_command_lacks_no_sandbox_flag() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output =
        generate_process_compose(&agents, exe, false, false, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        !output.contains("--no-sandbox"),
        "--no-sandbox must be absent from command when sandbox enabled:\n{output}"
    );
}

#[test]
fn no_sandbox_true_command_has_no_sandbox_flag() {
    let agents = vec![make_bot_agent("myagent", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output =
        generate_process_compose(&agents, exe, false, true, Path::new("/tmp/run"), Path::new("/home/user/.rightclaw"), None, None).unwrap();
    assert!(
        output.contains("--no-sandbox"),
        "expected --no-sandbox in command when no_sandbox=true:\n{output}"
    );
}

// ── Login process ───────────────────────────────────────────────────────────

// ── RC_PC_PORT env var ──────────────────────────────────────────────────────

#[test]
fn bot_process_has_rc_pc_port_env() {
    let agents = vec![make_bot_agent("right", "123:tok")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(
        &agents, exe, false, false, Path::new("/tmp/run"),
        Path::new("/home/user/.rightclaw"), None, None,
    ).unwrap();
    assert!(
        output.contains("RC_PC_PORT="),
        "expected RC_PC_PORT env var on bot process:\n{output}"
    );
}

// ── right-mcp-server process ─────────────────────────────────────────────────

#[test]
fn right_mcp_server_process_included_when_token_map_provided() {
    let dir = tempdir().unwrap();
    let token_map = dir.path().join("agent-tokens.json");
    std::fs::write(&token_map, "{}").unwrap();
    let agents = vec![make_bot_agent("test", "123:tok")];
    let yaml = generate_process_compose(
        &agents,
        Path::new("/usr/bin/rightclaw"),
        false,
        false,
        dir.path(),
        dir.path(),
        None,
        Some(&token_map),
    )
    .unwrap();
    assert!(yaml.contains("right-mcp-server:"), "must have right-mcp-server process");
    assert!(yaml.contains("memory-server-http"), "must run memory-server-http command");
    assert!(yaml.contains("--port 8100"), "must specify port");
    assert!(yaml.contains("depends_on:"), "bot must depend on mcp server");
}
