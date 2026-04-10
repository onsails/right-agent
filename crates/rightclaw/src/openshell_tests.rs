use super::*;
use serial_test::serial;

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
// Ephemeral test sandbox — created per test, destroyed explicitly.
// Leftovers from panicked tests are cleaned up by the next create() call.
// ---------------------------------------------------------------------------

struct TestSandbox {
    name: String,
    mtls_dir: PathBuf,
    _tmp: tempfile::TempDir, // keeps policy file alive
}

impl TestSandbox {
    /// Create an ephemeral sandbox for testing. Cleans up any leftover from previous runs.
    async fn create(test_name: &str) -> Self {
        let name = format!("rightclaw-test-{test_name}");

        let mtls_dir = match super::preflight_check() {
            super::OpenShellStatus::Ready(dir) => dir,
            other => panic!("OpenShell not ready: {other:?}"),
        };

        // Clean up leftover from a previous failed run.
        let mut client = super::connect_grpc(&mtls_dir).await.unwrap();
        if super::sandbox_exists(&mut client, &name).await.unwrap() {
            super::delete_sandbox(&name).await;
            super::wait_for_deleted(&mut client, &name, 60, 2)
                .await
                .expect("cleanup of leftover sandbox failed");
        }

        // Minimal policy — fast startup, no restrictive network rules.
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let policy = "\
version: 1
filesystem_policy:
  include_workdir: true
  read_write:
    - /tmp
    - /sandbox
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  outbound:
    endpoints:
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: \"**\"
";
        std::fs::write(&policy_path, policy).unwrap();

        let mut child = super::spawn_sandbox(&name, &policy_path, None)
            .expect("failed to spawn sandbox");
        super::wait_for_ready(&mut client, &name, 120, 2)
            .await
            .expect("sandbox did not become READY");

        // Kill the create process — it doesn't exit on its own after READY.
        let _ = child.kill().await;

        Self { name, mtls_dir, _tmp: tmp }
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Execute a command inside the sandbox, return (stdout, exit_code).
    async fn exec(&self, cmd: &[&str]) -> (String, i32) {
        let mut client = super::connect_grpc(&self.mtls_dir).await.unwrap();
        let id = super::resolve_sandbox_id(&mut client, &self.name)
            .await
            .unwrap();
        super::exec_in_sandbox(&mut client, &id, cmd)
            .await
            .unwrap()
    }

    /// Delete the sandbox and wait for deletion to complete.
    async fn destroy(self) {
        super::delete_sandbox(&self.name).await;
        let mut client = super::connect_grpc(&self.mtls_dir).await.unwrap();
        let _ = super::wait_for_deleted(&mut client, &self.name, 60, 2).await;
    }
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
// Live sandbox integration tests (require running OpenShell)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn exec_in_sandbox_runs_command() {
    let sbox = TestSandbox::create("exec-run").await;
    let (stdout, exit_code) = sbox.exec(&["echo", "hello-from-test"]).await;

    assert_eq!(exit_code, 0, "echo should exit 0");
    assert!(
        stdout.contains("hello-from-test"),
        "expected 'hello-from-test' in stdout, got: {stdout:?}"
    );

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
async fn exec_in_sandbox_returns_exit_code() {
    let sbox = TestSandbox::create("exec-exit").await;
    let (_, exit_code) = sbox.exec(&["sh", "-c", "exit 42"]).await;

    assert_eq!(exit_code, 42, "should propagate remote exit code");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
async fn verify_sandbox_files_detects_missing_and_reuploads() {
    let sbox = TestSandbox::create("verify-missing").await;

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

    sbox.destroy().await;
}

/// Reproduces the exact flow of `rightclaw init`:
/// create sandbox → immediately exec_in_sandbox.
///
/// This is the scenario where gRPC reports READY but SSH transport
/// may not be up yet, causing "Connection reset by peer".
#[tokio::test]
#[serial]
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
#[serial]
async fn verify_sandbox_files_passes_when_all_present() {
    let sbox = TestSandbox::create("verify-present").await;

    let tmp = tempfile::tempdir().unwrap();
    let host_dir = tmp.path();
    std::fs::write(host_dir.join("PRESENT_TEST.md"), "exists\n").unwrap();

    super::upload_file(sbox.name(), &host_dir.join("PRESENT_TEST.md"), "/sandbox/")
        .await
        .unwrap();

    super::verify_sandbox_files(sbox.name(), host_dir, "/sandbox/", &["PRESENT_TEST.md"])
        .await
        .expect("verify should pass when file exists");

    sbox.destroy().await;
}

// ---------------------------------------------------------------------------
// upload_file integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn upload_file_to_directory() {
    let sbox = TestSandbox::create("upload-dir").await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello sandbox\n").unwrap();

    super::upload_file(sbox.name(), &tmp.path().join("hello.txt"), "/sandbox/")
        .await
        .expect("upload to directory should succeed");

    let (content, code) = sbox.exec(&["cat", "/sandbox/hello.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "hello sandbox\n");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
async fn upload_file_overwrites_existing() {
    let sbox = TestSandbox::create("upload-overwrite").await;

    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("data.txt");

    // First upload.
    std::fs::write(&file, "version 1\n").unwrap();
    super::upload_file(sbox.name(), &file, "/sandbox/")
        .await
        .unwrap();

    // Second upload with different content.
    std::fs::write(&file, "version 2\n").unwrap();
    super::upload_file(sbox.name(), &file, "/sandbox/")
        .await
        .unwrap();

    let (content, _) = sbox.exec(&["cat", "/sandbox/data.txt"]).await;
    assert_eq!(content, "version 2\n", "second upload should overwrite");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
async fn upload_file_to_nested_dir() {
    let sbox = TestSandbox::create("upload-nested").await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("nested.txt"), "deep\n").unwrap();

    // Upload to a directory that doesn't exist yet — openshell should create it.
    super::upload_file(sbox.name(), &tmp.path().join("nested.txt"), "/sandbox/a/b/c/")
        .await
        .expect("upload to nested dir should succeed");

    let (content, code) = sbox.exec(&["cat", "/sandbox/a/b/c/nested.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "deep\n");

    sbox.destroy().await;
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

    assert!(result.is_err(), "upload_file must reject file-path destination");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("must be a directory"),
        "error should mention directory requirement, got: {msg}"
    );
}
