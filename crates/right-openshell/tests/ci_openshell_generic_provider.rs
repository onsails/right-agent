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
const SECOND_UPSTREAM_HOST: &str = "httpbin.org";
const HEADER_NAME: &str = "x-api-key";
const ENV_VAR: &str = "MY_API_KEY";
const FAKE_CREDENTIAL: &str = "right-test-fake-api-key";
const CLIENT_SCHEME_HEADER_NAME: &str = "Authorization";
const CLIENT_SCHEME_ENV_VAR: &str = "RIGHT_TEST_KEY";
const CLIENT_SCHEME_FAKE_CREDENTIAL: &str = "right-test-fake-client-scheme-key";
/// Success path: retries transient upstream failures (postman-echo is a public
/// service prone to 5xx/429/timeout) so the gate measures gateway substitution,
/// not third-party uptime. A proxy CONNECT rejection is not transient and still
/// surfaces deterministically.
const CURL_ECHO_HEADER: &str = "curl -sS --fail-with-body --max-time 30 \
--retry 3 --retry-delay 2 --retry-all-errors \
https://postman-echo.com/get -H \"x-api-key: ${MY_API_KEY}\" 2>&1";
const CURL_ECHO_AUTHORIZATION_KEY_HEADER: &str = "curl -sS --fail-with-body --max-time 30 \
--retry 3 --retry-delay 2 --retry-all-errors \
https://postman-echo.com/get -H \"Authorization: Key ${RIGHT_TEST_KEY}\" 2>&1";
/// Block path: no retries — the test asserts the proxy rejects CONNECT, which is
/// immediate and deterministic; retrying would only add latency.
const CURL_ECHO_HEADER_NORETRY: &str = "curl -sS --fail-with-body --max-time 30 \
https://postman-echo.com/get -H \"x-api-key: ${MY_API_KEY}\" 2>&1";
/// Diagnostics path: verbose curl so the proxy CONNECT exchange (the `403`
/// denial line and any reason body) is captured when the success curl fails.
const CURL_ECHO_HEADER_VERBOSE: &str = "curl -sS -v --max-time 30 \
https://postman-echo.com/get -H \"x-api-key: ${MY_API_KEY}\" 2>&1";
/// Diagnostics path: speak HTTP CONNECT to the proxy by hand and print the full
/// response **including the body**. curl discards the proxy's CONNECT-failure
/// body; the OpenShell proxy returns its denial reason there (e.g. no matching
/// terminated endpoint vs. binary not allowed), which is what distinguishes the
/// composition-failure cause from the enforcement cause.
const RAW_CONNECT_PROBE: &str = "timeout 10 bash -c '\
exec 3<>/dev/tcp/10.200.0.1/3128 || exit 7; \
printf \"CONNECT postman-echo.com:443 HTTP/1.1\\r\\nHost: postman-echo.com:443\\r\\n\\r\\n\" >&3; \
cat <&3' 2>&1 | head -c 1000";

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
    fake_provider_spec_with_credential(provider_name, profile_id, ENV_VAR, FAKE_CREDENTIAL)
}

fn fake_provider_spec_with_credential(
    provider_name: &str,
    profile_id: &str,
    env_var: &str,
    fake_credential: &str,
) -> ProviderSpec {
    let mut credentials = HashMap::new();
    credentials.insert(env_var.to_string(), fake_credential.to_string());
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
    let upstream_hosts = vec![UPSTREAM_HOST.to_string()];
    let mut profile = author_generic_profile(profile_id, &upstream_hosts, None, ENV_VAR);
    if !include_binaries {
        profile.binaries.clear();
    }
    let managed = ManagedProfile::Authored(Box::new(profile));
    ensure_profiles(client, &[managed])
        .await
        .expect("ensure generic profile");
}

async fn ensure_generic_profile_for_hosts(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    profile_id: &str,
    upstream_hosts: &[String],
    env_var: &str,
) {
    let profile = author_generic_profile(profile_id, upstream_hosts, None, env_var);
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

async fn wait_for_provider_placeholder(sandbox: &TestSandbox, env_var: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let (out, code) = sandbox.exec(&["printenv", env_var]).await;
        if code == 0 && out.trim().starts_with("openshell:resolve:env:") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{env_var} provider placeholder did not appear in sandbox before timeout"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// On a CONNECT failure, gather the data needed to tell apart the two causes of
/// a `403 CONNECT tunnel failed`: provider-profile composition never landing the
/// terminated `postman-echo.com` endpoint with its `binaries: **` rule on the
/// gateway (visible in the composed policy), vs. the gateway enforcing a denial
/// despite the rule being present. The success path passes locally, so this only
/// fires on the CI-only regression and makes the next run self-diagnosing instead
/// of opaque. Best-effort: never panics itself.
async fn connect_failure_diagnostics(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    sandbox: &TestSandbox,
) -> String {
    let (verbose, vcode) = sandbox
        .exec_with_timeout(&["sh", "-lc", CURL_ECHO_HEADER_VERBOSE], 60)
        .await;
    let (raw_connect, rcode) = sandbox
        .exec_with_timeout(&["sh", "-lc", RAW_CONNECT_PROBE], 30)
        .await;
    let policy =
        match right_openshell::openshell::get_effective_policy(client, sandbox.name()).await {
            Ok(Some(policy)) => format!("{policy:#?}"),
            Ok(None) => "<no composed policy returned>".to_string(),
            Err(e) => format!("<get_active_policy failed: {e:#}>"),
        };
    format!(
        "\n--- curl -v (exit {vcode}) ---\n{verbose}\n\
         --- raw proxy CONNECT response (exit {rcode}) ---\n{raw_connect}\n\
         --- composed sandbox policy ---\n{policy}"
    )
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
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");

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
        right_openshell::openshell::wait_for_provider_composed(
            &mut client,
            sandbox.name(),
            &provider_name,
        )
        .await
        .expect("provider composed into active policy");
        wait_for_provider_placeholder(&sandbox, ENV_VAR).await;

        let (out, code) = sandbox
            .exec_with_timeout(&["sh", "-lc", CURL_ECHO_HEADER], 60)
            .await;

        if code != 0 {
            let diag = connect_failure_diagnostics(&mut client, &sandbox).await;
            panic!("curl command should exit successfully (code {code}); output: {out}{diag}");
        }
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
async fn ci_openshell_generic_multi_host_composes_all_and_substitutes_client_scheme() {
    let profile_id = unique_profile_id("generic-multi-host");
    let provider_name = unique_name("generic-multi-host");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");

        let upstream_hosts = vec![UPSTREAM_HOST.to_string(), SECOND_UPSTREAM_HOST.to_string()];
        ensure_generic_profile_for_hosts(
            &mut client,
            &profile_id,
            &upstream_hosts,
            CLIENT_SCHEME_ENV_VAR,
        )
        .await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(
            &mut client,
            &fake_provider_spec_with_credential(
                &provider_name,
                &profile_id,
                CLIENT_SCHEME_ENV_VAR,
                CLIENT_SCHEME_FAKE_CREDENTIAL,
            ),
        )
        .await
        .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox = TestSandbox::create_with_policy(
            "ci-openshell-generic-multi-host",
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
        let expected_endpoints = upstream_hosts
            .iter()
            .map(|host| (host.clone(), String::new()))
            .collect();
        right_openshell::openshell::wait_for_provider_composed_with_all_endpoints(
            &mut client,
            sandbox.name(),
            &provider_name,
            expected_endpoints,
        )
        .await
        .expect("provider composed into active policy with all upstream hosts");
        wait_for_provider_placeholder(&sandbox, CLIENT_SCHEME_ENV_VAR).await;

        let (out, code) = sandbox
            .exec_with_timeout(&["sh", "-lc", CURL_ECHO_AUTHORIZATION_KEY_HEADER], 60)
            .await;

        if code != 0 {
            let diag = connect_failure_diagnostics(&mut client, &sandbox).await;
            panic!("curl command should exit successfully (code {code}); output: {out}{diag}");
        }
        let echoed = echoed_header(&out, CLIENT_SCHEME_HEADER_NAME)
            .unwrap_or_else(|| panic!("echo response must contain Authorization; output: {out}"));
        assert_eq!(
            echoed,
            format!("Key {CLIENT_SCHEME_FAKE_CREDENTIAL}"),
            "echoed Authorization header did not preserve the client-written scheme"
        );
    })
    .await;
}

// NOTE: do not add a live test that toggles the gateway-global
// `providers_v2_enabled` setting (e.g. set it false then assert reconcile
// self-enables it). That setting is gateway-global, and these provider tests
// run with RIGHT_MAX_CONCURRENT_SANDBOX_TESTS=2 — a false window poisons the
// concurrently-running provider test and can wedge the shared gateway. The
// "reconcile self-enables v2 when providers are declared" property is covered
// deterministically by the providers.rs unit test against the mock gateway.

#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_profile_without_binaries_blocks_connect() {
    let profile_id = unique_profile_id("generic-no-binaries");
    let provider_name = unique_name("generic-no-binaries");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");

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
        wait_for_provider_placeholder(&sandbox, ENV_VAR).await;

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
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");

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
        wait_for_provider_placeholder(&sandbox, ENV_VAR).await;

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
