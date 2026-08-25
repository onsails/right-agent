use super::*;
use crate::test_support::setup_crypto;
use right_runtime_state::PC_PORT;

/// Regression: process-compose v1.94+ reads the API token from header
/// `X-PC-Token-Key`. Sending `Authorization: Bearer …` (the previous
/// implementation) caused every REST call to 401 silently — see the
/// rebootstrap-skipped-the-bot incident.
#[tokio::test]
async fn health_check_sends_x_pc_token_key_header() {
    setup_crypto();
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
    setup_crypto();
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

#[tokio::test]
async fn restart_cloudflared_if_config_changed_posts_restart_when_changed() {
    setup_crypto();
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process/restart/cloudflared"))
        .and(header("X-PC-Token-Key", "the-token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let port = server.address().port();
    let client = PcClient::new(port, Some("the-token".to_string())).unwrap();

    client
        .restart_cloudflared_if_config_changed(true)
        .await
        .expect("changed cloudflared config must restart cloudflared");
}

#[tokio::test]
async fn restart_cloudflared_if_config_changed_skips_restart_when_unchanged() {
    setup_crypto();
    let server = wiremock::MockServer::start().await;
    let port = server.address().port();
    let client = PcClient::new(port, Some("the-token".to_string())).unwrap();

    client
        .restart_cloudflared_if_config_changed(false)
        .await
        .expect("unchanged cloudflared config must not call process-compose");
}

#[test]
fn pc_client_constructs_with_port() {
    setup_crypto();
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
    setup_crypto();
    use right_runtime_state::{AgentState, RuntimeState, write_state};

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

// --- shutdown_and_wait (offline db-repair quiescence) ---

/// Minimal raw-TCP process-compose stand-in. wiremock's graceful shutdown
/// keeps pooled keep-alive connections serving, which cannot simulate the
/// connection-refused signal a real process-compose exit produces — this
/// fake hard-closes the listener and every connection task on `shutdown()`.
struct FakePc {
    port: u16,
    stop_received: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakePc {
    /// `running_body` is served until project stop arrives; afterwards
    /// `terminal_body` is served (processes stopped, PC about to exit).
    async fn start(running_body: String, terminal_body: String) -> Self {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop_received = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let task = {
            let stop_received = Arc::clone(&stop_received);
            tokio::spawn(async move {
                let mut connections = Vec::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut socket, _) = accepted.expect("accept");
                            let stop_received = Arc::clone(&stop_received);
                            let running_body = running_body.clone();
                            let terminal_body = terminal_body.clone();
                            connections.push(tokio::spawn(async move {
                                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                                let mut buf = vec![0u8; 8192];
                                let n = socket.read(&mut buf).await.expect("read request");
                                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                                let path = request
                                    .split_whitespace()
                                    .nth(1)
                                    .unwrap_or("/")
                                    .to_string();
                                let (status, body) = if path == "/live" {
                                    ("200 OK", "ok".to_string())
                                } else if path == "/processes" {
                                    if stop_received.load(Ordering::SeqCst) {
                                        ("200 OK", terminal_body)
                                    } else {
                                        ("200 OK", running_body)
                                    }
                                } else if path == "/project/stop" {
                                    stop_received.store(true, Ordering::SeqCst);
                                    ("200 OK", "{}".to_string())
                                } else {
                                    ("404 Not Found", "{}".to_string())
                                };
                                let response = format!(
                                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                                     connection: close\r\ncontent-length: {}\r\n\r\n{body}",
                                    body.len()
                                );
                                socket
                                    .write_all(response.as_bytes())
                                    .await
                                    .expect("write response");
                            }));
                        }
                        _ = &mut shutdown_rx => break,
                    }
                }
                // Hard close: drop the listener and abort every connection so
                // clients observe connection-refused, like process-compose
                // exiting after project stop.
                drop(listener);
                for connection in connections {
                    connection.abort();
                }
            })
        };
        Self {
            port,
            stop_received,
            shutdown_tx: Some(shutdown_tx),
            task,
        }
    }

    fn stop_was_requested(&self) -> bool {
        self.stop_received.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn shutdown(mut self) {
        self.shutdown_tx.take().unwrap().send(()).unwrap();
        self.task.await.unwrap();
    }
}

/// Full quiescence: snapshot, project stop, poll until every previously
/// active `*-bot`/`right-mcp-server` is terminal AND the server is gone.
#[tokio::test]
async fn shutdown_and_wait_succeeds_when_server_goes_down() {
    setup_crypto();
    let running = r#"{"data":[
        {"name":"riskoff-bot","status":"Running","pid":1001,"system_time":"0:01","exit_code":0},
        {"name":"right-mcp-server","status":"Running","pid":1002,"system_time":"0:01","exit_code":0},
        {"name":"cloudflared","status":"Running","pid":1003,"system_time":"0:01","exit_code":0}
    ]}"#
    .to_string();
    let terminal = r#"{"data":[
        {"name":"riskoff-bot","status":"Completed","pid":0,"system_time":"0:01","exit_code":0},
        {"name":"right-mcp-server","status":"Completed","pid":0,"system_time":"0:01","exit_code":0},
        {"name":"cloudflared","status":"Completed","pid":0,"system_time":"0:01","exit_code":0}
    ]}"#
    .to_string();
    let server = FakePc::start(running, terminal).await;
    let port = server.port;

    let client = PcClient::new(port, None).unwrap();
    let task = tokio::spawn(async move {
        client
            .shutdown_and_wait(std::time::Duration::from_secs(10))
            .await
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !server.stop_was_requested() {
        assert!(
            std::time::Instant::now() < deadline,
            "shutdown_and_wait never issued project stop"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Processes terminate, then process-compose exits (no --keep-project).
    server.shutdown().await;

    let stopped = task
        .await
        .expect("shutdown_and_wait task panicked")
        .expect("shutdown_and_wait must succeed once the server is down");
    assert_eq!(
        stopped,
        vec![
            "cloudflared".to_string(),
            "right-mcp-server".to_string(),
            "riskoff-bot".to_string()
        ],
        "the snapshot of previously active processes is returned, sorted"
    );
}

/// Fail closed: state.json exists (a client could be constructed) but the
/// endpoint is dead before any shutdown request — repair must not proceed.
#[tokio::test]
async fn shutdown_and_wait_fails_closed_when_health_unreachable() {
    setup_crypto();
    // A bound-then-dropped listener yields a port nothing serves.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let client = PcClient::new(port, None).unwrap();

    let start = std::time::Instant::now();
    let err = client
        .shutdown_and_wait(std::time::Duration::from_secs(10))
        .await
        .expect_err("unreachable runtime must fail closed");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "fail-closed must be immediate, not a poll-loop timeout"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unreachable") || msg.contains("health"),
        "error must explain the fail-closed reason: {msg}"
    );
}

/// A process still Running at the deadline fails the wait and names the
/// offender; no mutation may proceed after this error.
#[tokio::test]
async fn shutdown_and_wait_times_out_while_process_still_running() {
    setup_crypto();
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/live"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/processes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"name": "riskoff-bot", "status": "Running", "pid": 1001, "system_time": "0:01", "exit_code": 0}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/project/stop"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = PcClient::new(server.address().port(), None).unwrap();
    let err = client
        .shutdown_and_wait(std::time::Duration::from_millis(600))
        .await
        .expect_err("still-running process must time out the wait");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("riskoff-bot"),
        "error must name the process: {msg}"
    );
}

/// A rejected project-stop request is a hard failure, never a fallback to
/// best-effort behavior.
#[tokio::test]
async fn shutdown_and_wait_fails_when_project_stop_rejected() {
    setup_crypto();
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/live"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/processes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"name": "riskoff-bot", "status": "Running", "pid": 1001, "system_time": "0:01", "exit_code": 0}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/project/stop"))
        .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
        .mount(&server)
        .await;

    let client = PcClient::new(server.address().port(), None).unwrap();
    let err = client
        .shutdown_and_wait(std::time::Duration::from_secs(5))
        .await
        .expect_err("rejected shutdown must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("stop") || msg.contains("shutdown"),
        "error must describe the failed shutdown request: {msg}"
    );
}
