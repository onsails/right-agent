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
        .get_provider(proto_v1::GetProviderRequest {
            name: name.clone(),
            workspace: String::new(),
        })
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
        .get_provider(proto_v1::GetProviderRequest {
            name: name.clone(),
            workspace: String::new(),
        })
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
            &["example.invalid".to_string()],
            None,
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
                credential_handles: Default::default(),
                profile_workspace: String::new(),
            }),
            credential_expires_at_ms: Default::default(),
            workspace: String::new(),
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
    use right_openshell::managed_profiles::{
        author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
    };
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-envvisible");
    // OpenShell v0.0.105 has no implicit `generic` profile: without one the
    // gateway logs "provider type has no profile; skipping provider policy
    // layer" and no env var is injected. Author + import a minimal profile.
    let profile_id = generic_provider_profile_id(&prov);
    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, &profile_id).await;
    lint_and_import(
        &mut client,
        author_generic_profile(
            &profile_id,
            &[ECHO_HOST.to_string()],
            None,
            "RIGHTPROBE_ENVVISIBLE",
        ),
    )
    .await
    .expect("import env-visible profile");
    let mut creds = std::collections::HashMap::new();
    creds.insert("RIGHTPROBE_ENVVISIBLE".into(), "secret".into());
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
    use right_openshell::managed_profiles::{
        author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
    };
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-rotate");
    // No implicit `generic` profile on v0.0.105 — import one so env
    // injection/rotation is exercised.
    let profile_id = generic_provider_profile_id(&prov);
    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, &profile_id).await;
    lint_and_import(
        &mut client,
        author_generic_profile(&profile_id, &[ECHO_HOST.to_string()], None, "ROT_TOKEN"),
    )
    .await
    .expect("import rotate profile");
    let mut creds = std::collections::HashMap::new();
    creds.insert("ROT_TOKEN".into(), "first".into());
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
            type_: profile_id.clone(),
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

/// Same filesystem/landlock section as `test_support::MINIMAL_POLICY` so a
/// later `policy set --wait` (composition reload) is accepted on the live
/// sandbox. Network section is intentionally minimal — provider-profile
/// composition adds the upstream endpoint. OpenShell v0.0.97+ rejects
/// endpoint overlap on a port with conflicting metadata (egress-pipeline
/// consolidation, NVIDIA/OpenShell#2373): any 443 endpoint we declare here
/// (even `tls: skip` + `allowed_ips`) conflicts with the composed provider
/// endpoint, so port 443 belongs to the provider profile alone. Port 80
/// stays as the connectivity floor.
const EXPERIMENT_POLICY: &str = "\
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
      - port: 80
        allowed_ips:
          - \"1.1.1.1/32\"
        tls: skip
    binaries:
      - path: \"**\"
";

// Echo host reflects the received Authorization header. Hardcoded (no string
// interpolation) so the egress probe argv is a fixed literal.
const ECHO_HOST: &str = "postman-echo.com";
const PROBE_ENV_VAR: &str = "PROBE_TOKEN";
const EGRESS_PROBE_CMD: &str =
    "curl -sk --max-time 30 -H \"Authorization: Bearer $PROBE_TOKEN\" https://postman-echo.com/get";

/// End-to-end RESOLUTION guard for a generic provider. The pre-existing tests
/// only asserted the `openshell:resolve:env:` placeholder becomes *visible*
/// after attach — they never verified the proxy actually *substitutes* it on
/// egress. A provider can compose and inject a visible placeholder yet still
/// fail to resolve (the production symptom that motivated these tests: a 401
/// because the placeholder reached the upstream unsubstituted). This test
/// composes a generic provider for a header-reflecting echo host and asserts
/// the egress request carries the real credential, not the placeholder.
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_generic_provider_substitutes_on_egress() {
    use right_openshell::managed_profiles::{
        author_generic_profile, delete_profile, generic_provider_profile_id, lint_and_import,
    };
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;

    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();
    ensure_v2_enabled(&mut client).await.unwrap();

    let pid = std::process::id();
    let secret = format!("rightprobe-SECRET-{pid}");
    let prov = format!("rightprobe-{pid}-egress");
    let profile_id = generic_provider_profile_id(&prov);

    let _ = delete_provider(&mut client, &prov).await;
    let _ = delete_profile(&mut client, &profile_id).await;

    lint_and_import(
        &mut client,
        author_generic_profile(&profile_id, &[ECHO_HOST.to_string()], None, PROBE_ENV_VAR),
    )
    .await
    .expect("import echo profile");

    let mut creds = std::collections::HashMap::new();
    creds.insert(PROBE_ENV_VAR.to_string(), secret.clone());
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
    .unwrap();

    let sandbox =
        TestSandbox::create_with_policy("ci-openshell-egress-experiment", EXPERIMENT_POLICY).await;
    attach_to_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, EXPERIMENT_POLICY).unwrap();
    right_openshell::openshell::ensure_provider_policy_loaded(sandbox.name(), &policy_path)
        .await
        .expect("reload composition");
    right_openshell::openshell::wait_for_provider_composed_with_endpoint(
        &mut client,
        sandbox.name(),
        &prov,
        ECHO_HOST,
        "",
    )
    .await
    .expect("composition confirmed");

    // Placeholder must be visible (the weak guarantee current code gives).
    let placeholder = poll_sandbox_env(&sandbox, PROBE_ENV_VAR, 30, is_provider_placeholder)
        .await
        .expect("placeholder visible after attach");
    eprintln!("PLACEHOLDER: {placeholder}");

    // END-TO-END: does the proxy actually substitute on egress? The echo host
    // reflects the received Authorization header back to us.
    let (out, rc) = sandbox.exec(&["sh", "-c", EGRESS_PROBE_CMD]).await;
    eprintln!("EGRESS rc={rc} body:\n{out}");

    let substituted = out.contains(&secret);
    let placeholder_leaked = out.contains("openshell:resolve:env:");
    eprintln!("SUBSTITUTED={substituted} PLACEHOLDER_LEAKED={placeholder_leaked}");

    detach_from_sandbox(&mut client, sandbox.name(), &prov)
        .await
        .ok();
    delete_provider(&mut client, &prov).await.ok();
    delete_profile(&mut client, &profile_id).await.ok();

    assert!(
        substituted && !placeholder_leaked,
        "proxy must substitute the placeholder on egress (substituted={substituted}, leaked={placeholder_leaked})"
    );
}

// Probe FAL_KEY (built-in provider) against fal's own endpoint. OpenShell
// v0.0.103+ binds static credentials to their provider endpoints
// (`credential_endpoint_mismatch` on any other host), so the old cross-host
// echo probe can no longer observe the substituted value. The fal profile
// terminates TLS on `fal.run`; an unknown path returns a fal router 404
// page, which contains the placeholder only if substitution did NOT happen.
const EGRESS_PROBE_FAL_CMD: &str = "curl -sk --max-time 30 -H \"Authorization: Key $FAL_KEY\" https://fal.run/api/v0/nonexistent-probe";

/// End-to-end RESOLUTION guard for a BUILT-IN provider (`right-fal`), the
/// category that failed in production. Asserts the FAL_KEY placeholder never
/// reaches the wire on fal's own endpoint (it is substituted) — cross-host
/// observation is impossible by design since v0.0.103 endpoint binding.
#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_builtin_provider_substitutes_on_egress() {
    use right_openshell::managed_profiles::{
        ManagedProfile, author_generic_profile, delete_profile, ensure_profiles, fal_profile,
        generic_provider_profile_id, lint_and_import,
    };
    use right_openshell::providers::*;
    use right_openshell::test_support::TestSandbox;

    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();
    ensure_v2_enabled(&mut client).await.unwrap();

    let pid = std::process::id();
    let fal_secret = format!("falsecret-{pid}");
    let echo_prov = format!("rightprobe-{pid}-echo");
    let echo_profile_id = generic_provider_profile_id(&echo_prov);
    let fal_prov = format!("rightprobe-{pid}-fal");

    let _ = delete_provider(&mut client, &echo_prov).await;
    let _ = delete_provider(&mut client, &fal_prov).await;
    let _ = delete_profile(&mut client, &echo_profile_id).await;

    // Observation endpoint: generic echo provider (env var unused by us, but it
    // composes postman-echo.com as a terminated L7 endpoint).
    lint_and_import(
        &mut client,
        author_generic_profile(&echo_profile_id, &[ECHO_HOST.to_string()], None, "ECHO_OBS"),
    )
    .await
    .expect("import echo profile");
    let mut echo_creds = std::collections::HashMap::new();
    echo_creds.insert("ECHO_OBS".to_string(), "unused".to_string());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: echo_prov.clone(),
            type_: echo_profile_id.clone(),
            credentials: echo_creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    // Subject: BUILT-IN fal provider. Production provisions the `right-fal`
    // profile on the startup/reconcile path (`ensure_profiles` in
    // `sandbox_supervisor`); a fresh CI gateway has none persisted, so the test
    // provisions it itself. `ensure_profiles` is create-or-skip — safe whether
    // or not the profile already exists (the persistent dev gateway has it).
    ensure_profiles(
        &mut client,
        &[ManagedProfile::Authored(Box::new(fal_profile()))],
    )
    .await
    .expect("ensure right-fal built-in profile");
    let mut fal_creds = std::collections::HashMap::new();
    fal_creds.insert("FAL_KEY".to_string(), fal_secret.clone());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: fal_prov.clone(),
            type_: "right-fal".into(),
            credentials: fal_creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let sandbox =
        TestSandbox::create_with_policy("ci-openshell-builtin-fal-experiment", EXPERIMENT_POLICY)
            .await;
    attach_to_sandbox(&mut client, sandbox.name(), &echo_prov)
        .await
        .unwrap();
    attach_to_sandbox(&mut client, sandbox.name(), &fal_prov)
        .await
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("policy.yaml");
    std::fs::write(&policy_path, EXPERIMENT_POLICY).unwrap();
    right_openshell::openshell::ensure_provider_policy_loaded(sandbox.name(), &policy_path)
        .await
        .expect("reload composition");
    right_openshell::openshell::wait_for_provider_composed_with_endpoint(
        &mut client,
        sandbox.name(),
        &echo_prov,
        ECHO_HOST,
        "",
    )
    .await
    .expect("echo composition confirmed");
    right_openshell::openshell::wait_for_provider_composed(&mut client, sandbox.name(), &fal_prov)
        .await
        .expect("fal composition confirmed");

    let placeholder = poll_sandbox_env(&sandbox, "FAL_KEY", 30, is_provider_placeholder)
        .await
        .expect("FAL_KEY placeholder visible after attach");
    eprintln!("FAL_KEY PLACEHOLDER: {placeholder}");

    let (out, rc) = sandbox.exec(&["sh", "-c", EGRESS_PROBE_FAL_CMD]).await;
    eprintln!("FAL EGRESS rc={rc} body:\n{out}");
    // With endpoint binding (v0.0.103+) the credential value cannot be
    // observed off-host; what we CAN prove is that the raw placeholder never
    // reaches the wire on fal's own endpoint (substitution happened).
    let placeholder_leaked = out.contains("openshell:resolve:env:");
    eprintln!("FAL PLACEHOLDER_LEAKED={placeholder_leaked}");

    detach_from_sandbox(&mut client, sandbox.name(), &fal_prov)
        .await
        .ok();
    detach_from_sandbox(&mut client, sandbox.name(), &echo_prov)
        .await
        .ok();
    delete_provider(&mut client, &fal_prov).await.ok();
    delete_provider(&mut client, &echo_prov).await.ok();
    delete_profile(&mut client, &echo_profile_id).await.ok();

    assert!(
        !placeholder_leaked,
        "built-in fal FAL_KEY placeholder must not reach the wire on fal.run (leaked={placeholder_leaked})"
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
