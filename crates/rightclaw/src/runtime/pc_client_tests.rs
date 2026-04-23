use super::*;

#[test]
fn pc_client_constructs_with_port() {
    let client = PcClient::new(PC_PORT, None);
    assert!(client.is_ok(), "PcClient::new should succeed with any port");
}

#[test]
fn from_home_returns_none_when_state_absent() {
    let dir = tempfile::tempdir().unwrap();
    // No <home>/run/state.json.
    let result = PcClient::from_home(dir.path()).unwrap();
    assert!(
        result.is_none(),
        "from_home must return None when runtime state is absent",
    );
}

#[test]
fn from_home_reads_port_from_state() {
    use crate::runtime::state::{AgentState, RuntimeState, write_state};

    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();

    let state = RuntimeState {
        agents: vec![AgentState {
            name: "from-home-test".to_string(),
        }],
        socket_path: "/tmp/pc.sock".to_string(),
        started_at: "2026-04-22T00:00:00Z".to_string(),
        pc_port: 19999,
        pc_api_token: Some("test-token-123".to_string()),
    };
    write_state(&state, &run_dir.join("state.json")).unwrap();

    let client = PcClient::from_home(dir.path())
        .unwrap()
        .expect("state.json exists, expected Some(client)");
    assert!(
        client.base_url.contains("19999"),
        "base_url should carry pc_port from state; got {}",
        client.base_url,
    );
}

#[test]
fn from_home_errors_on_malformed_state() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("state.json"), "not valid json").unwrap();

    let result = PcClient::from_home(dir.path());
    assert!(
        result.is_err(),
        "from_home must propagate malformed-state errors",
    );
}

#[test]
fn process_info_deserializes_from_json() {
    let json = r#"{
        "name": "agent1",
        "status": "Running",
        "pid": 1234,
        "system_time": "10s",
        "exit_code": 0
    }"#;
    let info: ProcessInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.name, "agent1");
    assert_eq!(info.status, "Running");
    assert_eq!(info.pid, 1234);
    assert_eq!(info.system_time, "10s");
    assert_eq!(info.exit_code, 0);
}

#[test]
fn processes_response_deserializes_from_json() {
    let json = r#"{
        "data": [
            {
                "name": "agent1",
                "status": "Running",
                "pid": 1234,
                "system_time": "10s",
                "exit_code": 0
            },
            {
                "name": "agent2",
                "status": "Completed",
                "pid": 0,
                "system_time": "5m30s",
                "exit_code": 1
            }
        ]
    }"#;
    let resp: ProcessesResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.data.len(), 2);
    assert_eq!(resp.data[0].name, "agent1");
    assert_eq!(resp.data[1].name, "agent2");
    assert_eq!(resp.data[1].exit_code, 1);
}

#[test]
fn process_info_handles_negative_pid() {
    let json = r#"{
        "name": "agent1",
        "status": "Pending",
        "pid": -1,
        "system_time": "",
        "exit_code": 0
    }"#;
    let info: ProcessInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.pid, -1);
}

#[test]
fn processes_response_handles_empty_data() {
    let json = r#"{"data": []}"#;
    let resp: ProcessesResponse = serde_json::from_str(json).unwrap();
    assert!(resp.data.is_empty());
}

#[test]
fn logs_response_deserializes_from_json() {
    let json = r#"{"logs": ["line 1", "line 2", "auth url: https://example.com"]}"#;
    let resp: LogsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.logs.len(), 3);
    assert_eq!(resp.logs[2], "auth url: https://example.com");
}

#[test]
fn logs_response_handles_empty_logs() {
    let json = r#"{"logs": []}"#;
    let resp: LogsResponse = serde_json::from_str(json).unwrap();
    assert!(resp.logs.is_empty());
}
