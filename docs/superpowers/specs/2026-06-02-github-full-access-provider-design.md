# GitHub full-access provider — design

**Status:** design (brainstorm output). Supersedes
`docs/superpowers/specs/2026-06-01-github-write-provider-profile-provisioning*`
and `docs/superpowers/plans/2026-06-01-github-write-provider-profile-provisioning.md`.

**Goal:** ship a single full-access GitHub provider (`right-github`) so a
sandboxed agent can `git push` (and otherwise use `git`/`gh` without
restriction); hide OpenShell's read-only built-in `github` from the dashboard;
no read/write toggle, no collision resolver, no migration.

---

## Problem

The OpenShell built-in `github` provider profile sets every endpoint to
`access: read-only`. Read-only blocks all non-GET/HEAD methods, so `git push`
(an HTTP `POST` to `…/git-receive-pack` on `github.com`) is rejected with
`403 X-OpenShell-Policy`. Agents cannot push. The user's agent worked around it
by reconstructing commits through the REST Git Data API — a costly hack, not a
fix.

## Findings that shaped this design

Established empirically during the investigation (see memory
`project_openshell_credential_substitution_global` and issue
`onsails/right-agent#92`):

1. **Providers are a credential-injection mechanism, not a method/read-write
   boundary.** A provider exists to TLS-terminate its hosts and substitute the
   real credential for the sandbox's opaque placeholder. Its natural access
   level is "let the request through".
2. **On `permissive` (the recommended default) read/write L7 is largely moot.**
   `permissive` is a `tls: skip` raw tunnel over the entire public internet;
   provider read-only only bites where the provider endpoint wins the match
   (it caught `git push` on `github.com`, while `api.github.com` writes leaked
   through the catch-all). So a read/write distinction is not a real security
   boundary on permissive.
3. **Credential confinement:** the placeholder is substituted by env-var name on
   any TLS-terminated host (global, not host-scoped — OpenShell limitation,
   tracked in #92, accepted). It is **never** substituted on raw-tunnel
   (`tls: skip`) hosts, so a credential cannot reach the open internet. This
   feature does not change that posture.

**Conclusion:** the right shape is one full-access GitHub provider. Read/write
scoping is deferred until it is both meaningful (restrictive agents) and
enforceable (OpenShell endpoint-scoped credential injection, #92).

## Design

### 1. Managed profile `right-github`

- **Derived** from the live built-in `github` profile: read it, keep its
  endpoints (`api.github.com` rest+graphql, `github.com` rest), and set each
  endpoint's `access` from `read-only` to **`full`**. Rename `id` →
  `right-github`, `display_name` → `GitHub`.
  - Deriving (vs authoring) keeps us in sync if OpenShell adds/changes the
    base `github` endpoints.
  - `access: full` is the OpenShell preset that permits all HTTP methods; it
    replaces the previous branch's allow-all `rules` derivation (simpler, no
    `rules`/`access` exclusivity juggling).
- **Provisioned** by `right_openshell::managed_profiles::ensure_profiles` on
  every `right up`, gated by `any_sandboxed` (already committed). Idempotent
  structural reconcile; re-import only on real drift.
- **Base-missing handling:** if the built-in `github` profile is absent on the
  gateway, `ensure_profiles` logs a warning and skips `right-github` — it does
  **not** abort `right up`. (Downgrade the current `BaseMissing` hard error to a
  per-profile non-fatal warn, consistent with the `any_sandboxed`-gated,
  optional nature of managed-profile provisioning.)

### 2. Catalog & dashboard

- `right_openshell::providers::profile_catalog()`:
  - Add `right-github` (`type_slug: "right-github"`, `display_name: "GitHub"`,
    `env_var: "GITHUB_TOKEN"`, category `SourceControl`).
  - Keep the built-in `github` entry but mark it **`hidden: true`** so existing
    `github`-typed providers still resolve their `env_var`/display while new
    adds are not offered it. (Add a `hidden: bool` field; default `false`.)
    > **As shipped:** the hide is a dashboard-boundary concern, so no
    > `hidden` field was added to the shared catalog struct — the built-in
    > `github` slug is filtered by a `HIDDEN_FROM_DASHBOARD` const in
    > `internal_api_providers.rs::handle_provider_types` instead. Same
    > effect, no UI flag on the catalog `ProviderProfile`.
  - **Remove** the `group` field added on the prior branch (no grouping).
- `internal_api_providers.rs::handle_provider_types`: filter out `hidden`
  entries; **remove** the `group` field from `ProviderProfileView`.
- `right-dashboard` frontend: revert the grouped-card UI to a flat provider-type
  list (the hidden built-in `github` simply never appears). Keep the eyebrow
  copy change (`AI Providers` → `Integrations`). `right-github` renders as
  "GitHub" via the existing `right-*` label rule.

### 3. Removed from the `feat/github-write` branch

- Two-type model (`github` read + `right-github-write` write) → one
  (`right-github`).
- `providersGrouping.ts`, `providersGrouping.test.ts`,
  `ProvidersView.grouping.test.ts`, the grouped `<article>`/access-variant
  markup and CSS in `ProvidersView.vue`.
- `group` field across `providers.rs`, `internal_api_providers.rs`, `types.ts`.
- The collision-resolver design (never implemented).
- Rename: `ManagedProfile::GithubWrite` → `Github`; `github_write()` →
  `github()`; profile id `right-github-write` → `right-github`; test file
  `ci_openshell_github_write.rs` → `ci_openshell_github.rs`.

### 4. Kept

- `ensure_profiles` / `managed_profiles` machinery + the `any_sandboxed`-gated
  provisioning hook in `crates/right/src/main.rs`.
- The `#92` documentation corrections (README, SECURITY.md, providers.md,
  ARCHITECTURE.md).
- The AGENTS.md "Simplest for the user, most maintainable for us" principle.

## Data flow

`right up` → `ensure_profiles` provisions `right-github` (full) to the gateway →
user adds a "GitHub" provider in the dashboard (`/providers`, type
`right-github`, pastes token) → gateway holds the token, contributes the
`right-github` endpoints to the sandbox policy on attach (Path A) → agent runs
`git push` → proxy TLS-terminates `github.com`, substitutes the real token,
`access: full` permits the `POST` → push succeeds.

## Error handling

- `ensure_profiles`: gRPC/lint errors propagate (`{:#}` chains). Missing base
  `github` is a non-fatal per-profile warn (see §1). FAIL FAST elsewhere.
- Existing `github`-typed provider + new `right-github` on the same agent
  **collide** on `GITHUB_TOKEN` (the pre-existing env-var guard, unchanged).
  This is expected; pre-prod, the operator removes the old one and adds the new
  "GitHub". No resolver, no migration.
- T2 (cross-provider credential delivery) is the accepted, documented OpenShell
  limitation (#92) — not addressed here.

## Testing

- **Unit:** `derive()` sets `access: full` on every endpoint and renames
  id/display while preserving category and endpoints; `managed_profiles()` is
  `right-*` prefixed; `profile_catalog()` exposes `right-github` (full,
  `GITHUB_TOKEN`) and `github` as hidden; `handle_provider_types` omits hidden
  entries.
- **Live `ci_openshell_` (load-bearing risk gate):**
  1. **De-risk, no real token** — `ci_openshell_full_access_allows_post`: a
     throwaway provider on a public echo host with `access: full`; from the
     sandbox `POST` to it and assert it is **not** `403` (proves `full` permits
     POST on a terminated provider endpoint).
  2. **Real-token push gate** — `ci_openshell_github_push_succeeds`
     (env-gated on `RIGHT_TEST_GH_TOKEN` + a throwaway repo, operator-run):
     ensure `right-github` → create provider with the token → attach to a
     `TestSandbox` → `git push` a throwaway branch → assert success (not a
     `403 X-OpenShell-Policy`); redact the token from captured output. **This is
     the proof the whole feature depends on and has never been run green.**
- **Frontend:** SSR test that `ProvidersView` renders a flat list, never shows
  the hidden built-in `github`, and shows `right-github` as "GitHub". Remove the
  grouping tests.
- **Contingency** (documented, not pre-built): if the push gate fails,
  `access: full` is **not** the lever — the `github.com` git block is something
  else. Investigate, in order: (a) the `github.com` rest endpoint may need
  raw-tunnel (`tls: skip`) treatment for git's binary pack protocol rather than
  L7; (b) `codeload.github.com` / additional hosts; (c) a substitution-vs-L7
  ordering interaction. Re-open the design before proceeding.

## Non-goals

- Read/write toggle (revisit only when restrictive-mode github + OpenShell
  endpoint-scoped injection make it meaningful and enforceable).
- Migrating existing providers (pre-prod; re-add manually).
- Closing T2 (OpenShell roadmap, #92).
- `codeload.github.com`, Git LFS (archive/LFS are separate sandbox-tooling
  concerns; on permissive the catch-all already covers archive downloads).
- `aws`/`gcloud`/`kubectl` zero-token providers (README "next").

## Verification cadence

Targeted package/module unit tests during development (TDD red/green per slice);
the two `ci_openshell_` tests for the live path (the push gate run by the
operator with a throwaway token); one final `cargo test --workspace` from the
worktree before completion. No full-workspace runs after every task.
