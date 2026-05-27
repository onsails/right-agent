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

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_get_delete_roundtrip() {
    use right_openshell::providers::*;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let name = format!("rightprobe-{}-roundtrip", std::process::id());
    let mut creds = std::collections::HashMap::new();
    creds.insert("MY_TOKEN".to_string(), "secret-value".to_string());
    let spec = ProviderSpec {
        name: name.clone(),
        type_: "generic".into(),
        credentials: creds,
        config: Default::default(),
    };
    let created = create_provider(&endpoint, &spec).await.unwrap();
    assert_eq!(created.name, name);
    assert_eq!(created.type_, "generic");

    let got = get_provider(&endpoint, &name).await.unwrap();
    assert_eq!(got.name, name);

    delete_provider(&endpoint, &name).await.unwrap();

    let after = get_provider(&endpoint, &name).await;
    assert!(matches!(after, Err(ProviderError::NotFound(_))));
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_attach_detach() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let pid = std::process::id();
    let prov_name = format!("rightprobe-{pid}-attachprov");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_TOKEN".into(), "secret".into());
    create_provider(
        &endpoint,
        &ProviderSpec {
            name: prov_name.clone(),
            type_: "generic".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let sandbox = TestSandbox::create("ci-openshell-provider-attach-detach").await;
    attach_to_sandbox(&endpoint, sandbox.name(), &prov_name)
        .await
        .unwrap();
    detach_from_sandbox(&endpoint, sandbox.name(), &prov_name)
        .await
        .unwrap();

    delete_provider(&endpoint, &prov_name).await.unwrap();
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_attach_env_visible() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let endpoint = right_openshell::openshell::resolve_gateway_endpoint()
        .await
        .unwrap();
    let _ = ensure_v2_enabled(&endpoint).await.unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-envvisible");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_ENVVISIBLE".into(), "secret".into());
    create_provider(
        &endpoint,
        &ProviderSpec {
            name: prov.clone(),
            type_: "generic".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();
    let sandbox = TestSandbox::create("ci-openshell-provider-env-visible").await;
    attach_to_sandbox(&endpoint, sandbox.name(), &prov)
        .await
        .unwrap();

    let (output, status) = sandbox.exec(&["printenv", "RIGHTPROBE_ENVVISIBLE"]).await;
    assert_eq!(status, 0, "printenv failed: {output}");
    assert!(
        output.starts_with("openshell:resolve:env:"),
        "expected placeholder, got: {output}"
    );

    detach_from_sandbox(&endpoint, sandbox.name(), &prov)
        .await
        .unwrap();
    delete_provider(&endpoint, &prov).await.unwrap();
}
