#![cfg(feature = "test-support")]

//! Live OpenShell gateway tests for authored generic provider profiles.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use right_openshell::managed_profiles::{
    ManagedProfile, author_generic_profile, delete_profile, ensure_profiles,
};
use right_openshell::openshell::{connect_grpc, default_mtls_dir, ensure_provider_policy_loaded};
use right_openshell::providers::{
    ProviderSpec, attach_to_sandbox, create_provider, delete_provider, detach_from_sandbox,
};
use right_openshell::test_support::TestSandbox;

/// Raw-tunnel base policy mirroring production `permissive`: 443 and 80 remain
/// reachable while provider-profile composition inserts the terminated L7
/// endpoint that can substitute placeholders.
const RAW_TUNNEL_BASE_POLICY: &str = r#"version: 1
filesystem_policy: { include_workdir: true, read_write: [/tmp, /sandbox] }
process: { run_as_user: sandbox, run_as_group: sandbox }
network_policies:
  outbound:
    endpoints:
      - { host: "0.0.0.0/0", port: 443, tls: skip }
      - { host: "0.0.0.0/0", port: 80, tls: skip }
    binaries: [{ path: "**" }]
"#;

const UPSTREAM_HOST: &str = "postman-echo.com";
const HEADER_NAME: &str = "x-api-key";
const ENV_VAR: &str = "MY_API_KEY";
const FAKE_CREDENTIAL: &str = "right-test-fake-api-key";
/// Success path: retries transient upstream failures (postman-echo is a public
/// service prone to 5xx/429/timeout) so the gate measures gateway substitution,
/// not third-party uptime. A proxy CONNECT rejection is not transient and still
/// surfaces deterministically.
const CURL_ECHO_HEADER: &str = "curl -sS --fail-with-body --max-time 30 \
--retry 3 --retry-delay 2 --retry-all-errors \
https://postman-echo.com/get -H \"x-api-key: ${MY_API_KEY}\" 2>&1";
/// Block path: no retries — the test asserts the proxy rejects CONNECT, which is
/// immediate and deterministic; retrying would only add latency.
const CURL_ECHO_HEADER_NORETRY: &str = "curl -sS --fail-with-body --max-time 30 \
https://postman-echo.com/get -H \"x-api-key: ${MY_API_KEY}\" 2>&1";

fn raw_tunnel_policy_file() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create policy tempdir");
    let path = tmp.path().join("policy.yaml");
    std::fs::write(&path, RAW_TUNNEL_BASE_POLICY).expect("write test policy");
    (tmp, path)
}

fn unique_name(label: &str) -> String {
    format!("rightprobe-{}-{label}", std::process::id())
}

fn unique_profile_id(label: &str) -> String {
    format!("right-test-{}-{label}", std::process::id())
}

fn fake_provider_spec(provider_name: &str, profile_id: &str) -> ProviderSpec {
    let mut credentials = HashMap::new();
    credentials.insert(ENV_VAR.to_string(), FAKE_CREDENTIAL.to_string());
    ProviderSpec {
        name: provider_name.to_string(),
        type_: profile_id.to_string(),
        credentials,
        config: Default::default(),
    }
}

async fn ensure_generic_profile(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    profile_id: &str,
    include_binaries: bool,
) {
    let mut profile = author_generic_profile(profile_id, UPSTREAM_HOST, None, HEADER_NAME, ENV_VAR);
    if !include_binaries {
        profile.binaries.clear();
    }
    let managed = ManagedProfile::Authored(Box::new(profile));
    ensure_profiles(client, &[managed])
        .await
        .expect("ensure generic profile");
}

async fn cleanup_generic_resources(
    provider_name: &str,
    profile_id: &str,
    sandbox_name: Option<&str>,
) {
    let Ok(mut client) = connect_grpc(&default_mtls_dir()).await else {
        return;
    };
    if let Some(sandbox_name) = sandbox_name {
        let _ = detach_from_sandbox(&mut client, sandbox_name, provider_name).await;
    }
    let _ = delete_provider(&mut client, provider_name).await;
    let _ = delete_profile(&mut client, profile_id).await;
    right_openshell::test_cleanup::unregister_test_provider(provider_name);
}

async fn with_generic_cleanup<Fut>(
    provider_name: &str,
    profile_id: &str,
    sandbox_name: Arc<Mutex<Option<String>>>,
    fut: Fut,
) where
    Fut: Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
    let sandbox_name = sandbox_name.lock().expect("sandbox name lock").clone();
    cleanup_generic_resources(provider_name, profile_id, sandbox_name.as_deref()).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn echoed_header(response: &str, header_name: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(response).ok()?;
    let headers = parsed.get("headers")?.as_object()?;
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(header_name))
        .and_then(|(_, value)| value.as_str())
        .map(str::to_string)
}

async fn wait_for_provider_placeholder(sandbox: &TestSandbox) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let (out, code) = sandbox.exec(&["printenv", ENV_VAR]).await;
        if code == 0 && out.trim().starts_with("openshell:resolve:env:") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{ENV_VAR} provider placeholder did not appear in sandbox before timeout"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_generic_profile_substitutes_custom_header() {
    let profile_id = unique_profile_id("generic-header");
    let provider_name = unique_name("generic-header");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

        ensure_generic_profile(&mut client, &profile_id, true).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(
            &mut client,
            &fake_provider_spec(&provider_name, &profile_id),
        )
        .await
        .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox =
            TestSandbox::create_with_policy("ci-openshell-generic-header", RAW_TUNNEL_BASE_POLICY)
                .await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());
        attach_to_sandbox(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("attach provider");
        right_openshell::test_cleanup::register_test_provider_attachment(
            &provider_name,
            sandbox.name(),
        );
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        wait_for_provider_placeholder(&sandbox).await;

        let (out, code) = sandbox
            .exec_with_timeout(&["sh", "-lc", CURL_ECHO_HEADER], 60)
            .await;

        assert_eq!(
            code, 0,
            "curl command should exit successfully (code {code}); output: {out}"
        );
        let echoed = echoed_header(&out, HEADER_NAME)
            .unwrap_or_else(|| panic!("echo response must contain x-api-key; output: {out}"));
        assert!(
            echoed == FAKE_CREDENTIAL,
            "echoed x-api-key did not match fake credential (got {echoed:?})"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_profile_without_binaries_blocks_connect() {
    let profile_id = unique_profile_id("generic-no-binaries");
    let provider_name = unique_name("generic-no-binaries");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

        ensure_generic_profile(&mut client, &profile_id, false).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(
            &mut client,
            &fake_provider_spec(&provider_name, &profile_id),
        )
        .await
        .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox = TestSandbox::create_with_policy(
            "ci-openshell-generic-no-binaries",
            RAW_TUNNEL_BASE_POLICY,
        )
        .await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());
        attach_to_sandbox(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("attach provider");
        right_openshell::test_cleanup::register_test_provider_attachment(
            &provider_name,
            sandbox.name(),
        );
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        wait_for_provider_placeholder(&sandbox).await;

        let (out, code) = sandbox
            .exec_with_timeout(&["sh", "-lc", CURL_ECHO_HEADER_NORETRY], 60)
            .await;

        assert_ne!(
            code, 0,
            "profile without binaries must reject CONNECT; command unexpectedly succeeded; output: {out}"
        );
        assert!(
            out.contains("CONNECT") && out.contains("403"),
            "profile without binaries failed for an unexpected reason: {out}"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_provider_capabilities_reports_attached_provider() {
    let profile_id = unique_profile_id("generic-caps");
    let provider_name = unique_name("generic-caps");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

        ensure_generic_profile(&mut client, &profile_id, true).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(
            &mut client,
            &fake_provider_spec(&provider_name, &profile_id),
        )
        .await
        .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox =
            TestSandbox::create_with_policy("ci-openshell-generic-caps", RAW_TUNNEL_BASE_POLICY)
                .await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());
        attach_to_sandbox(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("attach provider");
        right_openshell::test_cleanup::register_test_provider_attachment(
            &provider_name,
            sandbox.name(),
        );
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        wait_for_provider_placeholder(&sandbox).await;

        let caps = right_openshell::provider_capabilities::provider_capabilities_for_sandbox(
            &mut client,
            sandbox.name(),
        )
        .await
        .expect("gather provider capabilities");

        let rendered_caps = format!("{caps:?}");
        assert!(
            !rendered_caps.contains(FAKE_CREDENTIAL),
            "capabilities must not include credential values"
        );
        assert!(
            !rendered_caps.contains("openshell:resolve:env:"),
            "capabilities must not include provider placeholder values"
        );

        let cap = caps
            .iter()
            .find(|c| c.env_vars.iter().any(|v| v == ENV_VAR))
            .unwrap_or_else(|| panic!("capabilities must include {ENV_VAR}; got {caps:?}"));
        assert!(
            cap.allowed_binaries.iter().any(|b| b == "**"),
            "generic profile uses binaries ** ; got {:?}",
            cap.allowed_binaries
        );
        assert!(
            cap.endpoint_hosts.iter().any(|h| h == UPSTREAM_HOST),
            "endpoint host must include {UPSTREAM_HOST}; got {:?}",
            cap.endpoint_hosts
        );
    })
    .await;
}
