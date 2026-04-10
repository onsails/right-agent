use std::fs;

use tempfile::tempdir;

use super::*;

// --- validate_agent_name ---

#[test]
fn validate_accepts_simple_name() {
    assert!(validate_agent_name("right").is_ok());
}

#[test]
fn validate_accepts_hyphenated_name() {
    assert!(validate_agent_name("my-agent").is_ok());
}

#[test]
fn validate_accepts_underscored_name() {
    assert!(validate_agent_name("agent_01").is_ok());
}

#[test]
fn validate_rejects_name_with_spaces() {
    let err = validate_agent_name("my agent").unwrap_err();
    assert!(
        matches!(err, AgentError::InvalidName { .. }),
        "expected InvalidName, got: {err:?}"
    );
}

#[test]
fn validate_rejects_name_with_dots() {
    assert!(validate_agent_name("agent.1").is_err());
}

#[test]
fn validate_rejects_name_with_slashes() {
    assert!(validate_agent_name("agent/bad").is_err());
}

#[test]
fn validate_rejects_empty_name() {
    assert!(validate_agent_name("").is_err());
}

#[test]
fn validate_rejects_name_starting_with_hyphen() {
    assert!(validate_agent_name("-agent").is_err());
}

// --- parse_agent_config ---

#[test]
fn parse_config_returns_none_when_no_file() {
    let dir = tempdir().unwrap();
    let result = parse_agent_config(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn parse_config_parses_valid_yaml() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("agent.yaml"),
        "restart: always\nmax_restarts: 5\n",
    )
    .unwrap();
    let config = parse_agent_config(dir.path()).unwrap().unwrap();
    assert_eq!(config.restart, crate::agent::types::RestartPolicy::Always);
    assert_eq!(config.max_restarts, 5);
}

#[test]
fn parse_config_rejects_unknown_fields() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("agent.yaml"),
        "restart: never\nunknown_field: bad\n",
    )
    .unwrap();
    let result = parse_agent_config(dir.path());
    assert!(result.is_err(), "expected error for unknown fields");
}

// --- discover_agents ---

#[test]
fn discover_empty_dir_returns_empty() {
    let dir = tempdir().unwrap();
    let agents = discover_agents(dir.path()).unwrap();
    assert!(agents.is_empty());
}

#[test]
fn discover_finds_valid_agent() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("test-agent");
    fs::create_dir(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# Test Agent").unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "test-agent");
    assert!(agents[0].identity_path.ends_with("IDENTITY.md"));
}

#[test]
fn discover_accepts_agent_without_policy() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("no-policy");
    fs::create_dir(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# No Policy").unwrap();
    // No policy.yaml -- should still be discovered

    let agents = discover_agents(dir.path()).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "no-policy");
}

#[test]
fn discover_skips_non_directory_entries() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("not-a-dir.txt"), "just a file").unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert!(agents.is_empty());
}

#[test]
fn discover_skips_directories_without_agent_yaml() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("no-agent-yaml");
    fs::create_dir(&agent_dir).unwrap();

    // Has IDENTITY.md but no agent.yaml — should be skipped
    fs::write(agent_dir.join("IDENTITY.md"), "# No agent.yaml").unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert!(agents.is_empty());
}

#[test]
fn discover_parses_agent_yaml_when_present() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("configured");
    fs::create_dir(&agent_dir).unwrap();

    fs::write(
        agent_dir.join("agent.yaml"),
        "restart: always\nmax_restarts: 7\n",
    )
    .unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert_eq!(agents.len(), 1);
    let config = agents[0].config.as_ref().unwrap();
    assert_eq!(config.restart, crate::agent::types::RestartPolicy::Always);
    assert_eq!(config.max_restarts, 7);
}

#[test]
fn discover_detects_mcp_json() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("mcp-agent");
    fs::create_dir(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();

    fs::write(agent_dir.join("mcp.json"), "{}").unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert_eq!(agents.len(), 1);
    assert!(agents[0].path.join("mcp.json").exists());
}

#[test]
fn discover_detects_optional_files() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("full-agent");
    fs::create_dir(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();
    fs::write(agent_dir.join("IDENTITY.md"), "# Full").unwrap();

    fs::write(agent_dir.join("SOUL.md"), "soul").unwrap();
    fs::write(agent_dir.join("USER.md"), "user").unwrap();
    fs::write(agent_dir.join("AGENTS.md"), "agents").unwrap();
    fs::write(agent_dir.join("TOOLS.md"), "tools").unwrap();
    fs::write(agent_dir.join("BOOTSTRAP.md"), "bootstrap").unwrap();
    fs::write(agent_dir.join("HEARTBEAT.md"), "heartbeat").unwrap();

    let agents = discover_agents(dir.path()).unwrap();
    assert_eq!(agents.len(), 1);
    let a = &agents[0];
    assert!(a.soul_path.is_some());
    assert!(a.user_path.is_some());
    assert!(a.agents_path.is_some());
    assert!(a.tools_path.is_some());
    assert!(a.bootstrap_path.is_some());
    assert!(a.heartbeat_path.is_some());
}

#[test]
fn discover_rejects_invalid_agent_name() {
    let dir = tempdir().unwrap();
    let agent_dir = dir.path().join("bad.name");
    fs::create_dir(&agent_dir).unwrap();
    fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();


    let result = discover_agents(dir.path());
    assert!(result.is_err());
}

#[test]
fn discover_sorts_agents_by_name() {
    let dir = tempdir().unwrap();
    for name in ["zebra", "alpha", "middle"] {
        let agent_dir = dir.path().join(name);
        fs::create_dir(&agent_dir).unwrap();
        fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();
    }

    let agents = discover_agents(dir.path()).unwrap();
    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "middle", "zebra"]);
}
