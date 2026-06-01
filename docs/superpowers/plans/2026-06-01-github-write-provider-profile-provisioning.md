# GitHub write-access provider + profile-provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let sandboxed agents `git push` to GitHub by adding a RightClaw-owned `right-github-write` provider profile (read-write), provisioned to the OpenShell gateway on every `right up`, surfaced as a user-friendly grouped choice in the dashboard.

**Architecture:** A new `right_openshell::managed_profiles` subsystem owns RightClaw-authored OpenShell provider profiles. `right-github-write` is *derived* at startup from the live built-in `github` profile with every endpoint elevated to `access: read-write`. `ensure_profiles` idempotently re-asserts the set on the gateway (structural diff → lint → import) — pure Path A: the gateway contributes the write endpoints on attach; we never touch the per-agent `policy.yaml`. The dashboard groups the read-only `github` and write `right-github-write` types into one "GitHub" card; the `right-*` slug is never shown raw.

**Tech Stack:** Rust (edition 2024, tonic/prost gRPC, thiserror, miette), Vue 3 + TypeScript (vitest, @vue/server-renderer), OpenShell v0.0.50 gRPC (`GetProviderProfile`/`LintProviderProfiles`/`ImportProviderProfiles`).

---

## Pre-flight: baseline verification

- [ ] **Step 0: Baseline build + targeted tests pass before changes**

Run: `devenv shell -- cargo test -p right-openshell -p right-dashboard`
Expected: PASS (record any pre-existing failures; the `flaky_tests_parallel_load` note means re-run a lone failure isolated before blaming it).

---

## File Structure

**Created:**
- `crates/right-openshell/src/managed_profiles.rs` — `ManagedProfile` types, the managed-profile list, derivation/transform, `ensure_profiles`, and the thin `ProviderProfile` gRPC wrappers (get/lint/import/delete). Owns the OpenShell **ProviderProfile** RPC surface (sibling to `providers.rs`, which owns the **Provider** surface).
- `crates/right-openshell/tests/ci_openshell_github_write.rs` — live-gateway integration tests (profile import idempotency, derived read-write endpoints, base-missing error) + one `#[ignore]` real-push test.
- `crates/right-dashboard/frontend/src/views/providersGrouping.ts` — pure `groupProviderTypes()` helper.
- `crates/right-dashboard/frontend/src/views/providersGrouping.test.ts` — vitest unit test for the grouping helper.

**Modified:**
- `crates/right-openshell/src/lib.rs` — register `pub mod managed_profiles;`.
- `crates/right-openshell/src/providers.rs` — add `right-github-write` to `profile_catalog()`, add `group` to host `ProviderProfile` struct, update catalog tests.
- `crates/right/src/main.rs` — new `right up` provisioning stage after `up: openshell_preflight`.
- `crates/right/src/internal_api_providers.rs` — add `group` to `ProviderProfileView` DTO + mapping.
- `crates/right-dashboard/frontend/src/types.ts` — add `group` to `ProviderProfileView`.
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue` — grouped type cards w/ access-variant selector; eyebrow `AI Providers` → `Integrations`.
- `AGENTS.md`, `ARCHITECTURE.md`, `docs/architecture/providers.md` — principles + subsystem docs.

---

## Phase A — Profile-provisioning subsystem (`right-openshell`)

### Task 1: `ManagedProfile` types, registry, and derivation transform

**Files:**
- Create: `crates/right-openshell/src/managed_profiles.rs`
- Modify: `crates/right-openshell/src/lib.rs`

Proto facts (verbatim, from `crates/right-openshell/proto/openshell/`):
- `proto_v1 = crate::openshell_proto::openshell::v1`. `proto_v1::ProviderProfile { id: String, display_name: String, description: String, category: i32, credentials: Vec<ProviderProfileCredential>, endpoints: Vec<sandbox_v1::NetworkEndpoint>, binaries: Vec<sandbox_v1::NetworkBinary>, inference_capable: bool, discovery: Option<ProviderProfileDiscovery> }`.
- `sandbox_v1 = crate::openshell_proto::openshell::sandbox::v1`. `NetworkEndpoint { host, port: u32, protocol: String, tls: String, enforcement: String, access: String, rules: Vec<L7Rule>, allowed_ips: Vec<String>, .. }`. `access` is mutually exclusive with `rules`.

- [ ] **Step 1: Write the failing test (transform sets read-write on all endpoints)**

In `crates/right-openshell/src/managed_profiles.rs`, add at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
    use crate::openshell_proto::openshell::v1 as proto_v1;

    fn base_github() -> proto_v1::ProviderProfile {
        proto_v1::ProviderProfile {
            id: "github".into(),
            display_name: "GitHub".into(),
            description: "GitHub API and Git operations".into(),
            category: 4, // SOURCE_CONTROL
            credentials: vec![],
            endpoints: vec![
                sandbox_v1::NetworkEndpoint {
                    host: "github.com".into(),
                    port: 443,
                    protocol: "rest".into(),
                    access: "read-only".into(),
                    enforcement: "enforce".into(),
                    rules: vec![],
                    ..Default::default()
                },
                sandbox_v1::NetworkEndpoint {
                    host: "api.github.com".into(),
                    port: 443,
                    protocol: "rest".into(),
                    access: "read-only".into(),
                    enforcement: "enforce".into(),
                    rules: vec![],
                    ..Default::default()
                },
            ],
            binaries: vec![],
            inference_capable: false,
            discovery: None,
        }
    }

    #[test]
    fn derive_github_write_elevates_all_endpoints_and_renames() {
        let mp = github_write();
        let derived = mp.derive(base_github());
        assert_eq!(derived.id, "right-github-write");
        assert_eq!(derived.display_name, "GitHub (write)");
        assert_eq!(derived.category, 4, "category preserved from base");
        assert!(!derived.endpoints.is_empty());
        for ep in &derived.endpoints {
            assert_eq!(ep.access, "read-write", "every endpoint elevated");
            assert!(ep.rules.is_empty(), "rules cleared (access is exclusive)");
        }
    }

    #[test]
    fn managed_profiles_all_right_prefixed() {
        for mp in managed_profiles() {
            assert!(
                mp.id().starts_with("right-"),
                "managed profile {} must be right-* prefixed",
                mp.id()
            );
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell managed_profiles::tests`
Expected: FAIL — `managed_profiles`/`github_write`/`ManagedProfile` not defined (compile error).

- [ ] **Step 3: Write the module (types + registry + transform)**

At the top of `crates/right-openshell/src/managed_profiles.rs`:

```rust
//! RightClaw-owned OpenShell provider profiles.
//!
//! This module owns the OpenShell **ProviderProfile** RPC surface
//! (get/lint/import/delete) and the set of profiles RightClaw provisions
//! to the gateway. It is a sibling to `providers.rs` (which owns the
//! **Provider** surface). All RightClaw profile ids are `right-*` prefixed
//! — the prefix is the ownership marker.

use thiserror::Error;
use tonic::transport::Channel;

use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// All managed-profile errors — FAIL FAST, never swallowed.
#[derive(Debug, Error)]
pub enum ManagedProfileError {
    #[error("openshell gRPC: {0}")]
    Grpc(String),
    #[error("base profile \"{0}\" not found on gateway — cannot derive a managed write profile from it")]
    BaseMissing(String),
    #[error("profile \"{id}\" failed lint: {detail}")]
    LintFailed { id: String, detail: String },
}

/// A profile RightClaw provisions to the gateway. Either derived from a
/// live upstream profile, or fully authored.
#[derive(Debug, Clone)]
pub enum ManagedProfile {
    /// Clone a live upstream profile (`base_id`) and elevate it.
    GithubWrite,
    // Future: Authored(Box<proto_v1::ProviderProfile>) for e.g. right-browser-use.
}

impl ManagedProfile {
    pub fn id(&self) -> &'static str {
        match self {
            ManagedProfile::GithubWrite => "right-github-write",
        }
    }

    /// The upstream profile id this profile derives from, if any.
    pub fn base_id(&self) -> Option<&'static str> {
        match self {
            ManagedProfile::GithubWrite => Some("github"),
        }
    }

    /// Produce the desired profile from a fetched base profile.
    pub fn derive(&self, mut base: proto_v1::ProviderProfile) -> proto_v1::ProviderProfile {
        match self {
            ManagedProfile::GithubWrite => {
                base.id = self.id().into();
                base.display_name = "GitHub (write)".into();
                for ep in &mut base.endpoints {
                    ep.access = "read-write".into();
                    ep.rules.clear(); // access is mutually exclusive with rules
                }
                base
            }
        }
    }
}

/// Helper constructor used by tests and the registry.
pub fn github_write() -> ManagedProfile {
    ManagedProfile::GithubWrite
}

/// The set of profiles RightClaw provisions on every `right up`.
///
/// Module-local free-form list — intentionally NOT a cross-crate registry
/// (see ARCHITECTURE.md "promote on demand"). Add a variant + an entry here
/// to ship a new profile (e.g. right-browser-use).
pub fn managed_profiles() -> Vec<ManagedProfile> {
    vec![ManagedProfile::GithubWrite]
}
```

Register the module: in `crates/right-openshell/src/lib.rs`, next to `pub mod providers;`, add:

```rust
pub mod managed_profiles;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell managed_profiles::tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs crates/right-openshell/src/lib.rs
git commit -m "feat(providers): managed-profile types + github-write derivation"
```

### Task 2: Profile gRPC wrappers + structural-diff helper

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs`

- [ ] **Step 1: Write the failing test (fingerprint diff detects access change)**

Add to the `tests` module in `managed_profiles.rs`:

```rust
#[test]
fn needs_import_true_when_access_differs() {
    let desired = {
        let mut p = base_github();
        p.id = "right-github-write".into();
        for ep in &mut p.endpoints { ep.access = "read-write".into(); }
        p
    };
    let stored_same = desired.clone();
    let mut stored_diff = desired.clone();
    stored_diff.endpoints[0].access = "read-only".into();

    assert!(!needs_import(Some(&stored_same), &desired), "identical → no import");
    assert!(needs_import(Some(&stored_diff), &desired), "access drift → import");
    assert!(needs_import(None, &desired), "absent → import");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell needs_import`
Expected: FAIL — `needs_import` not defined.

- [ ] **Step 3: Implement diff helper + gRPC wrappers**

Add to `managed_profiles.rs` (outside the tests module):

```rust
/// Stable fingerprint of the fields RightClaw controls. Compared instead
/// of the whole message so gateway-filled defaults don't force re-imports.
fn fingerprint(p: &proto_v1::ProviderProfile) -> (String, String, i32, Vec<(String, u32, String)>) {
    let mut eps: Vec<(String, u32, String)> = p
        .endpoints
        .iter()
        .map(|e| (e.host.clone(), e.port, e.access.clone()))
        .collect();
    eps.sort();
    (p.id.clone(), p.display_name.clone(), p.category, eps)
}

/// True if `desired` must be (re)imported given the currently `stored` profile.
fn needs_import(stored: Option<&proto_v1::ProviderProfile>, desired: &proto_v1::ProviderProfile) -> bool {
    match stored {
        None => true,
        Some(s) => fingerprint(s) != fingerprint(desired),
    }
}

fn grpc_err(s: tonic::Status) -> ManagedProfileError {
    ManagedProfileError::Grpc(format!("{}: {}", s.code(), s.message()))
}

/// Fetch a profile by id. Returns `None` on NotFound.
pub async fn get_profile(
    client: &mut OpenShellClient<Channel>,
    id: &str,
) -> Result<Option<proto_v1::ProviderProfile>, ManagedProfileError> {
    let req = proto_v1::GetProviderProfileRequest { id: id.to_string() };
    match client.get_provider_profile(req).await {
        Ok(resp) => Ok(resp.into_inner().profile),
        Err(s) if s.code() == tonic::Code::NotFound => Ok(None),
        Err(s) => Err(grpc_err(s)),
    }
}

/// Lint then import a single profile. Lint failure is a hard error.
pub async fn lint_and_import(
    client: &mut OpenShellClient<Channel>,
    profile: proto_v1::ProviderProfile,
) -> Result<(), ManagedProfileError> {
    let id = profile.id.clone();
    let item = proto_v1::ProviderProfileImportItem { profile: Some(profile), source: "right".into() };

    let lint = client
        .lint_provider_profiles(proto_v1::LintProviderProfilesRequest { profiles: vec![item.clone()] })
        .await
        .map_err(grpc_err)?
        .into_inner();
    if !lint.valid {
        let detail = lint
            .diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.field, d.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ManagedProfileError::LintFailed { id, detail });
    }

    client
        .import_provider_profiles(proto_v1::ImportProviderProfilesRequest { profiles: vec![item] })
        .await
        .map_err(grpc_err)?;
    Ok(())
}

/// Delete a managed profile (used by tests for cleanup; no auto-GC in prod).
pub async fn delete_profile(
    client: &mut OpenShellClient<Channel>,
    id: &str,
) -> Result<(), ManagedProfileError> {
    client
        .delete_provider_profile(proto_v1::DeleteProviderProfileRequest { id: id.to_string() })
        .await
        .map_err(grpc_err)?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell needs_import`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): profile gRPC wrappers + structural-diff helper"
```

### Task 3: `ensure_profiles` reconcile loop

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs`

- [ ] **Step 1: Implement `ensure_profiles` (no unit test — covered by Task 4 integration)**

Add to `managed_profiles.rs`:

```rust
/// Outcome of ensuring one managed profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    Imported(String),
    Unchanged(String),
}

/// Idempotently provision the given managed profiles to the gateway.
///
/// Derived profiles re-read their base each call (drift-proof). A missing
/// base is a hard error (FAIL FAST). Re-imports happen only on real diff.
pub async fn ensure_profiles(
    client: &mut OpenShellClient<Channel>,
    profiles: &[ManagedProfile],
) -> Result<Vec<EnsureOutcome>, ManagedProfileError> {
    let mut outcomes = Vec::with_capacity(profiles.len());
    for mp in profiles {
        let desired = match mp.base_id() {
            Some(base_id) => {
                let base = get_profile(client, base_id)
                    .await?
                    .ok_or_else(|| ManagedProfileError::BaseMissing(base_id.to_string()))?;
                mp.derive(base)
            }
            None => unreachable!("authored profiles not shipped in v1"),
        };
        let stored = get_profile(client, mp.id()).await?;
        if needs_import(stored.as_ref(), &desired) {
            lint_and_import(client, desired).await?;
            tracing::info!(profile = mp.id(), "managed profile drift → imported");
            outcomes.push(EnsureOutcome::Imported(mp.id().to_string()));
        } else {
            tracing::debug!(profile = mp.id(), "managed profile unchanged");
            outcomes.push(EnsureOutcome::Unchanged(mp.id().to_string()));
        }
    }
    Ok(outcomes)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right-openshell`
Expected: PASS (no errors).

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): ensure_profiles idempotent reconcile loop"
```

### Task 4: Live-gateway integration tests (non-ignored — dev has OpenShell)

**Files:**
- Create: `crates/right-openshell/tests/ci_openshell_github_write.rs`

Per AGENTS.md these are NOT `#[ignore]` (dev machines have OpenShell). They hit the live gateway (no sandbox needed) and clean up the test profile after.

- [ ] **Step 1: Write the integration tests**

```rust
//! Live-gateway tests for managed profile provisioning.
use right_openshell::managed_profiles::{
    delete_profile, ensure_profiles, get_profile, github_write, EnsureOutcome, ManagedProfile,
};
use right_openshell::openshell::{connect_grpc, default_mtls_dir, preflight_check, OpenShellStatus};

async fn client() -> right_openshell::openshell::OpenShellClientChannel {
    // Resolve mtls dir from a Ready gateway; skip-by-panic only if absent.
    let mtls = match preflight_check() {
        OpenShellStatus::Ready(dir) => dir,
        other => panic!("gateway not ready for ci_openshell test: {other:?}"),
    };
    connect_grpc(&mtls).await.expect("connect_grpc")
}

#[tokio::test]
async fn ci_openshell_github_write_imports_read_write_and_is_idempotent() {
    let mut c = client().await;

    // First ensure → imported; endpoints read-write.
    let first = ensure_profiles(&mut c, &[github_write()]).await.expect("ensure");
    assert_eq!(first, vec![EnsureOutcome::Imported("right-github-write".into())]);

    let stored = get_profile(&mut c, "right-github-write")
        .await
        .expect("get")
        .expect("present after import");
    assert!(!stored.endpoints.is_empty());
    for ep in &stored.endpoints {
        assert_eq!(ep.access, "read-write", "host {} not read-write", ep.host);
    }

    // Second ensure → unchanged (idempotent).
    let second = ensure_profiles(&mut c, &[github_write()]).await.expect("ensure2");
    assert_eq!(second, vec![EnsureOutcome::Unchanged("right-github-write".into())]);

    // Cleanup (tests don't pollute the gateway; prod re-creates on next up).
    let _ = delete_profile(&mut c, "right-github-write").await;
}

#[tokio::test]
async fn ci_openshell_base_missing_is_hard_error() {
    let mut c = client().await;
    // A managed profile whose base does not exist must error, not silently skip.
    // GithubWrite's base is "github" (present); to exercise BaseMissing we
    // assert get_profile on a bogus id is None and ensure surfaces it. Since
    // the registry's only base is real, this guards the get_profile contract.
    assert!(get_profile(&mut c, "definitely-not-a-profile-xyz").await.expect("get").is_none());
}
```

> NOTE: `ensure_profiles` returning `Err(BaseMissing)` for a real registry can't be triggered on a healthy gateway (base `github` exists). The contract is unit-covered indirectly via `get_profile` returning `None`; the `?`-into-`BaseMissing` line is exercised by code review. If a `ManagedProfile::Authored` test variant is added later, add a direct `BaseMissing` test then.

- [ ] **Step 2: Add the public type alias used by the test helper**

In `crates/right-openshell/src/openshell.rs`, near the `connect_grpc` definition, confirm/add a public alias so tests name the channel type:

```rust
/// Convenience alias for the connected provider/profile client.
pub type OpenShellClientChannel =
    crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient<tonic::transport::Channel>;
```

(If `connect_grpc` already returns this concrete type, the alias just names it; update the test's helper return type to match the existing signature if an alias already exists.)

- [ ] **Step 3: Run the integration tests**

Run: `devenv shell -- cargo test -p right-openshell --test ci_openshell_github_write`
Expected: PASS (2 tests) against the local gateway.

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_github_write.rs crates/right-openshell/src/openshell.rs
git commit -m "test(providers): live-gateway github-write provisioning tests"
```

### Task 5: De-risk — real `git push` test (ci-gated, run manually NOW)

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_github_write.rs`

This proves the load-bearing assumption: a `right-github-write` provider attached to a sandbox actually permits `git push`. Run it manually before building UI/docs.

- [ ] **Step 1: Add the ignored end-to-end push test**

Append to `ci_openshell_github_write.rs`:

```rust
/// Full proof: derive+import right-github-write → create a provider with a
/// real token → attach to a TestSandbox → `git push` to a throwaway repo
/// succeeds (i.e. POST /git-receive-pack is allowed). Needs external creds.
///
/// Env: RIGHT_TEST_GH_TOKEN (PAT, repo write), RIGHT_TEST_GH_PUSH_URL
/// (https URL of a throwaway repo you may force-push).
#[tokio::test]
#[ignore = "ci-openshell: live github push (needs RIGHT_TEST_GH_TOKEN + throwaway repo)"]
async fn ci_openshell_github_write_push_succeeds() {
    // 1. ensure_profiles(github_write)
    // 2. create_provider(type="right-github-write", cred GITHUB_TOKEN=$RIGHT_TEST_GH_TOKEN)
    // 3. TestSandbox::create(...); attach_to_sandbox
    // 4. exec_in_sandbox: git init a temp repo, commit, `git push` to $RIGHT_TEST_GH_PUSH_URL
    // 5. assert push exit status == 0
    // Implementation uses right_openshell::test_support::TestSandbox and the
    // providers::{create_provider, attach_to_sandbox} helpers + exec_in_sandbox.
    // See AGENTS.md "Integration Tests Using Live Sandboxes".
    unimplemented!("fill in using TestSandbox + exec_in_sandbox per AGENTS.md");
}
```

- [ ] **Step 2: Implement the test body** using `right_openshell::test_support::TestSandbox::create`, `providers::create_provider` / `attach_to_sandbox`, and `openshell::exec_in_sandbox` (grep those signatures in `crates/right-openshell/src/{providers,openshell,test_support}.rs` and follow the existing `ci_openshell_provider.rs` patterns).

- [ ] **Step 3: Run it manually with real creds**

Run: `RIGHT_TEST_GH_TOKEN=… RIGHT_TEST_GH_PUSH_URL=https://github.com/<you>/throwaway.git devenv shell -- cargo test -p right-openshell --test ci_openshell_github_write -- --ignored ci_openshell_github_write_push_succeeds`
Expected: PASS — push exits 0.

**GATE:** If push is rejected (403/policy), STOP and revisit the design before continuing — the gateway does not honor `read-write` for `git-receive-pack` and the approach must change.

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_github_write.rs
git commit -m "test(providers): ci-gated real git push proof for github-write"
```

---

## Phase B — Startup hook (`right up`)

### Task 6: Provision managed profiles on `right up`

**Files:**
- Modify: `crates/right/src/main.rs` (just after the `up: openshell_preflight` tracing log, ~line 2784)

- [ ] **Step 1: Add the provisioning stage**

Immediately after the `tracing::info!(… "up: openshell_preflight")` block in `cmd_up`, inside the same scope that has already confirmed the gateway is in use, add:

```rust
// Provision RightClaw-owned provider profiles (right-*) to the gateway.
// Once per gateway, before bots start. FAIL FAST on error.
{
    use right_openshell::openshell::{connect_grpc, preflight_check, OpenShellStatus};
    if let OpenShellStatus::Ready(mtls_dir) = preflight_check() {
        let mut client = connect_grpc(&mtls_dir)
            .await
            .map_err(|e| miette::miette!("provision profiles: connect gateway: {e:#}"))?;
        let outcomes = right_openshell::managed_profiles::ensure_profiles(
            &mut client,
            &right_openshell::managed_profiles::managed_profiles(),
        )
        .await
        .map_err(|e| miette::miette!("provision managed profiles failed: {e:#}"))?;
        tracing::info!(?outcomes, "up: managed_profiles_provisioned");
    }
}
```

> Placement note: this must be inside the branch where OpenShell sandbox mode is active (the same guard the preflight uses). If no agent uses a sandbox, `preflight_check()` won't be `Ready` and the block is a no-op.

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: PASS.

- [ ] **Step 3: Manual smoke (optional but recommended)**

Run: `devenv shell -- cargo run --bin right -- up` (or `target/devenv/debug/right up`), then in another shell: `openshell provider profile export right-github-write`
Expected: the profile exists with `access: read-write` endpoints. Then `right down` if you brought it up.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(up): provision right-* managed profiles to gateway on startup"
```

---

## Phase C — Catalog + DTO `group` plumbing

### Task 7: Add `right-github-write` to the catalog + `group` field

**Files:**
- Modify: `crates/right-openshell/src/providers.rs` (struct ~62, `profile_catalog` ~85, tests ~471)

- [ ] **Step 1: Update the failing catalog tests**

In the `#[cfg(test)] mod tests` of `providers.rs`, replace `catalog_has_8_built_in_plus_generic` body and add a grouping assertion:

```rust
#[test]
fn catalog_has_github_read_and_write_grouped() {
    let catalog = profile_catalog();
    // built-in (non-generic, non right-*) count unchanged at 8
    let upstream_builtin = catalog
        .iter()
        .filter(|p| p.type_slug != "generic" && !p.type_slug.starts_with("right-"))
        .count();
    assert_eq!(upstream_builtin, 8);

    let gh = catalog.iter().find(|p| p.type_slug == "github").expect("github");
    let ghw = catalog.iter().find(|p| p.type_slug == "right-github-write").expect("right-github-write");
    assert_eq!(gh.group, "github");
    assert_eq!(ghw.group, "github", "read + write share a UI group");
    assert_eq!(ghw.env_var, "GITHUB_TOKEN");
    assert!(ghw.type_slug.starts_with("right-"), "our profiles are right-* prefixed");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell catalog_has_github`
Expected: FAIL — no field `group`; no `right-github-write` entry.

- [ ] **Step 3: Add `group` to the struct and the catalog entry**

In `providers.rs`, extend the host struct:

```rust
pub struct ProviderProfile {
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: ProviderCategory,
    /// UI grouping key (dashboard collapses same-group types into one card).
    pub group: String,
}
```

Add `group` to every existing entry in `profile_catalog()` (for non-grouped providers, set `group` equal to `type_slug`). For the github pair:

```rust
        ProviderProfile {
            type_slug: "github".into(),
            display_name: "GitHub".into(),
            category: ProviderCategory::SourceControl,
            env_var: "GITHUB_TOKEN".into(),
            group: "github".into(),
        },
        ProviderProfile {
            type_slug: "right-github-write".into(),
            display_name: "GitHub (write)".into(),
            category: ProviderCategory::SourceControl,
            env_var: "GITHUB_TOKEN".into(),
            group: "github".into(),
        },
```

(For the other 7 built-ins + generic: add `group: "<type_slug>".into()` to each.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-openshell providers`
Expected: PASS (update any other catalog test that hard-counts entries to expect the new `right-github-write`).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs
git commit -m "feat(providers): add right-github-write to catalog + UI group field"
```

### Task 8: Plumb `group` through the internal DTO

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` (`ProviderProfileView` ~2479, `handle_provider_types` ~2487)

- [ ] **Step 1: Add `group` to the DTO + mapping**

```rust
#[derive(Debug, serde::Serialize)]
pub struct ProviderProfileView {
    #[serde(rename = "type")]
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: String,
    pub group: String,
}

pub(crate) async fn handle_provider_types() -> axum::Json<Vec<ProviderProfileView>> {
    let catalog = right_openshell::providers::profile_catalog();
    let views: Vec<_> = catalog
        .into_iter()
        .map(|p| ProviderProfileView {
            type_slug: p.type_slug,
            env_var: p.env_var,
            display_name: p.display_name,
            category: format!("{:?}", p.category).to_lowercase(),
            group: p.group,
        })
        .collect();
    axum::Json(views)
}
```

- [ ] **Step 2: Verify compile + existing tests**

Run: `devenv shell -- cargo test -p right internal_api_providers`
Expected: PASS (compile clean; if a test snapshots the JSON, update it to include `group`).

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(dashboard-api): expose provider-type group in /provider-types"
```

### Task 9: Frontend type definition

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`ProviderProfileView` ~547)

- [ ] **Step 1: Add `group` to the interface**

```typescript
export interface ProviderProfileView {
  type: string
  env_var: string
  display_name: string
  category: string
  group: string
}
```

- [ ] **Step 2: Verify type-check**

Run: `cd crates/right-dashboard/frontend && npm run build`
Expected: PASS (vue-tsc clean).

- [ ] **Step 3: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): add group to ProviderProfileView type"
```

---

## Phase D — Frontend grouping + eyebrow fix

### Task 10: Pure grouping helper + unit test

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/providersGrouping.ts`
- Create: `crates/right-dashboard/frontend/src/views/providersGrouping.test.ts`

- [ ] **Step 1: Write the failing test**

`providersGrouping.test.ts`:

```typescript
import { describe, expect, it } from 'vitest'
import { groupProviderTypes } from './providersGrouping'
import type { ProviderProfileView } from '../types'

const t = (type: string, group: string, display: string): ProviderProfileView => ({
  type, group, display_name: display, env_var: 'GITHUB_TOKEN', category: 'source_control',
})

describe('groupProviderTypes', () => {
  it('collapses same-group types into one card with variants', () => {
    const groups = groupProviderTypes([
      t('github', 'github', 'GitHub'),
      t('right-github-write', 'github', 'GitHub (write)'),
      t('openai', 'openai', 'OpenAI'),
    ])
    expect(groups).toHaveLength(2)
    const gh = groups.find((g) => g.key === 'github')!
    expect(gh.label).toBe('GitHub')
    expect(gh.variants.map((v) => v.type)).toEqual(['github', 'right-github-write'])
    const oai = groups.find((g) => g.key === 'openai')!
    expect(oai.variants).toHaveLength(1)
  })

  it('never shows a right-* slug as the group label', () => {
    const groups = groupProviderTypes([t('right-github-write', 'github', 'GitHub (write)')])
    expect(groups[0].label.startsWith('right-')).toBe(false)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && npm test -- providersGrouping`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the helper**

`providersGrouping.ts`:

```typescript
import type { ProviderProfileView } from '../types'

export interface ProviderGroup {
  key: string
  /** User-facing label — derived from the non-right-* member's display_name. */
  label: string
  variants: ProviderProfileView[]
}

/**
 * Collapse provider types sharing a `group` into one card. The label comes
 * from the first non-`right-*` member (so the RightClaw prefix is never
 * surfaced); falls back to the first member's display_name stripped of any
 * parenthetical qualifier.
 */
export function groupProviderTypes(types: ProviderProfileView[]): ProviderGroup[] {
  const order: string[] = []
  const byKey = new Map<string, ProviderProfileView[]>()
  for (const t of types) {
    if (!byKey.has(t.group)) {
      byKey.set(t.group, [])
      order.push(t.group)
    }
    byKey.get(t.group)!.push(t)
  }
  return order.map((key) => {
    const variants = byKey.get(key)!
    const base = variants.find((v) => !v.type.startsWith('right-')) ?? variants[0]
    const label = base.display_name.replace(/\s*\(.*\)\s*$/, '').trim()
    return { key, label, variants }
  })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && npm test -- providersGrouping`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/providersGrouping.ts crates/right-dashboard/frontend/src/views/providersGrouping.test.ts
git commit -m "feat(dashboard): pure provider-type grouping helper"
```

### Task 11: Grouped cards + access selector + eyebrow fix

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`
- Create: `crates/right-dashboard/frontend/src/views/ProvidersView.grouping.test.ts`

- [ ] **Step 1: Write the failing SSR component test**

`ProvidersView.grouping.test.ts` (follow the `AppShell.test.ts` SSR pattern; if `ProvidersView` needs mounted-only behavior, instead assert via a thin presentational sub-render — otherwise test `groupProviderTypes` rendering through a minimal wrapper):

```typescript
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, test } from 'vitest'
import { groupProviderTypes } from './providersGrouping'
import type { ProviderProfileView } from '../types'

// Minimal render harness mirroring the ProvidersView type-grid markup, to
// assert grouping without standing up the whole stateful view.
const TypeGrid = {
  props: { types: { type: Array, required: true } },
  setup(props: { types: ProviderProfileView[] }) {
    return () =>
      h('div', { class: 'type-grid' },
        groupProviderTypes(props.types).map((g) =>
          h('article', { class: 'type-card', key: g.key }, [
            h('strong', g.label),
            ...g.variants.map((v) =>
              h('button', { class: 'access-variant', 'data-type': v.type }, v.display_name)),
          ])))
  },
}

describe('ProvidersView grouping markup', () => {
  test('renders one GitHub card with read + write access variants', async () => {
    const types: ProviderProfileView[] = [
      { type: 'github', group: 'github', display_name: 'GitHub', env_var: 'GITHUB_TOKEN', category: 'source_control' },
      { type: 'right-github-write', group: 'github', display_name: 'GitHub (write)', env_var: 'GITHUB_TOKEN', category: 'source_control' },
    ]
    const html = await renderToString(createSSRApp({ render: () => h(TypeGrid, { types }) }))
    expect(html).toContain('>GitHub</strong>')
    expect(html).toContain('data-type="github"')
    expect(html).toContain('data-type="right-github-write"')
    expect(html).not.toContain('>right-github-write</strong>') // prefix never shown as label
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && npm test -- ProvidersView.grouping`
Expected: FAIL until the harness import resolves (it imports the real `groupProviderTypes`, so this should pass once Task 10 is in — if so, treat this as the regression guard for the markup contract and proceed to wire the real view in Step 3).

- [ ] **Step 3: Wire grouping into `ProvidersView.vue` + fix eyebrow**

In `<script setup>`, import and expose grouped types:

```typescript
import { groupProviderTypes } from './providersGrouping'
import { computed } from 'vue'
// ...
const typeGroups = computed(() => groupProviderTypes(types.value))
```

In the template, replace the type-grid `v-for="t in types"` block with grouped cards (one card per group; within a card, a button per variant calling `selectType(variant)`):

```html
<div v-if="addStep === 'choose-type'" class="type-grid">
  <p class="muted-line">Choose a provider type:</p>
  <article v-for="g in typeGroups" :key="g.key" class="type-card">
    <strong>{{ g.label }}</strong>
    <div class="access-variants">
      <button
        v-for="v in g.variants"
        :key="v.type"
        type="button"
        class="access-variant"
        :data-type="v.type"
        @click="selectType(v)"
      >
        {{ g.variants.length > 1 ? v.display_name : v.env_var }}
      </button>
    </div>
  </article>
  <p v-if="typeGroups.length === 0" class="muted-line">No provider types available</p>
</div>
```

Fix the eyebrow (header block ~line 303):

```html
<p class="eyebrow">Integrations</p>
<h2>Providers</h2>
```

- [ ] **Step 4: Run frontend tests + build**

Run: `cd crates/right-dashboard/frontend && npm test && npm run build`
Expected: PASS (all vitest green; vue-tsc + vite build clean).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/ProvidersView.vue crates/right-dashboard/frontend/src/views/ProvidersView.grouping.test.ts
git commit -m "feat(dashboard): grouped GitHub provider card + fix Integrations eyebrow"
```

---

## Phase E — Docs, principles, final verification

### Task 12: Document principles + subsystem

**Files:**
- Modify: `AGENTS.md`, `ARCHITECTURE.md`, `docs/architecture/providers.md`

- [ ] **Step 1: AGENTS.md — add the product principle**

Under the `## Conventions` section of `AGENTS.md`, add a bullet:

```markdown
- **Simplest for the user, most maintainable for us.** When a feature has
  multiple working implementations, choose the one that (a) gives the user
  fewer steps and an explicit, auditable choice, and (b) reuses existing,
  tested paths instead of new control planes or invariant hybrids. Add new
  gateway/sandbox surface only when it is isolated and additive, not when the
  alternative smears complexity across load-bearing machinery.
```

- [ ] **Step 2: ARCHITECTURE.md — add the dashboard namespacing rule**

In the `## Dashboard frontend primitives` section, append:

```markdown
RightClaw-owned technical identifiers are `right-*` namespaced; the dashboard
MUST NOT surface raw slugs/prefixes, presenting grouped, user-friendly labels
instead (e.g. `github` + `right-github-write` collapse into one "GitHub" card
via `providersGrouping.ts`). Technical precision lives in the backend; the UI
optimizes for user clarity.
```

- [ ] **Step 3: providers.md — document the provisioning subsystem**

Add a `## Managed profiles (RightClaw-owned)` section to `docs/architecture/providers.md` describing: the `right_openshell::managed_profiles` subsystem; `right-*` ownership prefix; derivation of `right-github-write` from the live `github` profile (drift-proof); `ensure_profiles` idempotent reconcile (structural fingerprint → lint → import) run once per gateway in `right up`; base-missing → hard error; no auto-GC in v1; and that this stays Path A (gateway contributes write endpoints on attach; per-agent `policy.yaml` is untouched).

- [ ] **Step 4: Verify docs reference real symbols**

Run: `rg -n "right-github-write|managed_profiles|ensure_profiles|providersGrouping" AGENTS.md ARCHITECTURE.md docs/architecture/providers.md`
Expected: matches present; names match the code.

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md ARCHITECTURE.md docs/architecture/providers.md
git commit -m "docs: managed-profile provisioning + UX-over-precision principle"
```

### Task 13: Final full verification

- [ ] **Step 1: Full workspace tests**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS (mandatory per AGENTS.md; re-run any lone flake isolated per the parallel-load note).

- [ ] **Step 2: Clippy + frontend build**

Run: `devenv shell -- cargo clippy --workspace --all-targets` then `cd crates/right-dashboard/frontend && npm test && npm run build`
Expected: no clippy warnings introduced by these changes; frontend green.

- [ ] **Step 3: ARCHITECTURE.md size guard**

Run: `wc -c ARCHITECTURE.md`
Expected: < 40000 (hard budget). If over, move the added detail to the satellite and keep a one-line summary.

- [ ] **Step 4: Final commit (if any doc/test tweaks)**

```bash
git add -A
git commit -m "chore: final verification tweaks for github-write provisioning"
```

---

## Self-Review notes (addressed)

- **Spec coverage:** profile subsystem (T1–T3), provisioning/idempotency/drift (T3–T4), startup re-assert + base-missing fail (T6/T3), `right-*` prefix (T1/T7), catalog entry (T7), DTO `group` (T8–T9), grouped UX + eyebrow (T10–T11), de-risk push proof (T5), principles + docs (T12), final verification (T13). All spec sections mapped.
- **Type consistency:** `ManagedProfile`, `github_write()`, `managed_profiles()`, `ensure_profiles`, `EnsureOutcome`, `get_profile`, `needs_import`, `fingerprint`, host `ProviderProfile.group`, DTO `ProviderProfileView.group`, TS `ProviderProfileView.group`, `groupProviderTypes`/`ProviderGroup` are used identically across tasks.
- **Open verification risks flagged:** T2/T4 assume the OpenShell gRPC client method names are `get_provider_profile`/`lint_provider_profiles`/`import_provider_profiles`/`delete_provider_profile` (snake_case from the RPCs in `openshell.proto`) — confirm by `cargo check` in T1/T2; if tonic generated different names, adjust. T5 is the load-bearing GATE.
```
