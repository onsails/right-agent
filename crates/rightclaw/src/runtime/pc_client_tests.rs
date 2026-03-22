use super::*;

#[test]
fn pc_client_constructs_with_dummy_path() {
    let path = std::path::Path::new("/tmp/test-pc.sock");
    let client = PcClient::new(path);
    assert!(client.is_ok(), "PcClient::new should succeed with any path");
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
