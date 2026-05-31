//! Live OpenShell gateway tests. Each test is `#[ignore]` (ci-openshell:)
//! and runs only in CI; see AGENTS.md cadence rules.

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_get_delete_roundtrip() {
    use right_openshell::providers::*;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let name = format!("rightprobe-{}-roundtrip", std::process::id());
    let mut creds = std::collections::HashMap::new();
    creds.insert("MY_TOKEN".to_string(), "secret-value".to_string());
    let spec = ProviderSpec {
        name: name.clone(),
        type_: "generic".into(),
        credentials: creds,
        config: Default::default(),
    };
    let created = create_provider(&mut client, &spec).await.unwrap();
    assert_eq!(created.name, name);
    assert_eq!(created.type_, "generic");

    let got = get_provider(&mut client, &name).await.unwrap();
    assert_eq!(got.name, name);

    delete_provider(&mut client, &name).await.unwrap();

    let after = get_provider(&mut client, &name).await;
    assert!(matches!(after, Err(ProviderError::NotFound(_))));
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_attach_detach() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov_name = format!("rightprobe-{pid}-attachprov");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_TOKEN".into(), "secret".into());
    create_provider(
        &mut client,
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
    attach_to_sandbox(&mut client, sandbox.name(), &prov_name)
        .await
        .unwrap();
    detach_from_sandbox(&mut client, sandbox.name(), &prov_name)
        .await
        .unwrap();

    delete_provider(&mut client, &prov_name).await.unwrap();
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_create_attach_env_visible() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-envvisible");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_ENVVISIBLE".into(), "secret".into());
    create_provider(
        &mut client,
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
    attach_to_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();

    let (output, status) = sandbox.exec(&["printenv", "RIGHTPROBE_ENVVISIBLE"]).await;
    assert_eq!(status, 0, "printenv failed: {output}");
    assert!(
        output.starts_with("openshell:resolve:env:"),
        "expected placeholder, got: {output}"
    );

    detach_from_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();
    delete_provider(&mut client, &prov).await.unwrap();
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_rotate_no_restart() {
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-rotate");
    let mut creds = std::collections::HashMap::new();
    creds.insert("ROT_TOKEN".into(), "first".into());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: "generic".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let sandbox = TestSandbox::create("ci-openshell-provider-rotate").await;
    attach_to_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();

    let (output_first, status) = sandbox.exec(&["printenv", "ROT_TOKEN"]).await;
    assert_eq!(status, 0, "printenv failed before rotate: {output_first}");
    let placeholder_first = output_first.trim().to_string();
    assert!(
        placeholder_first.starts_with("openshell:resolve:env:"),
        "expected placeholder before rotate, got: {placeholder_first}"
    );

    let mut creds2 = std::collections::HashMap::new();
    creds2.insert("ROT_TOKEN".into(), "second".into());
    update_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: "generic".into(),
            credentials: creds2,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let (output_second, status) = sandbox.exec(&["printenv", "ROT_TOKEN"]).await;
    assert_eq!(status, 0, "printenv failed after rotate: {output_second}");
    let placeholder_second = output_second.trim().to_string();
    assert!(
        placeholder_second.starts_with("openshell:resolve:env:"),
        "expected placeholder after rotate, got: {placeholder_second}"
    );

    assert_ne!(
        placeholder_first, placeholder_second,
        "placeholder must change after credential rotation (version suffix differs)"
    );

    detach_from_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();
    delete_provider(&mut client, &prov).await.unwrap();
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_policy_hot_apply() {
    use right_codegen::contract::write_apply_with_snapshot;
    use right_codegen::policy::providers_append;
    use right_openshell::test_support::TestSandbox;

    // Generic providers are only ever appended to permissive policies: the
    // provider API rejects generic + restrictive (`NetworkPolicyForbidsGeneric`,
    // enforced by `provider_create_generic_rejected_in_restrictive_mode`), and
    // restrictive mode renders no `# right-providers: insert-above` anchor. Use
    // the permissive base — the real production shape — so this exercises the
    // live `policy set` apply of an appended provider endpoint.
    let base = right_codegen::policy::generate_policy(
        8100,
        &right_agent_config::NetworkPolicy::Permissive,
        right_codegen::policy::HostMcpAccess::BootstrapUnresolved,
    );

    let appended = providers_append(&base, "ci-myagent-acme", "api.acme.invalid", None);
    assert!(
        appended.contains("managed-by: right-providers:ci-myagent-acme"),
        "appended policy must contain provider tag"
    );
    assert!(
        appended.contains("api.acme.invalid"),
        "appended policy must contain new domain"
    );

    // Boot the sandbox WITH `base` so its startup landlock/filesystem policy
    // matches what we hot-apply next. `providers_append` touches only the
    // network section, so `policy set` of `appended` is a network-only change —
    // OpenShell rejects landlock changes on a live sandbox, exactly as the
    // production provider-add path relies on.
    let sandbox =
        TestSandbox::create_with_policy("ci-openshell-provider-policy-hot-apply", &base).await;

    let tmp = tempfile::TempDir::new().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, &base).unwrap();

    let snap = write_apply_with_snapshot(sandbox.name(), &policy_path, appended)
        .await
        .unwrap();

    let on_disk = std::fs::read_to_string(&policy_path).unwrap();
    assert!(
        on_disk.contains("api.acme.invalid"),
        "on-disk policy must contain new domain after hot-apply"
    );

    snap.restore().await.unwrap();

    let restored = std::fs::read_to_string(&policy_path).unwrap();
    assert_eq!(
        restored, base,
        "restored policy must be byte-for-byte identical to base"
    );
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_destroy_cascade() {
    use right_openshell::providers::*;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-cascade");
    let mut creds = std::collections::HashMap::new();
    creds.insert("CASCADE_TOKEN".into(), "value".into());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: prov.clone(),
            type_: "generic".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    delete_provider(&mut client, &prov).await.unwrap();

    let after = get_provider(&mut client, &prov).await;
    assert!(
        matches!(after, Err(ProviderError::NotFound(_))),
        "provider must be NotFound after delete, got: {after:?}"
    );
}
