# GitHub write-access provider + profile-provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let sandboxed agents use GitHub fully (push, fetch-data, LFS, REST/GraphQL writes) by adding a RightClaw-owned `right-github-write` provider profile that opens the github hosts to all HTTP methods via explicit L7 rules, provisioned to the OpenShell gateway on every `right up`, surfaced as a user-friendly grouped choice in the dashboard.

**Architecture:** A new `right_openshell::managed_profiles` subsystem owns RightClaw-authored OpenShell provider profiles. `right-github-write` is *derived* at startup from the live built-in `github` profile by clearing the coarse `access` preset on every endpoint and setting an explicit allow-all L7 rule (`allow { method:"*", path:"**" }` for REST, `operation_type:"*"` for GraphQL). `ensure_profiles` idempotently re-asserts the set on the gateway (structural diff → lint → import) — pure Path A: the gateway contributes the endpoints on attach; we never touch the per-agent `policy.yaml`. The dashboard groups the read-only `github` and write `right-github-write` types into one "GitHub" card; the `right-*` slug is never shown raw.

**Why explicit allow-all rules (not `access: read-write`):** A live experiment proved that the built-in `github` profile's `access: read-only`, once the proxy TLS-terminates github.com to inject the credential, blocks ALL git POSTs (`git-upload-pack` fetch + `git-receive-pack` push) with `X-OpenShell-Policy`. An explicit-rules profile allowing all methods unblocked both (a real push succeeded). The `read-write`/`full` presets were NOT validated; `access` and `rules` are mutually exclusive (proto), so the transform clears `access` and sets rules. Allow-all (not a git-only whitelist) because the repos use Git LFS and agents need more than push; the real authorization boundary is the token's GitHub permissions, not OpenShell's method filter on a trusted credential-injected host.

**Tech Stack:** Rust (edition 2024, tonic/prost gRPC, thiserror, miette), Vue 3 + TypeScript (vitest, @vue/server-renderer), OpenShell v0.0.50 gRPC (`GetProviderProfile`/`LintProviderProfiles`/`ImportProviderProfiles`/`DeleteProviderProfile`).

---

## Verified proto & helper facts (trust these — confirmed against the real code this cycle)

- Module paths: `proto_v1 = crate::openshell_proto::openshell::v1`, `sandbox_v1 = crate::openshell_proto::openshell::sandbox::v1`.
- `proto_v1::ProviderProfile { id: String, display_name: String, description: String, category: i32, credentials: Vec<ProviderProfileCredential>, endpoints: Vec<sandbox_v1::NetworkEndpoint>, binaries: Vec<sandbox_v1::NetworkBinary>, inference_capable: bool, discovery: Option<ProviderProfileDiscovery> }` (derives `Default`, `Clone`).
- `sandbox_v1::NetworkEndpoint { host: String, port: u32, protocol: String, tls: String, enforcement: String, access: String, rules: Vec<sandbox_v1::L7Rule>, allowed_ips: Vec<String>, .. }` (derives `Default`). `access` is mutually exclusive with `rules`.
- `sandbox_v1::L7Rule { allow: Option<sandbox_v1::L7Allow> }`.
- `sandbox_v1::L7Allow { method: String, path: String, command: String, query: <map>, operation_type: String, operation_name: String, fields: Vec<String> }` (derives `Default`). Empty `fields` = match all GraphQL fields.
- gRPC client: `crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient<tonic::transport::Channel>`. Async methods: `get_provider_profile`, `lint_provider_profiles`, `import_provider_profiles`, `delete_provider_profile`.
- Request/response: `GetProviderProfileRequest { id }`, `GetProviderProfileResponse { profile: Option<ProviderProfile> }`, `LintProviderProfilesRequest { profiles: Vec<ProviderProfileImportItem> }`, `LintProviderProfilesResponse { diagnostics: Vec<ProviderProfileDiagnostic>, valid: bool }`, `ImportProviderProfilesRequest { profiles }`, `DeleteProviderProfileRequest { id }`, `ProviderProfileImportItem { profile: Option<ProviderProfile>, source: String }`, `ProviderProfileDiagnostic { field: String, message: String, .. }`.
- `right_openshell::openshell::{default_mtls_dir() -> PathBuf, connect_grpc(&Path) -> miette::Result<OpenShellClient<Channel>>, preflight_check() -> OpenShellStatus, OpenShellStatus::Ready(PathBuf)}`.
- Host catalog (`crates/right-openshell/src/providers.rs`): struct `ProviderProfile { type_slug, env_var, display_name, category: ProviderCategory }` (~line 62); `ProviderCategory { Inference, Agent, SourceControl, Messaging, Other }` (~70); `profile_catalog()` returns 9 entries — 8 built-in (`anthropic, openai, nvidia, codex, copilot, opencode, github, gitlab`) + `generic` (~85); test `catalog_has_8_built_in_plus_generic` (~480).
- The real built-in `github` profile has 3 endpoints: `api.github.com` (`protocol: rest`), `api.github.com` (`protocol: graphql`, `path: /graphql`), `github.com` (`protocol: rest`) — all `access: read-only`, no `rules`.
- `right_openshell::providers::{create_provider(client,&ProviderSpec), attach_to_sandbox(client,sandbox,provider), detach_from_sandbox, delete_provider, ProviderSpec { name:String, type_:String, credentials: HashMap<String,String>, config }}`.
- `right_openshell::test_support::TestSandbox` — constructors `create(name)` and `create_with_policy(name,yaml)`; accessor `name()`; the two command-runners are `exec` and `exec_with_timeout`, both taking `&[&str]` and returning `(stdout, exit_code)` as `(String, i32)`.
- **Integration-test contract:** live-gateway / TestSandbox tests MUST be `#[ignore = "ci-openshell: …"]` with a `ci_openshell_` fn-name prefix (`crates/right/tests/ci_ignored_contract.rs` enforces the marker→name rule). Mirror the existing `crates/right-openshell/tests/ci_openshell_provider.rs`.

---

## Pre-flight: baseline verification

- [ ] **Step 0: Baseline build + targeted tests pass before changes**

Run: `devenv shell -- cargo test -p right-openshell -p right-dashboard`
Expected: PASS (record any pre-existing failures; per the parallel-load flakiness note, re-run a lone failure isolated before blaming it).

---

## File Structure

**Created:**
- `crates/right-openshell/src/managed_profiles.rs` — `ManagedProfile` types, the managed-profile list, derivation/transform (allow-all rules), `ensure_profiles`, and the thin `ProviderProfile` gRPC wrappers (get/lint/import/delete). Owns the OpenShell **ProviderProfile** RPC surface (sibling to `providers.rs`, which owns the **Provider** surface).
- `crates/right-openshell/tests/ci_openshell_github_write.rs` — live-gateway integration tests (idempotency, allow-all rules present) + the regression GATE (read-only blocks git POST vs allow-all reaches GitHub).
- `crates/right-dashboard/frontend/src/views/providersGrouping.ts` + `.test.ts` — pure `groupProviderTypes()` helper + unit test.
- `crates/right-dashboard/frontend/src/views/ProvidersView.grouping.test.ts` — SSR markup regression test.

**Modified:**
- `crates/right-openshell/src/lib.rs` — register `pub mod managed_profiles;`.
- `crates/right-openshell/src/providers.rs` — add `right-github-write` to `profile_catalog()`, add `group` to host `ProviderProfile`, update catalog tests.
- `crates/right-openshell/Cargo.toml` — `[[test]]` entry for the new test (with `required-features = ["test-support"]`).
- `crates/right/src/main.rs` — new `right up` provisioning stage after `up: openshell_preflight`.
- `crates/right/src/internal_api_providers.rs` — add `group` to `ProviderProfileView` DTO + mapping.
- `crates/right-dashboard/frontend/src/types.ts` — add `group` to `ProviderProfileView`.
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue` — grouped type cards w/ access-variant selector; eyebrow `AI Providers` → `Integrations`.
- `AGENTS.md`, `ARCHITECTURE.md`, `docs/architecture/providers.md` — principles + subsystem docs.

---

## Phase A — Profile-provisioning subsystem (`right-openshell`)

### Task 1: `ManagedProfile` types, registry, and the allow-all derivation transform

**Files:**
- Create: `crates/right-openshell/src/managed_profiles.rs`
- Modify: `crates/right-openshell/src/lib.rs`

- [ ] **Step 1: Write the failing test**

In `crates/right-openshell/src/managed_profiles.rs`, add at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
    use crate::openshell_proto::openshell::v1 as proto_v1;

    fn base_github() -> proto_v1::ProviderProfile {
        let ro = |host: &str, protocol: &str| sandbox_v1::NetworkEndpoint {
            host: host.into(),
            port: 443,
            protocol: protocol.into(),
            access: "read-only".into(),
            enforcement: "enforce".into(),
            rules: vec![],
            ..Default::default()
        };
        proto_v1::ProviderProfile {
            id: "github".into(),
            display_name: "GitHub".into(),
            description: "GitHub API and Git operations".into(),
            category: 4, // SOURCE_CONTROL
            credentials: vec![],
            endpoints: vec![
                ro("api.github.com", "rest"),
                ro("api.github.com", "graphql"),
                ro("github.com", "rest"),
            ],
            binaries: vec![],
            inference_capable: false,
            discovery: None,
        }
    }

    #[test]
    fn derive_github_write_opens_all_methods_and_renames() {
        let derived = github_write().derive(base_github());
        assert_eq!(derived.id, "right-github-write");
        assert_eq!(derived.display_name, "GitHub (write)");
        assert_eq!(derived.category, 4, "category preserved from base");
        assert!(!derived.endpoints.is_empty());
        for ep in &derived.endpoints {
            assert!(ep.access.is_empty(), "access cleared (exclusive with rules)");
            assert_eq!(ep.rules.len(), 1, "exactly one allow rule per endpoint");
            let allow = ep.rules[0].allow.as_ref().expect("allow set");
            if ep.protocol == "graphql" {
                assert_eq!(allow.operation_type, "*", "graphql: any operation");
            } else {
                assert_eq!(allow.method, "*", "rest: any method");
                assert_eq!(allow.path, "**", "rest: any path");
            }
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

At the TOP of `crates/right-openshell/src/managed_profiles.rs`:

```rust
//! RightClaw-owned OpenShell provider profiles.
//!
//! This module owns the OpenShell **ProviderProfile** RPC surface
//! (get/lint/import/delete) and the set of profiles RightClaw provisions to
//! the gateway. It is a sibling to `providers.rs` (which owns the **Provider**
//! surface). All RightClaw profile ids are `right-*` prefixed — the prefix is
//! the ownership marker.

use crate::openshell_proto::openshell::sandbox::v1 as sandbox_v1;
use crate::openshell_proto::openshell::v1 as proto_v1;
use thiserror::Error;

/// All managed-profile errors — FAIL FAST, never swallowed.
#[derive(Debug, Error)]
pub enum ManagedProfileError {
    #[error("openshell gRPC: {0}")]
    Grpc(String),
    #[error(
        "base profile \"{0}\" not found on gateway — cannot derive a managed profile from it"
    )]
    BaseMissing(String),
    #[error("profile \"{id}\" failed lint: {detail}")]
    LintFailed { id: String, detail: String },
}

/// A profile RightClaw provisions to the gateway.
#[derive(Debug, Clone)]
pub enum ManagedProfile {
    /// Clone the live `github` profile and open all endpoints to every method.
    GithubWrite,
    // Future: Authored(Box<proto_v1::ProviderProfile>) for e.g. right-browser-use.
}

/// An allow-all L7 rule for an endpoint of the given `protocol`. REST endpoints
/// get `method:"*", path:"**"`; GraphQL endpoints get `operation_type:"*"`
/// (empty `fields` matches all root fields). This is the empirically-verified
/// shape that lets git POSTs through once the proxy TLS-terminates github.com
/// for credential injection.
fn allow_all(protocol: &str) -> sandbox_v1::L7Rule {
    let allow = if protocol == "graphql" {
        sandbox_v1::L7Allow {
            operation_type: "*".into(),
            operation_name: "*".into(),
            ..Default::default()
        }
    } else {
        sandbox_v1::L7Allow {
            method: "*".into(),
            path: "**".into(),
            ..Default::default()
        }
    };
    sandbox_v1::L7Rule { allow: Some(allow) }
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

    /// Produce the desired profile from a fetched base profile. For
    /// `GithubWrite`: clear the `access` preset on every endpoint (mutually
    /// exclusive with `rules`) and set an explicit allow-all rule.
    pub fn derive(&self, mut base: proto_v1::ProviderProfile) -> proto_v1::ProviderProfile {
        match self {
            ManagedProfile::GithubWrite => {
                base.id = self.id().into();
                base.display_name = "GitHub (write)".into();
                for ep in &mut base.endpoints {
                    let rule = allow_all(&ep.protocol);
                    ep.access.clear();
                    ep.rules = vec![rule];
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
/// (see ARCHITECTURE.md "promote on demand"). Add a variant + an entry here to
/// ship a new profile (e.g. right-browser-use).
pub fn managed_profiles() -> Vec<ManagedProfile> {
    vec![ManagedProfile::GithubWrite]
}
```

Register the module: in `crates/right-openshell/src/lib.rs`, next to `pub mod providers;`, add `pub mod managed_profiles;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell managed_profiles::tests`
Expected: PASS (2 tests). Confirm no new warnings: `devenv shell -- cargo build -p right-openshell 2>&1 | rg -i "warning:|error" || echo CLEAN` (ignore the pre-existing toolchain `provenance` note).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs crates/right-openshell/src/lib.rs
git commit -m "feat(providers): managed-profile types + github-write allow-all derivation"
```

### Task 2: Profile gRPC wrappers + structural-diff helper

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs`

- [ ] **Step 1: Write the failing test (fingerprint diff detects a rules change)**

Add to the `tests` module in `managed_profiles.rs`:

```rust
    #[test]
    fn needs_import_true_when_rules_differ() {
        let desired = github_write().derive(base_github());
        let stored_same = desired.clone();
        // A stored profile still on the old read-only preset (no rules) must be
        // detected as drift, since the signal now lives in `rules`/`access`.
        let stored_old = base_github();

        assert!(!needs_import(Some(&stored_same), &desired), "identical → no import");
        assert!(needs_import(Some(&stored_old), &desired), "rules/access drift → import");
        assert!(needs_import(None, &desired), "absent → import");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell needs_import`
Expected: FAIL — `needs_import` not defined.

- [ ] **Step 3: Implement diff helper + gRPC wrappers**

Add to `managed_profiles.rs` imports (top of file):

```rust
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use tonic::transport::Channel;
```

Add (outside the tests module):

```rust
/// Per-endpoint fingerprint of the fields RightClaw controls. Includes
/// `access` AND `rules` (the allow-all signal lives in `rules`), so a profile
/// still on the old read-only preset is detected as drift.
fn endpoint_fp(
    e: &sandbox_v1::NetworkEndpoint,
) -> (String, u32, String, String, Vec<(String, String, String, String)>) {
    let mut rules: Vec<(String, String, String, String)> = e
        .rules
        .iter()
        .map(|r| {
            let a = r.allow.clone().unwrap_or_default();
            (a.method, a.path, a.command, a.operation_type)
        })
        .collect();
    rules.sort();
    (e.host.clone(), e.port, e.protocol.clone(), e.access.clone(), rules)
}

/// Stable structural fingerprint of a profile. Compared instead of the whole
/// message so gateway-filled defaults don't force re-imports.
#[allow(clippy::type_complexity)]
fn fingerprint(
    p: &proto_v1::ProviderProfile,
) -> (
    String,
    String,
    i32,
    Vec<(String, u32, String, String, Vec<(String, String, String, String)>)>,
) {
    let mut eps: Vec<_> = p.endpoints.iter().map(endpoint_fp).collect();
    eps.sort();
    (p.id.clone(), p.display_name.clone(), p.category, eps)
}

/// True if `desired` must be (re)imported given the currently `stored` profile.
fn needs_import(
    stored: Option<&proto_v1::ProviderProfile>,
    desired: &proto_v1::ProviderProfile,
) -> bool {
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
Expected: PASS. Then `devenv shell -- cargo build -p right-openshell 2>&1 | rg -i "warning:|error" || echo CLEAN`.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): profile gRPC wrappers + rules-aware structural diff"
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
/// Derived profiles re-read their base each call (drift-proof). A missing base
/// is a hard error (FAIL FAST). Re-imports happen only on real diff.
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

Run: `devenv shell -- cargo check -p right-openshell` → PASS, then `devenv shell -- cargo test -p right-openshell managed_profiles` → existing 3 unit tests still PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): ensure_profiles idempotent reconcile loop"
```

### Task 4: Live-gateway provisioning tests (`#[ignore]`, `ci_openshell_`)

**Files:**
- Create: `crates/right-openshell/tests/ci_openshell_github_write.rs`
- Modify: `crates/right-openshell/Cargo.toml`

Per AGENTS.rust.md §5 and `ci_ignored_contract.rs`, these are `#[ignore = "ci-openshell: …"]` with `ci_openshell_` names. Read `crates/right-openshell/tests/ci_openshell_provider.rs` first to mirror the client-setup idiom.

- [ ] **Step 1: Add the `[[test]]` entry (Task 5 adds TestSandbox use, so require the feature now)**

In `crates/right-openshell/Cargo.toml`, next to the existing `[[test]] name = "ci_openshell_provider"` entry, add:

```toml
[[test]]
name = "ci_openshell_github_write"
required-features = ["test-support"]
```

- [ ] **Step 2: Write the provisioning tests**

```rust
//! Live OpenShell gateway tests for RightClaw managed profile provisioning.
//! Each test is `#[ignore]` (ci-openshell:) — requires a live gateway with the
//! built-in `github` base profile present. Invoked explicitly by CI.

use right_openshell::managed_profiles::{
    EnsureOutcome, delete_profile, ensure_profiles, get_profile, github_write,
};
use right_openshell::openshell::{connect_grpc, default_mtls_dir};

#[tokio::test]
#[ignore = "ci-openshell: live github-write profile provisioning"]
async fn ci_openshell_github_write_imports_allow_all_and_is_idempotent() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();

    // Clean slate in case a prior run left it behind (ignore NotFound).
    let _ = delete_profile(&mut client, "right-github-write").await;

    let first = ensure_profiles(&mut client, &[github_write()]).await.expect("ensure");
    assert_eq!(first, vec![EnsureOutcome::Imported("right-github-write".into())]);

    let stored = get_profile(&mut client, "right-github-write")
        .await
        .expect("get")
        .expect("present after import");
    assert!(!stored.endpoints.is_empty());
    for ep in &stored.endpoints {
        assert!(ep.access.is_empty(), "host {} still has access preset", ep.host);
        assert_eq!(ep.rules.len(), 1, "host {} missing allow rule", ep.host);
    }

    let second = ensure_profiles(&mut client, &[github_write()]).await.expect("ensure2");
    assert_eq!(second, vec![EnsureOutcome::Unchanged("right-github-write".into())]);

    delete_profile(&mut client, "right-github-write").await.expect("cleanup delete");
}

#[tokio::test]
#[ignore = "ci-openshell: live github-write profile provisioning"]
async fn ci_openshell_get_profile_absent_returns_none() {
    let mtls_dir = default_mtls_dir();
    let mut client = connect_grpc(&mtls_dir).await.unwrap();
    assert!(
        get_profile(&mut client, "definitely-not-a-profile-xyz")
            .await
            .expect("get")
            .is_none()
    );
}
```

- [ ] **Step 3: Compile + run against the local gateway**

Run: `devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_github_write --no-run` (clean compile), then `devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_github_write -- --ignored --test-threads=1 --nocapture`
Expected: 2 passed. If gateway lint rejects the allow-all rules, STOP and report verbatim — it bears on Task 5.

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_github_write.rs crates/right-openshell/Cargo.toml
git commit -m "test(providers): live-gateway github-write provisioning tests (ci-openshell)"
```

### Task 5: Regression GATE — allow-all unblocks git POST; read-only blocks it

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_github_write.rs`

This encodes the **already-verified** matrix (a regression test, not a go/no-go spike): under a raw-tunnel base mimicking production, the read-only github profile blocks `POST git-receive-pack` with `X-OpenShell-Policy`, while the allow-all `right-github-write` lets it reach GitHub. Env-guarded; no-op without a real token so an accidental `--ignored` run does not panic. NEVER echo the token.

- [ ] **Step 1: Add the regression GATE test**

Append to `ci_openshell_github_write.rs`:

```rust
/// Regression GATE (run deliberately with creds):
///   RIGHT_TEST_GH_TOKEN=<PAT/OAuth with push to RIGHT_TEST_GH_PUSH_REPO>
///   RIGHT_TEST_GH_PUSH_REPO=<owner/repo the token may force-push a throwaway branch to>
/// Proves the design end-to-end: ensure right-github-write → create a provider
/// with the real token → attach to a TestSandbox (raw-tunnel base, mirroring
/// production) → inside the sandbox `git push` a throwaway branch using the
/// provider's injected GITHUB_TOKEN placeholder (proxy substitutes it) → assert
/// success (NOT a 403 X-OpenShell-Policy) → delete the branch. The token is
/// spliced into the URL INSIDE the sandbox only and redacted from captured
/// output. Without both env vars set, this is a no-op.
#[tokio::test]
#[ignore = "ci-openshell: live github push regression (needs RIGHT_TEST_GH_TOKEN + throwaway repo)"]
async fn ci_openshell_github_write_push_succeeds() {
    use right_openshell::managed_profiles::{ensure_profiles, github_write};
    use right_openshell::providers::{
        ProviderSpec, attach_to_sandbox, create_provider, delete_provider, detach_from_sandbox,
    };
    use right_openshell::test_support::TestSandbox;

    let token = match std::env::var("RIGHT_TEST_GH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => { eprintln!("skip: set RIGHT_TEST_GH_TOKEN + RIGHT_TEST_GH_PUSH_REPO"); return; }
    };
    let repo = match std::env::var("RIGHT_TEST_GH_PUSH_REPO") {
        Ok(r) if !r.is_empty() => r,
        _ => { eprintln!("skip: set RIGHT_TEST_GH_TOKEN + RIGHT_TEST_GH_PUSH_REPO"); return; }
    };

    let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();
    ensure_profiles(&mut client, &[github_write()]).await.expect("ensure");

    let pid = std::process::id();
    let prov = format!("rightprobe-{pid}-ghwrite");
    let mut creds = std::collections::HashMap::new();
    creds.insert("GITHUB_TOKEN".to_string(), token);
    create_provider(&mut client, &ProviderSpec {
        name: prov.clone(),
        type_: "right-github-write".into(),
        credentials: creds,
        config: Default::default(),
    }).await.expect("create provider");

    // Raw-tunnel base (mirrors production permissive policy): github.com:443
    // reachable as tls:skip; the provider injects the L7 segment on top.
    let base = "version: 1\n\
filesystem_policy: { include_workdir: true, read_write: [/tmp, /sandbox] }\n\
process: { run_as_user: sandbox, run_as_group: sandbox }\n\
network_policies:\n  outbound:\n    endpoints:\n\
      - { host: \"0.0.0.0/0\", port: 443, tls: skip }\n\
      - { host: \"0.0.0.0/0\", port: 80, tls: skip }\n\
    binaries: [{ path: \"**\" }]\n";
    let sandbox = TestSandbox::create_with_policy("ci-openshell-ghwrite-push", base).await;
    attach_to_sandbox(&mut client, sandbox.name(), &prov).await.expect("attach");

    let branch = format!("zz-rightclaw-probe-{pid}");
    let script = format!(
        "set -e; set +x; export GIT_TERMINAL_PROMPT=0; d=$(mktemp -d); cd \"$d\"; \
git init -q; git config user.email p@e.invalid; git config user.name p; \
echo probe > p.txt; git add p.txt; git commit -q -m probe; \
authed=\"https://x-access-token:${{GITHUB_TOKEN}}@github.com/{repo}.git\"; \
git push --no-verify \"$authed\" HEAD:refs/heads/{branch} >/dev/null 2>e.txt && echo PUSH_OK || \
{{ echo PUSH_FAIL; sed -E 's#x-access-token:[^@]*@#x-access-token:***@#g' e.txt; }}; \
git push \"$authed\" :refs/heads/{branch} >/dev/null 2>&1 || true"
    );
    let (out, code) = sandbox.exec_with_timeout(&["sh", "-lc", &script], 120).await;

    let _ = detach_from_sandbox(&mut client, sandbox.name(), &prov).await;
    let _ = delete_provider(&mut client, &prov).await;

    assert!(!out.contains("x-access-token:ghp_") && !out.contains("x-access-token:gho_"),
        "refusing to print a raw token");
    eprintln!("push regression exit={code}\n{out}");
    assert!(out.contains("PUSH_OK"),
        "git push was blocked (read-only would 403 X-OpenShell-Policy here); output: {out}");
}
```

- [ ] **Step 2: Compile**

Run: `devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_github_write --no-run`
Expected: clean compile.

- [ ] **Step 3: Run the regression with real creds (deliberate)**

Run: `RIGHT_TEST_GH_TOKEN=… RIGHT_TEST_GH_PUSH_REPO=<owner/repo> devenv shell -- cargo test -p right-openshell --features test-support --test ci_openshell_github_write -- --ignored --test-threads=1 --nocapture ci_openshell_github_write_push_succeeds`
Expected: PASS — `PUSH_OK`. (Mechanism already verified manually this cycle; this guards against OpenShell drift.)

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_github_write.rs
git commit -m "test(providers): regression GATE — allow-all unblocks git push"
```

---

## Phase B — Startup hook (`right up`)

### Task 6: Provision managed profiles on `right up`

**Files:**
- Modify: `crates/right/src/main.rs` (just after the `up: openshell_preflight` tracing log, ~line 2784)

- [ ] **Step 1: Add the provisioning stage**

Immediately after the `tracing::info!(… "up: openshell_preflight")` block in `cmd_up`, add:

```rust
// Provision RightClaw-owned provider profiles (right-*) to the gateway,
// once per gateway, before bots start. FAIL FAST on error.
{
    use right_openshell::openshell::{OpenShellStatus, connect_grpc, preflight_check};
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

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: PASS. (If `preflight_check`/`OpenShellStatus` import paths differ, fix per the compiler — they live in `right_openshell::openshell`.)

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/main.rs
git commit -m "feat(up): provision right-* managed profiles to gateway on startup"
```

---

## Phase C — Catalog + DTO `group` plumbing

### Task 7: Add `right-github-write` to the catalog + `group` field

**Files:**
- Modify: `crates/right-openshell/src/providers.rs` (struct ~62, `profile_catalog` ~85, tests ~480)

- [ ] **Step 1: Update the failing catalog test**

In the `#[cfg(test)] mod tests` of `providers.rs`, add:

```rust
    #[test]
    fn catalog_has_github_read_and_write_grouped() {
        let catalog = profile_catalog();
        let upstream_builtin = catalog
            .iter()
            .filter(|p| p.type_slug != "generic" && !p.type_slug.starts_with("right-"))
            .count();
        assert_eq!(upstream_builtin, 8, "8 upstream built-ins unchanged");

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

Extend the host struct (`providers.rs` ~62):

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

Add `group` to EVERY existing entry in `profile_catalog()` — for non-grouped providers set `group` equal to `type_slug`. For the github pair:

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
Expected: PASS. `catalog_has_8_built_in_plus_generic` filters `!= "generic"` and asserts the 8 named built-ins; `right-github-write` is neither, so it still passes — but verify, and if any other test counts the whole catalog, bump it to include `right-github-write`.

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
Expected: PASS (if a test snapshots the JSON, update it to include `group`).

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
  type, group, display_name: display, env_var: 'GITHUB_TOKEN', category: 'sourcecontrol',
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
    expect(groups.find((g) => g.key === 'openai')!.variants).toHaveLength(1)
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
  /** User-facing label — from the first non-right-* member's display_name. */
  label: string
  variants: ProviderProfileView[]
}

/**
 * Collapse provider types sharing a `group` into one card. The label comes from
 * the first non-`right-*` member (so the RightClaw prefix is never surfaced),
 * stripped of any parenthetical qualifier.
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

- [ ] **Step 1: Write the SSR markup test**

`ProvidersView.grouping.test.ts` (a thin harness mirroring the grouped type-grid markup, so we assert grouping without standing up the stateful view):

```typescript
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, test } from 'vitest'
import { groupProviderTypes } from './providersGrouping'
import type { ProviderProfileView } from '../types'

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
      { type: 'github', group: 'github', display_name: 'GitHub', env_var: 'GITHUB_TOKEN', category: 'sourcecontrol' },
      { type: 'right-github-write', group: 'github', display_name: 'GitHub (write)', env_var: 'GITHUB_TOKEN', category: 'sourcecontrol' },
    ]
    const html = await renderToString(createSSRApp({ render: () => h(TypeGrid, { types }) }))
    expect(html).toContain('>GitHub</strong>')
    expect(html).toContain('data-type="github"')
    expect(html).toContain('data-type="right-github-write"')
    expect(html).not.toContain('>right-github-write</strong>')
  })
})
```

- [ ] **Step 2: Run test to verify it passes (guards the markup contract; relies on Task 10)**

Run: `cd crates/right-dashboard/frontend && npm test -- ProvidersView.grouping`
Expected: PASS once Task 10's helper exists (it imports the real `groupProviderTypes`).

- [ ] **Step 3: Wire grouping into `ProvidersView.vue` + fix eyebrow**

In `<script setup>`:

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

Under `## Conventions` in `AGENTS.md`, add a bullet:

```markdown
- **Simplest for the user, most maintainable for us.** When a feature has
  multiple working implementations, choose the one that (a) gives the user
  fewer steps and an explicit, auditable choice, and (b) reuses existing,
  tested paths instead of new control planes or invariant hybrids. Add new
  gateway/sandbox surface only when it is isolated and additive, not when the
  alternative smears complexity across load-bearing machinery.
```

- [ ] **Step 2: ARCHITECTURE.md — add the dashboard namespacing rule**

In `## Dashboard frontend primitives`, append:

```markdown
RightClaw-owned technical identifiers are `right-*` namespaced; the dashboard
MUST NOT surface raw slugs/prefixes, presenting grouped, user-friendly labels
instead (e.g. `github` + `right-github-write` collapse into one "GitHub" card
via `providersGrouping.ts`). Technical precision lives in the backend; the UI
optimizes for user clarity.
```

- [ ] **Step 3: providers.md — document the subsystem + the verified mechanism**

Add a `## Managed profiles (RightClaw-owned)` section to `docs/architecture/providers.md` describing: the `right_openshell::managed_profiles` subsystem; `right-*` ownership prefix; derivation of `right-github-write` from the live `github` profile by clearing `access` and setting allow-all L7 `rules` (drift-proof); the **verified mechanism** (read-only blocks ALL git POSTs once credential injection forces TLS termination — `access`/`rules` are method-level and mutually exclusive; explicit allow-all rules unblock fetch + push, validated live); `ensure_profiles` idempotent reconcile (fingerprint includes `rules` + `access`) run once per gateway in `right up`; base-missing → hard error; no auto-GC in v1; Path A purity (gateway contributes endpoints on attach; per-agent `policy.yaml` untouched). Note Git LFS is a separate sandbox-tooling concern (out of scope).

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
Expected: PASS (mandatory per AGENTS.md; the `ci_openshell_*` tests are `#[ignore]` so they do not run here — that is intended; re-run any lone flake isolated per the parallel-load note).

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

- **Spec coverage:** provisioning subsystem (T1–T3), provisioning/idempotency/drift (T3–T4), allow-all derivation via explicit rules (T1), `rules`-aware fingerprint (T2), startup re-assert + base-missing fail (T6/T3), `right-*` prefix (T1/T7), catalog entry (T7), DTO `group` (T8–T9), grouped UX + eyebrow (T10–T11), regression GATE proving the verified matrix (T5), principles + docs incl. verified mechanism + git-lfs non-goal (T12), final verification (T13). All revised-spec sections mapped.
- **Type consistency:** `ManagedProfile`, `allow_all`, `github_write()`, `managed_profiles()`, `derive`, `ensure_profiles`, `EnsureOutcome`, `get_profile`, `needs_import`, `fingerprint`/`endpoint_fp`, `lint_and_import`, `delete_profile`, host `ProviderProfile.group`, DTO `ProviderProfileView.group`, TS `ProviderProfileView.group`, `groupProviderTypes`/`ProviderGroup` used identically across tasks.
- **Integration-test contract:** every live-gateway/TestSandbox test is `#[ignore = "ci-openshell: …"]` with a `ci_openshell_` name (T4/T5), matching `ci_ignored_contract.rs` (corrects the prior plan's "non-ignored" mistake). Final `cargo test --workspace` stays green without a gateway.
- **Verified, not assumed:** the load-bearing behavior (read-only blocks git POSTs; allow-all rules unblock) was confirmed live this cycle; T5 encodes it as a regression rather than a go/no-go gate. The `read-write`/`full` presets were deliberately NOT used (unverified); explicit allow-all rules are the proven mechanism.
- **Open verification risk:** gateway lint must accept the cross-protocol allow-all rule on the GraphQL endpoint (T4 Step 3 catches a rejection). The tonic method names (`get_provider_profile`/`lint_provider_profiles`/`import_provider_profiles`/`delete_provider_profile`) are confirmed against the generated client this cycle.
