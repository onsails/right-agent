//! Shared in-process mock OpenShell gRPC server, used by sibling test
//! modules (`openshell_tests`, `preflight_tests`, `providers_tests`).
//! Plain-HTTP (no TLS) — tests connect via `mock_client(addr)` to skip
//! the mTLS setup required by production `connect_grpc`.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Server};

use crate::openshell_proto::openshell as os_proto;
use os_proto::v1::open_shell_client::OpenShellClient;
use os_proto::v1::open_shell_server::{OpenShell, OpenShellServer};

type ReceiverStream<T> = tokio_stream::wrappers::ReceiverStream<T>;

/// Unary RPC mock: takes a request, returns a response or `tonic::Status`.
type UnaryMockFn<Req, Resp> = Box<dyn Fn(Req) -> Result<Resp, tonic::Status> + Send + Sync>;

/// Zero-arg mock for RPCs with empty/ignored request bodies (e.g. `health`).
type EmptyArgMockFn<Resp> = Box<dyn Fn() -> Result<Resp, tonic::Status> + Send + Sync>;

#[derive(Default)]
pub(crate) struct MockOpenShell {
    pub(crate) get_sandbox_phase: Arc<AtomicI32>,
    pub(crate) get_sandbox_status: Option<os_proto::v1::SandboxStatus>,
    pub(crate) get_sandbox_include_status: bool,

    pub(crate) mock_health: Option<EmptyArgMockFn<os_proto::v1::HealthResponse>>,
    pub(crate) mock_create_provider:
        Option<UnaryMockFn<os_proto::v1::CreateProviderRequest, os_proto::v1::ProviderResponse>>,
    pub(crate) mock_get_provider:
        Option<UnaryMockFn<os_proto::v1::GetProviderRequest, os_proto::v1::ProviderResponse>>,
    pub(crate) mock_update_provider:
        Option<UnaryMockFn<os_proto::v1::UpdateProviderRequest, os_proto::v1::ProviderResponse>>,
    pub(crate) mock_delete_provider: Option<
        UnaryMockFn<os_proto::v1::DeleteProviderRequest, os_proto::v1::DeleteProviderResponse>,
    >,
    pub(crate) mock_list_providers: Option<
        UnaryMockFn<os_proto::v1::ListProvidersRequest, os_proto::v1::ListProvidersResponse>,
    >,
    pub(crate) mock_attach_sandbox_provider: Option<
        UnaryMockFn<
            os_proto::v1::AttachSandboxProviderRequest,
            os_proto::v1::AttachSandboxProviderResponse,
        >,
    >,
    pub(crate) mock_detach_sandbox_provider: Option<
        UnaryMockFn<
            os_proto::v1::DetachSandboxProviderRequest,
            os_proto::v1::DetachSandboxProviderResponse,
        >,
    >,
    pub(crate) mock_list_sandbox_providers: Option<
        UnaryMockFn<
            os_proto::v1::ListSandboxProvidersRequest,
            os_proto::v1::ListSandboxProvidersResponse,
        >,
    >,
    pub(crate) mock_get_sandbox_provider_environment: Option<
        UnaryMockFn<
            os_proto::v1::GetSandboxProviderEnvironmentRequest,
            os_proto::v1::GetSandboxProviderEnvironmentResponse,
        >,
    >,
    pub(crate) mock_update_config:
        Option<UnaryMockFn<os_proto::v1::UpdateConfigRequest, os_proto::v1::UpdateConfigResponse>>,
    pub(crate) mock_get_sandbox_policy_status: Option<
        UnaryMockFn<
            os_proto::v1::GetSandboxPolicyStatusRequest,
            os_proto::v1::GetSandboxPolicyStatusResponse,
        >,
    >,
    pub(crate) mock_get_sandbox_config: Option<
        UnaryMockFn<
            os_proto::sandbox::v1::GetSandboxConfigRequest,
            os_proto::sandbox::v1::GetSandboxConfigResponse,
        >,
    >,
    pub(crate) mock_get_provider_profile: Option<
        UnaryMockFn<os_proto::v1::GetProviderProfileRequest, os_proto::v1::ProviderProfileResponse>,
    >,
    pub(crate) mock_lint_provider_profiles: Option<
        UnaryMockFn<
            os_proto::v1::LintProviderProfilesRequest,
            os_proto::v1::LintProviderProfilesResponse,
        >,
    >,
    pub(crate) mock_import_provider_profiles: Option<
        UnaryMockFn<
            os_proto::v1::ImportProviderProfilesRequest,
            os_proto::v1::ImportProviderProfilesResponse,
        >,
    >,
    pub(crate) mock_delete_provider_profile: Option<
        UnaryMockFn<
            os_proto::v1::DeleteProviderProfileRequest,
            os_proto::v1::DeleteProviderProfileResponse,
        >,
    >,
}

impl MockOpenShell {
    pub(crate) fn not_found() -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(-1)),
            ..Default::default()
        }
    }

    pub(crate) fn with_phase(phase: i32) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            get_sandbox_include_status: true,
            ..Default::default()
        }
    }

    pub(crate) fn with_phase_and_status(phase: i32, status: os_proto::v1::SandboxStatus) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            get_sandbox_status: Some(status),
            get_sandbox_include_status: true,
            ..Default::default()
        }
    }

    pub(crate) fn with_missing_status(phase: i32) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            ..Default::default()
        }
    }

    pub(crate) fn with_shared_phase(phase: Arc<AtomicI32>) -> Self {
        Self {
            get_sandbox_phase: phase,
            get_sandbox_include_status: true,
            ..Default::default()
        }
    }
}

// Streaming type stubs — never used, but the trait requires them.
type EmptyExecStream = ReceiverStream<Result<os_proto::v1::ExecSandboxEvent, tonic::Status>>;
type EmptyWatchStream = ReceiverStream<Result<os_proto::v1::SandboxStreamEvent, tonic::Status>>;
type EmptyForwardTcpStream = ReceiverStream<Result<os_proto::v1::TcpForwardFrame, tonic::Status>>;
type EmptyExecInteractiveStream =
    ReceiverStream<Result<os_proto::v1::ExecSandboxEvent, tonic::Status>>;
type EmptyConnectSupervisorStream =
    ReceiverStream<Result<os_proto::v1::GatewayMessage, tonic::Status>>;
type EmptyRelayStreamStream = ReceiverStream<Result<os_proto::v1::RelayFrame, tonic::Status>>;

#[tonic::async_trait]
impl OpenShell for MockOpenShell {
    // --- get_sandbox: the method most test mock usage exercises ---
    async fn get_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::GetSandboxRequest>,
    ) -> Result<tonic::Response<os_proto::v1::SandboxResponse>, tonic::Status> {
        let phase = self.get_sandbox_phase.load(Ordering::Relaxed);
        if phase < 0 {
            return Err(tonic::Status::not_found("sandbox not found"));
        }
        // OpenShell 0.0.56 carries the gateway-derived lifecycle phase in
        // `SandboxStatus.phase`, not on the top-level `Sandbox` message. Mirror
        // that: fold the configured phase into the status the gateway returns.
        let status = self.get_sandbox_include_status.then(|| {
            let mut status = self.get_sandbox_status.clone().unwrap_or_default();
            status.phase = phase;
            status
        });
        Ok(tonic::Response::new(os_proto::v1::SandboxResponse {
            sandbox: Some(os_proto::v1::Sandbox {
                metadata: Some(os_proto::datamodel::v1::ObjectMeta {
                    id: "mock-sandbox-id".into(),
                    name: "mock-sandbox".into(),
                    ..Default::default()
                }),
                status,
                ..Default::default()
            }),
        }))
    }

    // --- health: configurable via mock_health ---
    async fn health(
        &self,
        _: tonic::Request<os_proto::v1::HealthRequest>,
    ) -> Result<tonic::Response<os_proto::v1::HealthResponse>, tonic::Status> {
        match &self.mock_health {
            Some(f) => f().map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    // --- Provider CRUD: configurable via mock_* fields ---
    async fn create_provider(
        &self,
        req: tonic::Request<os_proto::v1::CreateProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_create_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn get_provider(
        &self,
        req: tonic::Request<os_proto::v1::GetProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_get_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn update_provider(
        &self,
        req: tonic::Request<os_proto::v1::UpdateProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_update_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn delete_provider(
        &self,
        req: tonic::Request<os_proto::v1::DeleteProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteProviderResponse>, tonic::Status> {
        match &self.mock_delete_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn list_providers(
        &self,
        req: tonic::Request<os_proto::v1::ListProvidersRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListProvidersResponse>, tonic::Status> {
        match &self.mock_list_providers {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn attach_sandbox_provider(
        &self,
        req: tonic::Request<os_proto::v1::AttachSandboxProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::AttachSandboxProviderResponse>, tonic::Status> {
        match &self.mock_attach_sandbox_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn detach_sandbox_provider(
        &self,
        req: tonic::Request<os_proto::v1::DetachSandboxProviderRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DetachSandboxProviderResponse>, tonic::Status> {
        match &self.mock_detach_sandbox_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn list_sandbox_providers(
        &self,
        req: tonic::Request<os_proto::v1::ListSandboxProvidersRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListSandboxProvidersResponse>, tonic::Status> {
        match &self.mock_list_sandbox_providers {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn get_sandbox_provider_environment(
        &self,
        req: tonic::Request<os_proto::v1::GetSandboxProviderEnvironmentRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetSandboxProviderEnvironmentResponse>, tonic::Status>
    {
        match &self.mock_get_sandbox_provider_environment {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    // --- Sandbox lifecycle stubs ---
    async fn create_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::CreateSandboxRequest>,
    ) -> Result<tonic::Response<os_proto::v1::SandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn list_sandboxes(
        &self,
        _: tonic::Request<os_proto::v1::ListSandboxesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListSandboxesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn delete_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::DeleteSandboxRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteSandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn create_ssh_session(
        &self,
        _: tonic::Request<os_proto::v1::CreateSshSessionRequest>,
    ) -> Result<tonic::Response<os_proto::v1::CreateSshSessionResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn revoke_ssh_session(
        &self,
        _: tonic::Request<os_proto::v1::RevokeSshSessionRequest>,
    ) -> Result<tonic::Response<os_proto::v1::RevokeSshSessionResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type ExecSandboxStream = EmptyExecStream;
    async fn exec_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::ExecSandboxRequest>,
    ) -> Result<tonic::Response<Self::ExecSandboxStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.50 service endpoint stubs ---
    async fn expose_service(
        &self,
        _: tonic::Request<os_proto::v1::ExposeServiceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ServiceEndpointResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_service(
        &self,
        _: tonic::Request<os_proto::v1::GetServiceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ServiceEndpointResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn list_services(
        &self,
        _: tonic::Request<os_proto::v1::ListServicesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListServicesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn delete_service(
        &self,
        _: tonic::Request<os_proto::v1::DeleteServiceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteServiceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.50 streaming stubs ---
    type ForwardTcpStream = EmptyForwardTcpStream;
    async fn forward_tcp(
        &self,
        _: tonic::Request<tonic::Streaming<os_proto::v1::TcpForwardFrame>>,
    ) -> Result<tonic::Response<Self::ForwardTcpStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type ExecSandboxInteractiveStream = EmptyExecInteractiveStream;
    async fn exec_sandbox_interactive(
        &self,
        _: tonic::Request<tonic::Streaming<os_proto::v1::ExecSandboxInput>>,
    ) -> Result<tonic::Response<Self::ExecSandboxInteractiveStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.50 provider profile stubs ---
    async fn list_provider_profiles(
        &self,
        _: tonic::Request<os_proto::v1::ListProviderProfilesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListProviderProfilesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_provider_profile(
        &self,
        request: tonic::Request<os_proto::v1::GetProviderProfileRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ProviderProfileResponse>, tonic::Status> {
        match &self.mock_get_provider_profile {
            Some(f) => f(request.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn import_provider_profiles(
        &self,
        request: tonic::Request<os_proto::v1::ImportProviderProfilesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ImportProviderProfilesResponse>, tonic::Status> {
        match &self.mock_import_provider_profiles {
            Some(f) => f(request.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn lint_provider_profiles(
        &self,
        request: tonic::Request<os_proto::v1::LintProviderProfilesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::LintProviderProfilesResponse>, tonic::Status> {
        match &self.mock_lint_provider_profiles {
            Some(f) => f(request.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn delete_provider_profile(
        &self,
        request: tonic::Request<os_proto::v1::DeleteProviderProfileRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteProviderProfileResponse>, tonic::Status> {
        match &self.mock_delete_provider_profile {
            Some(f) => f(request.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    // --- New v0.0.50 provider refresh stubs ---
    async fn get_provider_refresh_status(
        &self,
        _: tonic::Request<os_proto::v1::GetProviderRefreshStatusRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetProviderRefreshStatusResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn configure_provider_refresh(
        &self,
        _: tonic::Request<os_proto::v1::ConfigureProviderRefreshRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ConfigureProviderRefreshResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn rotate_provider_credential(
        &self,
        _: tonic::Request<os_proto::v1::RotateProviderCredentialRequest>,
    ) -> Result<tonic::Response<os_proto::v1::RotateProviderCredentialResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn delete_provider_refresh(
        &self,
        _: tonic::Request<os_proto::v1::DeleteProviderRefreshRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteProviderRefreshResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.50 supervisor / relay streaming stubs ---
    type ConnectSupervisorStream = EmptyConnectSupervisorStream;
    async fn connect_supervisor(
        &self,
        _: tonic::Request<tonic::Streaming<os_proto::v1::SupervisorMessage>>,
    ) -> Result<tonic::Response<Self::ConnectSupervisorStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type RelayStreamStream = EmptyRelayStreamStream;
    async fn relay_stream(
        &self,
        _: tonic::Request<tonic::Streaming<os_proto::v1::RelayFrame>>,
    ) -> Result<tonic::Response<Self::RelayStreamStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.50 sandbox token stubs ---
    async fn issue_sandbox_token(
        &self,
        _: tonic::Request<os_proto::v1::IssueSandboxTokenRequest>,
    ) -> Result<tonic::Response<os_proto::v1::IssueSandboxTokenResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn refresh_sandbox_token(
        &self,
        _: tonic::Request<os_proto::v1::RefreshSandboxTokenRequest>,
    ) -> Result<tonic::Response<os_proto::v1::RefreshSandboxTokenResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- Config / policy / logs stubs ---
    async fn get_sandbox_config(
        &self,
        req: tonic::Request<
            crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigRequest,
        >,
    ) -> Result<
        tonic::Response<crate::openshell_proto::openshell::sandbox::v1::GetSandboxConfigResponse>,
        tonic::Status,
    > {
        match &self.mock_get_sandbox_config {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
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
        req: tonic::Request<os_proto::v1::UpdateConfigRequest>,
    ) -> Result<tonic::Response<os_proto::v1::UpdateConfigResponse>, tonic::Status> {
        match &self.mock_update_config {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn get_sandbox_policy_status(
        &self,
        req: tonic::Request<os_proto::v1::GetSandboxPolicyStatusRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetSandboxPolicyStatusResponse>, tonic::Status> {
        match &self.mock_get_sandbox_policy_status {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn list_sandbox_policies(
        &self,
        _: tonic::Request<os_proto::v1::ListSandboxPoliciesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListSandboxPoliciesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn report_policy_status(
        &self,
        _: tonic::Request<os_proto::v1::ReportPolicyStatusRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ReportPolicyStatusResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_sandbox_logs(
        &self,
        _: tonic::Request<os_proto::v1::GetSandboxLogsRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetSandboxLogsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn push_sandbox_logs(
        &self,
        _: tonic::Request<tonic::Streaming<os_proto::v1::PushSandboxLogsRequest>>,
    ) -> Result<tonic::Response<os_proto::v1::PushSandboxLogsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    type WatchSandboxStream = EmptyWatchStream;
    async fn watch_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::WatchSandboxRequest>,
    ) -> Result<tonic::Response<Self::WatchSandboxStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- Policy analysis / draft stubs ---
    async fn submit_policy_analysis(
        &self,
        _: tonic::Request<os_proto::v1::SubmitPolicyAnalysisRequest>,
    ) -> Result<tonic::Response<os_proto::v1::SubmitPolicyAnalysisResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_draft_policy(
        &self,
        _: tonic::Request<os_proto::v1::GetDraftPolicyRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetDraftPolicyResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn approve_draft_chunk(
        &self,
        _: tonic::Request<os_proto::v1::ApproveDraftChunkRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ApproveDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn reject_draft_chunk(
        &self,
        _: tonic::Request<os_proto::v1::RejectDraftChunkRequest>,
    ) -> Result<tonic::Response<os_proto::v1::RejectDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn approve_all_draft_chunks(
        &self,
        _: tonic::Request<os_proto::v1::ApproveAllDraftChunksRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ApproveAllDraftChunksResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn edit_draft_chunk(
        &self,
        _: tonic::Request<os_proto::v1::EditDraftChunkRequest>,
    ) -> Result<tonic::Response<os_proto::v1::EditDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn undo_draft_chunk(
        &self,
        _: tonic::Request<os_proto::v1::UndoDraftChunkRequest>,
    ) -> Result<tonic::Response<os_proto::v1::UndoDraftChunkResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn clear_draft_chunks(
        &self,
        _: tonic::Request<os_proto::v1::ClearDraftChunksRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ClearDraftChunksResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_draft_history(
        &self,
        _: tonic::Request<os_proto::v1::GetDraftHistoryRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetDraftHistoryResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.105 identity/gateway stubs ---
    async fn get_current_user(
        &self,
        _: tonic::Request<os_proto::v1::GetCurrentUserRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetCurrentUserResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_gateway_info(
        &self,
        _: tonic::Request<os_proto::v1::GetGatewayInfoRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetGatewayInfoResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.105 sandbox stop/start stubs ---
    async fn stop_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::StopSandboxRequest>,
    ) -> Result<tonic::Response<os_proto::v1::SandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn start_sandbox(
        &self,
        _: tonic::Request<os_proto::v1::StartSandboxRequest>,
    ) -> Result<tonic::Response<os_proto::v1::SandboxResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.105 provider profile update stub ---
    async fn update_provider_profiles(
        &self,
        _: tonic::Request<os_proto::v1::UpdateProviderProfilesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::UpdateProviderProfilesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    // --- New v0.0.105 workspace stubs ---
    async fn create_workspace(
        &self,
        _: tonic::Request<os_proto::v1::CreateWorkspaceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::CreateWorkspaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn get_workspace(
        &self,
        _: tonic::Request<os_proto::v1::GetWorkspaceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::GetWorkspaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn list_workspaces(
        &self,
        _: tonic::Request<os_proto::v1::ListWorkspacesRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListWorkspacesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn delete_workspace(
        &self,
        _: tonic::Request<os_proto::v1::DeleteWorkspaceRequest>,
    ) -> Result<tonic::Response<os_proto::v1::DeleteWorkspaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn add_workspace_member(
        &self,
        _: tonic::Request<os_proto::v1::AddWorkspaceMemberRequest>,
    ) -> Result<tonic::Response<os_proto::v1::AddWorkspaceMemberResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn remove_workspace_member(
        &self,
        _: tonic::Request<os_proto::v1::RemoveWorkspaceMemberRequest>,
    ) -> Result<tonic::Response<os_proto::v1::RemoveWorkspaceMemberResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }

    async fn list_workspace_members(
        &self,
        _: tonic::Request<os_proto::v1::ListWorkspaceMembersRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ListWorkspaceMembersResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stub"))
    }
}

/// Spin up mock server; returns (bound address, shutdown sender).
pub(crate) async fn start_mock_server(
    mock: MockOpenShell,
) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(OpenShellServer::new(mock))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, tx)
}

/// Connect a plain (non-TLS) `OpenShellClient` to the mock at `addr`.
pub(crate) async fn mock_client(addr: SocketAddr) -> OpenShellClient<Channel> {
    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    OpenShellClient::new(channel)
}
