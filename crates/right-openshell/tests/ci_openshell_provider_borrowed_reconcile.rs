//! Live OpenShell regression: a BORROWED provider record (one owned by another
//! agent and shared to this one) MUST survive `reconcile_for_sandbox` — i.e.
//! it is attached and stays attached. The borrower never re-imports or repairs
//! the profile; the owner already did that. `#[ignore]` (ci-openshell:).
//!
//! Scenario:
//!   1. Import a generic profile + create one provider record (simulates owner).
//!   2. Create one TestSandbox (the borrower).
//!   3. Call `reconcile_for_sandbox` with the record in `declared`.
//!   4. Assert the record is in the sandbox's attached list AND egress resolves
//!      the real secret via the composed profile (not a placeholder leak).
//!   5. Clean up.
//!
//! See `docs/architecture/providers.md` and the provider-sharing feature spec.

use right_openshell::managed_profiles::{
    author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
};
use right_openshell::openshell::{
    connect_grpc, default_mtls_dir, ensure_provider_policy_loaded, exec_in_sandbox,
    resolve_sandbox_id, wait_for_provider_composed_with_endpoint,
};
use right_openshell::providers::{
    ProviderSpec, create_provider, delete_provider, list_attached, reconcile_for_sandbox,
};
use right_openshell::test_support::TestSandbox;

const ECHO_HOST: &str = "postman-echo.com";
const ENV_VAR: &str = "BORROWED_KEY";
const EGRESS_PROBE_CMD: &str = "curl -sk --max-time 30 -H \"Authorization: Bearer $BORROWED_KEY\" https://postman-echo.com/get";

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

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_borrowed_survives_reconcile() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-borrowed-{pid}");
    let profile_id = generic_provider_profile_id(&prov);
    let secret = format!("BORROWED-SECRET-{pid}");

    // Clean up any leftovers from a prior interrupted run.
    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, &profile_id).await;

    // Import the profile (simulates what the OWNER agent does).
    lint_and_import(
        &mut client,
        author_generic_profile(&profile_id, &[ECHO_HOST.to_string()], None, ENV_VAR),
    )
    .await
    .expect("import generic profile (owner step)");

    // Create one provider record (simulates the owner's gateway record).
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
    .expect("create provider record (owner step)");

    // Create the borrower sandbox.
    let sb = TestSandbox::create_with_policy("ci-openshell-borrowed-reconcile", POLICY).await;
    let sid = resolve_sandbox_id(&mut client, sb.name())
        .await
        .expect("resolve borrower sandbox id");

    // reconcile_for_sandbox with the record declared — it must attach and NOT detach.
    let report = reconcile_for_sandbox(&mut client, sb.name(), "borrower", &[prov.clone()])
        .await
        .expect("reconcile_for_sandbox");

    // Write policy + wait for composition, then probe egress.
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();
    ensure_provider_policy_loaded(sb.name(), &policy_path)
        .await
        .expect("policy reload");
    wait_for_provider_composed_with_endpoint(&mut client, sb.name(), &prov, ECHO_HOST, "")
        .await
        .expect("composition confirmed for borrower");

    // Wait for the env var to propagate, then probe egress.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        if let Ok((o, rc)) = exec_in_sandbox(&mut client, &sid, &["printenv", ENV_VAR], 30).await {
            if rc == 0 && !o.trim().is_empty() {
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{ENV_VAR} never propagated to borrower sandbox {}",
                sb.name()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    let (egress_out, _) = exec_in_sandbox(&mut client, &sid, &["sh", "-c", EGRESS_PROBE_CMD], 40)
        .await
        .expect("egress probe");
    let egress_resolves =
        egress_out.contains(&secret) && !egress_out.contains("openshell:resolve:env:");

    let attached_list = list_attached(&mut client, sb.name())
        .await
        .unwrap_or_default();

    // Cleanup before asserting (sandbox cleaned up by TestSandbox drop).
    delete_provider(&mut client, &prov).await.ok();
    delete_profile(&mut client, &profile_id).await.ok();

    assert!(
        attached_list.contains(&prov),
        "borrowed record must appear in the sandbox's attached list after reconcile; got: {attached_list:?}"
    );
    assert!(
        !report.detached.contains(&prov),
        "reconcile must NOT detach a declared borrowed record; report.detached: {:?}",
        report.detached
    );
    assert!(
        egress_resolves,
        "borrowed record must substitute the real secret on egress; output: {egress_out}"
    );
}
