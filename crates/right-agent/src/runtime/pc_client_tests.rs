use super::*;
use crate::runtime::PC_PORT;

/// Regression: process-compose v1.94+ reads the API token from header
/// `X-PC-Token-Key`. Sending `Authorization: Bearer …` (the previous
/// implementation) caused every REST call to 401 silently — see the
/// rebootstrap-skipped-the-bot incident.
#[tokio::test]
async fn health_check_sends_x_pc_token_key_header() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/live"))
        .and(header("X-PC-Token-Key", "the-token"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // Default 404 for any request missing the header — proves the matcher
    // above is what makes `health_check` succeed, not a permissive default.

    let port = server.address().port();
    let client = PcClient::new(port, Some("the-token".to_string())).unwrap();
    client
        .health_check()
        .await
        .expect("health check must succeed when X-PC-Token-Key matches");
}

#[tokio::test]
async fn health_check_fails_when_token_missing() {
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // Only respond 200 if the token header is present.
    Mock::given(method("GET"))
        .and(path("/live"))
        .and(header_exists("X-PC-Token-Key"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/live"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let port = server.address().port();
    let client = PcClient::new(port, None).unwrap();
    let result = client.health_check().await;
    assert!(
        result.is_err(),
        "health check must fail when no token is configured but PC requires one",
    );
}

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
