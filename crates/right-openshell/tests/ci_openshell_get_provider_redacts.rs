//! Live OpenShell regression: `GetProvider` REDACTS credential values.
//! `#[ignore]` (ci-openshell:) — runs only in CI. Pure gateway op (no sandbox).
//!
//! ## What this proves
//!
//! OpenShell's `GetProvider` RPC does NOT return stored credential values — it
//! returns the literal string `"REDACTED"`. There is no host-callable
//! unredacted read (`GetSandboxProviderEnvironment`, which returns real values,
//! requires a sandbox principal the host cannot present).
//!
//! ## Why it matters (the provider-copy bug)
//!
//! Cross-agent provider copy/import (`handle_provider_copy` →
//! `get_provider_credentials` → write to destination) reads the source secret
//! via `GetProvider`. Because `GetProvider` returns `"REDACTED"`, the copy
//! writes the 8-char string `"REDACTED"` as the destination credential. The
//! sandbox resolver then faithfully substitutes `"REDACTED"` on egress → the
//! upstream rejects it (HTTP 401). The SOURCE keeps working (its real value was
//! entered directly and is stored intact); only COPIES break. Every re-copy
//! re-writes `"REDACTED"`.
//!
//! Any host-side feature that depends on reading a credential back MUST NOT use
//! `get_provider_credentials` for the value. See
//! `/tmp/provider-copy-investigation-handoff.md` §14.
//!
//! ## Canary semantics
//!
//! Asserts the CURRENT behavior (read-back is redacted). If OpenShell ever adds
//! an operator reveal path and this returns the real value, the assertion flips
//! — revisit the copy design.

use right_openshell::providers::{
    ProviderSpec, create_provider, delete_provider, ensure_v2_enabled, get_provider_credentials,
};
use secrecy::ExposeSecret;

#[tokio::test]
#[ignore = "ci-openshell: requires a live OpenShell gateway"]
async fn ci_openshell_get_provider_redacts_credentials() {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .unwrap();
    ensure_v2_enabled(&mut client).await.ok();

    let pid = std::process::id();
    let name = format!("rightprobe-redact-{pid}");
    // 55-char known secret; clearly longer than the 8-char "REDACTED" sentinel.
    let secret = format!("falid{pid:08}-aaaa-bbbb-cccc:0123456789abcdef0123456789");
    assert!(secret.len() > 8);

    let _ = delete_provider(&mut client, &name).await;
    let mut creds = std::collections::HashMap::new();
    creds.insert("FAL_KEY".to_string(), secret.clone());
    create_provider(
        &mut client,
        &ProviderSpec {
            name: name.clone(),
            type_: "right-fal".into(),
            credentials: creds,
            config: Default::default(),
        },
    )
    .await
    .unwrap();

    let got = get_provider_credentials(&mut client, &name).await.unwrap();
    let read_back = got
        .get("FAL_KEY")
        .map(|s| s.expose_secret().to_string())
        .unwrap_or_default();

    delete_provider(&mut client, &name).await.ok();

    // The gateway redacts: read-back is NOT the stored value.
    assert_ne!(
        read_back, secret,
        "GetProvider unexpectedly returned the real credential — OpenShell may \
         have added an operator reveal path; revisit the copy design"
    );
    // Specifically, it returns the literal "REDACTED" sentinel.
    assert_eq!(
        read_back,
        "REDACTED",
        "expected the redaction sentinel from GetProvider; got a {}-char value",
        read_back.len()
    );
}
