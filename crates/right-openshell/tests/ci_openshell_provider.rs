//! Live OpenShell gateway tests. Each test is `#[ignore]` (ci-openshell:)
//! and runs only in CI; see AGENTS.md cadence rules.

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_v2_flip() {
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .expect("resolve gateway");
    let first = right_openshell::providers::ensure_v2_enabled(&endpoint)
        .await
        .expect("ensure_v2_enabled #1");
    let second = right_openshell::providers::ensure_v2_enabled(&endpoint)
        .await
        .expect("ensure_v2_enabled #2");
    assert!(second.was_already_on);
    let _ = first;
}
