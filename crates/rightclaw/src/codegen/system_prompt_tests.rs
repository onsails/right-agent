use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::codegen::generate_combined_prompt;

fn make_agent_at(path: std::path::PathBuf) -> AgentDef {
    AgentDef {
        name: "testbot".to_owned(),
        path: path.clone(),
        identity_path: path.join("IDENTITY.md"),
        config: Some(AgentConfig {
            restart: RestartPolicy::OnFailure,
            max_restarts: 3,
            backoff_seconds: 5,
            start_prompt: None,
            model: None,
            sandbox: None,
            telegram_token_file: None,
            telegram_token: None,
            telegram_user_id: None,
            env: std::collections::HashMap::new(),
        }),
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

fn write_identity(dir: &std::path::Path) {
    std::fs::write(dir.join("IDENTITY.md"), "# Test Agent\nYou are a test agent.\n").unwrap();
}

#[test]
fn returns_ok_with_identity_content() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let agent = make_agent_at(agent_dir);
    let result = generate_combined_prompt(&agent);

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    let content = result.unwrap();
    assert!(
        content.contains("# Test Agent"),
        "expected identity content in:\n{content}"
    );
}

#[test]
fn returns_err_when_identity_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();

    let agent = make_agent_at(agent_dir);
    let result = generate_combined_prompt(&agent);

    assert!(result.is_err(), "expected Err when IDENTITY.md is missing");
}

#[test]
fn contains_default_start_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        content.contains("You are starting. Read your MEMORY.md to restore context."),
        "expected default start prompt in:\n{content}"
    );
}

#[test]
fn contains_rightcron_routing() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        content.contains("/rightcron"),
        "expected '/rightcron' routing in:\n{content}"
    );
    assert!(
        content.contains("Cron Management"),
        "expected 'Cron Management' section in:\n{content}"
    );
}

#[test]
fn contains_communication_section() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        content.contains("remote channel"),
        "expected remote channel instruction in:\n{content}"
    );
}
