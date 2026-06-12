# Generic-provider env-var UX + multi-host + built-in `Fal` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make any static-API-key provider configurable by reading the API's auth doc (name the env var + list host(s)); support multi-host generic providers; ship a one-click built-in `Fal` profile.

**Architecture:** OpenShell static-cred injection is verbatim placeholder substitution keyed by env-var name — the agent writes the auth header itself, so `header_name`/`auth_style` are inert. We drop the misleading `header_name` field, make `GenericProvider` carry `upstream_hosts: Vec<String>` (back-compat with legacy single `upstream_host`), author one OpenShell endpoint per host, confirm composition across **all** hosts, and add a `right-fal` catalog entry + managed profile.

**Tech Stack:** Rust (edition 2024, workspace crates `right-agent-config`, `right-openshell`, `right`, `bot`), Vue 3 + TS (`right-dashboard` frontend), OpenShell gRPC, `cargo test`, `devenv shell`.

**Spec:** `docs/superpowers/specs/2026-06-12-generic-provider-env-var-ux-multihost-fal-design.md`

**Verification cadence:** targeted per-crate tests after each task; full `devenv shell -- cargo test --workspace` only at the end (Task 12). All commands are prefixed with `devenv shell --` per project convention.

---

## Baseline (run once at worktree start)

- [ ] **Step 0: Record a baseline.** Run the crates this plan touches and note any pre-existing failures (see memory: two workspace tests flake under parallel load — re-run isolated before blaming a change).

Run: `devenv shell -- cargo test -p right-agent-config -p right-openshell 2>&1 | tail -30`
Expected: compiles; record any red.

---

## File Structure

- `crates/right-agent-config/src/lib.rs` — `GenericProvider` model: drop `header_name`, add `upstream_hosts` with back-compat `try_from`.
- `crates/right-openshell/src/managed_profiles.rs` — multi-endpoint `author_generic_profile`, `GenericProviderProfileInput`, `fal_profile()`, registry.
- `crates/right-openshell/src/providers.rs` — `profile_catalog()` `right-fal` entry.
- `crates/right-openshell/src/provider_capabilities.rs` — `provider_is_composed_with_all_endpoints`, `build_usage_hint` rewrite.
- `crates/right-openshell/src/openshell.rs` — `wait_for_provider_composed_with_all_endpoints`.
- `crates/right/src/internal_api_providers.rs` — create/update request `upstream_hosts`, agent.yaml writer, multi-host authoring + all-hosts confirmation, drop `header_name`.
- `crates/bot/src/sandbox_supervisor.rs` + `crates/bot/src/telegram/dashboard/providers.rs` — adapt to new signatures/DTO.
- `crates/right/src/main.rs` — CLI `providers add` multi-host, deprecate `--header-name`.
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue`, `providersViewModel.ts` — drop header field, multi-host input, microcopy.
- `docs/architecture/providers.md`, `PROMPT_SYSTEM.md` — cite-on-touch.

---

## Task 1: `GenericProvider` model — drop `header_name`, add multi-host

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs:284-296` (struct + `default_header_name`)
- Test: same file, `#[cfg(test)]` module (existing test at ~`:927`)

- [ ] **Step 1: Write failing tests** — append to the tests module in `crates/right-agent-config/src/lib.rs`:

```rust
#[test]
fn generic_provider_deserializes_legacy_single_host_and_ignores_header_name() {
    let yaml = "env_var: ACME_TOKEN\nheader_name: X-Acme-Token\nupstream_host: api.acme.com\nupstream_path_prefix: /v1\n";
    let g: GenericProvider = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(g.env_var, "ACME_TOKEN");
    assert_eq!(g.upstream_hosts, vec!["api.acme.com".to_string()]);
    assert_eq!(g.upstream_path_prefix.as_deref(), Some("/v1"));
}

#[test]
fn generic_provider_deserializes_multi_host_and_dedups() {
    let yaml = "env_var: FAL_KEY\nupstream_hosts:\n  - fal.run\n  - queue.fal.run\n  - fal.run\n";
    let g: GenericProvider = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(g.upstream_hosts, vec!["fal.run".to_string(), "queue.fal.run".to_string()]);
}

#[test]
fn generic_provider_merges_legacy_and_new_host_fields() {
    let yaml = "env_var: K\nupstream_host: a.example.com\nupstream_hosts:\n  - b.example.com\n";
    let g: GenericProvider = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(g.upstream_hosts, vec!["a.example.com".to_string(), "b.example.com".to_string()]);
}

#[test]
fn generic_provider_rejects_zero_hosts() {
    let yaml = "env_var: K\n";
    assert!(serde_yaml::from_str::<GenericProvider>(yaml).is_err());
}

#[test]
fn generic_provider_roundtrips_to_upstream_hosts() {
    let g = GenericProvider { env_var: "K".into(), upstream_hosts: vec!["a.example.com".into()], upstream_path_prefix: None };
    let s = serde_yaml::to_string(&g).unwrap();
    assert!(s.contains("upstream_hosts"));
    assert!(!s.contains("header_name"));
}
```

- [ ] **Step 2: Run tests, verify they fail to compile** (struct still has `header_name`, `upstream_host`).

Run: `devenv shell -- cargo test -p right-agent-config generic_provider 2>&1 | tail -20`
Expected: compile errors / FAIL.

- [ ] **Step 3: Replace the struct + helper** at `crates/right-agent-config/src/lib.rs:284-296`:

```rust
/// Generic-only fields. Multi-host; the agent writes the auth header itself,
/// so no header/scheme field exists (inert for OpenShell static-cred injection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(try_from = "GenericProviderRaw")]
pub struct GenericProvider {
    pub env_var: String,
    pub upstream_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_path_prefix: Option<String>,
}

#[derive(Deserialize)]
struct GenericProviderRaw {
    env_var: String,
    #[serde(default)]
    upstream_host: Option<String>,
    #[serde(default)]
    upstream_hosts: Option<Vec<String>>,
    #[serde(default)]
    upstream_path_prefix: Option<String>,
    // Accepted for back-compat with pre-multi-host configs; intentionally ignored.
    #[serde(default, rename = "header_name")]
    _legacy_header_name: Option<String>,
}

impl TryFrom<GenericProviderRaw> for GenericProvider {
    type Error = String;
    fn try_from(r: GenericProviderRaw) -> Result<Self, String> {
        let mut hosts: Vec<String> = Vec::new();
        if let Some(h) = r.upstream_host {
            if !h.trim().is_empty() {
                hosts.push(h);
            }
        }
        if let Some(extra) = r.upstream_hosts {
            hosts.extend(extra.into_iter().filter(|h| !h.trim().is_empty()));
        }
        let mut seen = std::collections::HashSet::new();
        hosts.retain(|h| seen.insert(h.clone()));
        if hosts.is_empty() {
            return Err("generic provider requires at least one upstream host".into());
        }
        Ok(GenericProvider {
            env_var: r.env_var,
            upstream_hosts: hosts,
            upstream_path_prefix: r.upstream_path_prefix,
        })
    }
}
```

Delete the now-unused `fn default_header_name()` (lines ~294-296). Confirm `Deserialize` and `HashSet` are in scope (add `use serde::Deserialize;` import only if the file does not already import it — check the top of `lib.rs` first; do NOT touch pre-existing imports otherwise).

- [ ] **Step 4: Run tests, verify pass.**

Run: `devenv shell -- cargo test -p right-agent-config generic_provider 2>&1 | tail -20`
Expected: PASS (the crate's own tests; downstream crates still red — fixed in later tasks).

- [ ] **Step 5: Commit.**

```bash
git add crates/right-agent-config/src/lib.rs
git commit -m "feat(agent-config): GenericProvider multi-host upstream_hosts, drop inert header_name"
```

---

## Task 2: Multi-endpoint `author_generic_profile` + `GenericProviderProfileInput`

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs:128-208` (author fn + input struct + iterator)
- Test: `crates/right-openshell/src/managed_profiles_tests.rs` (existing test file `#[path]`-included)

- [ ] **Step 1: Write failing tests** — in `managed_profiles_tests.rs`, replace the existing `author_generic_profile_*` tests (they pass `header_name`/single host) with:

```rust
#[test]
fn author_generic_profile_emits_one_endpoint_per_host() {
    let hosts = vec!["fal.run".to_string(), "queue.fal.run".to_string()];
    let p = author_generic_profile("right-provider-x", &hosts, Some("/v1"), "FAL_KEY");
    let endpoint_hosts: Vec<&str> = p.endpoints.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(endpoint_hosts, vec!["fal.run", "queue.fal.run"]);
    for e in &p.endpoints {
        assert_eq!(e.protocol, "rest");
        assert_eq!(e.access, "full");
        assert_eq!(e.path, "/v1");
        assert_eq!(e.port, 443);
    }
}

#[test]
fn author_generic_profile_credential_is_fixed_inert_placement() {
    let p = author_generic_profile("right-provider-x", &["api.acme.com".to_string()], None, "ACME_TOKEN");
    let cred = &p.credentials[0];
    assert_eq!(cred.env_vars, vec!["ACME_TOKEN".to_string()]);
    // Fixed canonical-valid placement; inert for static keys.
    assert_eq!(cred.header_name, "Authorization");
    assert_eq!(cred.auth_style, "bearer");
}
```

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right-openshell author_generic_profile 2>&1 | tail -20`
Expected: compile errors (signature mismatch).

- [ ] **Step 3: Rewrite** `author_generic_profile` (`managed_profiles.rs:129-174`) and `GenericProviderProfileInput` (`:177-183`) and `generic_provider_profiles` (`:186-208`):

```rust
/// Author a self-contained OpenShell profile for a generic provider.
/// One endpoint per host. `header_name`/`auth_style` are fixed to the
/// canonical-valid `Authorization`/`bearer` pair: OpenShell stores+validates
/// the placement but does NOT use it for static-credential injection (the
/// agent writes the real auth header), so it is inert on the wire.
pub fn author_generic_profile(
    id: &str,
    upstream_hosts: &[String],
    upstream_path_prefix: Option<&str>,
    env_var: &str,
) -> proto_v1::ProviderProfile {
    let path = upstream_path_prefix.unwrap_or("").to_string();
    let endpoints = upstream_hosts
        .iter()
        .map(|host| sandbox_v1::NetworkEndpoint {
            host: host.clone(),
            port: 443,
            protocol: "rest".into(),
            enforcement: "enforce".into(),
            access: "full".into(),
            path: path.clone(),
            ..Default::default()
        })
        .collect();

    proto_v1::ProviderProfile {
        id: id.to_string(),
        display_name: id.to_string(),
        description: "Right-managed generic provider".into(),
        category: proto_v1::ProviderProfileCategory::Other as i32,
        credentials: vec![proto_v1::ProviderProfileCredential {
            name: "api_token".into(),
            description: String::new(),
            env_vars: vec![env_var.to_string()],
            required: true,
            auth_style: "bearer".into(),
            header_name: "Authorization".into(),
            query_param: String::new(),
            refresh: None,
            path_template: String::new(),
        }],
        endpoints,
        binaries: vec![sandbox_v1::NetworkBinary {
            path: "**".into(),
            ..Default::default()
        }],
        inference_capable: false,
        discovery: None,
    }
}

/// Config-free generic provider data used to author a managed OpenShell profile.
pub struct GenericProviderProfileInput<'a> {
    pub name: &'a str,
    pub upstream_hosts: &'a [String],
    pub upstream_path_prefix: Option<&'a str>,
    pub env_var: &'a str,
}
```

In `generic_provider_profiles`, change the `author_generic_profile(...)` call to:

```rust
profiles.push(ManagedProfile::Authored(Box::new(author_generic_profile(
    &id,
    provider.upstream_hosts,
    provider.upstream_path_prefix,
    provider.env_var,
))));
```

- [ ] **Step 4: Run, verify pass** (crate may still fail elsewhere — Fal task next; run the targeted filter).

Run: `devenv shell -- cargo test -p right-openshell author_generic_profile 2>&1 | tail -20`
Expected: the two tests PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/right-openshell/src/managed_profiles.rs crates/right-openshell/src/managed_profiles_tests.rs
git commit -m "feat(openshell): multi-endpoint author_generic_profile, drop header_name param"
```

---

## Task 3: Built-in `Fal` profile + `managed_profiles()` registry

**Research gate (do this step first, it produces the host list used below):**

- [ ] **Step 0: Confirm fal's authenticated hosts + env var.** Read the official fal client source (Python `fal-client` / JS `@fal-ai/client`) for the API base URLs and the auth env var. Run:

```bash
devenv shell -- bash -lc 'pip download fal-client --no-deps -d /tmp/falc 2>/dev/null; ls /tmp/falc'
```

or fetch the client repo and grep for `fal.run`, `queue.fal.run`, `rest.alpha.fal.ai`, `Authorization`, `FAL_KEY`, `FAL_API_KEY`. Record the confirmed **authenticated** host set and env var. If it differs from the defaults below, use the confirmed values in Step 3. Output-media CDN (`*.fal.media`) and upload targets are **out of scope** (spec §2).

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs` (add `fal_profile()` + registry)
- Test: `crates/right-openshell/src/managed_profiles_tests.rs`

- [ ] **Step 1: Write failing tests:**

```rust
#[test]
fn fal_profile_id_and_hosts() {
    let p = fal_profile();
    assert_eq!(p.id, "right-fal");
    assert_eq!(p.display_name, "fal.ai");
    let hosts: Vec<&str> = p.endpoints.iter().map(|e| e.host.as_str()).collect();
    assert!(hosts.contains(&"fal.run"));
    assert!(hosts.contains(&"queue.fal.run"));
    assert_eq!(p.credentials[0].env_vars, vec!["FAL_KEY".to_string()]);
}

#[test]
fn managed_profiles_registry_includes_fal() {
    let ids: Vec<String> = managed_profiles().iter().map(|p| p.id()).collect();
    assert!(ids.contains(&"right-fal".to_string()));
    assert!(ids.contains(&"right-github".to_string()));
}
```

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right-openshell fal_profile managed_profiles_registry 2>&1 | tail -20`
Expected: FAIL (`fal_profile` undefined).

- [ ] **Step 3: Add `fal_profile()`** near `author_generic_profile` in `managed_profiles.rs`. Use the host set confirmed in Step 0 (defaults shown):

```rust
/// RightClaw built-in fal.ai profile. Authenticated API hosts only — output
/// media CDNs and upload targets are out of scope (they carry no credential).
pub fn fal_profile() -> proto_v1::ProviderProfile {
    let hosts = ["fal.run", "queue.fal.run", "rest.alpha.fal.ai"];
    let endpoints = hosts
        .iter()
        .map(|host| sandbox_v1::NetworkEndpoint {
            host: (*host).to_string(),
            port: 443,
            protocol: "rest".into(),
            enforcement: "enforce".into(),
            access: "full".into(),
            path: String::new(),
            ..Default::default()
        })
        .collect();
    proto_v1::ProviderProfile {
        id: "right-fal".into(),
        display_name: "fal.ai".into(),
        description: "RightClaw-managed fal.ai provider".into(),
        category: proto_v1::ProviderProfileCategory::Other as i32,
        credentials: vec![proto_v1::ProviderProfileCredential {
            name: "api_token".into(),
            description: String::new(),
            env_vars: vec!["FAL_KEY".into()],
            required: true,
            auth_style: "bearer".into(),
            header_name: "Authorization".into(),
            query_param: String::new(),
            refresh: None,
            path_template: String::new(),
        }],
        endpoints,
        binaries: vec![sandbox_v1::NetworkBinary { path: "**".into(), ..Default::default() }],
        inference_capable: false,
        discovery: None,
    }
}
```

Update the registry (`managed_profiles.rs:215-217`):

```rust
pub fn managed_profiles() -> Vec<ManagedProfile> {
    vec![
        ManagedProfile::Github,
        ManagedProfile::Authored(Box::new(fal_profile())),
    ]
}
```

- [ ] **Step 4: Run, verify pass.**

Run: `devenv shell -- cargo test -p right-openshell fal_profile managed_profiles_registry 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/right-openshell/src/managed_profiles.rs crates/right-openshell/src/managed_profiles_tests.rs
git commit -m "feat(openshell): built-in fal.ai managed profile"
```

---

## Task 4: `right-fal` catalog entry

**Files:**
- Modify: `crates/right-openshell/src/providers.rs:92-153` (`profile_catalog()`)
- Test: `crates/right-openshell/src/providers_tests.rs` (or the inline test module that already asserts catalog membership, ~`:692-722`)

- [ ] **Step 1: Write failing test:**

```rust
#[test]
fn catalog_includes_fal() {
    let c = profile_catalog();
    let fal = c.iter().find(|p| p.type_slug == "right-fal").expect("right-fal present");
    assert_eq!(fal.display_name, "fal.ai");
    assert_eq!(fal.env_var, "FAL_KEY");
}
```

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right-openshell catalog_includes_fal 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Add the entry** inside the `vec![ ... ]` in `profile_catalog()` (alongside `right-github`), matching the surrounding `ProviderProfile { type_slug, display_name, .., env_var }` literal shape used at `:131-141`:

```rust
ProviderProfile {
    type_slug: "right-fal".into(),
    display_name: "fal.ai".into(),
    // copy the same remaining fields used by the `right-github` entry above
    env_var: "FAL_KEY".into(),
},
```

(Match every field present on the `right-github` entry — read `:137-141` and replicate field-for-field. Do NOT add it to `HIDDEN_FROM_DASHBOARD`; fal must be user-selectable.)

- [ ] **Step 4: Run, verify pass** — also run the catalog/validator sync tests that already exist:

Run: `devenv shell -- cargo test -p right-openshell catalog 2>&1 | tail -20` and `devenv shell -- cargo test -p right validate_type_slug 2>&1 | tail -20`
Expected: PASS (the `validate_type_slug_in_sync_with_catalog` guard now also covers `right-fal`).

- [ ] **Step 5: Commit.**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/providers_tests.rs
git commit -m "feat(openshell): add right-fal to provider catalog"
```

---

## Task 5: All-hosts composition confirmation

**Files:**
- Modify: `crates/right-openshell/src/provider_capabilities.rs` (add predicate near `provider_is_composed_with_endpoint:91-106`)
- Modify: `crates/right-openshell/src/openshell.rs:2140-2163` (add wait variant beside `wait_for_provider_composed_with_endpoint`)
- Test: `crates/right-openshell/src/provider_capabilities_tests.rs`

- [ ] **Step 1: Write failing test** in `provider_capabilities_tests.rs` (mirror existing composed-policy fixtures in that file):

```rust
#[test]
fn all_endpoints_present_requires_every_host() {
    // Build a SandboxPolicy whose `_provider_p` rule has only fal.run.
    let policy = policy_with_provider_rule("p", &[("fal.run", "")]); // existing helper in this test file
    assert!(provider_is_composed_with_all_endpoints(&policy, "p", &[("fal.run".into(), "".into())]));
    assert!(!provider_is_composed_with_all_endpoints(
        &policy, "p",
        &[("fal.run".into(), "".into()), ("queue.fal.run".into(), "".into())]
    ));
}
```

If no `policy_with_provider_rule` helper exists, reuse whatever fixture the existing `provider_is_composed_with_endpoint` tests use in this file (read them first and follow the same construction).

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right-openshell all_endpoints_present 2>&1 | tail -20`
Expected: FAIL (undefined).

- [ ] **Step 3: Add predicate** after `provider_is_composed_with_endpoint` in `provider_capabilities.rs`:

```rust
/// True when the composed `_provider_<name>` rule contains EVERY expected
/// (host, path). Multi-host providers must confirm all hosts so a stale rule
/// carrying only the unchanged first host cannot pass on an update.
pub fn provider_is_composed_with_all_endpoints(
    policy: &SandboxPolicy,
    provider_name: &str,
    expected: &[(String, String)],
) -> bool {
    rule_for_provider(policy, provider_name).is_some_and(|rule| {
        expected.iter().all(|(host, path)| {
            rule.endpoints.iter().any(|e| {
                e.host.eq_ignore_ascii_case(host) && e.path == *path
            })
        })
    })
}
```

Add the wait wrapper in `openshell.rs` beside `wait_for_provider_composed_with_endpoint`:

```rust
pub async fn wait_for_provider_composed_with_all_endpoints(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
    expected: Vec<(String, String)>,
) -> Result<(), OpenShellError> {
    let provider_name = provider_name.to_string();
    wait_for_provider_composed_where(
        client,
        sandbox_name,
        &provider_name,
        move |policy| {
            crate::provider_capabilities::provider_is_composed_with_all_endpoints(
                policy, &provider_name, &expected,
            )
        },
    )
    .await
}
```

(Match the exact closure/argument shape of `wait_for_provider_composed_with_endpoint:2140-2163` — read it and mirror the `wait_for_provider_composed_where` call signature, including how `provider_name` is moved/cloned.)

- [ ] **Step 4: Run, verify pass.**

Run: `devenv shell -- cargo test -p right-openshell all_endpoints_present 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/right-openshell/src/provider_capabilities.rs crates/right-openshell/src/provider_capabilities_tests.rs crates/right-openshell/src/openshell.rs
git commit -m "feat(openshell): confirm provider composition across all declared hosts"
```

---

## Task 6: Agent-facing usage hint rewrite

**Files:**
- Modify: `crates/right-openshell/src/provider_capabilities.rs:112-147` (`build_usage_hint`)
- Test: `crates/right-openshell/src/provider_capabilities_tests.rs`

- [ ] **Step 1: Write failing test:**

```rust
#[test]
fn usage_hint_names_env_and_tells_agent_to_write_header() {
    let hint = build_usage_hint(&["**".to_string()], &["fal.run".to_string()], true);
    assert!(hint.contains("fal.run"));
    // Steers the agent to write the auth header itself per the API docs.
    let lc = hint.to_lowercase();
    assert!(lc.contains("auth") && (lc.contains("api doc") || lc.contains("yourself") || lc.contains("header")));
}
```

(`build_usage_hint` takes `(allowed_binaries, hosts, active)` — env var names are not a current parameter. To name the env var, extend the signature; see Step 3. Adjust the test call accordingly.)

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right-openshell usage_hint 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Extend `build_usage_hint`** to accept `env_vars: &[String]` and rewrite the active/any-binary branches to name the env var(s) and instruct the agent to write the header per the API docs. Update the single call site in `correlate_provider_capabilities` (`:201`) to pass `&env_vars`. New active/any-binary copy:

```rust
// inside build_usage_hint, when active && any binary == "**":
let env_list = if env_vars.is_empty() { "the injected env var".to_string() } else { env_vars.join(", ") };
return format!(
    "Reach {hosts_list} using {env_list}. Write the auth header yourself exactly as the API documents (e.g. -H \"Authorization: Key ${first_env}\"); the gateway substitutes the secret for the placeholder on matching requests. Do not print the placeholder."
);
```

where `first_env = env_vars.first().map(String::as_str).unwrap_or("ENV")`. Keep the inactive / no-host / no-binary branches as-is (they remain accurate). Update the other unit tests in this file that call `build_usage_hint` to pass the new `env_vars` argument.

- [ ] **Step 4: Run, verify pass.**

Run: `devenv shell -- cargo test -p right-openshell usage_hint provider_capabilities 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/right-openshell/src/provider_capabilities.rs crates/right-openshell/src/provider_capabilities_tests.rs
git commit -m "feat(openshell): usage hint names env var and tells agent to write the auth header"
```

---

## Task 7: `right-openshell` green + callers in `bot`

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs:126,923-925,973` (GenericProviderProfileInput construction + any `generic.header_name` read)
- Modify: `crates/bot/src/telegram/dashboard/providers.rs:26,82` (DTO `header_name` → drop; surface `upstream_hosts`)

- [ ] **Step 1: Compile the workspace to list every break.**

Run: `devenv shell -- cargo build -p right-openshell -p bot 2>&1 | rg -n "error|header_name|upstream_host" | head -40`
Expected: errors at the sites above.

- [ ] **Step 2: Fix `sandbox_supervisor.rs`.** At `:126` the `GenericProviderProfileInput` is built from `&generic.*`:

```rust
// before: header_name: &generic.header_name, upstream_host: &generic.upstream_host
GenericProviderProfileInput {
    name: &generic_name,
    upstream_hosts: &generic.upstream_hosts,
    upstream_path_prefix: generic.upstream_path_prefix.as_deref(),
    env_var: &generic.env_var,
}
```

At `:923` and `:973` (two more `GenericProviderProfileInput { ... header_name: "X-Api-Key" ... }` constructions — these are test fixtures): drop `header_name`, change `upstream_host: "..."` to `upstream_hosts: &["...".to_string()]` (bind a local `let hosts = vec![...];` if a borrow is needed).

- [ ] **Step 3: Fix `telegram/dashboard/providers.rs`.** The DTO at `:26` has `header_name: Option<String>` and `:82` maps `g.header_name`. Remove the `header_name` field; add `upstream_hosts: Vec<String>` mapped from `g.upstream_hosts`. (Read the full struct + mapping to keep field order/types consistent with the dashboard contract.)

- [ ] **Step 4: Build, verify both crates compile.**

Run: `devenv shell -- cargo build -p right-openshell -p bot 2>&1 | tail -15`
Expected: clean build.

- [ ] **Step 5: Run targeted tests + commit.**

Run: `devenv shell -- cargo test -p right-openshell 2>&1 | tail -15`
Expected: PASS (ignored `ci_openshell_` tests skipped).

```bash
git add crates/bot/src/sandbox_supervisor.rs crates/bot/src/telegram/dashboard/providers.rs
git commit -m "refactor(bot): adapt provider callers to multi-host GenericProvider"
```

---

## Task 8: Internal API — multi-host create/update, agent.yaml writer, all-hosts confirm

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` — request structs (`ProviderCreateGeneric:~821`, update generic `~1895`), `handle_provider_create` validation (`~837`), `create_generic_provider` (`~1383-1500`), `generic_provider_update_profile`/authoring (`~1278-1300`), agent.yaml writer (`~1155-1160`), and the update path.

- [ ] **Step 1: Write/adjust failing tests.** Update the existing generic-provider tests in this file (constructions at `:335,510,546,692,2904` use `GenericProvider { header_name, upstream_host, .. }`) to the new shape `GenericProvider { env_var, upstream_hosts, upstream_path_prefix }`. Add:

```rust
#[test]
fn create_generic_accepts_multi_host_request() {
    let g = ProviderCreateGeneric {
        env_var: "FAL_KEY".into(),
        upstream_hosts: vec!["fal.run".into(), "queue.fal.run".into()],
        upstream_path_prefix: None,
    };
    // validation passes for >=1 host, each host valid
    assert!(validate_generic_request(&g).is_ok());
}

#[test]
fn create_generic_rejects_empty_hosts() {
    let g = ProviderCreateGeneric { env_var: "K".into(), upstream_hosts: vec![], upstream_path_prefix: None };
    assert!(validate_generic_request(&g).is_err());
}
```

(If no `validate_generic_request` helper exists, extract one from the inline validation in `handle_provider_create` as part of Step 3 so it is unit-testable.)

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right create_generic 2>&1 | tail -20`
Expected: FAIL / compile errors.

- [ ] **Step 3: Implement.**
  - `ProviderCreateGeneric` (`:821`) and the update generic struct (`:1895`): replace `header_name: Option<String>` + `upstream_host: String` with `upstream_hosts: Vec<String>`. Keep `upstream_path_prefix: Option<String>`. (For wire back-compat with any caller still sending `upstream_host`, add `#[serde(default)] upstream_host: Option<String>` + a normalizer that folds into `upstream_hosts`, mirroring Task 1; the dashboard/CLI will send `upstream_hosts` after Tasks 9-10.)
  - Validation (`handle_provider_create:~837`): replace single `validate_upstream_host` + `validate_header_name` with a loop validating **each** host via the existing `validate_upstream_host` (private/link-local block, HTTP warn) and require ≥1 host. Remove the `validate_header_name` call. Extract `validate_generic_request`.
  - `create_generic_provider` (`:1383-1500`): drop the `header_name` local (`:1396-1399`); call `author_generic_profile(&profile_id, &g.upstream_hosts, g.upstream_path_prefix.as_deref(), &env_var)`; replace the single-endpoint `wait_for_provider_composed_with_endpoint` (`:1481`) with `wait_for_provider_composed_with_all_endpoints(&mut client, &sandbox_name, &name, g.upstream_hosts.iter().map(|h| (h.clone(), g.upstream_path_prefix.clone().unwrap_or_default())).collect())`; build `GenericProvider { env_var, upstream_hosts: g.upstream_hosts.clone(), upstream_path_prefix: g.upstream_path_prefix.clone() }` (drop `header_name`).
  - `generic_provider_update_profile` / `generic_provider_profile_and_spec` (`:1261-1300`): update `author_generic_profile` call to multi-host, drop `header_name` arg.
  - agent.yaml writer (`:1155-1160`): stop writing the `header_name:` line; write each host under `upstream_hosts:` as a YAML list. Read the surrounding writer to match indentation; emit:
    ```
    generic:
      env_var: '<env>'
      upstream_hosts:
        - '<host1>'
        - '<host2>'
      upstream_path_prefix: '<prefix>'   # only if Some
    ```
  - The update handler (`:1948-1980`) similarly drops `header_name` defaulting/validation and switches to multi-host + all-hosts confirmation.

- [ ] **Step 4: Run, verify pass.**

Run: `devenv shell -- cargo test -p right provider 2>&1 | tail -25`
Expected: PASS (non-ignored).

- [ ] **Step 5: Commit.**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(internal-api): multi-host generic providers, all-host composition, drop header_name"
```

---

## Task 9: CLI `right agent providers add` — multi-host, deprecate `--header-name`

**Files:**
- Modify: `crates/right/src/main.rs:480,4695-4735,2983,5192` (CLI arg struct, plumbing, any GenericProvider construction)

- [ ] **Step 1: Write failing test** (CLI parse test near existing `providers add` tests, or an integration test asserting multiple `--upstream-host` collect):

```rust
#[test]
fn providers_add_collects_multiple_upstream_hosts() {
    let args = ProvidersAddArgs::try_parse_from([
        "add", "--agent", "a", "--type", "generic", "--env-var", "FAL_KEY",
        "--upstream-host", "fal.run", "--upstream-host", "queue.fal.run",
    ]).unwrap();
    assert_eq!(args.upstream_host, vec!["fal.run", "queue.fal.run"]);
}
```

(Use the real arg struct name found at `main.rs:480`/`:4695`; adjust field names to match.)

- [ ] **Step 2: Run, verify fail.**

Run: `devenv shell -- cargo test -p right providers_add 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement.** In the `providers add` arg struct: change `upstream_host: Option<String>` to `upstream_host: Vec<String>` (clap collects repeated occurrences). Change `--header-name` to `#[arg(long, hide = true)] header_name: Option<String>` (accepted, ignored — log a one-line deprecation `tracing::warn!` if present). Plumb `upstream_host` (Vec) into the create request as `upstream_hosts`. Update any `GenericProvider { .. }` construction (`:2983` reads `generic.header_name` — remove; `:5192` test fixture — update to multi-host).

- [ ] **Step 4: Run, verify pass + build.**

Run: `devenv shell -- cargo test -p right providers_add 2>&1 | tail -15 && devenv shell -- cargo build -p right 2>&1 | tail -5`
Expected: PASS + clean build.

- [ ] **Step 5: Commit.**

```bash
git add crates/right/src/main.rs
git commit -m "feat(cli): providers add multi --upstream-host, deprecate --header-name"
```

---

## Task 10: Dashboard — drop header field, multi-host input, microcopy, fal type

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/providersViewModel.ts:71-73` (microcopy + add hosts validator)
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue` (add modal `:383-405`, edit modal `:245-280`, submit payloads `:185-191,276-279`)
- Modify: `crates/right-dashboard/frontend/src/views/api_types` (ProviderView.generic / request types — drop `header_name`, add `upstream_hosts: string[]`)
- Test: `crates/right-dashboard/frontend/src/views/providersViewModel.test.ts`, `ProvidersView.test.ts`

- [ ] **Step 1: Write failing viewModel test** in `providersViewModel.test.ts`:

```ts
import { validateUpstreamHosts } from './providersViewModel'
test('requires at least one non-empty host', () => {
  expect(validateUpstreamHosts([])).toBeTruthy()       // returns error string
  expect(validateUpstreamHosts(['', '  '])).toBeTruthy()
  expect(validateUpstreamHosts(['fal.run'])).toBeNull() // ok
})
```

- [ ] **Step 2: Run, verify fail.**

Run: `cd crates/right-dashboard/frontend && pnpm test providersViewModel 2>&1 | tail -20`
Expected: FAIL (`validateUpstreamHosts` undefined).

- [ ] **Step 3: Implement viewModel.** In `providersViewModel.ts`: delete `HEADER_NAME_MICROCOPY` (`:71-73`); add:

```ts
/** Microcopy shown under the hosts field (add/edit generic). */
export const HOSTS_MICROCOPY =
  'Host(s) the agent may call with this key. The agent references the key as $ENV_VAR and writes the auth header itself, exactly as the API docs say (e.g. Authorization: Key $FAL_KEY). RightClaw stores the secret and allows these hosts.'

/** Returns an error string if no non-empty host is provided, else null. */
export function validateUpstreamHosts(hosts: string[]): string | null {
  return hosts.some((h) => h.trim().length > 0)
    ? null
    : 'At least one upstream host is required'
}
```

- [ ] **Step 4: Implement `ProvidersView.vue`.**
  - Replace the single `addUpstreamHost` string ref with `addUpstreamHosts = ref<string[]>([''])` and render a repeatable list of host inputs with add/remove buttons (follow the brand `right-ui`/component patterns already used in the file). Same for `editUpstreamHosts`.
  - **Delete** the `Header name` field block (`:392-396`) and `addHeaderName`/`editHeaderName` refs (`:189,250,277`).
  - Add the `HOSTS_MICROCOPY` helper text under the hosts field.
  - Submit payloads (`:185-191` add, `:276-279` edit): replace `header_name`/`upstream_host` with `upstream_hosts: addUpstreamHosts.value.map(h => h.trim()).filter(Boolean)`.
  - Validation guard (`:168,271`): use `validateUpstreamHosts(...)` instead of the single-host check.
  - Ghost re-create (`:305-307`): carry `upstream_hosts` through.
  - `fal.ai` appears automatically in `ProviderTypeList.vue` once the catalog returns `right-fal` (Task 4) — verify it renders; no per-type code needed.

- [ ] **Step 5: Update `ProvidersView.test.ts`** (SSR) to assert the Header-name field is gone and the hosts list + microcopy render; assert `fal.ai` appears in the type list when the catalog includes it. Run:

Run: `cd crates/right-dashboard/frontend && pnpm test 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add crates/right-dashboard/frontend/src/views
git commit -m "feat(dashboard): env-var-centric multi-host generic providers, fal type, drop header field"
```

---

## Task 11: Live `ci_openshell` test — substitution delivers client-written scheme

**Files:**
- Modify/Create: `crates/right-openshell/tests/ci_openshell_provider.rs`

- [ ] **Step 1: Study the existing harness.** Read `crates/right-openshell/tests/ci_openshell_provider.rs` end-to-end — `TestSandbox`, `poll_sandbox_env`, how it creates throwaway providers with fake creds and observes effective policy. Mirror it; do not invent infra.

- [ ] **Step 2: Write the live test** (TDD: write it red against current main behavior expectations). Assert: a generic provider with `env_var=RIGHT_TEST_KEY` and a terminated host composes; the placeholder env var is injected; and (if a host-controlled echo endpoint is feasible per the existing harness) the upstream receives `Authorization: Key <real-secret>` when the agent runs `curl -H "Authorization: Key $RIGHT_TEST_KEY"`. Multi-host: assert all declared hosts appear in the effective policy via `provider_is_composed_with_all_endpoints`.

```rust
#[tokio::test]
#[ignore = "ci-openshell: live sandbox + provider composition"]
async fn ci_openshell_generic_multi_host_composes_all_and_substitutes_client_scheme() {
    // ... build on TestSandbox + the existing provider-create/attach helpers ...
    // 1. create generic provider, env_var RIGHT_TEST_KEY, hosts [H1, H2]
    // 2. assert provider_is_composed_with_all_endpoints(effective_policy, name, [(H1,""),(H2,"")])
    // 3. assert poll_sandbox_env shows RIGHT_TEST_KEY placeholder injected
    // 4. (if echo endpoint feasible) curl H1 with "Authorization: Key $RIGHT_TEST_KEY";
    //    assert upstream saw "Authorization: Key <real-secret>"
}
```

If step 4's echo endpoint is infeasible under TLS termination, drop it and keep 1-3 (composition + injection) — still proves multi-host composition; note the limitation in a code comment.

- [ ] **Step 3: Run the live test explicitly** (dev machine has OpenShell):

Run: `devenv shell -- cargo test -p right-openshell ci_openshell_generic_multi_host -- --ignored --nocapture 2>&1 | tail -40`
Expected: PASS. If the root-finding assertion (step 4) fails — STOP and revisit the spec's "header_name is inert" decision before proceeding.

- [ ] **Step 4: Commit.**

```bash
git add crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "test(openshell): live multi-host composition + client-scheme substitution"
```

---

## Task 12: Docs (cite-on-touch) + final verification

**Files:**
- Modify: `docs/architecture/providers.md` — note multi-host generic providers + that `auth_style`/`header_name` are inert for static keys (the agent writes the scheme).
- Modify: `PROMPT_SYSTEM.md` — only if the agent-facing usage-hint text (Task 6) changed what agents see; reflect the "write the auth header yourself" guidance.
- Check: `ARCHITECTURE.md` Providers section — update only if a contract/invariant changed (the all-hosts composition-confirmation rule is a new invariant worth one line; respect the 40k budget — if over, trim elsewhere or put detail in `providers.md`).
- Check: `with_instructions()` in `memory_server.rs`/`aggregator.rs` — **no change** (no MCP tool added/renamed); confirm and move on.

- [ ] **Step 1: Update `docs/architecture/providers.md`** — add a short subsection: generic providers support multiple hosts (one composed endpoint each, confirmed across all hosts); reiterate the static-cred verbatim-substitution model and that the built-in `Fal` profile covers fal's authenticated hosts only.

- [ ] **Step 2: Update `PROMPT_SYSTEM.md`** if Task 6 altered agent-visible text; otherwise note "no change" in the commit body.

- [ ] **Step 3: Update `ARCHITECTURE.md`** Providers section with the one-line all-hosts-composition invariant (only if it fits the budget).

- [ ] **Step 4: Final full workspace test (mandatory).**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -30`
Expected: green. Re-run any flaky-under-load failures isolated (see memory) before concluding.

- [ ] **Step 5: Final debug build.**

Run: `devenv shell -- cargo build --workspace 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Commit.**

```bash
git add docs/architecture/providers.md PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: multi-host generic providers + static-cred injection model + fal"
```

---

## Self-Review notes (resolved)

- **Spec coverage:** §1→T1; §2→T2/T3/T4 (+research gate T3.0); §3→T6; §4→T10; §5→T9; §6→T8 (+all-hosts confirm); §7 upgrade→back-compat in T1/T8 (no recreation); §8 testing→per-task + T11 live + T12 final; Risks→T11 stop-gate + T3.0.
- **Type consistency:** `GenericProvider { env_var, upstream_hosts: Vec<String>, upstream_path_prefix }`; `author_generic_profile(id, &[String] hosts, Option<&str> prefix, env_var)`; `GenericProviderProfileInput { name, upstream_hosts: &[String], upstream_path_prefix, env_var }`; `provider_is_composed_with_all_endpoints(policy, name, &[(String,String)])`; `validateUpstreamHosts(string[])`. Used consistently across tasks.
- **Order:** model → authoring → fal/catalog → composition → hint → callers → API → CLI → dashboard → live → docs. Each task compiles its own crate and commits.
