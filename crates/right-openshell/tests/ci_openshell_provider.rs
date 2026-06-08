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
async fn ci_openshell_provider_update_empty_maps_preserves_existing_fields() {
    use right_openshell::openshell_proto::openshell::v1 as proto_v1;
    use right_openshell::providers::*;

    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let name = format!("rightprobe-{}-sparse-update", std::process::id());
    let _ = delete_provider(&mut client, &name).await;

    let mut creds = std::collections::HashMap::new();
    creds.insert("SPARSE_TOKEN".to_string(), "first".to_string());
    let mut config = std::collections::HashMap::new();
    config.insert("origin".to_string(), "https://example.invalid".to_string());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: name.clone(),
            type_: "generic".into(),
            credentials: creds,
            config,
        },
    )
    .await
    .unwrap();

    let raw = client
        .get_provider(proto_v1::GetProviderRequest { name: name.clone() })
        .await
        .unwrap()
        .into_inner()
        .provider
        .expect("provider response");
    let credential_available = !raw.credentials.is_empty();
    let config_available = raw
        .config
        .get("origin")
        .is_some_and(|value| value == "https://example.invalid");

    update_provider(
        &mut client,
        &ProviderSpec {
            name: name.clone(),
            type_: "generic".into(),
            credentials: Default::default(),
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let after_empty_update = client
        .get_provider(proto_v1::GetProviderRequest { name: name.clone() })
        .await
        .unwrap()
        .into_inner()
        .provider
        .expect("provider response after empty update");
    let credential_preserved = !after_empty_update.credentials.is_empty();
    let config_preserved = after_empty_update
        .config
        .get("origin")
        .is_some_and(|value| value == "https://example.invalid");

    delete_provider(&mut client, &name).await.unwrap();

    assert!(
        credential_available,
        "raw GetProvider must expose existing credentials for repair echo"
    );
    assert!(
        config_available,
        "raw GetProvider must expose existing config for repair echo"
    );
    assert!(
        credential_preserved,
        "UpdateProvider with empty credentials must preserve existing gateway credentials"
    );
    assert!(
        config_preserved,
        "UpdateProvider with empty config must preserve existing gateway config"
    );
}

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_provider_update_rejects_type_change() {
    use right_openshell::managed_profiles::{
        author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
    };
    use right_openshell::openshell_proto::openshell::datamodel::v1 as datamodel;
    use right_openshell::openshell_proto::openshell::v1 as proto_v1;
    use right_openshell::providers::*;

    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let name = format!("rightprobe-{}-type-change", std::process::id());
    let profile_id = generic_provider_profile_id(&name);
    let _ = delete_provider(&mut client, &name).await;
    let _ = delete_profile(&mut client, &profile_id).await;

    lint_and_import(
        &mut client,
        author_generic_profile(
            &profile_id,
            "example.invalid",
            None,
            "Authorization",
            "TYPECHANGE_TOKEN",
        ),
    )
    .await
    .expect("import throwaway target profile");

    let mut creds = std::collections::HashMap::new();
    creds.insert("TYPECHANGE_TOKEN".to_string(), "first".to_string());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: name.clone(),
            type_: "generic".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let err = client
        .update_provider(proto_v1::UpdateProviderRequest {
            provider: Some(datamodel::Provider {
                metadata: Some(datamodel::ObjectMeta {
                    name: name.clone(),
                    ..Default::default()
                }),
                r#type: profile_id.clone(),
                credentials: Default::default(),
                config: Default::default(),
                credential_expires_at_ms: Default::default(),
            }),
            credential_expires_at_ms: Default::default(),
        })
        .await
        .err()
        .expect("OpenShell must reject provider type changes through UpdateProvider");

    delete_provider(&mut client, &name).await.unwrap();
    let _ = delete_profile(&mut client, &profile_id).await;

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(
        err.message().contains("provider type cannot be changed"),
        "{err}"
    );
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
