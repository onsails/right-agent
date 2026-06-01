# GitHub write-access provider + profile-provisioning infrastructure

**Date:** 2026-06-01
**Status:** Design — revised after live verification (mechanism + fix empirically confirmed).

## Problem

A sandboxed agent with the built-in `github` provider attached cannot
`git push` — and, on the credential-injected path, cannot `git clone`/`fetch`
either. The 403 comes from the OpenShell **network policy**, not GitHub
(verified live: the blocked response carries
`X-OpenShell-Policy: _provider_<agent>_github` and **no**
`X-GitHub-Request-Id`; GitHub's own denials always carry the request id).

**Mechanism (empirically confirmed).** The built-in `github` profile ships
`github.com:443` as `access: read-only`. To inject the provider credential
(substitute the `openshell:resolve:env:…` placeholder with the real token),
the proxy MUST TLS-terminate github.com; termination activates the
read-only L7 segment, and `read-only` **blocks every HTTP POST** to
github.com. Git Smart-HTTP uses POST for both data transfers —
`POST …/git-upload-pack` (fetch) and `POST …/git-receive-pack` (push) — so
read-only breaks **both**. Unauthenticated or real-token-in-URL traffic
raw-tunnels through untouched, which is why the failure only surfaces on
the credentialed git path (and earlier black-box tests that probed
unauthenticated wrongly concluded the policy was permissive).

RightClaw does **not** control these rules. The `_provider_<agent>_github`
segment is rendered by the **OpenShell gateway** from the gateway-global
`github` profile when the provider is attached at sandbox-create
(`--provider`); `apply_provider_stanzas` folds only `Generic` providers and
skips `github`. There is no per-provider access knob (`Provider` carries
only `credentials` + freeform `config`; method/path policy lives in the
**profile**, which is gateway-global). So "let the agent use GitHub" must
change a **profile**, not the per-agent policy or a provider flag.

**Verified fix (live, raw-tunnel base mimicking production).** A github
profile whose endpoint uses explicit L7 `rules` allowing all methods
(`allow { method: "*", path: "**" }`) lets both git POSTs reach GitHub
(a real push to a throwaway branch succeeded); the `read-only` control
blocked both with `X-OpenShell-Policy`. `access` and `rules` are mutually
exclusive (proto); the `read-write`/`full` **presets were not validated** —
only the explicit-rules mechanism is proven, so the design uses rules.

## Approach

Add a **RightClaw-owned custom provider profile** `right-github-write`,
derived at startup from the live built-in `github` profile with the
`github.com` and `api.github.com` endpoints opened to **all methods** via
explicit L7 rules (`allow { method: "*", path: "**" }`), replacing the
coarse `read-only` preset. Users opt in per agent by attaching this
provider instead of the read-only built-in one. This stays **pure Path A** —
the gateway contributes the endpoints when the provider is attached;
RightClaw never mutates the agent's per-agent `policy.yaml` for it.

**Why allow-all, not a narrow git-only whitelist** (GET info/refs + POST
upload/receive-pack): the agents' repos use **Git LFS**, and an agent will
want more than push (releases, LFS objects, REST writes, gists). A narrow
whitelist re-breaks the moment it touches any of those. The real
authorization boundary is the **token's GitHub permissions** (enforced
server-side), not OpenShell's HTTP-method filter on a trusted,
TLS-terminated, credential-injected host — so opening all methods to the
github hosts adds negligible risk while being robust.

**Why explicit rules, not `access: read-write`/`full`:** only the
explicit-rules mechanism was empirically verified to unblock the git POSTs;
the presets were not. Since `access` and `rules` are mutually exclusive,
the transform clears `access` and sets the allow-all rule.

Because more RightClaw-authored profiles are coming (e.g. `browser-use`),
we build a small **profile-provisioning subsystem** now rather than a
one-off function. The subsystem re-asserts our profiles on the gateway on
every `right up`, so a change to our definition (or upstream drift in a
derived profile's base) is picked up automatically.

### Why this over the alternatives

- **Per-provider toggle → RightClaw-owned `policy.yaml` stanza.** Rejected:
  makes a built-in provider a Path A/B hybrid (breaks the "built-in =
  gateway owns policy" invariant), risks two-endpoint merge semantics,
  needs a new `agent.yaml` field + `config_watcher` diff extension. Touches
  far more load-bearing machinery.
- **Override the built-in `github` profile globally.** Rejected: flips
  every agent to write — breaks "security is the default" and per-agent
  granularity.

The chosen design is both simpler for the user (an explicit, auditable
read-vs-write choice that matches OpenShell's "writes are an explicit
capability" model) and more maintainable for us (isolated, additive
gateway surface; no changes to the per-agent policy pipeline).

## Components

### 1. Profile-provisioning subsystem (`right_openshell`)

A module owning RightClaw-authored OpenShell provider profiles. The set of
managed profiles is a **module-local free-form list** — not a cross-crate
enum or central registry (consistent with the codebase's
"per-module free-form types over shared registries" and
"promote on demand" principles).

A managed profile is one of two shapes:

- **Derived** `{ id, base_id, transform }` — fetch a live upstream profile
  via `GetProviderProfile(base_id)`, clone it, apply `transform`, set the
  new `id`. v1: `right-github-write`, base `github`, transform = on the
  `github.com` and `api.github.com` endpoints, clear `access` and set
  `rules: [allow { method: "*", path: "**" }]` (proven to unblock git
  POSTs; all other endpoints/fields untouched).
- **Authored** `{ id, profile }` — a full static `ProviderProfile`
  definition with no upstream base. Reserved for future profiles
  (`right-browser-use`); **not shipped in v1**.

All RightClaw profile ids are **`right-*`** prefixed. The prefix is the
ownership marker (no magic-string/label tagging needed) and prevents
collision with upstream/built-in profile ids.

`ensure_managed_profiles(grpc) -> Result<Report>`:

1. For each managed profile, compute `desired`:
   - Derived: `GetProviderProfile(base_id)`. **Absent → hard error**
     (FAIL FAST — `right up` fails with a clear message; a derived write
     profile whose base is gone is unrecoverable and must surface).
   - Authored: use the static definition.
2. `GetProviderProfile(id)` → `stored`.
3. If `stored` absent OR `stored != desired` (structural compare of
   endpoints / binaries / credentials / category / display_name — the
   endpoint fingerprint MUST include `rules` and `access`, since the signal
   now lives in `rules`, not `access`):
   `LintProviderProfiles(desired)` → if valid → `ImportProviderProfiles(desired)`.
   Lint failure → hard error (our bug). Log `drift → imported`.
4. Else log `unchanged` (debug).

Properties: idempotent, deterministic, drift-proof (re-derives from the
live base each run, so both our-logic changes and upstream base changes
propagate), minimal writes (only on real diff).

**No auto-GC in v1.** Profiles dropped from the managed list are left on
the gateway (deleting one still referenced by a provider would break
attach); log only. GC is future work.

### 2. Startup hook (`right up`)

A new stage in the `right up` pipeline, after the gateway is confirmed
`Ready` (just after the existing `up: openshell_preflight` stage in
`crates/right/src/main.rs`): connect a gRPC client and call
`ensure_managed_profiles` **once per gateway**, before bots start.

Per-agent bot startup (`sandbox_supervisor`) does **not** re-import
(avoids N redundant/racy gateway writes for one global object). It may
`GetProviderProfile(id)` and warn if a managed profile is unexpectedly
missing, but re-assertion is owned by `right up`.

### 3. Catalog + provider wiring (`right_openshell::providers`)

- Add `right-github-write` to `provider_catalog`: env `GITHUB_TOKEN`,
  category source_control, display "GitHub (write)", group `github`.
- The built-in `github` entry stays (read-only).
- `type_slug == profile id == provider.type` sent to the gateway =
  `right-github-write`.
- Update the catalog count test (`catalog_has_8_built_in_plus_generic` →
  9 built-in slugs; assert `right-github-write` present and `right-*`
  prefixed).

### 4. Dashboard UX

The dashboard prioritizes **user convenience over technical precision**:
RightClaw's `right-*` slugs are never shown raw. The provider catalog
exposes a friendly **group**.

- Add `group: String` to the provider-type DTO (`api_types.rs`) and
  `ProviderProfileView` (`types.ts`); plumb through the internal-socket
  `provider_types()` API. (`category` already exists but is too coarse —
  both github and gitlab are `source_control`.)
- Frontend collapses all types sharing a `group` into **one card per
  group**. The "GitHub" card offers an access choice: **read-only
  (recommended)** vs **write**, mapping to `github` / `right-github-write`
  under the hood. Single-variant groups render as today. Use
  `CollapsibleSection.vue`; render loading/empty/error via `AsyncState.vue`.
- Fix the inaccurate eyebrow in `ProvidersView.vue`:
  `<p class="eyebrow">AI Providers</p>` → `Integrations` (covers AI, source
  control, and future browser automation — these are not all AI).

## Documentation

- **AGENTS.md** — add the product principle:
  > **Simplest for the user, most maintainable for us.** When a feature has
  > multiple working implementations, choose the one that (a) gives the
  > user fewer steps and an explicit, auditable choice, and (b) reuses
  > existing, tested paths instead of new control planes or invariant
  > hybrids. Add new gateway/sandbox surface only when it is isolated and
  > additive, not when the alternative smears complexity across
  > load-bearing machinery.
- **ARCHITECTURE.md** ("Dashboard frontend primitives", prescriptive,
  ≤3 sentences) — add the namespacing/UX rule:
  > RightClaw-owned technical identifiers are `right-*` namespaced; the
  > dashboard MUST NOT surface raw slugs/prefixes, presenting grouped,
  > user-friendly labels instead. Technical precision lives in the backend;
  > the UI optimizes for user clarity.
  Satellite detail (grouping mechanism, access-variant card) →
  `docs/architecture/` dashboard doc.
- **docs/architecture/providers.md** — document the profile-provisioning
  subsystem, `right-*` ownership, the Path-A purity of derived write
  profiles, and the `right up` re-assert hook.

## Testing & verification

### Load-bearing assumption: ALREADY VERIFIED

The earlier de-risk gate is **done** — a live experiment (throwaway sandbox,
real token, raw-tunnel base mimicking production) confirmed: read-only
blocks both git POSTs with `X-OpenShell-Policy`; the allow-all-rules profile
lets both reach GitHub and a real push succeeded. So Task 1 is **not** a
go/no-go spike anymore — it is a **regression test** that encodes this
matrix, guarding against upstream OpenShell drift.

### Test split (respects the integration-test contract; tests are `#[ignore = "ci-openshell: …"]` with `ci_openshell_` names per AGENTS.rust.md — dev/CI run them explicitly)

- **`right_openshell` unit (non-gateway, default test path):** the derive
  transform clears `access` and sets the allow-all rule on github.com +
  api.github.com; `managed_profiles()` ids are `right-*`; `needs_import`
  fingerprint detects a `rules` change.
- **Live-gateway (`#[ignore = "ci-openshell: …"]`, `ci_openshell_` names):**
  provisioning idempotency (import → re-run no-op), structural-diff, import +
  `get_profile` shows the allow-all `rules` on github.com.
- **Regression GATE (`#[ignore = "ci-openshell: …"]`):** attach the
  allow-all provider to a `TestSandbox` (raw-tunnel base) with a real token
  via `--provider`; assert `POST …/git-receive-pack` reaches GitHub
  (success / `X-GitHub-Request-Id`), NOT a `403 X-OpenShell-Policy`; and the
  read-only control DOES get the `X-OpenShell-Policy` block. Needs a real
  token + network (env-guarded; no-op without creds).
- **Dashboard:** SSR component test for group rendering + access-variant
  card; pure grouping logic in a `*.ts` helper, unit-tested; `provider_types()`
  DTO includes `group`.

### Cadence

Targeted during implementation: `cargo test -p right-openshell <filter>`,
`-p right-dashboard`, frontend component tests. One final
`cargo test --workspace` before completion.

## Scope / non-goals

- v1 ships only `right-github-write`. The subsystem supports authored
  profiles so `right-browser-use` slots in later with no structural change.
- No profile auto-GC.
- No "promote existing github provider to write" in-place; switching access
  = attach the other provider (provider `type` is fixed at create).
- gitlab-write and other `source_control` write variants are deferred
  (mechanism generalizes; not built now).
- **Git LFS is out of scope.** A separate, *local* blocker exists: repos
  configured for LFS have a `pre-push` hook that aborts when `git-lfs` is
  absent from the sandbox. That is a sandbox-tooling concern (install
  `git-lfs`, or push non-LFS commits with `--no-verify`), independent of
  this network-policy feature. The allow-all rules already permit the LFS
  HTTPS endpoints once `git-lfs` is present.
