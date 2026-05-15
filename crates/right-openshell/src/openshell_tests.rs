use super::*;

#[test]
fn sandbox_name_prefixes_agent_name() {
    assert_eq!(sandbox_name("brain"), "rightclaw-brain");
    assert_eq!(sandbox_name("worker-1"), "rightclaw-worker-1");
}

#[test]
fn ssh_host_prefixes_sandbox_name() {
    assert_eq!(ssh_host("brain"), "openshell-rightclaw-brain");
    assert_eq!(ssh_host("worker-1"), "openshell-rightclaw-worker-1");
}

#[test]
fn resolve_sandbox_name_with_explicit_name() {
    assert_eq!(
        resolve_sandbox_name("brain", Some("rightclaw-brain-20260415-1430")),
        "rightclaw-brain-20260415-1430"
    );
}

#[test]
fn resolve_sandbox_name_falls_back_to_deterministic() {
    assert_eq!(resolve_sandbox_name("brain", None), "rightclaw-brain");
}

#[test]
fn ssh_host_for_sandbox_formats_correctly() {
    assert_eq!(
        ssh_host_for_sandbox("rightclaw-brain-20260415"),
        "openshell-rightclaw-brain-20260415"
    );
}

#[test]
fn sandbox_tar_download_args_reads_sandbox_dir_and_preserves_archive_root() {
    assert_eq!(
        sandbox_tar_download_args("sandbox").unwrap(),
        vec![
            "tar",
            "czpf",
            "-",
            "-C",
            "/sandbox",
            "--transform=s,^\\.$,sandbox,",
            "--transform=s,^\\./,sandbox/,",
            ".",
        ]
    );
}

#[test]
fn ensure_download_parent_creates_missing_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let host_dest = tmp.path().join("a/b/c/file.txt");

    ensure_download_parent(&host_dest).unwrap();

    assert!(host_dest.parent().unwrap().is_dir());
    assert!(!host_dest.exists());
}

#[test]
fn remove_stale_directory_at_dest_removes_directory_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let host_dest = tmp.path().join("collision.txt");
    std::fs::create_dir(&host_dest).unwrap();
    std::fs::write(host_dest.join("collision.txt"), "old junk").unwrap();

    remove_stale_directory_at_dest(&host_dest).unwrap();

    assert!(!host_dest.exists());
}

#[test]
fn move_downloaded_file_into_place_overwrites_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let staged = tmp.path().join("downloaded.txt");
    let host_dest = tmp.path().join("existing.txt");
    std::fs::write(&staged, "new\n").unwrap();
    std::fs::write(&host_dest, "stale\n").unwrap();

    move_downloaded_file_into_place(&staged, &host_dest).unwrap();

    assert_eq!(std::fs::read_to_string(&host_dest).unwrap(), "new\n");
    assert!(!staged.exists());
}

#[test]
fn move_downloaded_file_into_place_replaces_stale_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let staged = tmp.path().join("downloaded.txt");
    let host_dest = tmp.path().join("collision.txt");
    std::fs::write(&staged, "fresh\n").unwrap();
    std::fs::create_dir(&host_dest).unwrap();
    std::fs::write(host_dest.join("collision.txt"), "old junk").unwrap();

    move_downloaded_file_into_place(&staged, &host_dest).unwrap();

    assert!(host_dest.is_file());
    assert_eq!(std::fs::read_to_string(&host_dest).unwrap(), "fresh\n");
    assert!(!staged.exists());
}

#[test]
fn control_master_socket_path_uses_sandbox_name() {
    use std::path::Path;
    let dir = Path::new("/tmp/foo/run/ssh");
    assert_eq!(
        control_master_socket_path(dir, "rightclaw-brain-20260415-1430"),
        Path::new("/tmp/foo/run/ssh/rightclaw-brain-20260415-1430.cm"),
    );
}

#[test]
fn gateway_endpoint_from_status_output_reads_active_server_url() {
    let output = "\
\u{1b}[1m\u{1b}[36mServer Status\u{1b}[39m\u{1b}[0m

  \u{1b}[2mGateway:\u{1b}[0m openshell
  \u{1b}[2mServer:\u{1b}[0m https://127.0.0.1:17670
  \u{1b}[2mStatus:\u{1b}[0m \u{1b}[32mConnected\u{1b}[39m
";

    assert_eq!(
        gateway_endpoint_from_status_output(output),
        Some("https://127.0.0.1:17670".to_owned())
    );
}

// ---------------------------------------------------------------------------
// Mock gRPC server for is_sandbox_ready / wait_for_ready tests
// ---------------------------------------------------------------------------

use crate::openshell_proto::openshell::v1 as proto;
use crate::openshell_proto::openshell::v1::open_shell_server::{self, OpenShellServer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::openshell_proto::openshell::datamodel::v1::{Sandbox, SandboxCondition, SandboxStatus};
use crate::test_support::TestSandbox;

/// Shared sandbox for upload / download / verify tests that need a generic
/// working sandbox but don't care about its initial state. Booted once per
/// test process via `tokio::sync::OnceCell`. Each test must use a distinct
/// sandbox-side path to avoid stepping on its peers; all current users do.
///
/// Replaces N × ~5–7s sandbox boots with a single boot, dropping the
/// upload/download/verify suite from ~50s to ~10s. The shared sandbox
/// persists at process exit (statics never drop) — the next run cleans
/// the leftover at `TestSandbox::create("shared").await` time.
async fn shared_test_sandbox() -> &'static TestSandbox {
    use tokio::sync::OnceCell;
    struct Shared {
        sandbox: TestSandbox,
    }
    static SHARED: OnceCell<Shared> = OnceCell::const_new();
    let shared = SHARED
        .get_or_init(|| async {
            let sandbox = TestSandbox::create("shared").await;
            Shared { sandbox }
        })
        .await;
    &shared.sandbox
}

/// Minimal mock — only `get_sandbox` is meaningful; all other RPCs return Unimplemented.
///
/// `get_sandbox_phase` controls the sandbox phase returned.
/// Set to -1 to return `NotFound` instead of a sandbox.
struct MockOpenShell {
    get_sandbox_phase: Arc<AtomicI32>,
    get_sandbox_status: Option<SandboxStatus>,
}

impl MockOpenShell {
    fn not_found() -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(-1)),
            get_sandbox_status: None,
        }
    }

    fn with_phase(phase: i32) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            get_sandbox_status: None,
        }
    }

    fn with_phase_and_status(phase: i32, status: SandboxStatus) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            get_sandbox_status: Some(status),
        }
    }

    /// Create mock with a shared phase handle for external mutation during tests.
    fn with_shared_phase(phase: Arc<AtomicI32>) -> Self {
        Self {
            get_sandbox_phase: phase,
            get_sandbox_status: None,
        }
    }
}

// Streaming type stubs — never used, but the trait requires them.
type EmptyExecStream =
    tokio_stream::wrappers::ReceiverStream<Result<proto::ExecSandboxEvent, tonic::Status>>;
type EmptyWatchStream =
    tokio_stream::wrappers::ReceiverStream<Result<proto::SandboxStreamEvent, tonic::Status>>;

#[tonic::async_trait]
impl open_shell_server::OpenShell for MockOpenShell {
    // --- The method under test ---
    async fn get_sandbox(
        &self,
        _req: tonic::Request<proto::GetSandboxRequest>,
    ) -> Result<tonic::Response<proto::SandboxResponse>, tonic::Status> {
        let phase = self.get_sandbox_phase.load(Ordering::Relaxed);
        if phase < 0 {
            return Err(tonic::Status::not_found("sandbox not found"));
        }
        Ok(tonic::Response::new(proto::SandboxResponse {
            sandbox: Some(Sandbox {
                phase,
                status: self.get_sandbox_status.clone(),
                ..Default::default()
            }),
        }))
    }

    // --- Stubs (all return Unimplemented) ---

    async fn health(
        &self,
        _: tonic::Request<proto::HealthRequest>,
    ) -> Result<tonic::Response<proto::HealthResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn create_sandbox(
        &self,
        _: tonic::Request<proto::CreateSandboxRequest>,
    ) -> Result<tonic::Response<proto::SandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn list_sandboxes(
        &self,
        _: tonic::Request<proto::ListSandboxesRequest>,
    ) -> Result<tonic::Response<proto::ListSandboxesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn delete_sandbox(
        &self,
        _: tonic::Request<proto::DeleteSandboxRequest>,
    ) -> Result<tonic::Response<proto::DeleteSandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn create_ssh_session(
        &self,
        _: tonic::Request<proto::CreateSshSessionRequest>,
    ) -> Result<tonic::Response<proto::CreateSshSessionResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn revoke_ssh_session(
        &self,
        _: tonic::Request<proto::RevokeSshSessionRequest>,
    ) -> Result<tonic::Response<proto::RevokeSshSessionResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type ExecSandboxStream = EmptyExecStream;
    async fn exec_sandbox(
        &self,
        _: tonic::Request<proto::ExecSandboxRequest>,
    ) -> Result<tonic::Response<Self::ExecSandboxStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn create_provider(
        &self,
        _: tonic::Request<proto::CreateProviderRequest>,
    ) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_provider(
        &self,
        _: tonic::Request<proto::GetProviderRequest>,
    ) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn list_providers(
        &self,
        _: tonic::Request<proto::ListProvidersRequest>,
    ) -> Result<tonic::Response<proto::ListProvidersResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn update_provider(
        &self,
        _: tonic::Request<proto::UpdateProviderRequest>,
    ) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn delete_provider(
        &self,
        _: tonic::Request<proto::DeleteProviderRequest>,
    ) -> Result<tonic::Response<proto::DeleteProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_sandbox_config(
        &self,
        _: tonic::Request<crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigRequest>,
    ) -> Result<
        tonic::Response<crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigResponse>,
        tonic::Status,
    > {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_gateway_config(
        &self,
        _: tonic::Request<crate::openshell_proto::openshell::sandbox::v1::GetGatewayConfigRequest>,
    ) -> Result<
        tonic::Response<crate::openshell_proto::openshell::sandbox::v1::GetGatewayConfigResponse>,
        tonic::Status,
    > {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn update_config(
        &self,
        _: tonic::Request<proto::UpdateConfigRequest>,
    ) -> Result<tonic::Response<proto::UpdateConfigResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_sandbox_policy_status(
        &self,
        _: tonic::Request<proto::GetSandboxPolicyStatusRequest>,
    ) -> Result<tonic::Response<proto::GetSandboxPolicyStatusResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn list_sandbox_policies(
        &self,
        _: tonic::Request<proto::ListSandboxPoliciesRequest>,
    ) -> Result<tonic::Response<proto::ListSandboxPoliciesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn report_policy_status(
        &self,
        _: tonic::Request<proto::ReportPolicyStatusRequest>,
    ) -> Result<tonic::Response<proto::ReportPolicyStatusResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_sandbox_provider_environment(
        &self,
        _: tonic::Request<proto::GetSandboxProviderEnvironmentRequest>,
    ) -> Result<tonic::Response<proto::GetSandboxProviderEnvironmentResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_sandbox_logs(
        &self,
        _: tonic::Request<proto::GetSandboxLogsRequest>,
    ) -> Result<tonic::Response<proto::GetSandboxLogsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn push_sandbox_logs(
        &self,
        _: tonic::Request<tonic::Streaming<proto::PushSandboxLogsRequest>>,
    ) -> Result<tonic::Response<proto::PushSandboxLogsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type WatchSandboxStream = EmptyWatchStream;
    async fn watch_sandbox(
        &self,
        _: tonic::Request<proto::WatchSandboxRequest>,
    ) -> Result<tonic::Response<Self::WatchSandboxStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn submit_policy_analysis(
        &self,
        _: tonic::Request<proto::SubmitPolicyAnalysisRequest>,
    ) -> Result<tonic::Response<proto::SubmitPolicyAnalysisResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_draft_policy(
        &self,
        _: tonic::Request<proto::GetDraftPolicyRequest>,
    ) -> Result<tonic::Response<proto::GetDraftPolicyResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn approve_draft_chunk(
        &self,
        _: tonic::Request<proto::ApproveDraftChunkRequest>,
    ) -> Result<tonic::Response<proto::ApproveDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn reject_draft_chunk(
        &self,
        _: tonic::Request<proto::RejectDraftChunkRequest>,
    ) -> Result<tonic::Response<proto::RejectDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn approve_all_draft_chunks(
        &self,
        _: tonic::Request<proto::ApproveAllDraftChunksRequest>,
    ) -> Result<tonic::Response<proto::ApproveAllDraftChunksResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn edit_draft_chunk(
        &self,
        _: tonic::Request<proto::EditDraftChunkRequest>,
    ) -> Result<tonic::Response<proto::EditDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn undo_draft_chunk(
        &self,
        _: tonic::Request<proto::UndoDraftChunkRequest>,
    ) -> Result<tonic::Response<proto::UndoDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn clear_draft_chunks(
        &self,
        _: tonic::Request<proto::ClearDraftChunksRequest>,
    ) -> Result<tonic::Response<proto::ClearDraftChunksResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
    async fn get_draft_history(
        &self,
        _: tonic::Request<proto::GetDraftHistoryRequest>,
    ) -> Result<tonic::Response<proto::GetDraftHistoryResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
}

/// Spin up mock server, return (address, shutdown_sender).
async fn start_mock_server(mock: MockOpenShell) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(OpenShellServer::new(mock))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = rx.await;
                },
            )
            .await
            .unwrap();
    });

    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, tx)
}

/// Connect a plain (non-TLS) client to the mock server.
async fn mock_client(addr: SocketAddr) -> OpenShellClient<Channel> {
    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    OpenShellClient::new(channel)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn is_sandbox_ready_returns_false_on_not_found() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::not_found()).await;
    let mut client = mock_client(addr).await;

    let result = is_sandbox_ready(&mut client, "nonexistent").await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
    assert!(!result.unwrap(), "NotFound should map to Ok(false)");
}

#[tokio::test]
async fn is_sandbox_ready_returns_false_when_not_ready() {
    // Phase 1 = Creating (not READY=2)
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_phase(1)).await;
    let mut client = mock_client(addr).await;

    let result = is_sandbox_ready(&mut client, "test").await;
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[tokio::test]
async fn is_sandbox_ready_returns_true_when_ready() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_phase(SANDBOX_PHASE_READY)).await;
    let mut client = mock_client(addr).await;

    let result = is_sandbox_ready(&mut client, "test").await;
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[tokio::test]
async fn wait_for_ready_succeeds_when_already_ready() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_phase(SANDBOX_PHASE_READY)).await;
    let mut client = mock_client(addr).await;

    let result = wait_for_ready(&mut client, "test", 5, 1).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn wait_for_ready_times_out_when_not_found() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::not_found()).await;
    let mut client = mock_client(addr).await;

    // Short timeout so test doesn't hang.
    let result = wait_for_ready(&mut client, "ghost", 2, 1).await;
    assert!(result.is_err(), "should timeout");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("did not become READY"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn wait_for_ready_timeout_reports_last_sandbox_status() {
    let status = SandboxStatus {
        sandbox_name: "right-test-stuck".to_owned(),
        agent_pod: "right-test-stuck-agent-0".to_owned(),
        conditions: vec![SandboxCondition {
            r#type: "ContainersReady".to_owned(),
            status: "False".to_owned(),
            reason: "ContainersNotReady".to_owned(),
            message: "containers with unready status: [agent]".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let (addr, _shutdown) =
        start_mock_server(MockOpenShell::with_phase_and_status(1, status)).await;
    let mut client = mock_client(addr).await;

    let result = wait_for_ready(&mut client, "right-test-stuck", 0, 1).await;
    assert!(result.is_err(), "should timeout");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("phase=PROVISIONING"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("agent_pod=right-test-stuck-agent-0"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("ContainersReady=False"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("ContainersNotReady"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn wait_for_ready_fails_immediately_on_error_phase() {
    let status = SandboxStatus {
        conditions: vec![SandboxCondition {
            r#type: "Ready".to_owned(),
            status: "False".to_owned(),
            reason: "PodFailed".to_owned(),
            message: "sandbox pod failed before SSH was available".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let (addr, _shutdown) =
        start_mock_server(MockOpenShell::with_phase_and_status(3, status)).await;
    let mut client = mock_client(addr).await;

    let result = wait_for_ready(&mut client, "right-test-error", 30, 1).await;
    assert!(result.is_err(), "ERROR phase should not wait for timeout");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("entered ERROR while waiting for READY"),
        "unexpected error: {msg}"
    );
    assert!(msg.contains("PodFailed"), "unexpected error: {msg}");
}

// ---------------------------------------------------------------------------
// sandbox_exists / wait_for_deleted tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sandbox_exists_returns_false_on_not_found() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::not_found()).await;
    let mut client = mock_client(addr).await;

    assert!(!super::sandbox_exists(&mut client, "ghost").await.unwrap());
}

#[tokio::test]
async fn sandbox_exists_returns_true_for_any_phase() {
    // Phase 1 = Creating (not READY), but sandbox exists.
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_phase(1)).await;
    let mut client = mock_client(addr).await;

    assert!(
        super::sandbox_exists(&mut client, "creating-sandbox")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn wait_for_deleted_returns_immediately_when_not_found() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::not_found()).await;
    let mut client = mock_client(addr).await;

    let result = super::wait_for_deleted(&mut client, "gone", 5, 1).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn wait_for_deleted_times_out_when_sandbox_persists() {
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_phase(1)).await;
    let mut client = mock_client(addr).await;

    let result = super::wait_for_deleted(&mut client, "stubborn", 2, 1).await;
    assert!(result.is_err(), "should timeout");
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("was not deleted"), "unexpected error: {msg}");
}

#[tokio::test]
async fn wait_for_deleted_succeeds_when_sandbox_disappears() {
    let phase = Arc::new(AtomicI32::new(1)); // starts as existing
    let (addr, _shutdown) =
        start_mock_server(MockOpenShell::with_shared_phase(Arc::clone(&phase))).await;
    let mut client = mock_client(addr).await;

    // Flip to NotFound after a short delay.
    tokio::spawn({
        let phase = Arc::clone(&phase);
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            phase.store(-1, Ordering::Relaxed);
        }
    });

    let result = super::wait_for_deleted(&mut client, "disappearing", 5, 1).await;
    assert!(
        result.is_ok(),
        "expected Ok after sandbox disappears, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Live sandbox integration tests (require running OpenShell)
// ---------------------------------------------------------------------------

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_verify_sandbox_files_detects_missing_and_reuploads() {
    let sbox = shared_test_sandbox().await;

    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("VERIFY_TEST.md"), "# verify test\n").unwrap();

    // Ensure file does NOT exist in sandbox.
    sbox.exec(&["rm", "-f", "/sandbox/VERIFY_TEST.md"]).await;

    // verify_sandbox_files should detect missing file and re-upload it.
    super::verify_sandbox_files(sbox.name(), host_dir, "/sandbox/", &["VERIFY_TEST.md"])
        .await
        .expect("verify should succeed after re-upload");

    // Confirm file actually exists in sandbox now.
    let (output, _) = sbox.exec(&["cat", "/sandbox/VERIFY_TEST.md"]).await;
    assert_eq!(output, "# verify test\n", "file content should match");
}

/// Reproduces the exact flow of `right init`:
/// create sandbox → immediately exec_in_sandbox.
///
/// This is the scenario where gRPC reports READY but SSH transport
/// may not be up yet, causing "Connection reset by peer".
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_exec_immediately_after_sandbox_create_reproduces_init_flow() {
    let _slot = super::acquire_sandbox_slot();
    // ensure_sandbox takes the explicit sandbox name. Use the same legacy
    // `sandbox_name()` helper here so the test asserts against a stable prefix
    // even if the production `right init` flow chooses a different one.
    const AGENT: &str = "test-lifecycle";
    let sandbox = super::sandbox_name(AGENT);

    let mtls_dir = match super::preflight_check() {
        super::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    // Cleanup from any previous failed run — wait for full deletion, not just CLI return.
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    if super::sandbox_exists(&mut client, &sandbox).await.unwrap() {
        super::delete_sandbox(&sandbox).await;
        super::wait_for_deleted(&mut client, &sandbox, 60, 2)
            .await
            .expect("cleanup: sandbox should be deleted");
    }

    // Realistic policy matching what `right init` generates (restrictive mode).
    // The network_policies with TLS termination and proxy setup is what makes
    // SSH transport take significantly longer to become ready.
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    let policy = r#"version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /var/log
  read_write:
    - /dev/null
    - /tmp
    - /sandbox
    - /platform

landlock:
  compatibility: best_effort

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
  anthropic:
    endpoints:
      - host: "*.anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "*.claude.com"
        port: 443
        protocol: rest
        access: full
      - host: "claude.com"
        port: 443
        protocol: rest
        access: full
      - host: "*.claude.ai"
        port: 443
        protocol: rest
        access: full
      - host: "claude.ai"
        port: 443
        protocol: rest
        access: full
      - host: "storage.googleapis.com"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "**"

  right:
    endpoints:
      - host: "host.openshell.internal"
        port: 18927
        allowed_ips:
          - "172.16.0.0/12"
          - "192.168.0.0/16"
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#;
    std::fs::write(&policy_path, policy).unwrap();

    // Create a staging dir with a small test file (same as init uploads agent defs).
    let staging = tmp.path().join("staging");
    std::fs::create_dir_all(staging.join(".claude")).unwrap();
    std::fs::write(staging.join(".claude/settings.json"), "{}").unwrap();

    // Create sandbox — returns when gRPC says READY.
    super::ensure_sandbox(&sandbox, &policy_path, Some(&staging), false)
        .await
        .expect("sandbox creation should succeed");

    // Immediately try exec — this is what init does.
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    let sandbox_id = super::resolve_sandbox_id(&mut client, &sandbox)
        .await
        .unwrap();

    let result = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["echo", "hello-after-create"],
        super::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await;

    // Cleanup sandbox regardless of test outcome.
    super::delete_sandbox(&sandbox).await;

    // Assert AFTER cleanup so we don't leave orphan sandboxes.
    let (stdout, exit_code) = result.expect(
        "exec_in_sandbox should succeed immediately after sandbox create — \
         if this fails with 'Connection reset by peer', ensure_sandbox returns \
         before SSH transport is ready",
    );
    assert_eq!(exit_code, 0);
    assert!(
        stdout.contains("hello-after-create"),
        "expected 'hello-after-create' in stdout, got: {stdout:?}"
    );
}

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_verify_sandbox_files_passes_when_all_present() {
    let sbox = shared_test_sandbox().await;

    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("PRESENT_TEST.md"), "exists\n").unwrap();

    super::upload_file(sbox.name(), &host_dir.join("PRESENT_TEST.md"), "/sandbox/")
        .await
        .unwrap();

    super::verify_sandbox_files(sbox.name(), host_dir, "/sandbox/", &["PRESENT_TEST.md"])
        .await
        .expect("verify should pass when file exists");
}

// ---------------------------------------------------------------------------
// upload_file integration tests
// ---------------------------------------------------------------------------

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_upload_file_to_directory() {
    let sbox = shared_test_sandbox().await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello sandbox\n").unwrap();

    super::upload_file(sbox.name(), &tmp.path().join("hello.txt"), "/sandbox/")
        .await
        .expect("upload to directory should succeed");

    let (content, code) = sbox.exec(&["cat", "/sandbox/hello.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "hello sandbox\n");
}

/// Regression test: upload_file must reject non-directory destination.
/// Before the fix, passing "/sandbox/mcp.json" as dest caused:
///   mkdir: cannot create directory '/sandbox/mcp.json': File exists
#[tokio::test]
async fn upload_file_rejects_non_directory_dest() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mcp.json"), "{}").unwrap();

    let result = super::upload_file(
        "any-sandbox",
        &tmp.path().join("mcp.json"),
        "/sandbox/mcp.json",
    )
    .await;

    assert!(
        result.is_err(),
        "upload_file must reject file-path destination"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("must be a directory"),
        "error should mention directory requirement, got: {msg}"
    );
}

/// Regression test: upload_file with a directory (not a single file) as source.
/// This is how sync.rs uploads builtin skills (e.g. right-cron/, right-mcp/).
/// OpenShell has a known bug where directory uploads silently drop small files.
/// Also tests overwrite: sync runs every 5 min, so repeated uploads must work.
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_upload_directory_preserves_files_and_overwrites() {
    let sbox = shared_test_sandbox().await;

    // Create a directory tree mimicking a skill: right-mcp/SKILL.md
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("right-mcp");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::create_dir_all(skill_dir.join("nested")).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# version 1\n").unwrap();
    std::fs::write(skill_dir.join("nested/TOOL.md"), "nested\n").unwrap();

    // First upload
    super::upload_file(sbox.name(), &skill_dir, "/sandbox/.claude/skills/")
        .await
        .expect("first directory upload should succeed");

    let (content, code) = sbox
        .exec(&["cat", "/sandbox/.claude/skills/right-mcp/SKILL.md"])
        .await;
    assert_eq!(code, 0, "SKILL.md must exist after first upload");
    assert_eq!(content, "# version 1\n");
    let (content, code) = sbox
        .exec(&["cat", "/sandbox/.claude/skills/right-mcp/nested/TOOL.md"])
        .await;
    assert_eq!(code, 0, "nested TOOL.md must exist after first upload");
    assert_eq!(content, "nested\n");

    // Second upload with updated content (simulates sync overwrite)
    std::fs::write(skill_dir.join("SKILL.md"), "# version 2\n").unwrap();
    super::upload_file(sbox.name(), &skill_dir, "/sandbox/.claude/skills/")
        .await
        .expect("second directory upload (overwrite) should succeed");

    let (content, code) = sbox
        .exec(&["cat", "/sandbox/.claude/skills/right-mcp/SKILL.md"])
        .await;
    assert_eq!(code, 0, "SKILL.md must exist after overwrite");
    assert_eq!(content, "# version 2\n", "overwrite must update content");
}

// ---------------------------------------------------------------------------
// download_file integration tests
// ---------------------------------------------------------------------------

/// Regression test for the photo-send bug: `openshell sandbox download` always
/// writes to DEST as a directory. `download_file` must hide that and deliver
/// the file at exactly the caller's `host_dest` path.
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_download_file_writes_to_exact_dest_path() {
    let sbox = shared_test_sandbox().await;

    // Put a known file in the sandbox.
    let (_, code) = sbox
        .exec(&[
            "sh",
            "-c",
            "printf 'payload\\n' > /sandbox/download_test.txt",
        ])
        .await;
    assert_eq!(code, 0, "sandbox write failed");

    let tmp = tempfile::tempdir().unwrap();
    // NOTE: host_dest basename differs from sandbox basename to prove the
    // function honors the dest path, not the source name.
    let host_dest = tmp.path().join("renamed_on_host.txt");

    super::download_file(sbox.name(), "/sandbox/download_test.txt", &host_dest)
        .await
        .expect("download should succeed");

    assert!(
        host_dest.is_file(),
        "host_dest must be a regular file, not a directory"
    );
    let content = std::fs::read_to_string(&host_dest).unwrap();
    assert_eq!(
        content, "payload\n",
        "downloaded content must match sandbox file"
    );
}

// ---------------------------------------------------------------------------
// filesystem_policy_changed tests
// ---------------------------------------------------------------------------

#[test]
fn filesystem_policy_changed_detects_difference() {
    use crate::openshell_proto::openshell::sandbox::v1::{
        FilesystemPolicy, LandlockPolicy, SandboxPolicy,
    };

    let old = SandboxPolicy {
        filesystem: Some(FilesystemPolicy {
            include_workdir: true,
            read_only: vec!["/usr".into(), "/lib".into()],
            read_write: vec!["/sandbox".into(), "/tmp".into()],
        }),
        landlock: Some(LandlockPolicy {
            compatibility: "best_effort".into(),
        }),
        ..Default::default()
    };

    let mut new_policy = old.clone();
    new_policy
        .filesystem
        .as_mut()
        .unwrap()
        .read_write
        .push("/data".into());

    assert!(super::filesystem_policy_changed(&old, &new_policy));
}

#[test]
fn filesystem_policy_unchanged_when_only_network_differs() {
    use crate::openshell_proto::openshell::sandbox::v1::*;

    let old = SandboxPolicy {
        filesystem: Some(FilesystemPolicy {
            include_workdir: true,
            read_only: vec!["/usr".into()],
            read_write: vec!["/sandbox".into()],
        }),
        landlock: Some(LandlockPolicy {
            compatibility: "best_effort".into(),
        }),
        ..Default::default()
    };

    let mut new_policy = old.clone();
    new_policy
        .network_policies
        .insert("test".into(), NetworkPolicyRule::default());

    assert!(!super::filesystem_policy_changed(&old, &new_policy));
}

// ---------------------------------------------------------------------------
// parse_policy_yaml_filesystem tests
// ---------------------------------------------------------------------------

#[test]
fn parse_policy_yaml_extracts_filesystem() {
    let yaml = r#"
version: 1
filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
  read_write:
    - /sandbox
    - /tmp
landlock:
  compatibility: best_effort
network_policies:
  claude_code:
    endpoints:
      - host: "*.anthropic.com"
"#;
    let policy = super::parse_policy_yaml_filesystem(yaml).unwrap();
    let fs = policy.filesystem.unwrap();
    assert!(fs.include_workdir);
    assert_eq!(fs.read_only, vec!["/usr", "/lib"]);
    assert_eq!(fs.read_write, vec!["/sandbox", "/tmp"]);
    let ll = policy.landlock.unwrap();
    assert_eq!(ll.compatibility, "best_effort");
}

#[test]
fn sandbox_slot_limit_defaults_on_absent_or_invalid_env() {
    assert_eq!(
        super::sandbox_slot_limit_from_env_value(None),
        super::DEFAULT_MAX_CONCURRENT_SANDBOX_TESTS
    );
    assert_eq!(
        super::sandbox_slot_limit_from_env_value(Some("0")),
        super::DEFAULT_MAX_CONCURRENT_SANDBOX_TESTS
    );
    assert_eq!(
        super::sandbox_slot_limit_from_env_value(Some("abc")),
        super::DEFAULT_MAX_CONCURRENT_SANDBOX_TESTS
    );
}

#[test]
fn sandbox_slot_limit_uses_positive_env_value() {
    assert_eq!(super::sandbox_slot_limit_from_env_value(Some("1")), 1);
    assert_eq!(super::sandbox_slot_limit_from_env_value(Some("2")), 2);
}

#[test]
fn test_name_lock_acquire_and_release() {
    let lock = super::acquire_test_name_lock("unit-test-acquire-release");
    drop(lock);
    // Re-acquiring after drop must succeed — same process, lock was released.
    let _lock2 = super::acquire_test_name_lock("unit-test-acquire-release");
}

#[test]
fn test_name_lock_blocks_when_held() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    let name = "unit-test-blocks-when-held";
    let held = super::acquire_test_name_lock(name);

    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_clone = Arc::clone(&acquired);
    let handle = thread::spawn(move || {
        let _lock = super::acquire_test_name_lock("unit-test-blocks-when-held");
        acquired_clone.store(true, Ordering::SeqCst);
    });

    // Give the thread time to attempt acquisition; it must still be blocked.
    thread::sleep(Duration::from_millis(500));
    assert!(
        !acquired.load(Ordering::SeqCst),
        "second acquire returned while first lock still held"
    );

    drop(held);
    handle.join().unwrap();
    assert!(
        acquired.load(Ordering::SeqCst),
        "second acquire never completed after first lock released"
    );
}

#[test]
fn test_name_lock_sanitizes_name() {
    // Names with path separators or other unsafe chars must not crash and
    // must round-trip — two distinct unsafe names are still distinct locks.
    let _a = super::acquire_test_name_lock("foo/bar:baz");
    let _b = super::acquire_test_name_lock("foo_bar_baz_other");
}

#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn ci_openshell_test_sandbox_holds_name_lock() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let sandbox = TestSandbox::create("name-lock-holds").await;

    // While `sandbox` is alive, acquire_test_name_lock with the same logical
    // name (the full sandbox name `right-test-name-lock-holds`) MUST block.
    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_clone = Arc::clone(&acquired);
    let handle = std::thread::spawn(move || {
        let _lock = super::acquire_test_name_lock("right-test-name-lock-holds");
        acquired_clone.store(true, Ordering::SeqCst);
    });

    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !acquired.load(Ordering::SeqCst),
        "name lock not held by live TestSandbox"
    );

    drop(sandbox);
    handle.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
}

#[test]
fn control_master_directives_block_content() {
    // Pure-content check: verify the appended block has the expected
    // shape without spawning the openshell CLI. Calls the helper that
    // builds the appended snippet.
    let dir = std::path::Path::new("/var/lib/right/run/ssh");
    let block = control_master_directives(dir, "rightclaw-brain-20260415");
    assert!(
        block.contains("\nControlMaster auto\n"),
        "missing ControlMaster auto: {block}"
    );
    assert!(
        block.contains("\nControlPath /var/lib/right/run/ssh/rightclaw-brain-20260415.cm\n"),
        "missing ControlPath: {block}",
    );
    assert!(
        block.contains("\nControlPersist yes\n"),
        "missing ControlPersist: {block}"
    );
}
