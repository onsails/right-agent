use super::*;

#[test]
fn sandbox_name_for_prefixes_with_rightclaw() {
    assert_eq!(sandbox_name_for("myagent"), "rightclaw-myagent");
    assert_eq!(sandbox_name_for("watch-dog"), "rightclaw-watch-dog");
    assert_eq!(sandbox_name_for("a"), "rightclaw-a");
}

#[test]
fn runtime_state_json_roundtrip() {
    let state = RuntimeState {
        agents: vec![
            AgentState {
                name: "agent1".to_string(),
                sandbox_name: "rightclaw-agent1".to_string(),
            },
            AgentState {
                name: "agent2".to_string(),
                sandbox_name: "rightclaw-agent2".to_string(),
            },
        ],
        socket_path: "/tmp/pc.sock".to_string(),
        started_at: "2026-03-22T12:00:00Z".to_string(),
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
            sandbox_name: "rightclaw-test-agent".to_string(),
        }],
        socket_path: "/run/pc.sock".to_string(),
        started_at: "2026-03-22T16:00:00Z".to_string(),
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
