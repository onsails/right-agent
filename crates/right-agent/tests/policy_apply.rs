//! Integration test: codegen-generated policies apply cleanly to a live
//! OpenShell sandbox. Catches future OpenShell deprecations-turned-errors
//! and policy-syntax regressions.
//!
//! Strategy: spawn a fresh sandbox using the codegen policy as the startup
//! policy, wait for READY, and verify `exec` works. This covers:
//!
//! * policy-syntax errors (OpenShell rejects the YAML at creation time)
//! * deprecated fields turning into hard errors (future OpenShell versions)
//! * any sandbox-startup regression introduced by codegen changes
//!
//! We intentionally do NOT go through `TestSandbox` +
//! `write_and_apply_sandbox_policy`: `openshell policy set` refuses to
//! hot-reload filesystem_policy changes on a live sandbox, so hot-reloading
//! the codegen policy on top of TestSandbox's permissive startup policy
//! fails with "landlock policy cannot be changed on a live sandbox". Fresh
//! creation exercises the same acceptance path (plus sandbox boot) and is
//! what real `right up` does on first run.
//!
//! Limitation: does not currently scrape sandbox-supervisor logs for
//! deprecation WARNs. The unit-level guard
//! (`codegen::policy::does_not_emit_deprecated_tls_terminate`) covers the
//! deprecated-field case at the source. Log-scraping would require
//! extending `TestSandbox` with a container-log accessor — deferred.

use std::path::Path;

use right_agent::agent::types::NetworkPolicy;
use right_codegen::policy::generate_policy;
use right_core::openshell::{
    self, DEFAULT_EXEC_TIMEOUT_SECS, acquire_sandbox_slot, OpenShellStatus,
};
use right_core::test_cleanup;

/// Spawn a sandbox using `policy_yaml` as its startup policy and return its
/// name once READY. Registers panic-hook cleanup so `panic = "abort"` still
/// destroys the sandbox. Caller is responsible for calling
/// [`cleanup_sandbox`] on success paths.
async fn spawn_with_policy(test_name: &str, policy_yaml: &str) -> String {
    let name = format!("right-test-{test_name}");

    // Clean up any leftover from prior SIGKILLed runs.
    test_cleanup::pkill_test_orphans(&name);
    test_cleanup::register_test_sandbox(&name);

    let mtls_dir = match openshell::preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let mut client = openshell::connect_grpc(&mtls_dir)
        .await
        .expect("connect_grpc failed");

    if openshell::sandbox_exists(&mut client, &name)
        .await
        .expect("sandbox_exists RPC failed")
    {
        openshell::delete_sandbox(&name).await;
        openshell::wait_for_deleted(&mut client, &name, 60, 2)
            .await
            .expect("cleanup of leftover sandbox failed");
    }

    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, policy_yaml).expect("write policy.yaml");

    let mut child = openshell::spawn_sandbox(&name, &policy_path, None)
        .expect("codegen policy must be accepted at sandbox creation");
    openshell::wait_for_ready(&mut client, &name, 180, 2)
        .await
        .expect("sandbox with codegen policy did not become READY");

    // Kill the create process — it doesn't exit on its own after READY.
    let _ = child.kill().await;

    // Keep tmp alive until after READY; now safe to drop.
    drop(tmp);

    name
}

async fn exec_true(sandbox_name: &str, mtls_dir: &Path) -> i32 {
    let mut client = openshell::connect_grpc(mtls_dir)
        .await
        .expect("connect_grpc");
    let id = openshell::resolve_sandbox_id(&mut client, sandbox_name)
        .await
        .expect("resolve_sandbox_id");
    let (_out, exit) =
        openshell::exec_in_sandbox(&mut client, &id, &["true"], DEFAULT_EXEC_TIMEOUT_SECS)
            .await
            .expect("exec_in_sandbox");
    exit
}

fn cleanup_sandbox(name: &str) {
    test_cleanup::unregister_test_sandbox(name);
    test_cleanup::delete_sandbox_sync(name);
}

#[tokio::test]
async fn generated_permissive_policy_applies_to_live_openshell() {
    let _slot = acquire_sandbox_slot();

    let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
    let name = spawn_with_policy("policy-apply-permissive", &policy).await;

    let mtls_dir = match openshell::preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let exit = exec_true(&name, &mtls_dir).await;
    cleanup_sandbox(&name);
    assert_eq!(
        exit, 0,
        "sandbox exec failed after creation with permissive codegen policy"
    );
}

#[tokio::test]
async fn generated_restrictive_policy_applies_to_live_openshell() {
    let _slot = acquire_sandbox_slot();

    let policy = generate_policy(8100, &NetworkPolicy::Restrictive, None);
    let name = spawn_with_policy("policy-apply-restrictive", &policy).await;

    let mtls_dir = match openshell::preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        other => panic!("OpenShell not ready: {other:?}"),
    };

    let exit = exec_true(&name, &mtls_dir).await;
    cleanup_sandbox(&name);
    assert_eq!(
        exit, 0,
        "sandbox exec failed after creation with restrictive codegen policy"
    );
}
