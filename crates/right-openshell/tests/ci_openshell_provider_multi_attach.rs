//! Live OpenShell capability test: ONE provider record can be attached to
//! MULTIPLE sandboxes and resolves correctly on each. `#[ignore]` (ci-openshell:).
//!
//! This is the load-bearing fact behind cross-agent credential SHARING (vs the
//! broken copy-by-readback path): the secret stays in the gateway, is never read
//! back, and a single record serves N sandboxes. Asserts the real secret is
//! substituted on egress from BOTH sandboxes, and that both attach lists carry
//! the record. See `/tmp/provider-copy-investigation-handoff.md` §15.

use right_openshell::managed_profiles::{
    author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
};
use right_openshell::openshell::{
    connect_grpc, default_mtls_dir, ensure_provider_policy_loaded, exec_in_sandbox,
    resolve_sandbox_id, wait_for_provider_composed_with_endpoint,
};
use right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use right_openshell::providers::{
    ProviderSpec, attach_to_sandbox, create_provider, delete_provider, ensure_v2_enabled,
    list_attached,
};
use right_openshell::test_support::TestSandbox;
use tonic::transport::Channel;

const ECHO_HOST: &str = "postman-echo.com";
const ENV_VAR: &str = "MAKEY";
const EGRESS_PROBE_CMD: &str =
    "curl -sk --max-time 30 -H \"Authorization: Bearer $MAKEY\" https://postman-echo.com/get";

// Minimal base policy; provider-profile composition adds the echo endpoint.
const POLICY: &str = "\
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
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
    binaries:
      - path: \"**\"
";

/// Attach the (already-created) provider to `sandbox_name`, compose, wait for the
/// placeholder to propagate, probe egress. Returns true iff the real secret is
/// substituted (not leaked as a placeholder).
async fn attach_and_probe(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    sid: &str,
    policy_path: &std::path::Path,
    prov: &str,
    secret: &str,
) -> bool {
    attach_to_sandbox(client, sandbox_name, prov)
        .await
        .expect("attach");
    ensure_provider_policy_loaded(sandbox_name, policy_path)
        .await
        .expect("policy reload");
    wait_for_provider_composed_with_endpoint(client, sandbox_name, prov, ECHO_HOST, "")
        .await
        .expect("composition confirmed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        if let Ok((o, rc)) = exec_in_sandbox(client, sid, &["printenv", ENV_VAR], 30).await {
            if rc == 0 && !o.trim().is_empty() {
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("{ENV_VAR} never propagated to sandbox {sandbox_name}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }

    let (out, _rc) = exec_in_sandbox(client, sid, &["sh", "-c", EGRESS_PROBE_CMD], 40)
        .await
        .expect("egress probe");
    out.contains(secret) && !out.contains("openshell:resolve:env:")
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_multi_attach_resolves_on_all() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();
    ensure_v2_enabled(&mut client).await.ok();

    let pid = std::process::id();
    let prov = format!("rightprobe-multi-{pid}");
    let profile_id = generic_provider_profile_id(&prov);
    let secret = format!("MULTI-SECRET-{pid}");

    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, &profile_id).await;
    lint_and_import(
        &mut client,
        author_generic_profile(&profile_id, &[ECHO_HOST.to_string()], None, ENV_VAR),
    )
    .await
    .expect("import echo profile");

    // ONE provider record.
    let mut creds = std::collections::HashMap::new();
    creds.insert(ENV_VAR.to_string(), secret.clone());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: profile_id.clone(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .expect("create one provider record");

    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();

    let sb_a = TestSandbox::create_with_policy("ci-openshell-multi-a", POLICY).await;
    let sid_a = resolve_sandbox_id(&mut client, sb_a.name())
        .await
        .expect("sid a");
    let sb_b = TestSandbox::create_with_policy("ci-openshell-multi-b", POLICY).await;
    let sid_b = resolve_sandbox_id(&mut client, sb_b.name())
        .await
        .expect("sid b");
    let name_a = sb_a.name().to_string();
    let name_b = sb_b.name().to_string();

    // Attach the SAME record to BOTH sandboxes and probe each.
    let a_resolves =
        attach_and_probe(&mut client, &name_a, &sid_a, &policy_path, &prov, &secret).await;
    let b_resolves =
        attach_and_probe(&mut client, &name_b, &sid_b, &policy_path, &prov, &secret).await;

    let att_a = list_attached(&mut client, &name_a)
        .await
        .unwrap_or_default();
    let att_b = list_attached(&mut client, &name_b)
        .await
        .unwrap_or_default();

    // cleanup before asserting
    delete_provider(&mut client, &prov).await.ok();
    delete_profile(&mut client, &profile_id).await.ok();

    assert!(
        att_a.contains(&prov),
        "record must appear in sandbox A's attached list"
    );
    assert!(
        att_b.contains(&prov),
        "record must appear in sandbox B's attached list"
    );
    assert!(
        a_resolves,
        "the shared record must substitute the real secret on sandbox A egress"
    );
    assert!(
        b_resolves,
        "the shared record must substitute the real secret on sandbox B egress"
    );
}
