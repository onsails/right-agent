//! Tests for `right agent db-repair` orchestration (Stage 1 offline repair).
//!
//! Runtime quiescence runs through the real `PcClient` against a raw-TCP
//! process-compose fake (wiremock's graceful shutdown cannot simulate
//! connection-refused); the repair path is the real
//! `right_db::repair_legacy_wal` against tempdir fixtures, except where an
//! injected repair function stages a mid-swap failure. Live `~/.right` is
//! never touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::run_db_repair;

const CANARY_TOKEN: &str = "CANARY-TOKEN-cli-55aa11";
const CANARY_PROMPT: &str = "CANARY-PROMPT-cli-defrag-the-moon";

/// Create `<home>/agents/<name>/data.db`, migrated, with canary rows and
/// legacy coordination sentinels. Returns the agent dir and a byte snapshot
/// of every `data.db*` artifact.
async fn fixture_agent(home: &Path, name: &str) -> (PathBuf, Vec<(String, Vec<u8>)>) {
    let agent_dir = home.join("agents").join(name);
    std::fs::create_dir_all(&agent_dir).unwrap();
    let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
    conn.execute(
        "INSERT INTO auth_tokens (token) VALUES (?1)",
        [CANARY_TOKEN],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO cron_specs (job_name, schedule, prompt, created_at, updated_at) \
         VALUES ('nightly', '0 3 * * *', ?1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [CANARY_PROMPT],
    )
    .await
    .unwrap();
    drop(conn);
    for suffix in ["-tshm", "-shm"] {
        let sidecar = agent_dir.join(format!("data.db{suffix}"));
        if !sidecar.exists() {
            std::fs::write(&sidecar, b"legacy coordination sentinel").unwrap();
        }
    }
    let snapshot = snapshot_artifacts(&agent_dir);
    (agent_dir, snapshot)
}

fn snapshot_artifacts(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "data.db" || name.starts_with("data.db-") {
                Some((name.clone(), std::fs::read(dir.join(name)).unwrap()))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

fn write_state(home: &Path, pc_port: u16) {
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let state = right_runtime_state::RuntimeState {
        agents: vec![right_runtime_state::AgentState {
            name: "riskoff".to_string(),
        }],
        socket_path: "/tmp/pc.sock".to_string(),
        started_at: "2026-08-25T00:00:00Z".to_string(),
        pc_port,
        pc_api_token: None,
    };
    right_runtime_state::write_state(&state, &run_dir.join("state.json")).unwrap();
}

/// Raw-TCP process-compose fake: serves /live, /processes, /project/stop.
/// With `auto_exit`, the server hard-closes the listener and every connection
/// once project stop arrived (connection-refused, like a real process-compose
/// exit — the generated config never sets --keep-project).
struct FakePc {
    port: u16,
    stop_received: Arc<AtomicBool>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakePc {
    async fn start(running_body: &str, terminal_body: &str, auto_exit: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop_received = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let task = {
            let stop_received = Arc::clone(&stop_received);
            let running_body = running_body.to_string();
            let terminal_body = terminal_body.to_string();
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
                                let path =
                                    request.split_whitespace().nth(1).unwrap_or("/").to_string();
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
                    if auto_exit && stop_received.load(Ordering::SeqCst) {
                        break;
                    }
                }
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
        self.stop_received.load(Ordering::SeqCst)
    }
    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            // Auto-exit may have dropped the receiver already; in that case
            // the server task must already be finished.
            match tx.send(()) {
                Ok(()) => {}
                Err(()) => assert!(
                    self.task.is_finished(),
                    "fake server receiver vanished early"
                ),
            }
        }
        self.task.await.expect("fake server task must not panic");
    }
}

const RUNNING_BODY: &str = r#"{"data":[
    {"name":"riskoff-bot","status":"Running","pid":1001,"system_time":"0:01","exit_code":0},
    {"name":"right-mcp-server","status":"Running","pid":1002,"system_time":"0:01","exit_code":0}
]}"#;
const TERMINAL_BODY: &str = r#"{"data":[
    {"name":"riskoff-bot","status":"Completed","pid":0,"system_time":"0:01","exit_code":0},
    {"name":"right-mcp-server","status":"Completed","pid":0,"system_time":"0:01","exit_code":0}
]}"#;

#[tokio::test]
async fn state_absent_repairs_without_runtime_contact() {
    let home = tempfile::tempdir().unwrap();
    let (agent_dir, _snapshot) = fixture_agent(home.path(), "riskoff").await;

    let reports = run_db_repair(
        home.path(),
        &["riskoff".to_string()],
        Duration::from_secs(30),
    )
    .await
    .expect("state.json absent must mean not-started, not failure");
    assert_eq!(reports.len(), 1);

    // Repaired live set is the standalone snapshot; recovery artifacts exist.
    let live = snapshot_artifacts(&agent_dir);
    assert_eq!(
        live.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["data.db"]
    );
    let manifest = std::fs::read_to_string(&reports[0].manifest_path).unwrap();
    assert!(manifest.contains("\"swap_status\": \"swapped\""));
    for canary in [CANARY_TOKEN, CANARY_PROMPT] {
        assert!(
            !manifest.contains(canary),
            "manifest must never contain row values: {canary}"
        );
    }
}

#[tokio::test]
async fn healthy_runtime_is_shutdown_before_repair() {
    let home = tempfile::tempdir().unwrap();
    fixture_agent(home.path(), "riskoff").await;
    let server = FakePc::start(RUNNING_BODY, TERMINAL_BODY, true).await;
    write_state(home.path(), server.port);

    let reports = run_db_repair(
        home.path(),
        &["riskoff".to_string()],
        Duration::from_secs(30),
    )
    .await
    .expect("healthy runtime must shut down and repair");
    assert!(
        server.stop_was_requested(),
        "project stop must be requested before any repair"
    );
    assert_eq!(reports.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn unreachable_runtime_fails_closed_before_mutation() {
    let home = tempfile::tempdir().unwrap();
    let (agent_dir, snapshot) = fixture_agent(home.path(), "riskoff").await;
    // state.json points at a port nothing serves.
    let dead_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    write_state(home.path(), dead_port);

    let err = run_db_repair(
        home.path(),
        &["riskoff".to_string()],
        Duration::from_secs(30),
    )
    .await
    .expect_err("retained state.json + dead endpoint must fail closed");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unreachable") || msg.contains("fail closed"),
        "error must explain fail-closed: {msg}"
    );
    assert_eq!(
        snapshot_artifacts(&agent_dir),
        snapshot,
        "no mutation may happen when quiescence is unproven"
    );
    assert!(
        !home.path().join("backups").exists(),
        "no recovery artifacts may be created"
    );
}

#[tokio::test]
async fn still_running_runtime_times_out_before_mutation() {
    let home = tempfile::tempdir().unwrap();
    let (agent_dir, snapshot) = fixture_agent(home.path(), "riskoff").await;
    // Server never exits and keeps reporting the bot as Running.
    let server = FakePc::start(RUNNING_BODY, RUNNING_BODY, false).await;
    write_state(home.path(), server.port);

    let err = run_db_repair(
        home.path(),
        &["riskoff".to_string()],
        Duration::from_millis(600),
    )
    .await
    .expect_err("still-running runtime must time out the wait");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("riskoff-bot"),
        "error names the process: {msg}"
    );
    assert_eq!(
        snapshot_artifacts(&agent_dir),
        snapshot,
        "timeout must happen before any mutation"
    );
    assert!(!home.path().join("backups").exists());
    server.shutdown().await;
}

#[tokio::test]
async fn preflight_rejects_unknown_agent_before_quiescence() {
    let home = tempfile::tempdir().unwrap();
    let (known_dir, snapshot) = fixture_agent(home.path(), "known").await;
    let server = FakePc::start(RUNNING_BODY, TERMINAL_BODY, true).await;
    write_state(home.path(), server.port);

    let err = run_db_repair(
        home.path(),
        &["known".to_string(), "ghost".to_string()],
        Duration::from_secs(30),
    )
    .await
    .expect_err("unknown agent must fail preflight");
    let msg = format!("{err:#}");
    assert!(msg.contains("ghost"), "error names the agent: {msg}");
    assert!(
        !server.stop_was_requested(),
        "preflight failure must happen before any shutdown"
    );
    assert_eq!(
        snapshot_artifacts(&known_dir),
        snapshot,
        "the valid agent must not be repaired when a sibling fails preflight"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn later_agent_failure_preserves_prior_successful_manifest() {
    let home = tempfile::tempdir().unwrap();
    fixture_agent(home.path(), "good").await;
    // Broken second agent: data.db exists but is not a database.
    let broken_dir = home.path().join("agents").join("broken");
    std::fs::create_dir_all(&broken_dir).unwrap();
    std::fs::write(broken_dir.join("data.db"), b"definitely not sqlite").unwrap();

    let err = run_db_repair(
        home.path(),
        &["good".to_string(), "broken".to_string()],
        Duration::from_secs(30),
    )
    .await
    .expect_err("a failing later agent must fail the invocation");
    let msg = format!("{err:#}");
    assert!(msg.contains("broken"), "error must name the agent: {msg}");

    // The first agent's successful manifest is preserved.
    let good_backups = home.path().join("backups").join("good");
    let mut manifests = 0;
    for entry in std::fs::read_dir(&good_backups).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("wal-recovery-")
        {
            let text = std::fs::read_to_string(entry.path().join("manifest.json")).unwrap();
            assert!(text.contains("\"swap_status\": \"swapped\""));
            manifests += 1;
        }
    }
    assert_eq!(manifests, 1, "prior successful manifest must be preserved");

    // The failed agent's live bytes are untouched.
    assert_eq!(
        std::fs::read(broken_dir.join("data.db")).unwrap(),
        b"definitely not sqlite",
        "failed agent's data.db must remain byte-identical"
    );
}

/// A mid-swap failure inside the repair restores the complete original set
/// and the runtime stays down (no restart is attempted on failure).
#[tokio::test]
async fn swap_failure_restores_original_set() {
    let home = tempfile::tempdir().unwrap();
    let (agent_dir, snapshot) = fixture_agent(home.path(), "riskoff").await;
    let server = FakePc::start(RUNNING_BODY, TERMINAL_BODY, true).await;
    write_state(home.path(), server.port);
    assert!(snapshot.iter().any(|(name, _)| name == "data.db-shm"));

    // Injected repair: pre-create a non-empty directory at the data.db-shm
    // live-pre-swap destination so the swap fails after data.db moved, then
    // run the REAL repair.
    let repair = |name: String, request: right_db::RepairRequest| async move {
        let blocker = request
            .backups_dir
            .join(format!("wal-recovery-{}", request.timestamp))
            .join("live-pre-swap")
            .join("data.db-shm");
        std::fs::create_dir_all(&blocker).unwrap();
        std::fs::write(blocker.join("occupant"), b"x").unwrap();
        drop(name);
        right_db::repair_legacy_wal(request).await
    };

    let err = super::run_db_repair_with(
        home.path(),
        &["riskoff".to_string()],
        Duration::from_secs(30),
        repair,
    )
    .await
    .expect_err("mid-swap failure must fail the invocation");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("rollback") || msg.contains("restored"),
        "error must carry rollback context: {msg}"
    );
    assert!(
        server.stop_was_requested(),
        "quiescence must be established before the repair"
    );
    assert_eq!(
        snapshot_artifacts(&agent_dir),
        snapshot,
        "the complete original set must be restored byte-identically"
    );
    server.shutdown().await;
}
