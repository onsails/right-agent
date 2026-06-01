# GitHub write-access provider + profile-provisioning infrastructure

**Date:** 2026-06-01
**Status:** Design — approved approach, pending spec review

## Problem

Sandboxed agents can `git clone`/`fetch`/`pull` from GitHub but cannot
`git push`. The 403 comes from the OpenShell **network policy**, not from
GitHub: the built-in `github` provider profile (NVIDIA, gateway-global)
ships every endpoint as `access: read-only`. The L7 REST renderer expands
`read-only` for the `github.com` git transport into an allow-list that
permits `GET …/info/refs` and `POST …/git-upload-pack` (fetch) but omits
`POST …/git-receive-pack` (push). This is intentional upstream: the
profile comment states *"writes require an explicit policy proposal so the
agentic loop + prover can audit each capability change."*

There is **no per-provider access knob** on the gateway: `Provider` carries
only `credentials` + freeform `config`; endpoint `access`
(`read-only|read-write|full`, `NetworkEndpoint.access`) lives in the
**profile**, which is gateway-global. So "make GitHub writable" must change
a profile or the sandbox policy — it cannot be a provider-config flag.

## Approach

Add a **RightClaw-owned custom provider profile** `right-github-write`,
derived at startup from the live built-in `github` profile with every
endpoint elevated to `access: read-write`. Users opt in per agent by
attaching the write provider instead of (or in addition to) the read-only
one. This stays **pure Path A** — the gateway contributes the write
endpoints when the provider is attached; RightClaw never mutates the
agent's per-agent `policy.yaml` for it. No hybrid, no two-`github.com`
-endpoint merge ambiguity.

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
  new `id`. v1: `right-github-write`, base `github`, transform = set
  `access: "read-write"` on all endpoints.
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
   endpoints / binaries / credentials / category / display_name):
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

### De-risk milestone (FIRST task in the plan)

Spike proving the load-bearing assumption before building UI/docs: derive +
import `right-github-write` → create a provider with a real token → attach
to a `TestSandbox` → confirm the effective sandbox policy grants
`read-write` on `github.com` (i.e. `git-receive-pack` is allowed). If the
gateway does **not** honor `read-write` for the git-receive-pack POST, the
whole approach is reconsidered here.

### Test split (respects the integration-test contract)

- **Non-ignored** (`right_openshell` unit / `TestSandbox`): provisioning
  idempotency (import → re-run is no-op), structural-diff detection,
  derived-base-missing → error, `right-github-write` import + attach, and
  the effective policy reflects `read-write` on `github.com`. Dev machines
  have OpenShell — no `#[ignore]`.
- **CI-gated** (`#[ignore = "ci-openshell: live github push"]`, name
  `ci_openshell_github_write_push_*`): an actual `git push` to a throwaway
  GitHub repo using a real token through the proxy. Needs external creds +
  network, so it is CI-explicit per the integration-test rule.
- **Dashboard**: SSR component test for group rendering + access-variant
  card; pure grouping logic extracted to a `*.ts` helper and unit-tested;
  `provider_types()` DTO includes `group`.

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
