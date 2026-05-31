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

    // Right's contract: attaching a generic provider wires its env var into the
    // sandbox's provider environment, observable at the gateway via
    // GetSandboxProviderEnvironment (which the supervisor reads at startup).
    // We assert at the gateway, NOT via in-sandbox `printenv`: the env is
    // injected at supervisor boot, not into ad-hoc gRPC-exec'd processes, so a
    // post-attach `printenv` legitimately sees nothing. We assert the var is
    // present (not its value): on OpenShell v0.0.50 this RPC returns the
    // resolved credential to the trusted supervisor — the
    // `openshell:resolve:env:` placeholder is what the proxy substitutes on
    // egress, not what this RPC returns. (`get_sandbox_provider_environment` is
    // otherwise an unused wrapper; this is its only live coverage.)
    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox.name())
        .await
        .unwrap();
    let env = get_sandbox_provider_environment(&mut client, &sandbox_id)
        .await
        .unwrap();
    let value = env
        .get("RIGHTPROBE_ENVVISIBLE")
        .expect("provider env var must be present at the gateway after attach");
    assert!(
        !value.is_empty(),
        "provider env var must resolve to a non-empty value"
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

    // Assert at the gateway (see ci_openshell_provider_create_attach_env_visible
    // for why not via in-sandbox `printenv`). The point of this test is that a
    // credential rotation propagates WITHOUT recreating/restarting the sandbox:
    // the attachment stays live and the resolved env value changes.
    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox.name())
        .await
        .unwrap();
    let value_first = get_sandbox_provider_environment(&mut client, &sandbox_id)
        .await
        .unwrap()
        .get("ROT_TOKEN")
        .cloned()
        .expect("ROT_TOKEN must be present at the gateway before rotate");

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

    let value_second = get_sandbox_provider_environment(&mut client, &sandbox_id)
        .await
        .unwrap()
        .get("ROT_TOKEN")
        .cloned()
        .expect("ROT_TOKEN must still be present at the gateway after rotate");

    assert_ne!(
        value_first, value_second,
        "resolved env value must change after credential rotation (no restart)"
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
