use std::path::PathBuf;

use crate::agent::{AgentConfig, AgentDef, RestartPolicy};
use crate::codegen::generate_combined_prompt;

fn make_agent_at(path: PathBuf) -> AgentDef {
    AgentDef {
        name: "testbot".to_owned(),
        path: path.clone(),
        identity_path: path.join("IDENTITY.md"),
        policy_path: path.join("policy.yaml"),
        config: Some(AgentConfig {
            restart: RestartPolicy::OnFailure,
            max_restarts: 3,
            backoff_seconds: 5,
            start_prompt: None,
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
    // No IDENTITY.md created

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
fn contains_custom_start_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let mut agent = make_agent_at(agent_dir);
    agent.config.as_mut().unwrap().start_prompt = Some("Custom startup task".to_owned());

    let content = generate_combined_prompt(&agent).unwrap();
    assert!(
        content.contains("Custom startup task"),
        "expected custom start prompt in:\n{content}"
    );
}

#[test]
fn includes_cronsync_when_crons_dir_has_yaml() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let crons_dir = agent_dir.join("crons");
    std::fs::create_dir(&crons_dir).unwrap();
    std::fs::write(crons_dir.join("daily.yaml"), "schedule: daily").unwrap();

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        content.contains("RightClaw System Instructions"),
        "expected 'RightClaw System Instructions' in:\n{content}"
    );
    assert!(
        content.contains("/cronsync"),
        "expected '/cronsync' in:\n{content}"
    );
}

#[test]
fn omits_cronsync_when_no_crons_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        !content.contains("/cronsync"),
        "expected no /cronsync when crons/ dir is absent:\n{content}"
    );
}

#[test]
fn omits_cronsync_when_crons_is_file_not_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);
    std::fs::write(agent_dir.join("crons"), "not a directory").unwrap();

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        !content.contains("/cronsync"),
        "expected no /cronsync when crons is a file, not a directory:\n{content}"
    );
}

#[test]
fn omits_cronsync_when_crons_dir_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().to_path_buf();
    write_identity(&agent_dir);
    std::fs::create_dir(agent_dir.join("crons")).unwrap();
    // No yaml files inside

    let agent = make_agent_at(agent_dir);
    let content = generate_combined_prompt(&agent).unwrap();

    assert!(
        !content.contains("/cronsync"),
        "expected no /cronsync when crons/ dir has no yaml files:\n{content}"
    );
}
