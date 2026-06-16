# Cross-agent Provider Sharing (multi-attach) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an operator SHARE one provider record across multiple agents by attaching the same gateway record to each agent's sandbox (no credential copy/read-back), and retire the broken copy-by-readback flow.

**Architecture:** Provider records become agent-agnostic (`{type-slug}-{short-uuid}`); ownership/borrowing is recorded in `agent.yaml` (`shared_from`), not the name. Reconcile and the destroy cascade switch from name-prefix heuristics to `agent.yaml` as the sole source of truth, with **refcount** deletion (a gateway record is deleted only when no agent references it; owner-deletion re-homes ownership to a surviving borrower). The secret never leaves the gateway.

**Tech Stack:** Rust (edition 2024), tonic gRPC (`right-openshell`), axum internal API (`right`), `right-agent-config` (serde/yaml), Vue 3 SSR dashboard (`right-dashboard`).

**Design spec:** `docs/superpowers/specs/2026-06-17-cross-agent-provider-share-design.md` (read first).

**Pre-existing environmental failure (not caused by this work):** workspace `cli_integration`/`wizard_brand`/`home_isolation` tests fail on a leftover cloudflared tunnel ("tunnel with name already exists"). Exclude those binaries during intermediate runs: `-E 'not (binary(cli_integration) | binary(wizard_brand) | binary(home_isolation))'`.

---

## File Structure

- `crates/right-agent-config/src/lib.rs` — `ProviderEntry` gains `shared_from: Option<String>`; helpers `is_borrowed()` / `is_owned()`.
- `crates/right/src/internal_api_providers.rs` — agent-agnostic name generator + relaxed `validate_name`; new `provider_share` / `provider_unshare` handlers; retire copy handlers.
- `crates/right-openshell/src/providers.rs` — `reconcile_for_sandbox` detach switches from prefix to declared-list; borrowed-aware attach.
- `crates/bot/src/sandbox_supervisor.rs` — `hot_reconcile_providers` treats `shared_from` entries as attach-only.
- `crates/right-agent/src/agent/destroy.rs` — refcount cascade + re-home.
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue` — Share/Unshare UI; borrowed read-only label.
- `crates/bot/src/telegram/dashboard/providers.rs` — dashboard routes for share/unshare; remove import/export-by-readback.
- `ARCHITECTURE.md` + `docs/architecture/providers.md` + `PROMPT_SYSTEM.md` — doc updates.
- New live test: `crates/right-openshell/tests/ci_openshell_provider_borrowed_reconcile.rs`.

---

## Task 0: Redaction guard on copy — DONE (this branch, commit `8d2412ee`)

Already implemented and verified this session; part of this plan, ships with it
(not a separate PR).

**Files:** `crates/right/src/internal_api_providers.rs` (guard `check_source_credential_readable` + `REDACTION_SENTINEL` + `ProviderApiError::SourceCredentialUnreadable` (422) + 3 unit tests); live canary `crates/right-openshell/tests/ci_openshell_get_provider_redacts.rs`.

- [x] **Guard `handle_provider_copy`:** detect the `"REDACTED"` (or empty) read-back and fail fast with an actionable error before either copy branch writes, instead of silently writing a broken credential. Forward-compatible (a non-redacting gateway's real value still passes).
- [x] **Live canary** proves `GetProvider` returns `"REDACTED"`.
- [x] **Verified:** `cargo nextest run --workspace` (excl. cloudflared-env binaries) green; rust-dev review approved.

**Retained by Task 8** as defense-in-depth: even after copy-by-readback is removed, the guard protects any future host-side read-back caller. Task 8 must NOT delete `check_source_credential_readable` or its tests.

---

## Task 1: `agent.yaml` schema — `shared_from`

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs:349-357`
- Test: same file `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

In the existing test module of `crates/right-agent-config/src/lib.rs`:

```rust
#[test]
fn provider_entry_shared_from_round_trips_and_defaults_absent() {
    // Absent shared_from = owned (backward compatible).
    let owned: ProviderEntry =
        serde_yaml::from_str("name: fal-a1b2c3\ntype: !BuiltIn right-fal\n").unwrap();
    assert!(owned.shared_from.is_none());
    assert!(owned.is_owned() && !owned.is_borrowed());

    // Present shared_from = borrowed.
    let borrowed: ProviderEntry = serde_yaml::from_str(
        "name: fal-a1b2c3\ntype: !BuiltIn right-fal\nshared_from: agent-a\n",
    )
    .unwrap();
    assert_eq!(borrowed.shared_from.as_deref(), Some("agent-a"));
    assert!(borrowed.is_borrowed() && !borrowed.is_owned());

    // Serialization omits shared_from when None.
    let s = serde_yaml::to_string(&owned).unwrap();
    assert!(!s.contains("shared_from"), "owned entry must not emit shared_from; got: {s}");
}
```

- [ ] **Step 2: Run it — expect FAIL** (no `shared_from` field / no `is_owned`/`is_borrowed`)

Run: `devenv shell -- cargo nextest run -p right-agent-config provider_entry_shared_from`
Expected: FAIL (compile error: unknown field `shared_from`).

- [ ] **Step 3: Add the field + helpers**

In `crates/right-agent-config/src/lib.rs`, extend `ProviderEntry`:

```rust
pub struct ProviderEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<GenericProvider>,
    /// Present ⇒ this entry is a *borrowed* reference to a record owned by the
    /// named agent. Absent ⇒ this agent owns the record. Owned vs borrowed
    /// drives rotation rights, UI read-only state, and the destroy cascade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_from: Option<String>,
}

impl ProviderEntry {
    pub fn is_borrowed(&self) -> bool {
        self.shared_from.is_some()
    }
    pub fn is_owned(&self) -> bool {
        self.shared_from.is_none()
    }
}
```

- [ ] **Step 4: Run it — expect PASS**

Run: `devenv shell -- cargo nextest run -p right-agent-config provider_entry_shared_from`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent-config/src/lib.rs
git commit -m "feat(agent-config): add shared_from to ProviderEntry for borrowed records"
```

---

## Task 2: Agent-agnostic record names + relaxed `validate_name`

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs:99-140` (`validate_name`)
- Modify: `crates/right/src/internal_api_providers.rs` (add `new_record_name`)
- Test: same file `plan_copy_tests` or a new `naming_tests` module

**Context:** New records drop the `{agent}-` prefix requirement; legacy `{agent}-{slug}` must still validate. The name is `{type-slug}-{6 hex}`. Use the existing dependency for randomness (check `uuid` is already a workspace dep with `python3 scripts/check_crate_version.py uuid`; if absent, derive 6 hex chars from `uuid::Uuid::new_v4()` simple form). The slug is sanitized to `[a-z0-9-]`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn validate_name_accepts_legacy_agent_prefixed() {
    validate_name("agent-a", "agent-a-provider").expect("legacy {agent}-{slug} must validate");
}

#[test]
fn validate_name_accepts_agent_agnostic_uuid_form() {
    // No agent prefix required for the new form.
    validate_name("agent-a", "fal-a1b2c3").expect("agent-agnostic name must validate");
}

#[test]
fn new_record_name_has_type_slug_and_hex_suffix() {
    let n = new_record_name("right-fal");
    assert!(n.starts_with("fal-"), "got {n}");      // built-in slug normalized to fal
    let suffix = n.rsplit('-').next().unwrap();
    assert_eq!(suffix.len(), 6);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "got {n}");
}
```

- [ ] **Step 2: Run — expect FAIL** (`new_record_name` missing; `validate_name` rejects non-prefixed)

Run: `devenv shell -- cargo nextest run -p right -E 'test(validate_name) | test(new_record_name)'`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace the prefix-enforcing head of `validate_name` so the new form is accepted while legacy still passes:

```rust
pub fn validate_name(agent: &str, name: &str) -> Result<(), ProviderApiError> {
    // Accept either the legacy "{agent}-{slug}" form or the new agent-agnostic
    // "{type-slug}-{uuid}" form. The slug body is validated the same way.
    let slug = name
        .strip_prefix(&format!("{agent}-"))
        .unwrap_or(name);
    if slug.is_empty() || slug.len() > 40 {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: "1-40 chars after optional agent prefix".into(),
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || !name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
    {
        return Err(ProviderApiError::InvalidName {
            name: name.into(),
            reason: "lowercase a-z/0-9/'-', must start a-z".into(),
        });
    }
    Ok(())
}

/// Agent-agnostic record id: `{type-slug}-{6 hex}`. `type_slug` is the gateway
/// type (built-in slug like `right-fal` → `fal`; generic profile id → `generic`).
fn new_record_name(type_slug: &str) -> String {
    let base = type_slug.strip_prefix("right-").unwrap_or(type_slug);
    let base = if base.is_empty() || base.starts_with("generic") { "generic" } else { base };
    let hex: String = uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect();
    format!("{base}-{hex}")
}
```

> If `validate_name`'s existing tests assert the old prefix rule, update them to the relaxed contract (legacy still valid). Search: `rg -n 'validate_name' crates/right/src/internal_api_providers.rs`.

- [ ] **Step 4: Run — expect PASS** (and the full file's existing name tests)

Run: `devenv shell -- cargo nextest run -p right internal_api_providers`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(providers): agent-agnostic record names; relax validate_name (legacy still valid)"
```

---

## Task 3: Reconcile ownership = declared list, not name prefix

**Files:**
- Modify: `crates/right-openshell/src/providers.rs:778-787` (detach loop in `reconcile_for_sandbox`)
- Test: `crates/right-openshell/src/providers_tests.rs`

**Context:** Today detach is `name.starts_with("{agent_prefix}-") && !declared`. New rule: detach any attached provider not in `declared` — the agent.yaml declared list (owned + borrowed) is the complete set this sandbox should have. Keep `agent_prefix` param for signature stability but stop using it for detach; mark it `_agent_prefix` if now unused (only if YOUR change made it unused).

- [ ] **Step 1: Write the failing test**

In `providers_tests.rs` (uses the mock gRPC server already present — follow the existing reconcile test pattern; find it with `rg -n 'reconcile_for_sandbox' crates/right-openshell/src/providers_tests.rs`):

```rust
#[tokio::test]
async fn reconcile_detaches_undeclared_regardless_of_prefix() {
    // Attached: ["fal-a1b2c3"] (agent-agnostic, NOT prefixed with the agent).
    // Declared: [] → it must be detached even though it lacks the "{agent}-" prefix.
    let mut client = mock_client_with_attached(&["fal-a1b2c3"]);
    let report = reconcile_for_sandbox(&mut client, "sbox", "right", &[]).await.unwrap();
    assert_eq!(report.detached, vec!["fal-a1b2c3".to_string()]);
}
```

(Adapt `mock_client_with_attached` to the existing mock helpers in `providers_tests.rs` / `test_mock_server.rs`.)

- [ ] **Step 2: Run — expect FAIL** (current code only detaches prefixed names)

Run: `devenv shell -- cargo nextest run -p right-openshell reconcile_detaches_undeclared`
Expected: FAIL (nothing detached).

- [ ] **Step 3: Implement** — replace the detach loop:

```rust
    // Detach anything attached that this agent no longer declares. agent.yaml is
    // the source of truth; name prefixes are no longer load-bearing for ownership.
    for name in &attached {
        if !declared_set.contains(name) {
            match detach_from_sandbox(client, sandbox_name, name).await {
                Ok(()) => report.detached.push(name.clone()),
                Err(e) => report.errors.push((name.clone(), format!("detach: {e:#}"))),
            }
        }
    }
```

- [ ] **Step 4: Run — expect PASS** (plus the existing reconcile tests)

Run: `devenv shell -- cargo nextest run -p right-openshell reconcile`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/providers_tests.rs
git commit -m "refactor(providers): reconcile detach keyed on declared list, not name prefix"
```

---

## Task 4: Borrowed-aware attach (profile-ensure without import) + live test

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs` (the provider reconcile that ensures managed profiles — find with `rg -n 'ensure_profiles|managed_profiles|provider_composition_expectation' crates/bot/src/sandbox_supervisor.rs`)
- Create: `crates/right-openshell/tests/ci_openshell_provider_borrowed_reconcile.rs`

**Context:** A borrowed (`shared_from`) entry must be attached and composed but never owner-managed: do not import/own its profile, do not run legacy repair. Generic borrowed providers depend on the OWNER's already-imported profile existing on the gateway (it does, since the owner created it). The supervisor's profile-ensure step must skip borrowed entries.

- [ ] **Step 1: Write the failing live test**

```rust
//! Live: a borrowed (shared) record stays attached and resolves across a
//! reconcile pass on the borrower's sandbox. #[ignore] (ci-openshell:).
// (mirror ci_openshell_provider_multi_attach.rs structure: create ONE record,
// attach to sandbox B, declare it as borrowed for agent "B", run the bot
// reconcile path, then assert it is still attached AND egress resolves the real
// secret — i.e. reconcile did not detach or re-own it.)
```

Model it on `crates/right-openshell/tests/ci_openshell_provider_multi_attach.rs`. Name the fn `ci_openshell_provider_borrowed_survives_reconcile`, ignore reason `ci-openshell: requires a live OpenShell gateway`.

- [ ] **Step 2: Run — expect FAIL** (supervisor profile-ensure re-imports/owns or reconcile detaches)

Run: `devenv shell -- cargo nextest run -p right-openshell --features test-support --test ci_openshell_provider_borrowed_reconcile --run-ignored all --no-capture`
Expected: FAIL.

- [ ] **Step 3: Implement** — in `sandbox_supervisor.rs`, when building the profile-ensure / repair set, skip entries where `entry.is_borrowed()`. Pass only owned entries to `managed_profiles::ensure_*`; pass ALL declared names (owned + borrowed) to `reconcile_for_sandbox` so both attach. (Borrowed entries still get composed via the normal `wait_for_provider_entry_composed`.)

- [ ] **Step 4: Run — expect PASS**

Run: same as Step 2.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs crates/right-openshell/tests/ci_openshell_provider_borrowed_reconcile.rs
git commit -m "feat(providers): borrowed entries are attach-only (no profile import/repair)"
```

---

## Task 5: Internal API — `provider_share` / `provider_unshare`

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` (handlers + request structs)
- Modify: `crates/right/src/internal_api.rs` (route registration — mirror `handle_provider_copy` route at the existing `post(...)` lines)
- Test: `crates/right/src/internal_api_providers.rs` `#[cfg(test)]`

**Context:** `provider_share { actor_user_id, owner_agent, provider, dest_agent }`: require actor trusted in BOTH agents (reuse `require_trusted`); reject `owner_agent == dest_agent`; attach the owner's record to dest's sandbox; append a borrowed `ProviderEntry { name: provider, type_: <owner's type>, shared_from: Some(owner_agent), .. }` to dest's `agent.yaml` (reuse `append_provider_to_yaml`); apply policy + `wait_for_provider_composed`. `provider_unshare { actor_user_id, borrower_agent, provider }`: require the entry is `is_borrowed()` (reject unshare of an owned record with a clear error); detach + remove the entry from borrower's `agent.yaml`.

- [ ] **Step 1: Write failing unit tests** for the pure planning/guard logic (trust + self-share + owned-vs-borrowed). Extract a pure helper `plan_share(owner, dest, dest_providers) -> Result<(), ProviderApiError>` and `plan_unshare(entry) -> Result<(), ProviderApiError>` and test those (the gateway calls are integration-tested via the live test in Task 4 pattern):

```rust
#[test]
fn plan_share_rejects_self() {
    let e = plan_share("right", "right", &[]).unwrap_err();
    assert!(matches!(e, ProviderApiError::CopyConflict { .. }));
}
#[test]
fn plan_unshare_rejects_owned_entry() {
    let owned = ProviderEntry { name: "fal-a1b2c3".into(), type_: ProviderType::BuiltIn("right-fal".into()), label: None, generic: None, shared_from: None };
    assert!(matches!(plan_unshare(&owned).unwrap_err(), ProviderApiError::CopyConflict { .. }));
}
#[test]
fn plan_unshare_accepts_borrowed_entry() {
    let borrowed = ProviderEntry { name: "fal-a1b2c3".into(), type_: ProviderType::BuiltIn("right-fal".into()), label: None, generic: None, shared_from: Some("agent-a".into()) };
    plan_unshare(&borrowed).expect("borrowed entry can be unshared");
}
```

- [ ] **Step 2: Run — expect FAIL** (helpers missing). `devenv shell -- cargo nextest run -p right -E 'test(plan_share) | test(plan_unshare)'`
- [ ] **Step 3: Implement** `plan_share`, `plan_unshare`, and the two axum handlers + request structs; register routes in `internal_api.rs`. Reuse `require_trusted`, `append_provider_to_yaml`, `replace_provider_in_yaml`/removal, `ensure_provider_policy_loaded`, `wait_for_provider_composed`.
- [ ] **Step 4: Run — expect PASS.** `devenv shell -- cargo nextest run -p right internal_api_providers`
- [ ] **Step 5: Commit**

```bash
git add crates/right/src/internal_api_providers.rs crates/right/src/internal_api.rs
git commit -m "feat(providers): internal provider_share/provider_unshare (multi-attach, trust both sides)"
```

---

## Task 6: Refcount deletion + re-home on destroy

**Files:**
- Modify: `crates/right-agent/src/agent/destroy.rs:288-345` (provider cascade)
- Add: a helper to count references across agents (read each `agents/<name>/agent.yaml`)
- Test: `crates/right-agent/src/agent/destroy.rs` `#[cfg(test)]`

**Context:** Replace the unconditional `delete_provider(name)` with: always `detach`; `delete_provider` only when no OTHER agent's `agent.yaml` lists `name`. If the deleted agent OWNED a record still referenced by borrowers, re-home: in one surviving borrower clear `shared_from`; repoint the rest to that new owner.

- [ ] **Step 1: Write failing tests** for the pure refcount/re-home decision (operate on in-memory `Vec<(agent, Vec<ProviderEntry>)>`, not the gateway):

```rust
#[test]
fn refcount_keeps_record_when_borrower_remains() {
    let agents = vec![
        ("agent-a".to_string(), vec![owned("fal-a1b2c3")]),
        ("right".to_string(), vec![borrowed("fal-a1b2c3", "agent-a")]),
    ];
    let plan = plan_destroy_provider_cascade("agent-a", &agents);
    assert!(plan.detach.contains(&"fal-a1b2c3".to_string()));
    assert!(!plan.delete.contains(&"fal-a1b2c3".to_string()), "still referenced by right");
    assert_eq!(plan.rehome_owner_to.get("fal-a1b2c3").map(String::as_str), Some("right"));
}

#[test]
fn refcount_deletes_record_when_last_reference() {
    let agents = vec![("agent-a".to_string(), vec![owned("fal-a1b2c3")])];
    let plan = plan_destroy_provider_cascade("agent-a", &agents);
    assert!(plan.delete.contains(&"fal-a1b2c3".to_string()));
}
```

with local `fn owned(n)`/`fn borrowed(n, from)` helpers building `ProviderEntry`.

- [ ] **Step 2: Run — expect FAIL.** `devenv shell -- cargo nextest run -p right-agent plan_destroy_provider_cascade`
- [ ] **Step 3: Implement** `plan_destroy_provider_cascade(deleting: &str, agents: &[(String, Vec<ProviderEntry>)]) -> DestroyProviderPlan` (pure), then wire it into the destroy cascade: read sibling agent.yamls, compute the plan, detach all, delete per `plan.delete`, and write re-home edits to survivors' agent.yaml (`replace_provider_in_yaml` analog). Keep best-effort/log-on-error semantics already present (but propagate hard errors per FAIL FAST where a write is required).
- [ ] **Step 4: Run — expect PASS.** `devenv shell -- cargo nextest run -p right-agent`
- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/agent/destroy.rs
git commit -m "feat(providers): refcount provider deletion + re-home owner on agent destroy"
```

---

## Task 7: Dashboard — Share/Unshare UI

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/providers.rs` (add share/unshare routes calling `internal_client.provider_share/_unshare`; remove `handle_import`/`handle_export` by-readback or repoint to share)
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`
- Test: dashboard SSR component test alongside existing provider view tests (find: `rg -rln 'ProvidersView' crates/right-dashboard/frontend/src`)

**Context:** Follow the existing dashboard primitives (ARCHITECTURE.md "Dashboard frontend primitives"): `AsyncState.vue`, `CollapsibleSection.vue`, `identityLabels.ts`, `display_name` (never raw slugs). Read `ProvidersView.vue` and the peers API (`provider_peers`) used by today's copy UI; reuse the peer picker.

- [ ] **Step 1:** Write the failing pure-logic test for the view's decision helper (extract to a `*.ts` and unit-test per the dashboard convention): borrowed providers render read-only with "shared from {owner}" and no rotate/delete actions; owned providers show a "Share with…" action listing trusted peers.
- [ ] **Step 2:** Run the dashboard test suite — expect FAIL.
- [ ] **Step 3:** Implement the `*.ts` decision helper + wire `ProvidersView.vue` (Share button → peer picker → `POST .../provider/share`; Unshare on borrowed). Add backend routes in `providers.rs`.
- [ ] **Step 4:** Run the dashboard test suite — expect PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend crates/bot/src/telegram/dashboard/providers.rs
git commit -m "feat(dashboard): share/unshare providers across agents; borrowed read-only"
```

---

## Task 8: Retire copy-by-readback

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` (`handle_provider_copy`, `plan_copy`), `crates/right/src/internal_api.rs` (route), `crates/bot/src/telegram/dashboard/providers.rs` (`handle_import`/`handle_export`)

**Context:** Sharing replaces copy. Remove the copy route + `handle_provider_copy` + `plan_copy` + `ProviderCopyReq`, OR (smaller diff) keep the route returning the `SourceCredentialUnreadable` guard error permanently with a message pointing at Share. Decision: REMOVE (decision §Decisions.3 dropped independent copy). Delete the now-unused `plan_copy_tests` copy cases; KEEP the `check_source_credential_readable` guard + its tests as defense-in-depth for any future read-back caller.

- [ ] **Step 1:** Delete `handle_provider_copy`, `plan_copy`, `CopyPlan`, `ProviderCopyReq`, the copy route, and the dashboard import/export-by-readback handlers + their tests. Remove only what these deletions make unused (per AGENTS.md — don't touch unrelated dead code).
- [ ] **Step 2:** Build — `devenv shell -- cargo build -p right -p right-bot`. Fix unused-import fallout.
- [ ] **Step 3:** Run — `devenv shell -- cargo nextest run -p right internal_api_providers` — expect PASS (guard tests remain).
- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(providers): retire copy-by-readback in favor of sharing"
```

---

## Task 9: Docs

**Files:** `ARCHITECTURE.md` (Providers section: copy-only → share + agent-agnostic names + refcount), `docs/architecture/providers.md` (walkthrough), `PROMPT_SYSTEM.md` (only if an agent-facing tool/name changed — sharing is operator-only, likely no change).

- [ ] **Step 1:** Update `ARCHITECTURE.md` Providers rules (keep under 40k chars — cut/move if needed per AGENTS.md budget). Move walkthrough detail to `docs/architecture/providers.md`.
- [ ] **Step 2:** Verify char budget: `wc -c ARCHITECTURE.md` (< 40000).
- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/providers.md
git commit -m "docs(providers): document multi-attach sharing, agent-agnostic names, refcount lifecycle"
```

---

## Final verification (mandatory)

- [ ] `devenv shell -- cargo clippy --workspace --tests -- -D warnings` (pre-existing right-bot `collapsible_if` + `internal_api_providers.rs:3656 unnecessary_to_owned` are NOT this work — leave them; ensure no NEW warnings).
- [ ] `devenv shell -- cargo nextest run --workspace -E 'not (binary(cli_integration) | binary(wizard_brand) | binary(home_isolation))'` → all pass (the excluded binaries fail only on the leftover cloudflared tunnel; note that in the PR).
- [ ] `devenv shell -- cargo test --doc --workspace`
- [ ] Live share path: `devenv shell -- cargo nextest run -p right-openshell --features test-support --test ci_openshell_provider_multi_attach --test ci_openshell_provider_borrowed_reconcile --run-ignored all --no-capture`
- [ ] `git log --oneline` reads as a clean, type-correct sequence.

## Upgrade & migration

`shared_from` defaults to owned → deployed agents unaffected until an operator shares. Existing `{agent}-{slug}` records keep their names and stay owned. No sandbox recreation, no `right agent init`. Borrowed attachments self-heal on bot-startup reconcile.
