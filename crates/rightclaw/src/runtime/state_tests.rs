use super::*;

#[test]
fn runtime_state_json_roundtrip() {
    let state = RuntimeState {
        agents: vec![
            AgentState {
                name: "agent1".to_string(),
            },
            AgentState {
                name: "agent2".to_string(),
            },
        ],
        socket_path: "/tmp/pc.sock".to_string(),
        started_at: "2026-03-22T12:00:00Z".to_string(),
        pc_port: PC_PORT,
        pc_api_token: None,
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let deserialized: RuntimeState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, deserialized);
}

#[test]
fn write_state_and_read_state_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");

    let state = RuntimeState {
        agents: vec![AgentState {
            name: "test-agent".to_string(),
        }],
        socket_path: "/run/pc.sock".to_string(),
        started_at: "2026-03-22T16:00:00Z".to_string(),
        pc_port: PC_PORT,
        pc_api_token: None,
    };

    write_state(&state, &path).unwrap();
    let loaded = read_state(&path).unwrap();
    assert_eq!(state, loaded);
}

#[test]
fn read_state_fails_on_missing_file() {
    let result = read_state(std::path::Path::new("/nonexistent/state.json"));
    assert!(result.is_err());
}

#[test]
fn read_state_fails_on_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "not valid json").unwrap();

    let result = read_state(&path);
    assert!(result.is_err());
}

/// Verify that state.json files from v1.0 (with extra fields like
/// sandbox_name, no_sandbox) can still be deserialized by the new
/// simplified structs. Serde ignores unknown fields by default.
#[test]
fn read_state_ignores_extra_fields_from_v1() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1-state.json");
    let v1_json = r#"{
        "agents": [
            {"name": "agent1", "sandbox_name": "rightclaw-agent1"}
        ],
        "socket_path": "/tmp/pc.sock",
        "started_at": "2026-03-22T12:00:00Z",
        "no_sandbox": false
    }"#;
    std::fs::write(&path, v1_json).unwrap();

    let state = read_state(&path).unwrap();
    assert_eq!(state.agents.len(), 1);
    assert_eq!(state.agents[0].name, "agent1");
}
