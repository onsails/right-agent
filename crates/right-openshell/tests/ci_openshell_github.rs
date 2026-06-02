//! Live OpenShell gateway tests for RightClaw managed profile provisioning.
//! Each test is `#[ignore]` (ci-openshell:) — requires a live gateway with the
//! built-in `github` base profile present. Invoked explicitly by CI.

use right_openshell::managed_profiles::{
    EnsureOutcome, delete_profile, ensure_profiles, get_profile, github,
};
use right_openshell::openshell::{connect_grpc, default_mtls_dir};

/// Raw-tunnel base policy mirroring production `permissive`: 443 and 80 reachable
/// as `tls: skip`, so a provider's terminated L7 endpoint is the active policy
/// for its host. Shared by the live full-access tests below.
const RAW_TUNNEL_BASE_POLICY: &str = "version: 1\n\
filesystem_policy: { include_workdir: true, read_write: [/tmp, /sandbox] }\n\
process: { run_as_user: sandbox, run_as_group: sandbox }\n\
network_policies:\n  outbound:\n    endpoints:\n\
      - { host: \"0.0.0.0/0\", port: 443, tls: skip }\n\
      - { host: \"0.0.0.0/0\", port: 80, tls: skip }\n\
    binaries: [{ path: \"**\" }]\n";

#[tokio::test]
#[ignore = "ci-openshell: live github profile provisioning"]
async fn ci_openshell_github_imports_full_access_and_is_idempotent() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();

    // Clean slate in case a prior run left it behind (ignore NotFound).
    let _ = delete_profile(&mut client, "right-github").await;

    let first = ensure_profiles(&mut client, &[github()])
        .await
        .expect("ensure");
    assert_eq!(first, vec![EnsureOutcome::Imported("right-github".into())]);

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

    delete_profile(&mut client, "right-github")
        .await
        .expect("cleanup delete");
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

/// De-risk: proves that `access: full` on a terminated provider endpoint
/// permits POST requests (full-method access). No real credential required.
/// A throwaway profile is imported with one `httpbin.org` REST endpoint and
/// `access: full`. A provider is created with that type, attached to an
/// ephemeral sandbox, and `curl -X POST https://httpbin.org/post` is executed.
/// A 200 response proves the method-level policy is open; a 403 would mean
/// OpenShell's policy engine blocked the POST.
///
/// `#[ignore]` — requires live gateway; compiles with `--no-run`.
#[tokio::test]
#[ignore = "ci-openshell: full-access permits POST on a terminated endpoint"]
async fn ci_openshell_full_access_allows_post() {
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
