# Providers via OpenShell v2 composition — design

**Status:** design (brainstorm output). Supersedes the provider-policy
ownership model in `docs/architecture/providers.md` ("Path A vs Path B") and
the generic-only fold in `crates/right-codegen/src/policy.rs::apply_provider_stanzas`.

**Goal:** make Right deeply integrated with NVIDIA OpenShell Providers v2.
Move **all** provider network policy onto OpenShell profiles + gateway
composition. Right stops writing provider endpoints into `policy.yaml`
entirely; it owns profile provisioning, provider create/attach/detach, and a
reliability layer that guarantees the composed policy is loaded in the running
sandbox.

---

## Problem

Two inconsistent provider-policy models ship today:

- **Generic providers** (`type: generic`): Right folds their `upstream_host`
  into `policy.yaml` as a TLS-terminated endpoint via `apply_provider_stanzas`.
  This works but duplicates OpenShell, makes `policy.yaml` a second source of
  truth for provider network rules, and requires the endpoint-ordering
  invariant (provider host before the permissive `tls: skip` catch-all).
- **Profile providers** (`right-github`, a managed OpenShell profile): Right
  relies on OpenShell v2 composition and does **not** fold endpoints
  (`internal_api_providers.rs:773` "Built-in flow: OpenShell manages endpoints;
  no policy mutation needed"). But Right never reliably triggered nor verified
  the sandbox reload, so on the long-running production sandbox the composed
  `_provider_*` layer never reached the live OPA engine → `api.github.com` fell
  through the `tls: skip` catch-all → the credential placeholder was sent
  verbatim → `401 Bad credentials`.

`apply_provider_stanzas` (`policy.rs:1034`) `continue`s on any non-`Generic`
type, so a profile provider's endpoints are never folded. The intended fix is
**not** to extend folding to profiles (wrong layer, divergent source of truth)
— it is to make the OpenShell-owned path reliable and route generic providers
through it too.

## Verified facts (empirical, OpenShell v0.0.56, throwaway sandboxes)

All four load-bearing assumptions were proven on fresh sandboxes before this
design was accepted:

1. **Profile provider substitution works end-to-end.** A custom profile with a
   TLS-terminated REST endpoint substitutes the `openshell:resolve:env:*`
   placeholder for the real credential at the proxy. **Hard constraint:** the
   profile MUST declare a `binaries` allowlist (`'**'` or scoped paths) — without
   it the proxy rejects `CONNECT` regardless of the endpoint rules.
2. **Generic is fully expressible as a custom profile.** `auth_style: header`
   with an arbitrary `header_name` (e.g. `x-api-key`) + arbitrary env var name
   composes and substitutes correctly. No generic capability is lost. This is
   the make-or-break for Option A and it passed.
3. **Reload is automatic in ~5–10s** via the in-sandbox poll loop on
   attach/detach — no `policy set` or restart required. `policy set --policy
   <base> --wait` (~4s) is available when a guaranteed-loaded state is needed
   before the first invocation.
4. **No direct host-side "loaded" signal.** `openshell policy get` reflects
   gateway-side composition, not sandbox acknowledgement. Reliable "ensure
   loaded" options: wait the empirical ~10s, do a functional probe inside the
   sandbox, or re-apply the base policy with `--wait` after attach.

Faithful confirmation: the existing `right-github` provider attached to a
throwaway sandbox returned `gh api /user` → **HTTP 200**. (`curl` is not in the
github profile's `binaries` allowlist and is blocked; authenticated GitHub
access flows through `gh`/`git`. Raw `curl` to GitHub on a permissive base hits
the `tls: skip` catch-all and is never substituted.)

## Design

### 1. Profiles are the only provider→policy mechanism

Every Right provider — generic or built-in-derived — is backed by an OpenShell
profile and attached to the sandbox. The gateway composes each attached
profile's endpoints into the effective sandbox policy (v2 composition, gated by
`providers_v2_enabled=true`, already set). `policy.yaml` carries **only the base
sandbox policy** and never a provider endpoint.

### 2. Generic providers become Right-managed custom profiles

For each configured generic provider, Right provisions a custom profile
(`right-<provider-name>`-namespaced) derived from the provider's
`agent.yaml` config:

- endpoint: `upstream_host:443`, `protocol: rest`, `access: full`,
  `enforcement: enforce` (no `tls:` field → auto-terminate);
- credential: `auth_style` (`bearer` or `header`), `header_name`, env var name —
  all taken from the existing generic config;
- **`binaries: ['**']`** (mandatory — see verified fact 1; matches today's
  generic behavior where any process may use the folded endpoint).

The provider record is created from the profile and attached. Provisioning is
idempotent and lives in `right_openshell::managed_profiles::ensure_profiles`
(extended from the github-only set), gated by `any_sandboxed`, reconciled on
`right up` and on provider changes.

### 3. Delete Right's policy folding

Remove `apply_provider_stanzas`'s provider-folding, the
`# right-providers: insert-above` anchor, `providers_append_checked`, the
provider-endpoint-precedes-catch-all ordering invariant, and the
`generate_provider_aware_policy` provider argument. `policy.yaml` regen becomes
provider-free. The `Configuration Hierarchy` / codegen-category treatment of
`policy.yaml` no longer mentions provider stanzas.

### 4. Reliability layer: attach + ensure-loaded + verify

After any attach/detach (startup reconcile and `hot_reconcile_providers`),
Right MUST guarantee the running sandbox loads the recomposed policy before the
next Claude invocation, rather than racing the ~10s poll loop. Mechanism (to be
finalized in the plan against v0.0.56): re-apply the agent's base policy with
`openshell policy set --policy <base> --wait`, which forces the gateway to
recompose (now including the attached provider) and blocks until the sandbox
acknowledges the new `config_revision`. Open implementation question the plan
must resolve: confirm that a base-policy re-apply after an attach bumps the
served `config_revision` (composed hash changed) and that `--wait` therefore
blocks until loaded; if a no-op base re-apply returns "Policy unchanged"
without waiting, fall back to polling the sandbox-acknowledged revision or a
functional probe. No silent assumption — the verify step must observe a
concrete signal.

### 5. Proto + version

Re-vendor the OpenShell proto from `v0.0.50` to `v0.0.56`
(`scripts/vendor-openshell-proto.sh v0.0.56`, regenerate stubs, update
`proto/UPSTREAM.md`) to model the new effective-policy fields
(`config_revision`, `policy_source`, effective policy on the status RPC) used
by the verify step. Bump `MIN_OPENSHELL_VERSION` to the version this design is
validated against (`v0.0.56`); `openshell_preflight` enforces it at startup.

### 6. Upgrade & migration (no recreation)

Already-running agents must adopt this without sandbox recreation:

- `ensure_profiles` creates the custom profiles for existing generic providers;
- existing generic providers are re-created/attached as profile-backed (or
  attached if the record already matches) — idempotent reconcile;
- `policy.yaml` is regenerated provider-free on `right restart`; the now-removed
  folded stanzas drop out;
- the reliability layer (§4) forces the running sandbox to reload the composed
  policy, so the switch from folded→composed is seamless. No `right agent init`,
  no sandbox delete.

The production sandbox currently in `Error` (post-upgrade) is recovered by
`SandboxSupervisor`, independent of this change.

### 7. Agent-facing implication

With folding gone, raw `curl`/`python` requests to a provider host on a
permissive base policy hit the `tls: skip` catch-all and are **not** substituted;
authenticated access must go through the profile's allowlisted binaries (e.g.
`gh`/`git` for GitHub). Generic profiles use `binaries: '**'`, so arbitrary
tools still work for them. Update agent guidance (TOOLS/prompt) only where it
currently tells agents to hand-inject provider tokens via curl.

## Components / boundaries

- `right_openshell::managed_profiles` — owns profile derivation for github +
  generic; `ensure_profiles` provisions/reconciles all. One profile per generic
  provider; `binaries` always present.
- `right_openshell::providers` — create/attach/detach (gRPC), unchanged surface;
  the generic path now creates+attaches a profile-backed provider instead of
  Right folding policy.
- `right-codegen::policy` — loses provider folding; emits base policy only.
- bot `config_watcher` / `sandbox_supervisor::hot_reconcile_providers` — gains
  the ensure-loaded/verify step after attach/detach.
- `right::internal_api_providers` — generic create route provisions the custom
  profile then attaches; no policy mutation.

## Error handling

- Profile provisioning errors propagate (`{:#}` chains), FAIL FAST. Missing base
  built-in profile for derived profiles is a per-profile non-fatal warn (keep
  existing github behavior).
- The ensure-loaded/verify step must FAIL LOUDLY if the sandbox does not
  acknowledge the recomposed policy within a bounded timeout — never silently
  proceed to a Claude invocation against an un-reloaded policy.

## Testing (cadence: targeted during, full workspace at end)

- Unit: generic→custom-profile derivation includes `binaries`, correct
  `auth_style`/`header_name`/env var; github derivation unchanged; profile set
  is `right-*` namespaced.
- Unit: `policy.yaml` regen emits no provider stanzas and no
  `# right-providers` anchor; removal of `apply_provider_stanzas` provider path.
- `ci_openshell_` (live): (a) profile-backed generic provider composes +
  substitutes a custom header on a fresh sandbox; (b) `binaries`-less profile is
  rejected at CONNECT (guards the constraint); (c) attach → substitution active
  within the poll window; (d) the long-missing real-token GitHub gate
  (`gh api /user` 200 / push) now green via the reliability layer.
- Migration: an agent with a folded generic provider, after restart, has a
  provider-free `policy.yaml` and a working profile-backed generic provider with
  no recreation.

## Non-goals

- Read/write scoping of profile access (still deferred; `access: full`).
- Closing the global credential-substitution scope (OpenShell #92).
- The agentic policy-approval loop (`proposal_approval_mode`) — separate feature.
- Touching the production `Error` sandbox by hand (supervisor recovers it).

## Verification cadence

Targeted package tests after each slice (`-p right-openshell`,
`-p right-codegen`, bot); live `ci_openshell_` gates run explicitly; one final
`devenv shell -- cargo test --workspace` before completion.
