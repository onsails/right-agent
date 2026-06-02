# GitHub full-access provider — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the two-type `github` (read) + `right-github-write` model with a
single full-access **`right-github`** provider; hide OpenShell's read-only
built-in `github` from the dashboard; delete the grouping/toggle/resolver
scaffolding. Prove `git push` works live.

**Architecture:** `right-github` is a managed OpenShell provider profile derived
from the built-in `github`, with every endpoint set to `access: full`,
provisioned idempotently on `right up` (gated on `any_sandboxed`). The dashboard
offers one "GitHub" type; the built-in `github` stays in the catalog (so existing
providers still resolve their env var) but is filtered out of the add-provider
list. No migration (pre-prod).

**Tech Stack:** Rust 2024 (`right-openshell`, `right`, `bot`), tonic/prost gRPC,
Vue 3 + TypeScript dashboard (vitest + `@vue/server-renderer` SSR), live
OpenShell CI tests (`#[ignore = "ci-openshell: …"]`, `ci_openshell_` prefix).

**Context:** This reworks already-committed work on branch `feat/github-write`.
Read `docs/superpowers/specs/2026-06-02-github-full-access-provider-design.md`
and issue `onsails/right-agent#92` first. The current branch state has
`ManagedProfile::GithubWrite`, a `right-github-write` profile, a `group` field on
provider catalog entries, dashboard grouping (`providersGrouping.ts` + grouped
card), and `ci_openshell_github_write.rs`. All of that is replaced/removed below.

---

## File Structure

**Modify**
- `crates/right-openshell/src/managed_profiles.rs` — rename `GithubWrite`→`Github`,
  derive `access: full` (drop `allow_all`/rules), add `EnsureOutcome::Skipped`,
  make base-missing non-fatal.
- `crates/right-openshell/src/providers.rs` — drop `group` field; replace the
  `right-github-write` catalog entry with `right-github`; keep `github`.
- `crates/right-openshell/Cargo.toml` — rename the `[[test]]` entry.
- `crates/right/src/internal_api_providers.rs` — drop `group` from the DTO;
  filter hidden built-ins (`github`) out of `handle_provider_types`.
- `crates/right/src/main.rs` — **no edit needed**: the rename propagates through
  `managed_profiles()`, and after Task 2 base-missing no longer aborts `right up`.
  Optionally soften the inline `// FAIL FAST on error` comment (real gRPC/lint
  errors still propagate; base-missing now skips).
- `crates/right-dashboard/frontend/src/types.ts` — drop `group`.
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue` — revert to the
  flat provider-type list; keep the `Integrations` eyebrow.
- `docs/architecture/providers.md` — update the "Managed profiles" section.

**Rename**
- `crates/right-openshell/tests/ci_openshell_github_write.rs` →
  `crates/right-openshell/tests/ci_openshell_github.rs`.

**Delete**
- `crates/right-dashboard/frontend/src/views/providersGrouping.ts`
- `crates/right-dashboard/frontend/src/views/providersGrouping.test.ts`
- `crates/right-dashboard/frontend/src/views/ProvidersView.grouping.test.ts`

**Baseline (Task 0):** `devenv shell -- cargo build -p right-openshell` and
`cd crates/right-dashboard/frontend && npm run build` both succeed on the current
branch before starting. Record any pre-existing failure.

---

### Task 1: Profile `right-github` with `access: full`

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs`

- [ ] **Step 1: Update the unit tests first (red).** Replace the
  `derive_github_write_opens_all_methods_and_renames` test and adjust
  `needs_import_*`:

```rust
    #[test]
    fn derive_github_sets_full_access_and_renames() {
        let derived = github().derive(base_github());
        assert_eq!(derived.id, "right-github");
        assert_eq!(derived.display_name, "GitHub");
        assert_eq!(derived.category, 4, "category preserved from base");
        assert!(!derived.endpoints.is_empty());
        for ep in &derived.endpoints {
            assert_eq!(ep.access, "full", "every endpoint opened to full access");
            assert!(ep.rules.is_empty(), "rules cleared (exclusive with access)");
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

    #[test]
    fn needs_import_true_when_access_differs() {
        let desired = github().derive(base_github());
        let stored_same = desired.clone();
        let stored_old = base_github(); // still access: read-only

        assert!(!needs_import(Some(&stored_same), &desired), "identical → no import");
        assert!(needs_import(Some(&stored_old), &desired), "access drift → import");
        assert!(needs_import(None, &desired), "absent → import");
    }
```

- [ ] **Step 2: Run the tests; expect compile failure** (`github`, `Github`,
  `right-github` not defined yet). Run:
  `devenv shell -- cargo test -p right-openshell --lib managed_profiles`

- [ ] **Step 3: Rename the enum + helpers and rewrite `derive`.** In
  `managed_profiles.rs`:
  - Rename `ManagedProfile::GithubWrite` → `ManagedProfile::Github` (enum variant,
    and every match arm).
  - `id()` → `"right-github"`; `base_id()` stays `Some("github")`.
  - Rename `pub fn github_write()` → `pub fn github()` returning
    `ManagedProfile::Github`.
  - `managed_profiles()` returns `vec![ManagedProfile::Github]`.
  - Rewrite `derive`:

```rust
    pub fn derive(&self, mut base: proto_v1::ProviderProfile) -> proto_v1::ProviderProfile {
        match self {
            ManagedProfile::Github => {
                base.id = self.id().into();
                base.display_name = "GitHub".into();
                for ep in &mut base.endpoints {
                    // Full access: permit every HTTP method on each github host.
                    // read-only blocks git push (POST git-receive-pack). `access`
                    // and `rules` are mutually exclusive — clear rules, set preset.
                    ep.rules.clear();
                    ep.access = "full".into();
                }
                base
            }
        }
    }
```
  - **Delete** the now-unused `allow_all` function and the `use ... sandbox_v1`
    import if it becomes unused (the compiler will flag it).

- [ ] **Step 4: Run the tests; expect pass.** Run:
  `devenv shell -- cargo test -p right-openshell --lib managed_profiles`
  Expected: the three tests above pass.

- [ ] **Step 5: Commit.**
```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): right-github profile sets access: full (was right-github-write)"
```

---

### Task 2: Base-missing is non-fatal (`EnsureOutcome::Skipped`)

**Files:**
- Modify: `crates/right-openshell/src/managed_profiles.rs`

- [ ] **Step 1: Add the test (red).** A managed profile whose base is absent is
  skipped, not an error:

```rust
    #[test]
    fn ensure_outcome_skipped_variant_exists() {
        // Compile-time guard that the non-fatal Skipped outcome is available.
        let s = EnsureOutcome::Skipped("right-github".into());
        assert!(matches!(s, EnsureOutcome::Skipped(_)));
    }
```
  (The live behaviour is covered by the `ci_openshell_` tests in Task 6; this
  unit test just locks the variant.)

- [ ] **Step 2: Run; expect compile failure** (`Skipped` not defined).

- [ ] **Step 3: Implement.** In `managed_profiles.rs`:
  - Add to `EnsureOutcome`: `Skipped(String)`.
  - In `ensure_profiles`, replace the base-fetch that returns `BaseMissing` with
    a warn-and-skip:

```rust
        let desired = match mp.base_id() {
            Some(base_id) => match get_profile(client, base_id).await? {
                Some(base) => mp.derive(base),
                None => {
                    tracing::warn!(
                        profile = mp.id(),
                        base = base_id,
                        "base profile missing on gateway — skipping managed profile"
                    );
                    outcomes.push(EnsureOutcome::Skipped(mp.id().to_string()));
                    continue;
                }
            },
            None => unreachable!("authored profiles not shipped"),
        };
```
  - Remove the now-unused `ManagedProfileError::BaseMissing` variant (the
    compiler will flag it as unused).

- [ ] **Step 4: Run.** `devenv shell -- cargo test -p right-openshell --lib managed_profiles`
  Expected: pass.

- [ ] **Step 5: Commit.**
```bash
git add crates/right-openshell/src/managed_profiles.rs
git commit -m "feat(providers): missing base profile skips managed profile (non-fatal)"
```

---

### Task 3: Catalog — `right-github`, drop `group`, keep `github`

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`

- [ ] **Step 1: Update tests first (red).** Replace
  `catalog_has_github_read_and_write_grouped` with:

```rust
    #[test]
    fn catalog_has_full_github_and_keeps_builtin() {
        let catalog = profile_catalog();
        let upstream_builtin = catalog
            .iter()
            .filter(|p| p.type_slug != "generic" && !p.type_slug.starts_with("right-"))
            .count();
        assert_eq!(upstream_builtin, 8, "8 upstream built-ins unchanged");

        let gh = catalog.iter().find(|p| p.type_slug == "github").expect("github kept");
        let rgh = catalog
            .iter()
            .find(|p| p.type_slug == "right-github")
            .expect("right-github present");
        assert_eq!(gh.env_var, "GITHUB_TOKEN");
        assert_eq!(rgh.env_var, "GITHUB_TOKEN");
        assert_eq!(rgh.display_name, "GitHub");
        assert!(
            catalog.iter().all(|p| p.type_slug != "right-github-write"),
            "old right-github-write removed"
        );
    }
```

- [ ] **Step 2: Run; expect failure.**
  `devenv shell -- cargo test -p right-openshell --lib providers`

- [ ] **Step 3: Implement.** In `providers.rs`:
  - Remove the `pub group: String,` field (and its doc comment) from
    `struct ProviderProfile`.
  - In `profile_catalog()`, delete every `group: "...".into(),` line.
  - Replace the `right-github-write` entry with:

```rust
        ProviderProfile {
            type_slug: "right-github".into(),
            display_name: "GitHub".into(),
            category: ProviderCategory::SourceControl,
            env_var: "GITHUB_TOKEN".into(),
        },
```
  - Leave the existing `github` entry unchanged.

- [ ] **Step 4: Run.** `devenv shell -- cargo test -p right-openshell --lib providers`
  Expected: `catalog_has_full_github_and_keeps_builtin` and
  `catalog_has_8_built_in_plus_generic` pass.

- [ ] **Step 5: Commit.**
```bash
git add crates/right-openshell/src/providers.rs
git commit -m "feat(providers): catalog ships right-github, drops group field"
```

---

### Task 4: Dashboard API — drop `group`, hide built-in `github`

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs`

- [ ] **Step 1: Add a test (red).** `handle_provider_types` must omit `github`
  and must not carry a `group`:

```rust
    #[tokio::test]
    async fn provider_types_hides_builtin_github_and_shows_right_github() {
        let axum::Json(types) = handle_provider_types().await;
        assert!(
            types.iter().all(|t| t.type_ != "github"),
            "built-in read-only github is hidden from the dashboard"
        );
        assert!(
            types.iter().any(|t| t.type_ == "right-github" && t.display_name == "GitHub"),
            "right-github offered as GitHub"
        );
    }
```
  (Field name is `type_` per the existing DTO; adjust if the struct uses a serde
  rename — check `ProviderProfileView`.)

- [ ] **Step 2: Run; expect failure.**
  `devenv shell -- cargo test -p right -- internal_api_providers::`

- [ ] **Step 3: Implement.** In `internal_api_providers.rs`:
  - Remove `pub group: String,` from `struct ProviderProfileView`.
  - In `handle_provider_types`, drop the `group: p.group,` mapping line and add a
    filter for hidden built-ins:

```rust
    /// Built-in profile slugs kept in the catalog (so existing providers resolve)
    /// but not offered as new provider types — superseded by a right-* variant.
    const HIDDEN_FROM_DASHBOARD: &[&str] = &["github"];

pub(crate) async fn handle_provider_types() -> axum::Json<Vec<ProviderProfileView>> {
    let views = right_openshell::providers::profile_catalog()
        .into_iter()
        .filter(|p| !HIDDEN_FROM_DASHBOARD.contains(&p.type_slug.as_str()))
        .map(|p| ProviderProfileView {
            type_: p.type_slug,
            env_var: p.env_var,
            display_name: p.display_name,
            category: format!("{:?}", p.category).to_lowercase(),
        })
        .collect();
    axum::Json(views)
}
```
  (Match the existing field names in the `.map(...)` — the snippet above shows the
  shape; keep whatever the current struct uses minus `group`.)

- [ ] **Step 4: Run.** `devenv shell -- cargo test -p right -- internal_api_providers::`
  Expected: pass.

- [ ] **Step 5: Commit.**
```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(dashboard-api): hide built-in github, drop provider-type group"
```

---

### Task 5: Frontend — revert to a flat provider-type list

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`
- Delete: `providersGrouping.ts`, `providersGrouping.test.ts`,
  `ProvidersView.grouping.test.ts` (all under `.../src/views/`)

- [ ] **Step 1: Delete the grouping files.**
```bash
cd crates/right-dashboard/frontend
git rm src/views/providersGrouping.ts src/views/providersGrouping.test.ts src/views/ProvidersView.grouping.test.ts
```

- [ ] **Step 2: `types.ts` — remove the `group` field** from
  `interface ProviderProfileView` (delete the `group: string` line).

- [ ] **Step 3: `ProvidersView.vue` — revert the type chooser.** Remove
  `import { groupProviderTypes } from './providersGrouping'` and the
  `const typeGroups = computed(...)` line. Replace the grouped `<article v-for="g
  in typeGroups">` block with the flat card (keep the `Integrations` eyebrow that
  the branch added):

```html
        <article
          v-for="t in types"
          :key="t.type"
          class="type-card"
          @click="selectType(t)"
        >
          <strong>{{ t.display_name }}</strong>
          <small>{{ t.category }}</small>
          <small>{{ t.env_var }}</small>
        </article>
        <p v-if="types.length === 0" class="muted-line">No provider types available</p>
```
  In `<style>`, restore the original `.type-card` rules and remove the
  `.access-variants` / `.access-variant` rules:

```css
.type-card {
  display: grid;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  cursor: pointer;
}

.type-card:hover {
  border-color: var(--tg-theme-button_color, #2481cc);
}

.type-card strong {
  font-size: 0.84rem;
}

.type-card small {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
}
```

- [ ] **Step 4: Run frontend tests + build.**
```bash
npx vitest run
npm run build
```
  Expected: green (the deleted grouping tests no longer run; no import errors).

- [ ] **Step 5: Commit.**
```bash
cd ../../..   # back to worktree root
git add crates/right-dashboard/frontend/src/types.ts \
        crates/right-dashboard/frontend/src/views/ProvidersView.vue
git commit -m "feat(dashboard): flat provider-type list; drop grouping scaffolding"
```

---

### Task 6: Live tests — rename, full-access POST de-risk, push gate

**Files:**
- Rename: `crates/right-openshell/tests/ci_openshell_github_write.rs` →
  `crates/right-openshell/tests/ci_openshell_github.rs`
- Modify: `crates/right-openshell/Cargo.toml`

- [ ] **Step 1: Rename file + `[[test]]` entry.**
```bash
git mv crates/right-openshell/tests/ci_openshell_github_write.rs \
       crates/right-openshell/tests/ci_openshell_github.rs
```
  In `crates/right-openshell/Cargo.toml`, change the `[[test]]` block name from
  `ci_openshell_github_write` to `ci_openshell_github`.

- [ ] **Step 2: Update the existing tests for the new names.** In
  `ci_openshell_github.rs`: replace `github_write` → `github`,
  `right-github-write` → `right-github`, and `EnsureOutcome::Imported(...)`
  assertions accordingly. In the idempotency test, assert each stored endpoint
  has `ep.access == "full"` and `ep.rules.is_empty()` (was: access empty + one
  rule). Rename test fns `ci_openshell_github_write_*` → `ci_openshell_github_*`.

- [ ] **Step 3 (OPTIONAL de-risk — skip if finicky):** a full-access POST test
  that needs no real credential. The authoritative proof is the push gate
  (Step 7); this is only a cheaper early signal. Add to `ci_openshell_github.rs`,
  mirroring the helper calls in the push test below:
  1. Author a throwaway profile and import it:
```rust
let pid = std::process::id();
let id = format!("right-cftest-{pid}");
let profile = proto_v1::ProviderProfile {
    id: id.clone(),
    display_name: "cftest".into(),
    description: "full-access POST de-risk".into(),
    category: 0,
    credentials: vec![],
    endpoints: vec![sandbox_v1::NetworkEndpoint {
        host: "httpbin.org".into(), port: 443, protocol: "rest".into(),
        access: "full".into(), enforcement: "enforce".into(), rules: vec![],
        ..Default::default()
    }],
    binaries: vec![], inference_capable: false, discovery: None,
};
lint_and_import(&mut client, profile).await.expect("import");
```
  2. `create_provider` of `type_: id` with a fake credential, `attach_to_sandbox`
     to a `TestSandbox::create("cftest-fullpost")` (the profile contributes the
     httpbin L7 endpoint on attach — Path A).
  3. exec and assert it is **not** blocked:
```rust
let (out, _) = sandbox.exec_with_timeout(&["sh","-lc",
    "curl -s -o /dev/null -w '%{http_code}' -X POST https://httpbin.org/post -H 'Authorization: Bearer x' --max-time 30"], 60).await;
assert_eq!(out.trim(), "200", "access: full must permit POST (read-only would 403): {out}");
```
  4. Cleanup: `detach_from_sandbox`, `delete_provider`, `delete_profile`. Mark
     `#[tokio::test] #[ignore = "ci-openshell: full-access permits POST on a terminated endpoint"]`,
     fn `ci_openshell_full_access_allows_post`.
  If the sandbox networking proves finicky for a bare `TestSandbox` (DNS/CONNECT
  quirks were seen during the confinement experiment), **drop this test** and
  rely on the push gate — do not thrash on it.

- [ ] **Step 4: Keep the real-token push gate**, renamed
  `ci_openshell_github_push_succeeds` (env-gated on `RIGHT_TEST_GH_TOKEN` +
  `RIGHT_TEST_GH_PUSH_REPO`). It already: ensures `right-github` → creates a
  provider with the token → attaches to a `TestSandbox` (raw-tunnel base) →
  `git push` a throwaway branch → asserts `PUSH_OK` (not a 403) → deletes the
  branch, redacting the token from output. Verify the redaction assertions
  (`!out.contains("x-access-token:ghp_")` / `gho_`) are intact.

- [ ] **Step 5: Compile the ignored tests.**
  `devenv shell -- cargo test -p right-openshell --no-run --features test-support`
  Expected: compiles. (The `#[ignore]` tests are not run here.)

- [ ] **Step 6: Commit.**
```bash
git add crates/right-openshell/tests/ci_openshell_github.rs crates/right-openshell/Cargo.toml
git commit -m "test(providers): ci_openshell_github — full-access POST de-risk + push gate"
```

- [ ] **Step 7 (operator-run, the load-bearing gate):** with a throwaway GitHub
  token that can push to a throwaway repo, run:
```bash
RIGHT_TEST_GH_TOKEN=<throwaway PAT> RIGHT_TEST_GH_PUSH_REPO=<owner/throwaway-repo> \
  devenv shell -- cargo test -p right-openshell --features test-support \
  ci_openshell_github_push_succeeds -- --ignored --nocapture
```
  Expected: `PUSH_OK`. **If it fails with a 403, stop — `access: full` is not the
  lever.** Re-open the design per the spec's contingency (raw-tunnel `github.com`
  / `codeload.github.com` / L7-ordering) before continuing.

---

### Task 7: Docs — update the managed-profiles section

**Files:**
- Modify: `docs/architecture/providers.md`

- [ ] **Step 1: Rewrite the "Managed profiles (RightClaw-owned)" section** to
  describe `right-github` (derived from `github`, all endpoints `access: full`,
  the single GitHub provider; the read-only built-in is hidden from the
  dashboard; base-missing is a non-fatal skip; no read/write toggle). Remove
  references to `right-github-write`, the read/write distinction, and grouping.
  Keep the cross-reference to `onsails/right-agent#92` for the credential-scoping
  limitation.

- [ ] **Step 2: Commit.**
```bash
git add docs/architecture/providers.md
git commit -m "docs(arch): managed profiles — right-github full-access"
```

---

### Task 8: Final verification

- [ ] **Step 1: Full workspace test (mandatory, from the worktree).**
  `devenv shell -- cargo test --workspace`
  Expected: green. The two known flaky tests
  (`dashboard_overview_logs_malformed_curator_evidence`, the cc/invocation pid
  race) may flake under parallel load — re-run any failure isolated
  (`-p <crate> <name> -- --test-threads=1`) before blaming the change.

- [ ] **Step 2: Frontend build.**
  `cd crates/right-dashboard/frontend && npm run build` → clean.

- [ ] **Step 3: Confirm the ignored-test contract.**
  `devenv shell -- cargo test -p right ci_ignored_contract` → green (the renamed
  `ci_openshell_github_*` tests keep the `ci-openshell:` reason + `ci_openshell_`
  prefix).

---

## Notes for the implementer

- **DRY/YAGNI:** do not reintroduce a `group`/`hidden` struct field on
  `ProviderProfile`; hiding is a one-line const filter in the dashboard API.
- **No read/write toggle, no collision resolver** — out of scope (spec
  Non-goals). The pre-existing env-var collision guard is unchanged.
- **Credential confinement (T2)** is unchanged and out of scope (#92).
- **Verification cadence:** targeted package tests per task (above); one
  `cargo test --workspace` at the end (Task 8). Do not run the full workspace
  suite after every task. The real-token push gate (Task 6 Step 7) is operator-run.
