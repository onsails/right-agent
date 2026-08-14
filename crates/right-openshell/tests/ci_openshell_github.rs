//! Live OpenShell gateway tests for RightClaw managed profile provisioning.
//! CI-filtered tests use `#[ignore]` markers with `ci-openshell:` and
//! `ci_openshell_` names. Manual probes stay outside that filter.

use std::future::Future;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use right_openshell::managed_profiles::{
    EnsureOutcome, delete_profile, ensure_profiles, get_profile, github,
};
use right_openshell::openshell::{connect_grpc, default_mtls_dir, ensure_provider_policy_loaded};

/// Raw-tunnel base policy mirroring production `permissive`: 443 and 80 reachable
/// as `tls: skip`, so a provider's terminated L7 endpoint is the active policy
/// for its host. Shared by the live full-access tests below.
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

fn raw_tunnel_policy_file() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("create policy tempdir");
    let path = tmp.path().join("policy.yaml");
    std::fs::write(&path, RAW_TUNNEL_BASE_POLICY).expect("write test policy");
    (tmp, path)
}

fn output_has_http_200(output: &str) -> bool {
    output.lines().any(|line| {
        line.starts_with("HTTP/")
            && line
                .split_ascii_whitespace()
                .nth(1)
                .is_some_and(|status| status == "200")
    })
}

async fn cleanup_provider(provider_name: &str, sandbox_name: Option<&str>) {
    use right_openshell::providers::{delete_provider, detach_from_sandbox};

    let Ok(mut client) = connect_grpc(&default_mtls_dir()).await else {
        return;
    };
    if let Some(sandbox_name) = sandbox_name {
        let _ = detach_from_sandbox(&mut client, sandbox_name, provider_name).await;
    }
    let _ = delete_provider(&mut client, provider_name).await;
    right_openshell::test_cleanup::unregister_test_provider(provider_name);
}

async fn with_provider_cleanup<Fut>(
    provider_name: &str,
    sandbox_name: Arc<Mutex<Option<String>>>,
    fut: Fut,
) where
    Fut: Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
    let sandbox_name = sandbox_name.lock().expect("sandbox name lock").clone();
    cleanup_provider(provider_name, sandbox_name.as_deref()).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn raw_tunnel_base_policy_keeps_endpoint_list_nested() {
    assert!(
        RAW_TUNNEL_BASE_POLICY.contains("    endpoints:\n      - "),
        "endpoint list items must remain indented under network_policies.outbound.endpoints"
    );
}

#[tokio::test]
#[ignore = "ci-openshell: live github profile provisioning"]
async fn ci_openshell_github_ensures_full_access_and_is_idempotent() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();

    let first = ensure_profiles(&mut client, &[github()])
        .await
        .expect("ensure");
    assert!(
        matches!(
            first.as_slice(),
            [EnsureOutcome::Imported(id) | EnsureOutcome::Unchanged(id)] if id == "right-github"
        ),
        "first ensure should import or confirm right-github, got {first:?}"
    );

    let stored = get_profile(&mut client, "right-github")
        .await
        .expect("get")
        .expect("present after import");
    assert!(!stored.endpoints.is_empty());
    for ep in &stored.endpoints {
        assert_eq!(ep.access, "full", "host {} must have access:full", ep.host);
        assert!(
            ep.rules.is_empty(),
            "host {} must have empty rules (exclusive with access:full)",
            ep.host
        );
    }

    let second = ensure_profiles(&mut client, &[github()])
        .await
        .expect("ensure2");
    assert_eq!(
        second,
        vec![EnsureOutcome::Unchanged("right-github".into())]
    );
}

#[tokio::test]
#[ignore = "ci-openshell: requires RIGHT_TEST_GH_TOKEN"]
async fn ci_openshell_github_gh_api_user_succeeds() {
    use right_openshell::providers::{ProviderSpec, attach_to_sandbox, create_provider};
    use right_openshell::test_support::TestSandbox;

    let token = match std::env::var("RIGHT_TEST_GH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            eprintln!("skip: set RIGHT_TEST_GH_TOKEN");
            return;
        }
    };

    let provider_name = format!("rightprobe-{}-github-api", std::process::id());
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_provider(&provider_name, None).await;

    with_provider_cleanup(&provider_name, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");
        ensure_profiles(&mut client, &[github()])
            .await
            .expect("ensure right-github profile");

        let mut creds = std::collections::HashMap::new();
        creds.insert("GITHUB_TOKEN".to_string(), token);
        create_provider(
            &mut client,
            &ProviderSpec {
                name: provider_name.clone(),
                type_: "right-github".into(),
                credentials: creds,
                config: Default::default(),
            },
        )
        .await
        .expect("create provider");
        right_openshell::test_cleanup::register_test_provider(&provider_name, None);

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox =
            TestSandbox::create_with_policy("ci-openshell-github-api-user", RAW_TUNNEL_BASE_POLICY)
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
        .expect("built-in right-github composed into active policy");

        let (out, code) = sandbox
            .exec_with_timeout(&["gh", "api", "/user", "--silent", "-i"], 120)
            .await;

        assert_eq!(code, 0, "gh api /user should exit successfully");
        assert!(
            output_has_http_200(&out),
            "gh api /user did not return HTTP 200"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "ci-openshell: live github profile provisioning"]
async fn ci_openshell_get_profile_absent_returns_none() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();
    assert!(
        get_profile(&mut client, "definitely-not-a-profile-xyz")
            .await
            .expect("get")
            .is_none()
    );
}

/// Manual de-risk probe for `access: full` on a terminated provider endpoint.
/// No real credential required.
///
/// This is intentionally not a CI-filtered `ci_openshell_` test. The plan that
/// introduced it called out public echo-host networking as optional and finicky;
/// the load-bearing gate is `ci_openshell_github_push_succeeds`.
/// A throwaway profile is imported with one `httpbin.org` REST endpoint and
/// `access: full`. A provider is created with that type, attached to an
/// ephemeral sandbox, and `curl -X POST https://httpbin.org/post` is executed.
/// A 200 response proves the method-level policy is open; a 403 would mean
/// OpenShell's policy engine blocked the POST.
///
/// `#[ignore]` — requires live gateway; compiles with `--no-run`.
#[tokio::test]
#[ignore = "manual-live: full-access POST de-risk on a public echo host"]
async fn manual_live_full_access_allows_post() {
    use right_openshell::managed_profiles::lint_and_import;
    use right_openshell::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
    use right_openshell::openshell_proto::openshell::v1 as proto_v1;
    use right_openshell::providers::{
        ProviderSpec, attach_to_sandbox, create_provider, delete_provider, detach_from_sandbox,
    };
    use right_openshell::test_support::TestSandbox;

    let profile_id = "right-test-cftest-fullpost";
    let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

    // Clean up any leftover from a prior run.
    let _ = delete_profile(&mut client, profile_id).await;

    // Throwaway profile: one httpbin.org REST endpoint with access:full.
    let ep = sandbox_v1::NetworkEndpoint {
        host: "httpbin.org".into(),
        port: 443,
        protocol: "rest".into(),
        access: "full".into(),
        enforcement: "enforce".into(),
        rules: vec![],
        ..Default::default()
    };
    let profile = proto_v1::ProviderProfile {
        id: profile_id.into(),
        display_name: "test-fullpost".into(),
        description: "throwaway probe profile for full-access POST test".into(),
        category: 0,
        credentials: vec![proto_v1::ProviderProfileCredential {
            name: "CFTEST_TOKEN".into(),
            description: "throwaway credential".into(),
            ..Default::default()
        }],
        endpoints: vec![ep],
        binaries: vec![],
        inference_capable: false,
        discovery: None,
        annotations: Default::default(),
        resource_version: 0,
        source: String::new(),
        scope: String::new(),
    };

    lint_and_import(&mut client, profile)
        .await
        .expect("lint_and_import throwaway profile");

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-cftest-fullpost");
    let mut creds = std::collections::HashMap::new();
    creds.insert("CFTEST_TOKEN".to_string(), "fake-token".to_string());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: profile_id.into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .expect("create provider");

    // Boot with a raw-tunnel base so the provider's network section is the
    // active policy for httpbin.org.
    let sandbox = TestSandbox::create_with_policy("cftest-fullpost", RAW_TUNNEL_BASE_POLICY).await;
    attach_to_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .expect("attach");

    let (out, _code) = sandbox
        .exec_with_timeout(
            &[
                "sh",
                "-lc",
                "curl -s -o /dev/null -w '%{http_code}' -X POST https://httpbin.org/post --max-time 30",
            ],
            60,
        )
        .await;

    let _ = detach_from_sandbox(&mut client, sandbox.name(), &prov).await;
    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, profile_id).await;

    assert_eq!(
        out.trim(),
        "200",
        "POST to httpbin.org/post must return 200 when access:full is set; got: {out}"
    );
}

/// Regression GATE (run deliberately with creds):
///   RIGHT_TEST_GH_TOKEN=<PAT/OAuth with push to RIGHT_TEST_GH_PUSH_REPO>
///   RIGHT_TEST_GH_PUSH_REPO=<owner/repo the token may force-push a throwaway branch to>
/// Proves the design end-to-end: ensure right-github → create a provider
/// with the real token → attach to a TestSandbox (raw-tunnel base, mirroring
/// production) → inside the sandbox `git push` a throwaway branch using the
/// provider's injected GITHUB_TOKEN placeholder (proxy substitutes it) → assert
/// success (NOT a 403 X-OpenShell-Policy) → delete the branch. The token is
/// spliced into the URL INSIDE the sandbox only and redacted from captured
/// output. Without both env vars set, this is a no-op.
#[tokio::test]
#[ignore = "ci-openshell: live github push regression (needs RIGHT_TEST_GH_TOKEN + throwaway repo)"]
async fn ci_openshell_github_push_succeeds() {
    use right_openshell::managed_profiles::{ensure_profiles, github};
    use right_openshell::providers::{
        ProviderSpec, attach_to_sandbox, create_provider, delete_provider, detach_from_sandbox,
    };
    use right_openshell::test_support::TestSandbox;

    let token = match std::env::var("RIGHT_TEST_GH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            eprintln!("skip: set RIGHT_TEST_GH_TOKEN + RIGHT_TEST_GH_PUSH_REPO");
            return;
        }
    };
    let repo = match std::env::var("RIGHT_TEST_GH_PUSH_REPO") {
        Ok(r) if !r.is_empty() => r,
        _ => {
            eprintln!("skip: set RIGHT_TEST_GH_TOKEN + RIGHT_TEST_GH_PUSH_REPO");
            return;
        }
    };

    let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
    ensure_profiles(&mut client, &[github()])
        .await
        .expect("ensure");

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-github");
    let mut creds = std::collections::HashMap::new();
    creds.insert("GITHUB_TOKEN".to_string(), token);
    create_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: "right-github".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .expect("create provider");

    // Raw-tunnel base (mirrors production permissive policy): github.com:443
    // reachable as tls:skip; the provider injects the L7 segment on top.
    let sandbox =
        TestSandbox::create_with_policy("ci-openshell-github-push", RAW_TUNNEL_BASE_POLICY).await;
    attach_to_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .expect("attach");

    let branch = format!("zz-rightclaw-probe-{pid}");
    let script = format!(
        "set -e; set +x; export GIT_TERMINAL_PROMPT=0; d=$(mktemp -d); cd \"$d\"; \
git init -q; git config user.email p@e.invalid; git config user.name p; \
echo probe > p.txt; git add p.txt; git commit -q -m probe; \
authed=\"https://x-access-token:${{GITHUB_TOKEN}}@github.com/{repo}.git\"; \
git push --no-verify \"$authed\" HEAD:refs/heads/{branch} >/dev/null 2>e.txt && echo PUSH_OK || \
{{ echo PUSH_FAIL; sed -E 's#x-access-token:[^@]*@#x-access-token:***@#g' e.txt; }}; \
git push \"$authed\" :refs/heads/{branch} >/dev/null 2>&1 || true"
    );
    let (out, code) = sandbox
        .exec_with_timeout(&["sh", "-lc", &script], 120)
        .await;

    let _ = detach_from_sandbox(&mut client, sandbox.name(), &prov).await;
    let _ = delete_provider(&mut client, &prov).await;

    assert!(
        !out.contains("x-access-token:ghp_")
            && !out.contains("x-access-token:gho_")
            && !out.contains("x-access-token:github_pat_"),
        "refusing to print a raw token"
    );
    eprintln!("push regression exit={code}\n{out}");
    assert!(
        out.contains("PUSH_OK"),
        "git push was blocked (read-only would 403 X-OpenShell-Policy here); output: {out}"
    );
}
