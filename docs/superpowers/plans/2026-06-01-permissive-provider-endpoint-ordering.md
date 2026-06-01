# Permissive Provider Endpoint Ordering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make generic-provider credential substitution work for agents on `network_policy: permissive` by emitting provider host L7 endpoints BEFORE the hostless `tls: skip` catch-all in the generated OpenShell policy.

**Architecture:** OpenShell's proxy evaluates `network_policies.outbound.endpoints` in order. A leading hostless `tls: skip` catch-all whose `allowed_ips` cover a provider host's IP raw-tunnels the connection (no TLS termination → no header substitution → the literal `openshell:resolve:env:...` placeholder leaks upstream → 401). The fix moves the `# right-providers: insert-above` anchor — the insertion point used by `providers_append`/`providers_append_checked` — from the END of `permissive_endpoints()` (after the port 80 block) to the TOP (before the port 443 block), so appended provider host endpoints land first and win the match. This was proven empirically on a throwaway sandbox: with the host endpoint first, the proxy terminated TLS (cert issuer = OpenShell Sandbox CA) and substituted the real credential; with the catch-all first it raw-tunneled and the placeholder leaked. IP carve-out alone does NOT work (turns the leak into a CONNECT 403) — ordering is the dominant variable.

**Tech Stack:** Rust (edition 2024), `right-codegen` crate, `serde_saphyr` (YAML parsing in tests), OpenShell policy YAML.

---

## Pre-flight

This plan is small (one source edit + one new test + doc touch). Per the project's checkout-churn convention, execute in a dedicated worktree under `.worktrees/` and land via fast-forward push to `origin/master`; do not rewrite or switch `master` in the shared checkout.

Baseline verification before starting (record any pre-existing failures):

```bash
devenv shell -- cargo test -p right-codegen
```

Expected: PASS (the existing provider-policy tests are green on `master`).

## File Structure

- **Modify:** `crates/right-codegen/src/policy.rs` — `permissive_endpoints()` (~lines 154-173): relocate the anchor line; expand the doc comment to state the ordering invariant.
- **Modify (tests):** `crates/right-codegen/src/policy_provider_tests.rs` — add one regression test asserting the provider host endpoint precedes the first `tls: skip` catch-all in a real permissive policy.
- **Modify (docs):** `docs/architecture/providers.md` — document the ordering invariant (cite-on-touch is mandatory per AGENTS.md).

No new files. No public API changes — `permissive_endpoints()` is private; `providers_append`/`providers_append_checked`/`apply_provider_stanzas` signatures are unchanged (they already insert above the anchor; only the anchor's position changes).

---

### Task 1: Regression test — provider endpoint must precede the catch-all

**Files:**
- Test: `crates/right-codegen/src/policy_provider_tests.rs` (append a new `#[test]` near the existing `apply_provider_stanzas_folds_generic_above_anchor`, after line ~396)

- [ ] **Step 1: Write the failing test**

Append this test to `crates/right-codegen/src/policy_provider_tests.rs` (it uses `generate_policy`, `apply_provider_stanzas`, `HostMcpAccess`, `NetworkPolicy`, and the `generic_entry` helper already in scope in that file):

```rust
/// Regression for the permissive provider-shadowing bug: a generic provider's
/// host L7 endpoint MUST be emitted BEFORE the hostless `tls: skip` catch-all
/// in `network_policies.outbound.endpoints`. OpenShell evaluates endpoints in
/// order; a leading hostless `tls: skip` catch-all whose `allowed_ips` cover
/// the provider host's IP raw-tunnels the connection (no TLS termination, no
/// credential substitution → the placeholder leaks upstream → 401). Proven on
/// a throwaway sandbox: host-first terminates TLS and substitutes; catch-all
/// first leaks. See docs/architecture/providers.md.
#[test]
fn permissive_provider_endpoint_precedes_tls_skip_catch_all() {
    let base = generate_policy(
        8100,
        &NetworkPolicy::Permissive,
        HostMcpAccess::BootstrapUnresolved,
    );
    let out = apply_provider_stanzas(
        &base,
        &[generic_entry("right-twitterapi", "api.twitterapi.io", None)],
    )
    .unwrap();

    let host_idx = out
        .find("- host: api.twitterapi.io")
        .expect("provider host endpoint must be present");
    let catch_all_idx = out
        .find("tls: skip")
        .expect("permissive policy must contain a tls: skip catch-all");

    assert!(
        host_idx < catch_all_idx,
        "provider host endpoint must precede the tls: skip catch-all so the \
         proxy terminates TLS and substitutes the credential; otherwise the \
         catch-all shadows it and the placeholder leaks upstream.\n\
         host_idx={host_idx} catch_all_idx={catch_all_idx}\npolicy:\n{out}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-codegen permissive_provider_endpoint_precedes_tls_skip_catch_all -- --nocapture`

Expected: FAIL — assertion fires because on the current code the catch-all (`tls: skip`) is rendered before the anchor, so the appended `- host: api.twitterapi.io` lands AFTER the catch-all (`host_idx > catch_all_idx`).

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/right-codegen/src/policy_provider_tests.rs
git commit -m "test(codegen): assert permissive provider endpoint precedes tls:skip catch-all"
```

---

### Task 2: Fix — move the provider anchor to the top of the permissive endpoints list

**Files:**
- Modify: `crates/right-codegen/src/policy.rs:154-173` (`permissive_endpoints()`)

- [ ] **Step 1: Replace `permissive_endpoints()`**

Replace the entire current function body (the doc comment + `format!`) with this. The only structural change is that `# right-providers: insert-above` moves from the last line to the first line; the comment is expanded to record the ordering invariant:

```rust
fn permissive_endpoints() -> String {
    let allowed_ips = public_web_allowed_ips_yaml(10);
    // The `# right-providers: insert-above` line is the anchor used by
    // `providers_append`/`providers_append_checked` to locate the insertion
    // point; appended stanzas land immediately ABOVE it. It sits at the TOP of
    // the endpoints list so generic-provider host endpoints (`protocol: rest`,
    // TLS-terminated) are emitted BEFORE the hostless `tls: skip` catch-all.
    //
    // Ordering is load-bearing: OpenShell evaluates `outbound.endpoints` in
    // order. A leading hostless `tls: skip` catch-all whose `allowed_ips` cover
    // a provider host's IP raw-tunnels the connection (no TLS termination, no
    // credential substitution), so the literal `openshell:resolve:env:...`
    // placeholder leaks upstream and the API returns 401. Provider host
    // endpoints MUST precede the catch-all to win the match and terminate TLS.
    //
    // The anchor also keeps generic stanzas inside `outbound.endpoints` rather
    // than the first-rendered `anthropic` section in restrictive mode (the
    // `policy.find("endpoints:")` fallback would otherwise smuggle them into
    // the Anthropic-gated allowlist).
    format!(
        r#"      # right-providers: insert-above
      - port: 443
        allowed_ips:
{allowed_ips}
        tls: skip
      - port: 80
        allowed_ips:
{allowed_ips}
        tls: skip"#
    )
}
```

- [ ] **Step 2: Run the new regression test to verify it passes**

Run: `devenv shell -- cargo test -p right-codegen permissive_provider_endpoint_precedes_tls_skip_catch_all`

Expected: PASS — the appended `- host: api.twitterapi.io` now precedes the `tls: skip` catch-all.

- [ ] **Step 3: Run the full right-codegen suite to check for ordering regressions**

Run: `devenv shell -- cargo test -p right-codegen`

Expected: PASS for all, including:
- `apply_provider_stanzas_folds_generic_above_anchor` — still true (stanza inserts above the anchor; only the anchor moved).
- `append_then_strip_round_trips_against_real_policy` — still byte-identical (`providers_strip` removes the stanza position-independently).
- `append_targets_outbound_endpoints_in_permissive_real_policy` and `appended_provider_endpoint_uses_host_and_port_not_domain` — still parse valid YAML with the stanza in `outbound.endpoints`.

If any test fails because it hard-codes the catch-all-before-anchor shape, update it to reflect the corrected invariant (host endpoint first). Do NOT relax the new Task 1 assertion.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/src/policy.rs
git commit -m "fix(codegen): emit provider host endpoints before permissive tls:skip catch-all

A leading hostless tls:skip catch-all shadowed provider host L7 endpoints
in permissive policies: OpenShell raw-tunneled by IP match, never terminated
TLS, and never substituted the credential, so the openshell:resolve:env
placeholder leaked upstream and APIs returned 401. Move the
# right-providers: insert-above anchor to the top of the endpoints list so
appended provider endpoints precede the catch-all and win the match.

Verified on a throwaway sandbox: host-first => OpenShell CA + real credential
substituted; catch-all-first => real cert + placeholder leak."
```

---

### Task 3: Document the ordering invariant (cite-on-touch)

**Files:**
- Modify: `docs/architecture/providers.md` (the section near lines 50-54 that warns about placeholders being stranded on a raw tunnel)

- [ ] **Step 1: Read the current section**

Run: `sed -n '40,60p' docs/architecture/providers.md`

Confirm the existing text that mentions folding providers via `apply_provider_stanzas` and the "stranded on a raw tunnel → upstream 401" failure mode.

- [ ] **Step 2: Add the ordering invariant**

Insert the following paragraph immediately after the existing raw-tunnel-stranding sentence (adjust surrounding prose for flow; keep it concise):

```markdown
**Endpoint ordering is load-bearing.** In permissive policies the provider
host L7 endpoints (`protocol: rest`, TLS-terminated) MUST be emitted *before*
the hostless `tls: skip` catch-all. OpenShell evaluates `outbound.endpoints`
in order: a leading hostless `tls: skip` catch-all whose `allowed_ips` cover a
provider host's IP raw-tunnels the connection (no TLS termination, no
substitution → placeholder leaks → 401), and a *trailing* host endpoint is
never consulted. IP carve-out does not help — removing the covering range
while the catch-all stays first turns the leak into a CONNECT 403. The anchor
`# right-providers: insert-above` therefore sits at the TOP of the endpoints
list (`permissive_endpoints()` in `crates/right-codegen/src/policy.rs`), and
`permissive_provider_endpoint_precedes_tls_skip_catch_all` enforces it.
```

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/providers.md
git commit -m "docs(providers): record permissive endpoint ordering invariant"
```

---

### Task 4: Final workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Clippy (workspace, tests, deny warnings)**

Run: `devenv shell -- cargo clippy --workspace --tests -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Full workspace test (mandatory final check)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Note any pre-existing flakes recorded in the baseline (e.g. the cc/invocation pid race and dashboard warn-count tests can flake under parallel load — re-run isolated before attributing to this change).

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

---

### Task 5: Deploy to the live `right` agent and verify substitution (requires user confirmation)

> This task touches the user's running agent. Do not run it without explicit confirmation. It is the real-world validation that the user's `twitterapi` key starts working.

**Files:** none (operational)

- [ ] **Step 1: Land the branch**

Fast-forward push the feature branch to `origin/master` (per checkout-churn convention). Confirm `origin/master` advanced.

- [ ] **Step 2: Restart the agent so codegen regenerates and re-applies the policy**

The bot regenerates `policy.yaml` via `generate_provider_aware_policy` on startup and hot-applies the network section through `openshell policy set --wait` (policy.yaml is `Regenerated`; no sandbox recreation). Restart the `right` agent using the worktree-built binary:

Run: `target/devenv/release/right restart right` (or `cargo run --release --bin right -- restart right`).

Do NOT invoke bare `right` (stale `$PATH` copy).

- [ ] **Step 3: Confirm the regenerated policy has the corrected order**

Run: `grep -n "host: api.twitterapi.io\|tls: skip" ~/.right/agents/right/policy.yaml | head`
Expected: the `- host: api.twitterapi.io` line appears at a SMALLER line number than the first `tls: skip`.

- [ ] **Step 4: Verify substitution end-to-end via the agent**

Ask the `right` agent (in its Telegram chat, or by an SSH `claude -p` probe per AGENTS.md "Reproduce a sandbox `claude` invocation by hand") to call `GET https://api.twitterapi.io/oapi/my/info` with `x-api-key: $TWITTERAPI_IO_KEY`.
Expected: HTTP 200 (or a valid authenticated response), NOT `401 "Unauthorized.x-api-key is invalid"`. A 200 confirms the proxy substituted the real key. Repeat for `api.typefully.com` (`Authorization` header) if desired.

If still 401: confirm the gateway credential value is the real service key (`openshell provider get right-twitterapi` shows `CREDENTIAL_KEYS=1`; the value itself is write-only) — a correctly-ordered policy cannot fix a wrong stored key.

---

## Out of scope (with rationale)

- **Hardening `providers_append_checked` to detect hostless `tls: skip` shadowing.** The guard's `PolicyConflict::RawTunnel` check matches host-named `tls: skip` endpoints only; it never matched the hostless permissive catch-all, which is why the broken config was accepted. After Task 2, provider endpoints always precede the catch-all in the only mode where generic providers are allowed (permissive — restrictive rejects them via `NetworkPolicyForbidsGeneric`), so the shadowing scenario can no longer arise from codegen. Adding hostless-shadow detection would be dead defensive code (YAGNI). Revisit only if a future change lets providers coexist with operator-authored raw tunnels.
- **Switching `right` to a scoped (restrictive) policy.** Rejected: the agent legitimately needs broad HTTPS (browser-use, TradingView, GitHub, etc.); scoped mode would break it. The reorder fix keeps "all HTTPS" intact while making providers work.
- **IP carve-out of provider hosts from the catch-all.** Rejected: empirically it does not restore substitution on its own (CONNECT 403), and provider hosts behind CDNs (Cloudflare) rotate IPs, making carve-out fragile and requiring continuous re-resolution.

## Self-Review

- **Spec coverage:** Reorder fix (Task 2), regression test encoding the invariant (Task 1), mandatory cite-on-touch doc update (Task 3), full verification (Task 4), live deploy+verify (Task 5). All requirements from the diagnosis are covered.
- **Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result.
- **Type consistency:** Test uses `generate_policy`, `apply_provider_stanzas`, `HostMcpAccess::BootstrapUnresolved`, `NetworkPolicy::Permissive`, and the existing `generic_entry` helper — all already present in `policy_provider_tests.rs`. `permissive_endpoints()` stays private with an unchanged signature; the `format!` named-argument `{allowed_ips}` matches the local binding.
