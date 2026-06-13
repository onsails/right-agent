# Generic Provider Profile Update + Self-Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an already-provisioned OpenShell provider profile updatable when its endpoints genuinely change, stop spurious re-imports on behaviorally-inert credential fields, and self-heal drift automatically on `right up`, supervisor reconcile, and dashboard config-update — so `right up` no longer aborts with `custom provider profile '...' already exists`.

**Architecture:** OpenShell `import` never upserts and the gateway refuses to delete a profile while a sandbox references it (verified live: `provider profile '...' is in use by sandboxes: ...`). The only working update is **detach every referencing attachment → delete → re-import (same id) → re-attach**, which preserves the provider's gateway-only secret (detach/attach never carry it). We (1) drop inert credential fields from the drift fingerprint, (2) change `ensure_profiles` from "re-import on drift" (impossible) to "create-or-skip + report drift", (3) add a `update_referenced_profile` primitive in `providers.rs` that performs the detach-dance with rollback, and (4) call it from the three context-aware sites (`right up`, supervisor, dashboard).

**Tech Stack:** Rust 2024, tonic gRPC (vendored `openshell.v1`), `thiserror`, `MockOpenShell` in-process gRPC for unit tests, live `ci_openshell_` gateway tests.

---

## Background — facts established by a live probe (do not re-litigate)

A throwaway live probe (`TestSandbox` + throwaway provider/profile) established:

- **P1 = REFUSED.** `delete_provider_profile` fails while a sandbox references the profile:
  `The system is not in a state required ...: provider profile '...' is in use by sandboxes: <sandbox>`.
  The reference that blocks delete is the **sandbox attachment**, not the provider object.
- **Working update path = `detach → delete → import → attach` + policy reload.** After detaching the
  provider from the sandbox, the profile deletes, re-imports (same id, new endpoints), the provider
  re-attaches, and `get_effective_policy` (`GetSandboxConfig`) shows the new endpoint set. The provider
  **secret survives** (placeholder still present) — it is never re-supplied.
- `datamodel.v1.Provider` carries only `type` (= profile id) + credentials + config, **no endpoints**:
  reachability lives entirely in the profile, so updating the profile + reloading composition is
  sufficient; the provider object is never recreated.

Root cause of the `right up` break: commit `7941bad7` (merged in `ac48503c`) changed
`author_generic_profile` to emit fixed `auth_style: "bearer"` / `header_name: "Authorization"`. Already
-provisioned generic profiles store the old header-derived values, so the drift fingerprint flags them and
`ensure_profiles` attempts an impossible re-import. These fields are **inert** for OpenShell static-key
substitution (keyed by env-var name; the agent writes the real auth header — see
`docs/architecture/providers.md` and ARCHITECTURE.md "Provider credential isolation").

## Scope

In scope: generic providers and built-in managed profiles (`right-github`, `right-fal`) updated via the
detach-dance wherever a caller has sandbox+provider context.

**Non-goals (explicit):**
- No content-addressed profile ids (ids stay `hash(provider_name)`; `8577cfb5` kept them stable).
- No provider recreation (secret is gateway-only; only detach/attach are used).
- A profile referenced by a sandbox **not** in the caller's known attachment list cannot be healed by
  that caller (delete stays refused) — this surfaces as a propagated error, not silent corruption. In
  practice each generic profile is referenced by exactly one provider on one sandbox.

## File Structure

- `crates/right-openshell/src/managed_profiles.rs` — Fix A (fingerprint), `EnsureOutcome::DriftedSkipped`,
  `ensure_profiles` create-or-skip. Profile-only; does NOT gain attach/detach calls.
- `crates/right-openshell/src/providers.rs` — new `ProfileAttachment` + `update_referenced_profile`
  primitive (orchestrates profile RPCs via `managed_profiles` + its own attach/detach). This is the only
  module allowed to span both surfaces.
- `crates/right-openshell/src/test_mock_server.rs` — wire profile RPCs (`lint`/`import`/`get`/`delete`)
  to optional `mock_*` closures so the primitive and `ensure_profiles` get fast unit tests.
- `crates/right-openshell/src/providers_tests.rs` — unit tests for `update_referenced_profile`
  (order + rollback) and `ensure_profiles` (DriftedSkipped, no doomed import).
- `crates/right-openshell/tests/ci_openshell_generic_provider.rs` — live contract test for the primitive.
- `crates/bot/src/sandbox_supervisor.rs` — self-heal in `reconcile_and_confirm_providers`.
- `crates/right/src/internal_api_providers.rs` — dashboard config-update uses the primitive.
- `crates/right/src/main.rs` — `right up` self-heals drifted profiles after bulk ensure.

---

## Task 1: Fingerprint excludes inert credential fields (Fix A)

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs` (type `CredentialFp` ~line 267; `fingerprint` ~line 340)
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/right-openshell/src/managed_profiles.rs` (near the other
`needs_import_*` tests):

```rust
#[test]
fn needs_import_false_when_only_inert_credential_fields_differ() {
    // auth_style/header_name/query_param are inert for OpenShell static-key
    // substitution (keyed by env-var name; the agent writes the real auth
    // header). They MUST NOT be a drift signal: re-importing an existing id is
    // rejected by the gateway and a referenced profile cannot be deleted, so
    // spurious drift on these fields would wedge `right up`.
    let desired = author_generic_profile(
        "right-provider-x",
        &["api.acme.com".to_string()],
        None,
        "ACME_TOKEN",
    );
    let mut stored_old_inert = desired.clone();
    stored_old_inert.credentials[0].auth_style = "header".into();
    stored_old_inert.credentials[0].header_name = "x-api-key".into();
    stored_old_inert.credentials[0].query_param = "token".into();
    assert!(
        !needs_import(Some(&stored_old_inert), &desired),
        "inert credential fields must not force a re-import"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell --lib needs_import_false_when_only_inert`
Expected: FAIL on the assertion ("inert credential fields must not force a re-import").

- [ ] **Step 3: Narrow `CredentialFp`**

Replace the `CredentialFp` type and its doc comment (~line 267-269):

```rust
/// One credential fingerprint: `(name, sorted env vars, required)`.
///
/// `auth_style`/`header_name`/`query_param` are intentionally excluded: they are
/// inert for OpenShell static-key substitution (keyed by env-var name; the agent
/// writes the real auth header), so they carry no behavioral meaning. Including
/// them would make an already-provisioned profile read as drifted whenever the
/// authored placement changed — an unfixable churn, since the gateway rejects
/// re-importing an existing id and a referenced profile cannot be deleted.
type CredentialFp = (String, Vec<String>, bool);
```

Update the credential closure inside `fingerprint` (~line 343-356) to:

```rust
    let mut credentials: Vec<_> = p
        .credentials
        .iter()
        .map(|c| {
            let mut env_vars = c.env_vars.clone();
            env_vars.sort();
            (c.name.clone(), env_vars, c.required)
        })
        .collect();
    credentials.sort();
```

- [ ] **Step 4: Run test to verify it passes (and no regression)**

Run: `devenv shell -- cargo test -p right-openshell --lib needs_import`
Expected: PASS — all `needs_import_*` tests green, including the new one.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "fix(openshell): drop inert credential fields from profile drift fingerprint"
```

---

## Task 2: Wire profile RPCs in MockOpenShell

Enables unit-testing the profile-update logic without a live gateway.

**Files:**
- Modify: `crates/right-openshell/src/test_mock_server.rs` (struct ~line 28; impl methods ~line 364-396)

- [ ] **Step 1: Add mock fields to `MockOpenShell`**

In the `MockOpenShell` struct (after `mock_get_sandbox_config`, before the closing `}` ~line 84), add:

```rust
    pub(crate) mock_get_provider_profile: Option<
        UnaryMockFn<
            os_proto::v1::GetProviderProfileRequest,
            os_proto::v1::ProviderProfileResponse,
        >,
    >,
    pub(crate) mock_lint_provider_profiles: Option<
        UnaryMockFn<
            os_proto::v1::LintProviderProfilesRequest,
            os_proto::v1::LintProviderProfilesResponse,
        >,
    >,
    pub(crate) mock_import_provider_profiles: Option<
        UnaryMockFn<
            os_proto::v1::ImportProviderProfilesRequest,
            os_proto::v1::ImportProviderProfilesResponse,
        >,
    >,
    pub(crate) mock_delete_provider_profile: Option<
        UnaryMockFn<
            os_proto::v1::DeleteProviderProfileRequest,
            os_proto::v1::DeleteProviderProfileResponse,
        >,
    >,
```

- [ ] **Step 2: Route the trait methods to the closures**

Replace the four hardcoded stub methods (`get_provider_profile` ~371, `import_provider_profiles` ~378,
`lint_provider_profiles` ~385, `delete_provider_profile` ~392) so each dispatches like the Provider CRUD
methods. Example for `get_provider_profile` (apply the same shape to the other three, using the matching
`mock_*` field and request/response types):

```rust
    async fn get_provider_profile(
        &self,
        request: tonic::Request<os_proto::v1::GetProviderProfileRequest>,
    ) -> Result<tonic::Response<os_proto::v1::ProviderProfileResponse>, tonic::Status> {
        match &self.mock_get_provider_profile {
            Some(f) => f(request.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
```

`import_provider_profiles` → `mock_import_provider_profiles` (resp `ImportProviderProfilesResponse`),
`lint_provider_profiles` → `mock_lint_provider_profiles` (resp `LintProviderProfilesResponse`),
`delete_provider_profile` → `mock_delete_provider_profile` (resp `DeleteProviderProfileResponse`).
Leave `list_provider_profiles` as the hardcoded stub (unused here).

- [ ] **Step 3: Verify the crate still compiles**

Run: `devenv shell -- cargo test -p right-openshell --lib --no-run`
Expected: compiles (no test run yet).

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/src/test_mock_server.rs
git commit -m "test(openshell): mock provider-profile RPCs in MockOpenShell"
```

---

## Task 3: `ensure_profiles` becomes create-or-skip + reports drift

OpenShell `import` cannot update an existing id, so `ensure_profiles` must never attempt a re-import.
It imports absent profiles, skips unchanged ones, and reports drift via a new outcome for the
context-aware caller to heal.

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs` (`EnsureOutcome` ~line 474; `ensure_profiles` ~line 487-522)
- Test: `crates/right-openshell/src/providers_tests.rs`

- [ ] **Step 1: Add the `DriftedSkipped` outcome**

In `EnsureOutcome` (~line 474), add a variant with a doc line:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    Imported(String),
    Unchanged(String),
    /// Base profile was absent on the gateway — profile skipped (non-fatal).
    Skipped(String),
    /// Profile exists but drifted. `ensure_profiles` cannot update it (OpenShell
    /// rejects re-importing an existing id, and a referenced profile cannot be
    /// deleted without sandbox context). A context-aware caller must run
    /// `providers::update_referenced_profile` to heal it.
    DriftedSkipped(String),
}
```

- [ ] **Step 2: Change the drift branch to report, not re-import**

In `ensure_profiles` (~line 511-519), replace the `needs_import` branch:

```rust
        let stored = get_profile(client, &id).await?;
        match stored {
            None => {
                lint_and_import(client, desired).await?;
                tracing::info!(profile = id, "managed profile absent → imported");
                outcomes.push(EnsureOutcome::Imported(id));
            }
            Some(stored) if fingerprint(&stored) != fingerprint(&desired) => {
                tracing::warn!(
                    profile = id,
                    "managed profile drifted — needs detach-dance update (see update_referenced_profile)"
                );
                outcomes.push(EnsureOutcome::DriftedSkipped(id));
            }
            Some(_) => {
                tracing::debug!(profile = id, "managed profile unchanged");
                outcomes.push(EnsureOutcome::Unchanged(id));
            }
        }
```

(The `needs_import` free function may stay for tests; it is no longer the control-flow hinge here.)

- [ ] **Step 3: Write the failing unit test**

Add to `crates/right-openshell/src/providers_tests.rs` (it already imports `MockOpenShell`,
`mock_client`, `start_mock_server`). Add any missing imports: `use crate::managed_profiles::{author_generic_profile, ensure_profiles, EnsureOutcome, ManagedProfile};` and
`use crate::openshell_proto::openshell::v1 as os_v1;`.

```rust
#[tokio::test]
async fn ensure_profiles_reports_drift_without_reimport() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let import_calls = Arc::new(AtomicUsize::new(0));
    let import_calls_c = Arc::clone(&import_calls);

    // Stored profile differs from desired on a MEANINGFUL field (env var), so the
    // fingerprints differ → drift. Import must NOT be attempted.
    let desired = author_generic_profile("right-provider-x", &["api.acme.com".into()], None, "NEW_KEY");
    let mut stored = desired.clone();
    stored.credentials[0].env_vars = vec!["OLD_KEY".into()];

    let mock = MockOpenShell {
        mock_get_provider_profile: Some(Box::new(move |_req| {
            Ok(proto_v1::ProviderProfileResponse { profile: Some(stored.clone()) })
        })),
        mock_import_provider_profiles: Some(Box::new(move |_req| {
            import_calls_c.fetch_add(1, Ordering::SeqCst);
            Ok(proto_v1::ImportProviderProfilesResponse::default())
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let managed = ManagedProfile::Authored(Box::new(desired));
    let outcomes = ensure_profiles(&mut client, &[managed]).await.unwrap();

    assert_eq!(outcomes.len(), 1);
    assert!(matches!(outcomes[0], EnsureOutcome::DriftedSkipped(_)), "got {:?}", outcomes[0]);
    assert_eq!(import_calls.load(Ordering::SeqCst), 0, "drift must NOT trigger an import");
}
```

- [ ] **Step 4: Run the test**

Run: `devenv shell -- cargo test -p right-openshell --lib ensure_profiles_reports_drift_without_reimport`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs crates/right-openshell/src/providers_tests.rs
git commit -m "refactor(openshell): ensure_profiles reports drift instead of doomed re-import"
```

---

## Task 4: `update_referenced_profile` primitive + rollback

**Files:**
- Modify: `crates/right-openshell/src/providers.rs` (add `ProfileAttachment` + `update_referenced_profile` near the attach/detach section ~line 378)
- Test: `crates/right-openshell/src/providers_tests.rs`

- [ ] **Step 1: Add the primitive**

In `crates/right-openshell/src/providers.rs`, after `list_attached` (~line 398), add:

```rust
/// A sandbox attachment that references a managed profile.
#[derive(Debug, Clone)]
pub struct ProfileAttachment {
    pub sandbox_name: String,
    pub provider_name: String,
}

/// Update a managed/authored profile that may be referenced by live sandboxes.
///
/// OpenShell's `import` never upserts and the gateway refuses to delete a profile
/// while a sandbox references it, so an update is: detach every referencing
/// attachment, delete + re-import the profile (same id), then re-attach. Provider
/// secrets are never re-supplied (detach/attach do not carry them). On import
/// failure the prior profile is restored and detached attachments re-attached
/// before the error propagates (FAIL FAST). Callers still own the subsequent
/// policy reload + composition confirmation.
pub async fn update_referenced_profile(
    client: &mut OpenShellClient<Channel>,
    attachments: &[ProfileAttachment],
    desired: proto_v1::ProviderProfile,
) -> Result<(), ProviderError> {
    let id = desired.id.clone();
    let stored = crate::managed_profiles::get_profile(client, &id)
        .await
        .map_err(|e| ProviderError::Grpc(format!("get profile {id}: {e:#}")))?;

    // 1. Detach every currently-referencing attachment (NotFound = not attached).
    let mut to_reattach: Vec<&ProfileAttachment> = Vec::new();
    for att in attachments {
        match detach_from_sandbox(client, &att.sandbox_name, &att.provider_name).await {
            Ok(()) => to_reattach.push(att),
            Err(ProviderError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }

    // 2. Delete the now-unreferenced profile.
    if let Err(e) = crate::managed_profiles::delete_profile(client, &id).await {
        reattach_all(client, &to_reattach).await;
        return Err(ProviderError::Grpc(format!("delete profile {id}: {e:#}")));
    }

    // 3. Import the desired profile; restore the prior one on failure.
    if let Err(e) = crate::managed_profiles::lint_and_import(client, desired).await {
        if let Some(prev) = stored {
            if let Err(re) = crate::managed_profiles::lint_and_import(client, prev).await {
                tracing::error!(profile = %id, "rollback re-import of prior profile failed: {re:#}");
            }
        }
        reattach_all(client, &to_reattach).await;
        return Err(ProviderError::Grpc(format!("import profile {id}: {e:#}")));
    }

    // 4. Re-attach.
    for att in &to_reattach {
        attach_to_sandbox(client, &att.sandbox_name, &att.provider_name).await?;
    }
    Ok(())
}

async fn reattach_all(client: &mut OpenShellClient<Channel>, atts: &[&ProfileAttachment]) {
    for att in atts {
        if let Err(e) = attach_to_sandbox(client, &att.sandbox_name, &att.provider_name).await {
            tracing::error!(
                provider = %att.provider_name,
                sandbox = %att.sandbox_name,
                "rollback re-attach failed: {e:#}"
            );
        }
    }
}
```

- [ ] **Step 2: Write the failing happy-path test (call order)**

Add to `crates/right-openshell/src/providers_tests.rs`. This records the RPC order and asserts
detach → delete → import → attach:

```rust
#[tokio::test]
async fn update_referenced_profile_detaches_deletes_imports_reattaches_in_order() {
    use crate::managed_profiles::author_generic_profile;
    use crate::providers::{update_referenced_profile, ProfileAttachment};
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (o1, o2, o3, o4, o5) = (
        Arc::clone(&order), Arc::clone(&order), Arc::clone(&order), Arc::clone(&order), Arc::clone(&order),
    );

    let stored = author_generic_profile("right-provider-x", &["api.acme.com".into()], None, "KEY");
    let mock = MockOpenShell {
        mock_get_provider_profile: Some(Box::new(move |_| {
            o1.lock().unwrap().push("get");
            Ok(proto_v1::ProviderProfileResponse { profile: Some(stored.clone()) })
        })),
        mock_detach_sandbox_provider: Some(Box::new(move |_| {
            o2.lock().unwrap().push("detach");
            Ok(proto_v1::AttachSandboxProviderResponse::default().into_detach())
        })),
        mock_delete_provider_profile: Some(Box::new(move |_| {
            o3.lock().unwrap().push("delete");
            Ok(proto_v1::DeleteProviderProfileResponse { deleted: true })
        })),
        mock_lint_provider_profiles: Some(Box::new(|_| {
            Ok(proto_v1::LintProviderProfilesResponse { diagnostics: vec![], valid: true })
        })),
        mock_import_provider_profiles: Some(Box::new(move |_| {
            o4.lock().unwrap().push("import");
            Ok(proto_v1::ImportProviderProfilesResponse::default())
        })),
        mock_attach_sandbox_provider: Some(Box::new(move |_| {
            o5.lock().unwrap().push("attach");
            Ok(proto_v1::AttachSandboxProviderResponse::default())
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let desired = author_generic_profile(
        "right-provider-x",
        &["api.acme.com".into(), "api2.acme.com".into()],
        None,
        "KEY",
    );
    let atts = vec![ProfileAttachment { sandbox_name: "sbx".into(), provider_name: "prov".into() }];
    update_referenced_profile(&mut client, &atts, desired).await.unwrap();

    assert_eq!(*order.lock().unwrap(), vec!["get", "detach", "delete", "import", "attach"]);
}
```

NOTE on `into_detach()`: the mock's detach handler must return a
`DetachSandboxProviderResponse`. Use the correct response type directly —
`Ok(proto_v1::DetachSandboxProviderResponse::default())` — and delete the
`into_detach()` placeholder (it does not exist). The example above is corrected in
Step 3.

- [ ] **Step 3: Correct the detach response type**

In the test from Step 2, the `mock_detach_sandbox_provider` closure must return
`proto_v1::DetachSandboxProviderResponse::default()`:

```rust
        mock_detach_sandbox_provider: Some(Box::new(move |_| {
            o2.lock().unwrap().push("detach");
            Ok(proto_v1::DetachSandboxProviderResponse::default())
        })),
```

- [ ] **Step 4: Run the happy-path test**

Run: `devenv shell -- cargo test -p right-openshell --lib update_referenced_profile_detaches_deletes_imports_reattaches_in_order`
Expected: PASS with order `["get","detach","delete","import","attach"]`.

- [ ] **Step 5: Write the rollback test**

```rust
#[tokio::test]
async fn update_referenced_profile_restores_prior_and_reattaches_on_import_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::managed_profiles::author_generic_profile;
    use crate::providers::{update_referenced_profile, ProfileAttachment};

    let import_calls = Arc::new(AtomicUsize::new(0));
    let attach_calls = Arc::new(AtomicUsize::new(0));
    let import_c = Arc::clone(&import_calls);
    let attach_c = Arc::clone(&attach_calls);
    let stored = author_generic_profile("right-provider-x", &["api.acme.com".into()], None, "KEY");

    let mock = MockOpenShell {
        mock_get_provider_profile: Some(Box::new(move |_| {
            Ok(proto_v1::ProviderProfileResponse { profile: Some(stored.clone()) })
        })),
        mock_detach_sandbox_provider: Some(Box::new(|_| {
            Ok(proto_v1::DetachSandboxProviderResponse::default())
        })),
        mock_delete_provider_profile: Some(Box::new(|_| {
            Ok(proto_v1::DeleteProviderProfileResponse { deleted: true })
        })),
        mock_lint_provider_profiles: Some(Box::new(|_| {
            Ok(proto_v1::LintProviderProfilesResponse { diagnostics: vec![], valid: true })
        })),
        // First import (desired) fails; second import (rollback of prior) succeeds.
        mock_import_provider_profiles: Some(Box::new(move |_| {
            let n = import_c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(tonic::Status::internal("boom"))
            } else {
                Ok(proto_v1::ImportProviderProfilesResponse::default())
            }
        })),
        mock_attach_sandbox_provider: Some(Box::new(move |_| {
            attach_c.fetch_add(1, Ordering::SeqCst);
            Ok(proto_v1::AttachSandboxProviderResponse::default())
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let desired = author_generic_profile(
        "right-provider-x",
        &["api.acme.com".into(), "api2.acme.com".into()],
        None,
        "KEY",
    );
    let atts = vec![ProfileAttachment { sandbox_name: "sbx".into(), provider_name: "prov".into() }];
    let err = update_referenced_profile(&mut client, &atts, desired).await.unwrap_err();

    assert!(format!("{err:#}").contains("import profile"), "got {err:#}");
    assert_eq!(import_calls.load(Ordering::SeqCst), 2, "desired import + rollback re-import");
    assert_eq!(attach_calls.load(Ordering::SeqCst), 1, "detached attachment must be re-attached on rollback");
}
```

- [ ] **Step 6: Run the rollback test**

Run: `devenv shell -- cargo test -p right-openshell --lib update_referenced_profile_restores_prior`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/providers_tests.rs
git commit -m "feat(openshell): update_referenced_profile detach-dance primitive with rollback"
```

---

## Task 5: Live contract test for the primitive

Encodes the probe's finding as a permanent live contract: a generic profile can be updated under a live
provider via the primitive, the sandbox sees the new endpoint set, and the secret survives.

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_generic_provider.rs`

- [ ] **Step 1: Extend the managed_profiles import**

Change the import (top of file ~line 11):

```rust
use right_openshell::managed_profiles::{
    ManagedProfile, author_generic_profile, delete_profile, ensure_profiles,
};
```
to also pull nothing new from managed_profiles (the test uses `author_generic_profile` already) but add
to the providers import (~line 15):

```rust
use right_openshell::providers::{
    ProfileAttachment, ProviderSpec, attach_to_sandbox, create_provider, delete_provider,
    detach_from_sandbox, update_referenced_profile,
};
```

- [ ] **Step 2: Append the contract test**

Append at EOF:

```rust
/// CONTRACT: a generic provider profile can be updated in place under a live
/// provider via `update_referenced_profile` (detach → delete → import → attach),
/// the sandbox's effective policy reflects the new endpoint set after a reload,
/// and the provider secret is preserved (never re-supplied).
#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_update_referenced_profile_swaps_endpoints_preserving_secret() {
    let profile_id = unique_profile_id("update-referenced");
    let provider_name = unique_name("update-referenced");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
        right_openshell::providers::ensure_v2_enabled(&mut client)
            .await
            .expect("enable providers_v2");

        let host_a = UPSTREAM_HOST.to_string();
        let host_b = SECOND_UPSTREAM_HOST.to_string();

        // baseline: profile(A) + provider(secret) + attach + compose(A)
        ensure_generic_profile_for_hosts(&mut client, &profile_id, &[host_a.clone()], ENV_VAR).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(&mut client, &fake_provider_spec(&provider_name, &profile_id))
            .await
            .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox = TestSandbox::create_with_policy(
            "ci-openshell-update-referenced",
            RAW_TUNNEL_BASE_POLICY,
        )
        .await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());
        attach_to_sandbox(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("attach provider");
        right_openshell::test_cleanup::register_test_provider_attachment(&provider_name, sandbox.name());
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        right_openshell::openshell::wait_for_provider_composed_with_all_endpoints(
            &mut client,
            sandbox.name(),
            &provider_name,
            vec![(host_a.clone(), String::new())],
        )
        .await
        .expect("baseline HOST_A composed");
        wait_for_provider_placeholder(&sandbox, ENV_VAR).await;

        // update via the primitive (secret never re-supplied past create_provider)
        let desired = author_generic_profile(
            &profile_id,
            &[host_a.clone(), host_b.clone()],
            None,
            ENV_VAR,
        );
        let atts = vec![ProfileAttachment {
            sandbox_name: sandbox.name().to_string(),
            provider_name: provider_name.clone(),
        }];
        update_referenced_profile(&mut client, &atts, desired)
            .await
            .expect("update referenced profile");

        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("reload after profile update");
        right_openshell::openshell::wait_for_provider_composed_with_all_endpoints(
            &mut client,
            sandbox.name(),
            &provider_name,
            vec![(host_a.clone(), String::new()), (host_b.clone(), String::new())],
        )
        .await
        .expect("updated endpoints must compose into effective policy");

        // secret survived
        wait_for_provider_placeholder(&sandbox, ENV_VAR).await;
    })
    .await;
}
```

- [ ] **Step 3: Compile the live test**

Run: `devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_generic_provider --no-run`
Expected: compiles.

- [ ] **Step 4: Run the live contract test**

Run: `devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_generic_provider ci_openshell_update_referenced_profile_swaps_endpoints_preserving_secret -- --ignored --nocapture`
Expected: PASS (creates a throwaway sandbox; ~15-30s). If the gateway is cold, allow a retry.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_generic_provider.rs
git commit -m "test(openshell): live contract for generic profile update under live provider"
```

---

## Task 6: Dashboard config-update uses the primitive

Replace the doomed `ensure_profiles` call in the generic config-update handler with
`update_referenced_profile`.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` (`handle_provider_config_update` ~line 2191-2212)

- [ ] **Step 1: Swap the profile-update call**

Replace the block at ~line 2191-2212:

```rust
    let (_, profile) = generic_provider_update_profile(&req.name, &updated_generic);
    let managed_profile =
        right_openshell::managed_profiles::ManagedProfile::Authored(Box::new(profile));
    if let Err(e) =
        right_openshell::managed_profiles::ensure_profiles(&mut client, &[managed_profile]).await
    {
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &req.name,
            &current,
            format!("{e:#}"),
            "profile import failure",
        )
        .await;
        let mut msg = format!("profile import: {e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::Gateway(msg));
    }
```

with:

```rust
    let (_, profile) = generic_provider_update_profile(&req.name, &updated_generic);
    let attachments = vec![right_openshell::providers::ProfileAttachment {
        sandbox_name: sandbox_name.clone(),
        provider_name: req.name.clone(),
    }];
    if let Err(e) =
        right_openshell::providers::update_referenced_profile(&mut client, &attachments, profile)
            .await
    {
        let rollback_errors = reensure_generic_profile_after_rollback(
            &mut client,
            &req.name,
            &current,
            format!("{e:#}"),
            "profile update failure",
        )
        .await;
        let mut msg = format!("profile update: {e:#}");
        if !rollback_errors.is_empty() {
            msg.push_str(" (rollback also failed: ");
            msg.push_str(&rollback_errors.join("; "));
            msg.push(')');
        }
        return Err(ProviderApiError::Gateway(msg));
    }
```

(The subsequent `ensure_provider_policy_loaded` + `wait_for_provider_composed_with_exact_endpoints` +
their rollback blocks stay unchanged.)

- [ ] **Step 2: Compile the crate**

Run: `devenv shell -- cargo build -p right --bin right`
Expected: builds. Fix any unused-import warnings for `ensure_profiles` if it is now unused in this file
(only remove imports your change made unused).

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "fix(providers): dashboard config-update swaps profile via detach-dance"
```

---

## Task 7: Supervisor self-heals drifted generic profiles

When startup/hot reconcile detects a drifted generic profile, heal it with the primitive before the
attach reconcile + composition confirm.

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs` (`reconcile_and_confirm_providers` ~line 244-262; add a helper)

- [ ] **Step 1: Add the heal helper**

Add to `crates/bot/src/sandbox_supervisor.rs` (near `ensure_generic_provider_profiles_for_config`):

```rust
/// Heal any generic profile that `ensure_*` reported as drifted, using the
/// detach-dance primitive. Built-in profiles are not healed here (handled at
/// `right up`); only generic providers declare drift in this path.
async fn heal_drifted_generic_profiles(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    sandbox_name: &str,
    agent_name: &str,
    config: &AgentConfig,
    outcomes: &[right_openshell::managed_profiles::EnsureOutcome],
) -> miette::Result<()> {
    use right_openshell::managed_profiles::EnsureOutcome;
    for entry in config.providers() {
        let right_agent_config::ProviderType::Generic = entry.type_ else {
            continue;
        };
        let id = right_openshell::managed_profiles::generic_provider_profile_id(&entry.name);
        let drifted = outcomes
            .iter()
            .any(|o| matches!(o, EnsureOutcome::DriftedSkipped(d) if *d == id));
        if !drifted {
            continue;
        }
        let generic = entry.generic.as_ref().ok_or_else(|| {
            miette::miette!("agent {agent_name} generic provider {} missing generic config", entry.name)
        })?;
        let desired = right_openshell::managed_profiles::author_generic_profile(
            &id,
            &generic.upstream_hosts,
            generic.upstream_path_prefix.as_deref(),
            &generic.env_var,
        );
        let atts = vec![right_openshell::providers::ProfileAttachment {
            sandbox_name: sandbox_name.to_string(),
            provider_name: entry.name.clone(),
        }];
        right_openshell::providers::update_referenced_profile(client, &atts, desired)
            .await
            .map_err(|e| {
                miette::miette!("heal drifted generic profile {} failed: {e:#}", entry.name)
            })?;
        tracing::info!(agent = %agent_name, provider = %entry.name, "healed drifted generic provider profile");
    }
    Ok(())
}
```

- [ ] **Step 2: Call it in `reconcile_and_confirm_providers`**

After the `ensure_generic_provider_profiles_for_config` call (~line 252-253), insert the heal step:

```rust
    let profile_outcomes =
        ensure_generic_provider_profiles_for_config(client, agent, config).await?;
    heal_drifted_generic_profiles(client, sandbox, agent, config, &profile_outcomes).await?;
```

Apply the identical two-line addition in `hot_reconcile_providers` after its
`ensure_generic_provider_profiles_for_config` call (~line 567) — it has `agent`, `config`, and
`resolved_sandbox` in scope (use `resolved_sandbox` as the sandbox name).

- [ ] **Step 3: Compile the bot crate**

Run: `devenv shell -- cargo build -p right-bot`
Expected: builds.

- [ ] **Step 4: Targeted check of the supervisor module tests**

Run: `devenv shell -- cargo test -p right-bot sandbox_supervisor`
Expected: existing supervisor tests still pass (no behavior change when nothing drifts).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs
git commit -m "fix(bot): supervisor self-heals drifted generic provider profiles"
```

---

## Task 8: `right up` self-heals drifted profiles after bulk ensure

`right up`'s bulk `ensure_profiles` now reports drift instead of aborting. Heal every drifted profile
(built-in + generic) using attachments built from the loaded agent configs.

**Files:**
- Modify: `crates/right/src/main.rs` (the managed-profiles block ~line 3278-3287)

- [ ] **Step 1: Build attachments + heal drifted profiles**

Replace the block at ~line 3278-3287:

```rust
        let mut profiles = right_openshell::managed_profiles::managed_profiles();
        let loaded_agent_configs: Vec<(String, right_agent_config::AgentConfig)> = agents
            .iter()
            .filter_map(|a| a.config.as_ref().map(|cfg| (a.name.clone(), cfg.clone())))
            .collect();
        profiles.extend(generic_provider_profiles(&loaded_agent_configs)?);
        let outcomes = right_openshell::managed_profiles::ensure_profiles(&mut client, &profiles)
            .await
            .map_err(|e| miette::miette!("provision managed profiles failed: {e:#}"))?;
        tracing::info!(?outcomes, "up: managed_profiles_provisioned");
```

with:

```rust
        let mut profiles = right_openshell::managed_profiles::managed_profiles();
        let loaded_agent_configs: Vec<(String, right_agent_config::AgentConfig)> = agents
            .iter()
            .filter_map(|a| a.config.as_ref().map(|cfg| (a.name.clone(), cfg.clone())))
            .collect();
        profiles.extend(generic_provider_profiles(&loaded_agent_configs)?);
        let outcomes = right_openshell::managed_profiles::ensure_profiles(&mut client, &profiles)
            .await
            .map_err(|e| miette::miette!("provision managed profiles failed: {e:#}"))?;
        tracing::info!(?outcomes, "up: managed_profiles_provisioned");
        heal_drifted_managed_profiles(&mut client, &loaded_agent_configs, &outcomes).await?;
```

- [ ] **Step 2: Add the heal function + attachment map**

Add near `generic_provider_profiles` in `crates/right/src/main.rs` (it already uses
`right_openshell::managed_profiles`):

```rust
/// Map each managed-profile id to the sandbox attachments that reference it,
/// derived from loaded agent configs. Generic providers map by
/// `generic_provider_profile_id(name)`; built-in providers map by their profile id.
fn managed_profile_attachments(
    configs: &[(String, right_agent_config::AgentConfig)],
) -> std::collections::HashMap<String, Vec<right_openshell::providers::ProfileAttachment>> {
    let mut map: std::collections::HashMap<String, Vec<right_openshell::providers::ProfileAttachment>> =
        std::collections::HashMap::new();
    for (agent_name, cfg) in configs {
        let Some(sandbox) = cfg.sandbox.as_ref() else {
            continue;
        };
        let sandbox_name = sandbox.name.clone().unwrap_or_else(|| agent_name.clone());
        for entry in cfg.providers() {
            let profile_id = match &entry.type_ {
                right_agent_config::ProviderType::Generic => {
                    right_openshell::managed_profiles::generic_provider_profile_id(&entry.name)
                }
                right_agent_config::ProviderType::BuiltIn(slug) => slug.profile_id(),
            };
            map.entry(profile_id)
                .or_default()
                .push(right_openshell::providers::ProfileAttachment {
                    sandbox_name: sandbox_name.clone(),
                    provider_name: entry.name.clone(),
                });
        }
    }
    map
}

/// Heal every profile `ensure_profiles` reported as `DriftedSkipped`, using the
/// detach-dance primitive against all known referencing attachments.
async fn heal_drifted_managed_profiles(
    client: &mut right_openshell::managed_profiles::OpenShellGrpcClient,
    configs: &[(String, right_agent_config::AgentConfig)],
    outcomes: &[right_openshell::managed_profiles::EnsureOutcome],
) -> miette::Result<()> {
    use right_openshell::managed_profiles::EnsureOutcome;
    let attachments = managed_profile_attachments(configs);
    let all_profiles = {
        let mut p = right_openshell::managed_profiles::managed_profiles();
        p.extend(generic_provider_profiles(configs)?);
        p
    };
    for outcome in outcomes {
        let EnsureOutcome::DriftedSkipped(id) = outcome else {
            continue;
        };
        let Some(mp) = all_profiles.iter().find(|m| m.id() == *id) else {
            continue;
        };
        let desired = match right_openshell::managed_profiles::desired_profile_for(client, mp).await {
            Ok(d) => d,
            Err(e) => return Err(miette::miette!("author desired profile {id} for heal: {e:#}")),
        };
        let atts = attachments.get(id).cloned().unwrap_or_default();
        right_openshell::providers::update_referenced_profile(client, &atts, desired)
            .await
            .map_err(|e| miette::miette!("heal drifted managed profile {id}: {e:#}"))?;
        tracing::info!(profile = %id, "up: healed drifted managed profile");
    }
    Ok(())
}
```

- [ ] **Step 3: Add the `desired_profile_for` helper in managed_profiles**

`heal_drifted_managed_profiles` needs the resolved desired profile for any `ManagedProfile` (authored =
itself; github = derived from the live base). Add to `crates/right-openshell/src/managed_profiles.rs`
(make `DesiredProfileSource` usable) — add a public async resolver:

```rust
/// Resolve the desired profile body for a managed profile, fetching+deriving the
/// base for derived variants (e.g. `Github`). Errors if a derived base is absent.
pub async fn desired_profile_for(
    client: &mut OpenShellClient<Channel>,
    mp: &ManagedProfile,
) -> Result<proto_v1::ProviderProfile, ManagedProfileError> {
    match desired_profile_source(mp) {
        DesiredProfileSource::DeriveFromBase(base_id) => match get_profile(client, base_id).await? {
            Some(base) => Ok(mp.derive(base)),
            None => Err(ManagedProfileError::Grpc(format!(
                "base profile {base_id} absent on gateway"
            ))),
        },
        DesiredProfileSource::Authored(profile) => Ok(*profile),
    }
}
```

- [ ] **Step 4: Verify `BuiltIn::profile_id()` exists**

The attachment map calls `slug.profile_id()` on the built-in provider type. Confirm a method that yields
the managed-profile id for a built-in provider type exists in `right-agent-config`
(`crates/right-agent-config/src/lib.rs`, the `ProviderType::BuiltIn(_)` inner type). If it does not, add:

```rust
impl BuiltInProvider {            // adjust to the actual inner type name
    /// The managed OpenShell profile id this built-in provider attaches.
    pub fn profile_id(&self) -> String {
        format!("right-{}", self.slug())   // e.g. "right-github"; match managed_profiles ids
    }
}
```

Cross-check the produced id against `ManagedProfile::id()` in `managed_profiles.rs` (`right-github`,
`right-fal`) so the map keys match. Adjust to the real enum/method names before relying on it.

- [ ] **Step 5: Build**

Run: `devenv shell -- cargo build -p right --bin right`
Expected: builds.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/main.rs crates/right-openshell/src/managed_profiles.rs crates/right-agent-config/src/lib.rs
git commit -m "feat(up): self-heal drifted managed provider profiles on right up"
```

---

## Task 9: Final verification

- [ ] **Step 1: Clippy (workspace, tests)**

Run: `devenv shell -- cargo clippy --workspace --tests -- -D warnings`
Expected: no warnings. Fix any introduced.

- [ ] **Step 2: Full workspace test (MANDATORY)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Note any pre-existing flaky tests (see memory: cc/invocation pid race, dashboard
warn-count) and re-run them isolated before blaming this change. Live `ci_openshell_` tests are
`#[ignore]` and excluded from this run.

- [ ] **Step 3: Manual `right up` smoke (the original failure)**

The user's live gateway already carries the drifted `right-twitterapi`/`right-typefully` profiles. Verify
boot no longer aborts:

Run: `cargo run --release --bin right -- up`
Expected: completes without `provision managed profiles failed: ... already exists`. Logs show drifted
profiles either `Unchanged` (Fix A removed inert drift) or `healed`. Do NOT manually edit gateway state.

- [ ] **Step 4: Update docs (cite-on-touch)**

Re-read `docs/architecture/providers.md` and update the profile-update/reconcile narration to describe the
detach-dance + self-heal. Update ARCHITECTURE.md only if a contract/invariant changed (e.g. add: "managed
profile updates go through `providers::update_referenced_profile`; `ensure_profiles` never re-imports an
existing id"). Keep ARCHITECTURE.md under its 40k budget.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "docs(providers): detach-dance profile update + self-heal reconcile"
```

---

## Self-Review

- **Spec coverage:** Fix A (Task 1) ✓; create-or-skip ensure_profiles (Task 3) ✓; detach-dance primitive
  + rollback (Task 4) ✓; live contract (Task 5) ✓; self-heal on all three context-aware sites — dashboard
  (Task 6), supervisor startup + hot-reconcile (Task 7), `right up` (Task 8) ✓; mock enablement (Task 2)
  ✓; verification incl. the original `right up` repro (Task 9) ✓.
- **Type consistency:** `ProfileAttachment { sandbox_name, provider_name }` used identically in Tasks 4,
  6, 7, 8. `EnsureOutcome::DriftedSkipped(String)` defined in Task 3, matched in Tasks 7, 8.
  `update_referenced_profile(client, &[ProfileAttachment], ProviderProfile)` signature consistent across
  call sites. `desired_profile_for` added in Task 8 Step 3 and used in Task 8 Step 2.
- **Open risks flagged for the implementer (verify against real code, do not assume):**
  1. Task 8 Step 4 — the built-in provider type's `profile_id()` may not exist; confirm the real enum
     (`ProviderType::BuiltIn(_)` inner type) and that its id matches `ManagedProfile::id()`. If built-in
     providers cannot be mapped cleanly, scope Task 8 to generic profiles only (drop built-in from
     `managed_profile_attachments`) and note built-in drift as supervisor/`right up` warn-only — built-in
     derivation is stable, so this is acceptable.
  2. `desired_profile_source` / `DesiredProfileSource` are currently `pub(crate)`; Task 8 Step 3's public
     `desired_profile_for` wraps them, so they need not become public — keep them `pub(crate)`.
  3. Confirm `proto_v1::DetachSandboxProviderResponse` and `ImportProviderProfilesResponse::default()`
     exist as used in the mock tests; adjust field initializers to the actual generated types.
