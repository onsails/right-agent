//! Live OpenShell gateway tests. Each test is `#[ignore]` (ci-openshell:)
//! and runs only in CI; see AGENTS.md cadence rules.

/// Poll a sandbox's environment for `var` until its value satisfies `accept`,
/// or the timeout elapses (returns `None`).
///
/// Provider env propagates to a *running* sandbox a short time AFTER the
/// attach/update gateway call returns — empirically ~0.6-0.9s for an attach
/// and several seconds for a credential rotation. A single immediate
/// `printenv` races that propagation and reads nothing (this is exactly why
/// the pre-poll versions of these tests flaked as "printenv failed"). The
/// sandbox always sees the opaque `openshell:resolve:env:v<fp>_<NAME>`
/// placeholder, never the raw credential (the proxy substitutes the real value
/// on outbound HTTPS); `GetSandboxProviderEnvironment` returns the raw value
/// and is for the trusted supervisor, not the sandbox.
async fn poll_sandbox_env(
    sandbox: &right_openshell::test_support::TestSandbox,
    var: &str,
    timeout_secs: u64,
    accept: impl Fn(&str) -> bool,
) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let (out, rc) = sandbox.exec(&["printenv", var]).await;
        let val = out.trim();
        if rc == 0 && accept(val) {
            return Some(val.to_string());
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

fn is_provider_placeholder(v: &str) -> bool {
    v.starts_with("openshell:resolve:env:")
}

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

    // Right's contract: attaching a generic provider makes its env var visible
    // inside the running sandbox (no restart) as an opaque
    // `openshell:resolve:env:` placeholder. Propagation is not instantaneous —
    // poll rather than read once (see `poll_sandbox_env`).
    let placeholder = poll_sandbox_env(
        &sandbox,
        "RIGHTPROBE_ENVVISIBLE",
        30,
        is_provider_placeholder,
    )
    .await
    .expect("provider env var must become a placeholder inside the sandbox after attach");
    // Credential isolation: the sandbox must NEVER see the raw credential value
    // ("secret"); only the proxy resolves the placeholder on egress.
    assert!(
        !placeholder.contains("secret"),
        "sandbox must see the placeholder, never the raw credential: {placeholder}"
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

    // Rotation must propagate to the live sandbox WITHOUT recreating/restarting
    // it. The placeholder embeds a credential-input fingerprint
    // (`openshell:resolve:env:v<fp>_NAME`); rotating the credential changes the
    // fingerprint, so the in-sandbox placeholder changes. Poll both reads:
    // attach and (especially) rotation propagate after a delay.
    let placeholder_first = poll_sandbox_env(&sandbox, "ROT_TOKEN", 30, is_provider_placeholder)
        .await
        .expect("ROT_TOKEN must become a placeholder in the sandbox before rotate");

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

    let placeholder_second = poll_sandbox_env(&sandbox, "ROT_TOKEN", 30, |v| {
        is_provider_placeholder(v) && v != placeholder_first
    })
    .await
    .expect("placeholder must change in the sandbox after credential rotation (no restart)");

    assert_ne!(
        placeholder_first, placeholder_second,
        "placeholder must change after credential rotation (no restart)"
    );

    detach_from_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();
    delete_provider(&mut client, &prov).await.unwrap();
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
