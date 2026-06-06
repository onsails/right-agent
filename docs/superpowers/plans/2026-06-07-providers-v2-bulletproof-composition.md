# Bulletproof providers-v2 Composition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guarantee `providers_v2_enabled` on every provider-attach path and confirm composition success by reading the active sandbox policy, so the "flag off / composition silently skipped → upstream 401" class fails loudly instead of shipping.

**Architecture:** Composition stays the only mechanism (folding is NOT restored). `ensure_v2_enabled` is invoked at the two real attach funnels (`reconcile_for_sandbox` in `right-openshell`, and the dashboard create/config-update handlers in `right`). After each composition reload, a new `wait_for_provider_composed` polls `get_active_policy` until the composed `_provider_<name>` rule appears — this both fixes the fragile `policy set --wait` "loaded signal" and acts as a loud backstop for any future path that forgets to enable v2.

**Tech Stack:** Rust (edition 2024), tonic gRPC (`OpenShellClient`), tokio, thiserror/miette, axum (dashboard internal API). Tests: `devenv shell -- cargo test -p <crate>`; live tests are `#[ignore = "ci-openshell: ..."]` with `ci_openshell_` prefix.

**Spec:** `docs/superpowers/specs/2026-06-07-providers-v2-bulletproof-composition-design.md`

---

## File Structure

- `crates/right-openshell/src/provider_capabilities.rs` — add pure `provider_is_composed` predicate (reuses existing private `rule_for_provider`).
- `crates/right-openshell/src/provider_capabilities_tests.rs` — unit test for the predicate.
- `crates/right-openshell/src/openshell.rs` — add `wait_for_provider_composed` polling helper + timeout/poll consts.
- `crates/right-openshell/src/providers.rs` — call `ensure_v2_enabled` at the top of `reconcile_for_sandbox` when providers are declared.
- `crates/right-openshell/src/providers_tests.rs` — unit test for the reconcile v2-guarantee.
- `crates/bot/src/sandbox_supervisor.rs` — after the existing `ensure_provider_policy_loaded`, confirm composition for each declared (non-missing) provider, in both `bring_up_sandbox` and `hot_reconcile_providers`.
- `crates/right/src/internal_api_providers.rs` — ensure v2 in dashboard create (built-in + generic) and config-update; confirm composition after the generic-create reload; surface composed state in the provider list view.
- `crates/right-openshell/tests/ci_openshell_generic_provider.rs` — assert the `_provider_` rule composes; add a flag-reset-to-false end-to-end self-enable test.
- `crates/right-openshell/tests/ci_openshell_github.rs` — assert built-in `right-github` composition (answers the built-in-key risk).
- `ARCHITECTURE.md`, `docs/architecture/providers.md` — invariant + narrative.

---

### Task 1: Pure `provider_is_composed` predicate

**Files:**
- Modify: `crates/right-openshell/src/provider_capabilities.rs` (add fn near `rule_for_provider`, ~line 79)
- Test: `crates/right-openshell/src/provider_capabilities_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/right-openshell/src/provider_capabilities_tests.rs` (reuses the existing `policy_with` helper at the top of that file):

```rust
#[test]
fn provider_is_composed_true_when_rule_present() {
    // Provider gateway name `right-example` composes under rule key
    // `_provider_right_example` (the `_provider_` prefix + sanitized name).
    let policy = policy_with("_provider_right_example", &["**"], &["api.example.com"]);
    assert!(crate::provider_capabilities::provider_is_composed(
        &policy,
        "right-example"
    ));
}

#[test]
fn provider_is_composed_false_on_empty_policy() {
    let policy = crate::openshell_proto::openshell::sandbox::v1::SandboxPolicy::default();
    assert!(!crate::provider_capabilities::provider_is_composed(
        &policy,
        "right-example"
    ));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell provider_is_composed`
Expected: FAIL — `provider_is_composed` not found / does not compile.

- [ ] **Step 3: Write minimal implementation**

In `crates/right-openshell/src/provider_capabilities.rs`, add directly after `rule_for_provider` (it stays private; this is a thin public wrapper):

```rust
/// True when the provider's composed `_provider_<name>` rule is present in the
/// sandbox's active policy. This is the direct composition signal — use it to
/// confirm composition actually happened, never the `policy set` return value.
pub fn provider_is_composed(policy: &SandboxPolicy, provider_name: &str) -> bool {
    rule_for_provider(policy, provider_name).is_some()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell provider_is_composed`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/provider_capabilities.rs crates/right-openshell/src/provider_capabilities_tests.rs
git commit -m "feat(providers): provider_is_composed predicate over active policy"
```

---

### Task 2: `wait_for_provider_composed` polling helper

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs` (add near `get_active_policy`, ~line 2081)

This helper polls the active policy until the composed rule appears. The mock server's `get_sandbox_policy_status` is an unimplemented stub, so the loop is exercised by the live integration tests (Tasks 8–9), not a unit test. This task is verified by compilation; behavior is covered downstream.

- [ ] **Step 1: Add the consts and the helper**

In `crates/right-openshell/src/openshell.rs`, ensure `use std::time::{Duration, Instant};` is present (add if missing), then add:

```rust
/// Poll interval while waiting for provider-profile composition to appear.
const PROVIDER_COMPOSE_POLL: Duration = Duration::from_millis(250);
/// Upper bound for composition to appear after a policy reload. Composition is
/// sub-second empirically (see docs/architecture/providers.md); the margin
/// tolerates a cold gateway. Tune against the live container if flaky.
const PROVIDER_COMPOSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Wait until `provider_name` is composed into `sandbox_name`'s active policy.
///
/// This is the success signal for provider composition — it reads the composed
/// `_provider_<name>` rule directly via `get_active_policy`, rather than trusting
/// the `policy set --wait` return (which no-ops on an unchanged policy hash).
/// A timeout here means the provider attached but never composed — the loud
/// signal that `providers_v2_enabled` is off on the gateway, or that composition
/// otherwise failed. Errors propagate (FAIL FAST).
pub async fn wait_for_provider_composed(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
) -> miette::Result<()> {
    let deadline = Instant::now() + PROVIDER_COMPOSE_TIMEOUT;
    loop {
        let policy = get_active_policy(client, sandbox_name).await?.unwrap_or_default();
        if crate::provider_capabilities::provider_is_composed(&policy, provider_name) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "provider {provider_name} attached but not composed into sandbox {sandbox_name} \
                 within {PROVIDER_COMPOSE_TIMEOUT:?} — providers_v2_enabled may be off on the gateway"
            ));
        }
        tokio::time::sleep(PROVIDER_COMPOSE_POLL).await;
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right-openshell`
Expected: clean (no errors). If `Duration`/`Instant` import collides, reconcile with the existing imports at the top of the file.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/src/openshell.rs
git commit -m "feat(providers): wait_for_provider_composed polls active policy for composed rule"
```

---

### Task 3: Guarantee v2 inside `reconcile_for_sandbox`

`reconcile_for_sandbox` (`providers.rs:458`) is the single funnel for both supervisor paths (`bring_up_sandbox` and `hot_reconcile_providers`). Enabling v2 here covers both. Skip when nothing is declared (a detach-only reconcile needs no composition), matching the spec's "tolerate when none declared".

**Files:**
- Modify: `crates/right-openshell/src/providers.rs:458` (top of `reconcile_for_sandbox`)
- Test: `crates/right-openshell/src/providers_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/right-openshell/src/providers_tests.rs` (mirrors the existing `ensure_v2_enabled_*` tests; `mock_update_config` returning an error proves `ensure_v2_enabled` runs *before* any attach, because reconcile errors out before reaching `list_attached`/`attach`):

```rust
#[tokio::test]
async fn reconcile_ensures_v2_before_touching_providers_when_declared() {
    // update_config (ensure_v2) errors → reconcile must surface that error,
    // proving ensure_v2 runs at the very top before list/attach.
    let mock = MockOpenShell {
        mock_update_config: Some(Box::new(|_| Err(tonic::Status::internal("v2-boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = reconcile_for_sandbox(&mut client, "sbx", "agent", &["agent-x".to_string()])
        .await
        .unwrap_err();
    match err {
        ProviderError::Grpc(msg) => assert!(msg.contains("v2-boom"), "{msg}"),
        other => panic!("expected Grpc from ensure_v2, got: {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-openshell reconcile_ensures_v2`
Expected: FAIL — without the new call, reconcile reaches `list_attached` first (the list RPC stub errors differently or the assertion on `v2-boom` fails).

- [ ] **Step 3: Write minimal implementation**

In `crates/right-openshell/src/providers.rs`, insert at the very top of `reconcile_for_sandbox`, before `let attached = list_attached(...)`:

```rust
    // Provider composition is gated by the gateway-global providers_v2_enabled
    // flag (default false on fresh gateways). Guarantee it before any attach so
    // composition is not silently skipped. Skip when nothing is declared — a
    // detach-only reconcile needs no composition and must not fail on this.
    if !declared.is_empty() {
        ensure_v2_enabled(client).await?;
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-openshell reconcile_ensures_v2`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs crates/right-openshell/src/providers_tests.rs
git commit -m "feat(providers): ensure_v2_enabled at top of reconcile_for_sandbox when declared"
```

---

### Task 4: Confirm composition after reload in the supervisor

After the existing `ensure_provider_policy_loaded` (the reload *trigger*) in both supervisor paths, confirm each declared, non-missing provider actually composed. A timeout is fatal at bring-up and triggers the existing bounded-backoff retry at hot-reconcile.

**Files:**
- Modify: `crates/bot/src/sandbox_supervisor.rs:369-380` (bring_up_sandbox) and `:472-475` (hot_reconcile_providers)

This is integration-covered (Task 8). Verified here by compilation.

- [ ] **Step 1: Add confirmation in `bring_up_sandbox`**

In `crates/bot/src/sandbox_supervisor.rs`, inside the `if provider_policy_reload_needed(...)` block in `bring_up_sandbox`, immediately after the `ensure_provider_policy_loaded(&sandbox, &policy_path)...?;` call (currently ~line 376) and before the `tracing::info!(... "provider-profile composition loaded")`, add:

```rust
            for name in declared.iter().filter(|d| !report.missing.contains(*d)) {
                right_openshell::openshell::wait_for_provider_composed(
                    &mut grpc_client,
                    &sandbox,
                    name,
                )
                .await
                .map_err(|e| {
                    miette::miette!("provider composition not confirmed during startup reconcile: {e:#}")
                })?;
            }
```

- [ ] **Step 2: Add confirmation in `hot_reconcile_providers`**

In the same file, in `hot_reconcile_providers`, immediately after the `ensure_provider_policy_loaded(resolved_sandbox, &policy_path)...?;` call (~line 474) and before `Ok(())`, add:

```rust
        for name in declared.iter().filter(|d| !report.missing.contains(*d)) {
            right_openshell::openshell::wait_for_provider_composed(&mut client, resolved_sandbox, name)
                .await
                .map_err(|e| miette::miette!("provider composition not confirmed: {e:#}"))?;
        }
```

- [ ] **Step 3: Verify it compiles**

Run: `devenv shell -- cargo check -p bot`
Expected: clean. Confirm `grpc_client`/`client`, `declared`, and `report` are in scope at each insertion point (they are, per the surrounding code).

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/sandbox_supervisor.rs
git commit -m "feat(supervisor): confirm provider composition after reload (bring-up + hot-reconcile)"
```

---

### Task 5: Guarantee v2 in dashboard create + config-update

The dashboard bypasses `reconcile_for_sandbox` (direct `create_provider` + `attach_to_sandbox`). Add a small helper that ensures v2 right after opening the gateway client, and call it in every mutation handler that attaches or recomposes. Failure is a hard, surfaced `Gateway` error (the spec's dashboard semantics).

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` — add helper; call in `handle_provider_create` (built-in path, after client open ~line 772), `create_generic_provider` (after its client open), and `handle_provider_config_update` (after its client open).

Integration-covered (Task 9). Verified here by compilation.

- [ ] **Step 1: Add the helper**

In `crates/right/src/internal_api_providers.rs`, near `open_openshell_client`, add:

```rust
/// Ensure providers_v2 is enabled before a dashboard mutation that attaches or
/// recomposes a provider. The dashboard is an explicit user action, so a failure
/// here is a hard, surfaced error — the operation cannot work with the flag off.
async fn ensure_v2_for_mutation(
    client: &mut openshell_client_type(),
) -> Result<(), ProviderApiError> {
    right_openshell::providers::ensure_v2_enabled(client)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("{e:#}")))
}
```

Replace `openshell_client_type()` with the concrete client type used by `open_openshell_client` (it returns `OpenShellClient<Channel>` or a thin wrapper — match the exact return type; the `client` value is the same one passed to `create_provider`/`attach_to_sandbox`). If `open_openshell_client`'s return type is a local alias, use that alias.

- [ ] **Step 2: Call it in the three handlers**

In `handle_provider_create` (built-in path), immediately after `let mut client = open_openshell_client().await?;` (~line 772) and before `create_provider(...)`:

```rust
    ensure_v2_for_mutation(&mut client).await?;
```

In `create_generic_provider`, immediately after its client is opened and before `ensure_profiles`/`create_provider`, add the same line.

In `handle_provider_config_update`, immediately after its client is opened and before the re-author/`UpdateProvider`/reload sequence, add the same line.

- [ ] **Step 3: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: clean. If the client type doesn't match, fix `ensure_v2_for_mutation`'s parameter type to the exact type of the `client` binding in those handlers.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(dashboard): ensure providers_v2 before provider create/config-update"
```

---

### Task 6: Confirm composition after the generic-create reload (dashboard)

The generic create path already reloads via `ensure_provider_policy_loaded` (line ~1221). Confirm composition right after, and route a confirmation failure through the same rollback as a failed reload.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` `create_generic_provider` (~line 1221–1248)

Integration-covered (Task 9). Verified here by compilation.

- [ ] **Step 1: Add confirmation after the successful reload**

In `create_generic_provider`, after the `ensure_provider_policy_loaded(&mut client?, sandbox_name, &policy_path)` success path (the call at ~line 1221) and before `append_provider_to_yaml`, add a composition confirmation that, on failure, runs the existing rollback helper and returns a hard `Gateway` error:

```rust
    if let Err(compose_err) =
        right_openshell::openshell::wait_for_provider_composed(&mut client, &sandbox_name, &name).await
    {
        // Same recovery shape as a failed reload: best-effort detach+delete,
        // restore composition, surface the error.
        let _ = right_openshell::providers::detach_from_sandbox(&mut client, &sandbox_name, &name).await;
        let _ = right_openshell::providers::delete_provider(&mut client, &name).await;
        ensure_provider_policy_loaded_after_rollback(
            &name,
            &sandbox_name,
            &policy_path,
            format!("{compose_err:#}"),
            "composition not confirmed",
        )
        .await;
        return Err(ProviderApiError::Gateway(format!("{compose_err:#}")));
    }
```

Match the exact variable names in scope (`name`, `sandbox_name`, `policy_path`, `client`) and the exact `detach_from_sandbox`/`delete_provider` signatures in `right_openshell::providers` (confirm names; the create path already calls the rollback delete on attach failure, so reuse that exact call).

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p right`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(dashboard): confirm composition after generic provider create, rollback on timeout"
```

---

### Task 7: Surface composed state in the dashboard provider list

The operator must see composed/not-composed so a divergence is visible, not silent. The provider list handler fetches the active policy once and tags each provider via `provider_is_composed`.

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs` — the provider list handler (`handle_provider_list`) and its `ProviderView`/list-item struct.

- [ ] **Step 1: Add a `composed` field to the provider list item**

Find the struct serialized by `handle_provider_list` (the per-provider row; `ProviderView` or a list-item type). Add:

```rust
    /// Whether this provider's endpoints are composed into the sandbox's active
    /// policy. `false` means attached but not substituting (will 401 upstream).
    #[serde(default)]
    pub composed: bool,
```

- [ ] **Step 2: Populate it in `handle_provider_list`**

In `handle_provider_list`, after resolving `sandbox_name` and opening the client, fetch the active policy once and set `composed` per row. For `sandbox.mode = none` (no sandbox) leave `composed: false` (providers are unavailable there anyway):

```rust
    let active_policy = right_openshell::openshell::get_active_policy(&mut client, &sandbox_name)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    // ...when building each row:
    let composed = right_openshell::provider_capabilities::provider_is_composed(
        &active_policy,
        &gateway_provider_name,
    );
```

Use the gateway provider name (the same name passed to `attach_to_sandbox`), not the user label. If the list handler does not currently open a client (read-only list from `agent.yaml`), guard the policy fetch so a gateway error degrades to `composed: false` rather than failing the list (use `.ok().flatten()` as above — never `unwrap`).

- [ ] **Step 3: Verify it compiles and existing list tests pass**

Run: `devenv shell -- cargo test -p right internal_api_providers`
Expected: PASS. Update any list snapshot/test that asserts exact row JSON to include `composed`.

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/internal_api_providers.rs
git commit -m "feat(dashboard): surface provider composed state in provider list"
```

---

### Task 8: Live integration — composition asserted + self-enable on a fresh-flag gateway

Extend the generic provider live test to (a) assert the `_provider_` rule actually composes, and (b) add a test that resets the gateway flag to **false**, then drives `reconcile_for_sandbox` (which self-enables v2) and asserts substitution — reproducing the fresh-Linux-gateway condition.

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_generic_provider.rs`

- [ ] **Step 1: Assert composition in the existing header test**

In `ci_openshell_generic_profile_substitutes_custom_header`, after `ensure_provider_policy_loaded(sandbox.name(), &policy_path).await.expect(...)` and before the curl exec, add:

```rust
        right_openshell::openshell::wait_for_provider_composed(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("provider composed into active policy");
```

- [ ] **Step 2: Add a flag-reset self-enable test**

Add a new test in the same file (uses the same cleanup harness). It sets the global flag to false via `UpdateConfig`, then relies on `reconcile_for_sandbox` to re-enable it:

```rust
#[tokio::test]
#[ignore = "ci-openshell: live sandbox + gateway"]
async fn ci_openshell_reconcile_self_enables_v2_on_fresh_gateway() {
    let profile_id = unique_profile_id("generic-selfenable");
    let provider_name = unique_name("generic-selfenable");
    let sandbox_name = Arc::new(Mutex::new(None));
    cleanup_generic_resources(&provider_name, &profile_id, None).await;

    with_generic_cleanup(&provider_name, &profile_id, sandbox_name.clone(), async {
        let mut client = connect_grpc(&default_mtls_dir()).await.unwrap();

        // Simulate a fresh gateway: force providers_v2_enabled OFF.
        set_providers_v2(&mut client, false).await;

        ensure_generic_profile(&mut client, &profile_id, true).await;
        right_openshell::test_cleanup::register_test_provider(&provider_name, Some(&profile_id));
        create_provider(&mut client, &fake_provider_spec(&provider_name, &profile_id))
            .await
            .expect("create provider");

        let (_policy_tmp, policy_path) = raw_tunnel_policy_file();
        let sandbox =
            TestSandbox::create_with_policy("ci-openshell-selfenable", RAW_TUNNEL_BASE_POLICY).await;
        *sandbox_name.lock().expect("sandbox name lock") = Some(sandbox.name().to_string());

        // reconcile_for_sandbox must self-enable v2, then compose.
        right_openshell::providers::reconcile_for_sandbox(
            &mut client,
            sandbox.name(),
            "agent",
            &[provider_name.clone()],
        )
        .await
        .expect("reconcile");
        right_openshell::test_cleanup::register_test_provider_attachment(&provider_name, sandbox.name());
        ensure_provider_policy_loaded(sandbox.name(), &policy_path)
            .await
            .expect("provider policy loaded");
        right_openshell::openshell::wait_for_provider_composed(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("composed after self-enable");
        wait_for_provider_placeholder(&sandbox).await;

        let (out, code) = sandbox.exec_with_timeout(&["sh", "-lc", CURL_ECHO_HEADER], 60).await;
        assert_eq!(code, 0, "curl failed: {out}");
        assert!(out.contains(FAKE_CREDENTIAL), "credential not substituted: {out}");
    })
    .await;
}

/// Set the gateway-global providers_v2_enabled flag (test helper).
async fn set_providers_v2(client: &mut OpenShellClient<Channel>, on: bool) {
    use right_openshell::openshell_proto::openshell::{sandbox::v1 as sandbox_v1, v1 as proto_v1};
    client
        .update_config(proto_v1::UpdateConfigRequest {
            global: true,
            setting_key: right_openshell::providers::PROVIDERS_V2_ENABLED_KEY.to_string(),
            setting_value: Some(sandbox_v1::SettingValue {
                value: Some(sandbox_v1::setting_value::Value::BoolValue(on)),
            }),
            ..Default::default()
        })
        .await
        .expect("set providers_v2");
}
```

Confirm the imports (`OpenShellClient`, `Channel`, `connect_grpc`, `default_mtls_dir`, `create_provider`, `ensure_provider_policy_loaded`, the cleanup helpers, `CURL_ECHO_HEADER`, `FAKE_CREDENTIAL`) match what the file already imports; reuse the existing ones. If `openshell_proto` is not re-exported from `right_openshell`, construct the request the way the existing tests construct gateway requests.

- [ ] **Step 3: Verify the test crate compiles (do not run live tests locally)**

Run: `devenv shell -- cargo test -p right-openshell --test ci_openshell_generic_provider --no-run`
Expected: compiles. (Live execution is the CI ignored-test job.)

- [ ] **Step 4: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_generic_provider.rs
git commit -m "test(ci-openshell): assert composition + reconcile self-enables v2 on fresh-flag gateway"
```

---

### Task 9: Live integration — built-in `right-github` composition (resolve the built-in risk)

The spec flags an open risk: confirm built-in providers compose under a `_provider_<name>` rule that `provider_is_composed` matches. This test answers it; if it fails, the matcher must be widened (follow-up).

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_github.rs`

- [ ] **Step 1: Add a composition assertion to the existing github provider test**

In `ci_openshell_github_gh_api_user_succeeds` (the test that already calls `ensure_v2_enabled` + attaches `right-github`), after the policy reload / before the live `gh` call, add:

```rust
        right_openshell::openshell::wait_for_provider_composed(&mut client, sandbox.name(), &provider_name)
            .await
            .expect("built-in right-github composed into active policy");
```

Use the exact gateway provider name the test attached (the `right-github` provider name variable already in the test).

- [ ] **Step 2: Verify the test crate compiles**

Run: `devenv shell -- cargo test -p right-openshell --test ci_openshell_github --no-run`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_github.rs
git commit -m "test(ci-openshell): assert built-in right-github composes into active policy"
```

---

### Task 10: Documentation + invariants

**Files:**
- Modify: `ARCHITECTURE.md` (Providers section)
- Modify: `docs/architecture/providers.md`

- [ ] **Step 1: Add the prescriptive rule to `ARCHITECTURE.md`**

In the `### Providers` section of `ARCHITECTURE.md`, add (keep it to the rule — this file has a hard 40k budget):

```markdown
Every provider-attach path MUST guarantee `providers_v2_enabled` (via
`right_openshell::providers::ensure_v2_enabled`): `reconcile_for_sandbox`
(supervisor) and the dashboard create/config-update handlers. Composition
success MUST be confirmed by `openshell::wait_for_provider_composed` (reads the
composed `_provider_<name>` rule from the active policy), never inferred from the
`policy set --wait` return. Covers built-in and generic providers — both ride
composition.
```

- [ ] **Step 2: Update `docs/architecture/providers.md`**

In the composition/policy-interaction section, document: `wait_for_provider_composed` as the success signal (replacing reliance on the `policy set --wait` no-op-on-unchanged-hash return); the two-funnel v2 guarantee (reconcile + dashboard); the loud-backstop property (a missed v2-enable on a future path times out instead of silently 401-ing); and that the scope is built-in + generic. Note folding stays removed and the legacy strip stays cleanup-only.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/providers.md
git commit -m "docs(providers): bulletproof-composition invariant + wait_for_provider_composed"
```

---

### Task 11: Final workspace verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: all pass (live `ci_openshell_`/`ci_*`-ignored tests are skipped locally). Record any pre-existing failures noted in memory (`flaky_tests_parallel_load`: cc/invocation pid race + dashboard warn-count flake under load — re-run isolated before blaming this change).

- [ ] **Step 2: Clippy + build**

Run: `devenv shell -- cargo clippy --workspace --all-targets` then `devenv shell -- cargo build --workspace`
Expected: no new warnings; clean build. (Pre-existing `clippy::unnecessary_to_owned` at `internal_api_providers.rs` test helper is known — do not touch.)

- [ ] **Step 3: Live re-validation (CI or local OpenShell)**

On a host with OpenShell (or the Linux gateway container from the handoff recipe), run the ignored tests:
Run: `devenv shell -- cargo test -p right-openshell --test ci_openshell_generic_provider -- --ignored`
and `--test ci_openshell_github -- --ignored`.
Expected: composition asserted; self-enable test goes from flag-off → substituting. If the github composition test (Task 9) fails, widen `rule_for_provider`/`provider_is_composed` to the built-in rule-key shape and re-run.

---

## Self-Review

**Spec coverage:**
- Goal 1 (v2 on every attach path): Tasks 3 (reconcile/supervisor), 5 (dashboard create + config-update). ✓
- Goal 2 (confirm composition by reading policy): Tasks 1 (predicate), 2 (poll helper), 4 (supervisor wiring), 6 (dashboard wiring). ✓
- Goal 3 (loud backstop): falls out of Task 4/6 (timeout error) — asserted by Task 8's self-enable test. ✓
- Goal 4 (observability): Task 7 (dashboard composed field). ✓
- Non-goal "no folding / no proto RPC / leave #92": respected — no task touches them; Task 10 documents folding stays removed. ✓
- Spec risk "built-in `_provider_` key shape": Task 9 + Task 11 Step 3. ✓
- Spec risk "timeout value": Task 2 const + Task 11 tuning note. ✓
- Spec risk "hot-reconcile retry interaction": Task 4 routes the timeout through the existing `hot_reconcile_providers` error return (bounded backoff per ARCHITECTURE). ✓

**Type consistency:** `provider_is_composed(&SandboxPolicy, &str) -> bool` (Task 1) is called identically in Tasks 2, 7, and used by Task 8/9 via `wait_for_provider_composed`. `wait_for_provider_composed(&mut OpenShellClient<Channel>, &str, &str) -> miette::Result<()>` (Task 2) is called with that exact arity in Tasks 4, 6, 8, 9. `ensure_v2_enabled` and `PROVIDERS_V2_ENABLED_KEY` match the verbatim definitions in `providers.rs`. `ProviderApiError::Gateway(String)` matches the enum.

**Placeholder scan:** No TBD/TODO. The two `--no-run` compile checks and the integration-covered (no-unit) tasks are explicit verification choices, not omissions, because the mock's `get_sandbox_policy_status` is an unimplemented stub. Items requiring the implementer to match an exact in-scope name (client type in Task 5; detach/delete signatures in Task 6; list-item struct in Task 7; test imports in Task 8) are flagged inline with how to confirm — these are real local bindings, not invented symbols.
