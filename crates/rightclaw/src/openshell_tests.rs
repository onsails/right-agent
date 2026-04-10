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

// ---------------------------------------------------------------------------
// Mock gRPC server for is_sandbox_ready / wait_for_ready tests
// ---------------------------------------------------------------------------

use crate::openshell_proto::openshell::v1 as proto;
use crate::openshell_proto::openshell::v1::open_shell_server::{self, OpenShellServer};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// Minimal mock — only `get_sandbox` is meaningful; all other RPCs return Unimplemented.
///
/// `get_sandbox_phase` controls the sandbox phase returned.
/// Set to -1 to return `NotFound` instead of a sandbox.
struct MockOpenShell {
    get_sandbox_phase: Arc<AtomicI32>,
}

impl MockOpenShell {
    fn not_found() -> Self {
        Self { get_sandbox_phase: Arc::new(AtomicI32::new(-1)) }
    }

    fn with_phase(phase: i32) -> Self {
        Self { get_sandbox_phase: Arc::new(AtomicI32::new(phase)) }
    }

    /// Create mock with a shared phase handle for external mutation during tests.
    fn with_shared_phase(phase: Arc<AtomicI32>) -> Self {
        Self { get_sandbox_phase: phase }
    }
}

// Streaming type stubs — never used, but the trait requires them.
type EmptyExecStream = tokio_stream::wrappers::ReceiverStream<
    Result<proto::ExecSandboxEvent, tonic::Status>,
>;
type EmptyWatchStream = tokio_stream::wrappers::ReceiverStream<
    Result<proto::SandboxStreamEvent, tonic::Status>,
>;

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
            sandbox: Some(crate::openshell_proto::openshell::datamodel::v1::Sandbox {
                phase,
                ..Default::default()
            }),
        }))
    }

    // --- Stubs (all return Unimplemented) ---

    async fn health(&self, _: tonic::Request<proto::HealthRequest>) -> Result<tonic::Response<proto::HealthResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn create_sandbox(&self, _: tonic::Request<proto::CreateSandboxRequest>) -> Result<tonic::Response<proto::SandboxResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn list_sandboxes(&self, _: tonic::Request<proto::ListSandboxesRequest>) -> Result<tonic::Response<proto::ListSandboxesResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn delete_sandbox(&self, _: tonic::Request<proto::DeleteSandboxRequest>) -> Result<tonic::Response<proto::DeleteSandboxResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn create_ssh_session(&self, _: tonic::Request<proto::CreateSshSessionRequest>) -> Result<tonic::Response<proto::CreateSshSessionResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn revoke_ssh_session(&self, _: tonic::Request<proto::RevokeSshSessionRequest>) -> Result<tonic::Response<proto::RevokeSshSessionResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    type ExecSandboxStream = EmptyExecStream;
    async fn exec_sandbox(&self, _: tonic::Request<proto::ExecSandboxRequest>) -> Result<tonic::Response<Self::ExecSandboxStream>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    async fn create_provider(&self, _: tonic::Request<proto::CreateProviderRequest>) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_provider(&self, _: tonic::Request<proto::GetProviderRequest>) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn list_providers(&self, _: tonic::Request<proto::ListProvidersRequest>) -> Result<tonic::Response<proto::ListProvidersResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn update_provider(&self, _: tonic::Request<proto::UpdateProviderRequest>) -> Result<tonic::Response<proto::ProviderResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn delete_provider(&self, _: tonic::Request<proto::DeleteProviderRequest>) -> Result<tonic::Response<proto::DeleteProviderResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    async fn get_sandbox_config(&self, _: tonic::Request<crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigRequest>) -> Result<tonic::Response<crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_gateway_config(&self, _: tonic::Request<crate::openshell_proto::openshell::sandbox::v1::GetGatewayConfigRequest>) -> Result<tonic::Response<crate::openshell_proto::openshell::sandbox::v1::GetGatewayConfigResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    async fn update_config(&self, _: tonic::Request<proto::UpdateConfigRequest>) -> Result<tonic::Response<proto::UpdateConfigResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_sandbox_policy_status(&self, _: tonic::Request<proto::GetSandboxPolicyStatusRequest>) -> Result<tonic::Response<proto::GetSandboxPolicyStatusResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn list_sandbox_policies(&self, _: tonic::Request<proto::ListSandboxPoliciesRequest>) -> Result<tonic::Response<proto::ListSandboxPoliciesResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn report_policy_status(&self, _: tonic::Request<proto::ReportPolicyStatusRequest>) -> Result<tonic::Response<proto::ReportPolicyStatusResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_sandbox_provider_environment(&self, _: tonic::Request<proto::GetSandboxProviderEnvironmentRequest>) -> Result<tonic::Response<proto::GetSandboxProviderEnvironmentResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_sandbox_logs(&self, _: tonic::Request<proto::GetSandboxLogsRequest>) -> Result<tonic::Response<proto::GetSandboxLogsResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn push_sandbox_logs(&self, _: tonic::Request<tonic::Streaming<proto::PushSandboxLogsRequest>>) -> Result<tonic::Response<proto::PushSandboxLogsResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    type WatchSandboxStream = EmptyWatchStream;
    async fn watch_sandbox(&self, _: tonic::Request<proto::WatchSandboxRequest>) -> Result<tonic::Response<Self::WatchSandboxStream>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }

    async fn submit_policy_analysis(&self, _: tonic::Request<proto::SubmitPolicyAnalysisRequest>) -> Result<tonic::Response<proto::SubmitPolicyAnalysisResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_draft_policy(&self, _: tonic::Request<proto::GetDraftPolicyRequest>) -> Result<tonic::Response<proto::GetDraftPolicyResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn approve_draft_chunk(&self, _: tonic::Request<proto::ApproveDraftChunkRequest>) -> Result<tonic::Response<proto::ApproveDraftChunkResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn reject_draft_chunk(&self, _: tonic::Request<proto::RejectDraftChunkRequest>) -> Result<tonic::Response<proto::RejectDraftChunkResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn approve_all_draft_chunks(&self, _: tonic::Request<proto::ApproveAllDraftChunksRequest>) -> Result<tonic::Response<proto::ApproveAllDraftChunksResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn edit_draft_chunk(&self, _: tonic::Request<proto::EditDraftChunkRequest>) -> Result<tonic::Response<proto::EditDraftChunkResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn undo_draft_chunk(&self, _: tonic::Request<proto::UndoDraftChunkRequest>) -> Result<tonic::Response<proto::UndoDraftChunkResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn clear_draft_chunks(&self, _: tonic::Request<proto::ClearDraftChunksRequest>) -> Result<tonic::Response<proto::ClearDraftChunksResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
    async fn get_draft_history(&self, _: tonic::Request<proto::GetDraftHistoryRequest>) -> Result<tonic::Response<proto::GetDraftHistoryResponse>, tonic::Status> { Err(tonic::Status::unimplemented("stub")) }
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
                async { let _ = rx.await; },
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
    assert!(msg.contains("did not become READY"), "unexpected error: {msg}");
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

    assert!(super::sandbox_exists(&mut client, "creating-sandbox").await.unwrap());
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
    let (addr, _shutdown) = start_mock_server(MockOpenShell::with_shared_phase(Arc::clone(&phase))).await;
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
    assert!(result.is_ok(), "expected Ok after sandbox disappears, got: {result:?}");
}

// ---------------------------------------------------------------------------
// Live sandbox integration tests (require running OpenShell + rightclaw-right)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires live OpenShell sandbox 'rightclaw-right'"]
async fn exec_in_sandbox_runs_command() {
    let mtls_dir = match super::preflight_check() {
        super::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    let sandbox_id = super::resolve_sandbox_id(&mut client, "rightclaw-right")
        .await
        .unwrap();

    let (stdout, exit_code) = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["echo", "hello-from-test"],
    )
    .await
    .unwrap();

    assert_eq!(exit_code, 0, "echo should exit 0");
    assert!(
        stdout.contains("hello-from-test"),
        "expected 'hello-from-test' in stdout, got: {stdout:?}"
    );
}

#[tokio::test]
#[ignore = "requires live OpenShell sandbox 'rightclaw-right'"]
async fn exec_in_sandbox_returns_exit_code() {
    let mtls_dir = match super::preflight_check() {
        super::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    let sandbox_id = super::resolve_sandbox_id(&mut client, "rightclaw-right")
        .await
        .unwrap();

    let (_, exit_code) = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["sh", "-c", "exit 42"],
    )
    .await
    .unwrap();

    assert_eq!(exit_code, 42, "should propagate remote exit code");
}

#[tokio::test]
#[ignore = "requires live OpenShell sandbox 'rightclaw-right'"]
async fn verify_sandbox_files_detects_missing_and_reuploads() {
    // Create a temp dir with a test file.
    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("VERIFY_TEST.md"), "# verify test\n").unwrap();

    // First: ensure VERIFY_TEST.md does NOT exist in sandbox.
    let mtls_dir = match super::preflight_check() {
        super::OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    let sandbox_id = super::resolve_sandbox_id(&mut client, "rightclaw-right")
        .await
        .unwrap();
    let _ = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["rm", "-f", "/sandbox/VERIFY_TEST.md"],
    )
    .await;

    // verify_sandbox_files should detect missing file and re-upload it.
    super::verify_sandbox_files(
        "rightclaw-right",
        host_dir,
        "/sandbox/",
        &["VERIFY_TEST.md"],
    )
    .await
    .expect("verify should succeed after re-upload");

    // Confirm file actually exists in sandbox now.
    let (output, _) = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["cat", "/sandbox/VERIFY_TEST.md"],
    )
    .await
    .unwrap();
    assert_eq!(output, "# verify test\n", "file content should match");

    // Cleanup.
    let _ = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["rm", "-f", "/sandbox/VERIFY_TEST.md"],
    )
    .await;
}

/// Reproduces the exact flow of `rightclaw init`:
/// create sandbox → immediately exec_in_sandbox.
///
/// This is the scenario where gRPC reports READY but SSH transport
/// may not be up yet, causing "Connection reset by peer".
#[tokio::test]
async fn exec_immediately_after_sandbox_create_reproduces_init_flow() {
    // ensure_sandbox takes agent name and prepends "rightclaw-" via sandbox_name().
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

    // Realistic policy matching what `rightclaw init` generates (restrictive mode).
    // The network_policies with TLS termination and proxy setup is what makes
    // SSH transport take significantly longer to become ready.
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    let policy = crate::codegen::policy::generate_policy(18927, &crate::agent::types::NetworkPolicy::Restrictive, None);
    std::fs::write(&policy_path, &policy).unwrap();

    // Create a staging dir with a small test file (same as init uploads agent defs).
    let staging = tmp.path().join("staging");
    std::fs::create_dir_all(staging.join(".claude/agents")).unwrap();
    std::fs::write(
        staging.join(".claude/agents/test.md"),
        "# test agent def\n",
    )
    .unwrap();

    // Create sandbox — returns when gRPC says READY.
    super::ensure_sandbox(AGENT, &policy_path, Some(&staging), false)
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
    )
    .await;

    // Cleanup sandbox regardless of test outcome.
    super::delete_sandbox(&sandbox).await;

    // Assert AFTER cleanup so we don't leave orphan sandboxes.
    let (stdout, exit_code) = result.expect(
        "exec_in_sandbox should succeed immediately after sandbox create — \
         if this fails with 'Connection reset by peer', ensure_sandbox returns \
         before SSH transport is ready"
    );
    assert_eq!(exit_code, 0);
    assert!(
        stdout.contains("hello-after-create"),
        "expected 'hello-after-create' in stdout, got: {stdout:?}"
    );
}

#[tokio::test]
#[ignore = "requires live OpenShell sandbox 'rightclaw-right'"]
async fn verify_sandbox_files_passes_when_all_present() {
    // Upload a file first, then verify it passes.
    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("PRESENT_TEST.md"), "exists\n").unwrap();

    super::upload_file("rightclaw-right", &host_dir.join("PRESENT_TEST.md"), "/sandbox/")
        .await
        .unwrap();

    super::verify_sandbox_files(
        "rightclaw-right",
        host_dir,
        "/sandbox/",
        &["PRESENT_TEST.md"],
    )
    .await
    .expect("verify should pass when file exists");

    // Cleanup.
    let mtls_dir = match super::preflight_check() {
        super::OpenShellStatus::Ready(dir) => dir,
        _ => return,
    };
    let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
    let sandbox_id = super::resolve_sandbox_id(&mut client, "rightclaw-right")
        .await
        .unwrap();
    let _ = super::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &["rm", "-f", "/sandbox/PRESENT_TEST.md"],
    )
    .await;
}
