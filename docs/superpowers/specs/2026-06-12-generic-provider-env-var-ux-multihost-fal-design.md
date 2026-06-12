# Generic-provider env-var UX + multi-host + built-in `Fal`

**Date:** 2026-06-12
**Status:** Design approved, pending spec review

## Problem

A user wants to add fal.ai as a provider. fal's API docs say to send
`Authorization: Key $FAL_KEY`. The dashboard generic-provider form has a
`Header name` field and microcopy implying RightClaw builds the auth header,
but there is no place for the `Key` scheme. The user cannot map the API's
auth doc onto the form.

## Root-cause finding (overturns the premise)

OpenShell static-credential injection is **verbatim placeholder substitution
keyed by env-var name**, not header construction. Per the OpenShell
providers-v2 docs: "Static `auth_style`, `header_name`, `query_param` ...
placement metadata is stored and validated, but static credential injection
still depends on environment placeholders generated from provider
credentials."

Consequences:

- The **agent** writes the real auth header. With `curl`, the agent emits
  `Authorization: Key $FAL_KEY`; `$FAL_KEY` holds the opaque placeholder, and
  the gateway proxy swaps the placeholder for the secret after TLS
  termination. The `Key` scheme is the agent's, not ours.
- `GenericProvider.header_name` (and the derived `auth_style`) is **inert for
  static keys**. It feeds only the OpenShell profile credential metadata;
  nothing in RightClaw reads it to build a request, and OpenShell does not use
  it for static injection. The hardcoded `auth_style="bearer"` in
  `author_generic_profile` has no effect on the wire.
- `auth_style: bearer` drives injection **only** on the dynamic token-grant /
  OAuth-refresh path, which fal (a static API key) does not use.

So there is no real "Bearer-only" limitation. The actual gaps are UX clarity
and host coverage.

`header_name`/scheme and `host` are orthogonal axes:

- **`header_name`/scheme** — how the auth header looks. Agent-controlled,
  inert in our config.
- **`host`** — load-bearing for two independent reasons: (1) the sandbox is
  default-deny egress, so an un-allowlisted host blocks CONNECT; (2)
  placeholder→secret substitution fires only on TLS-terminated L7 (`protocol:
  rest`) endpoints declared in policy. A missing/raw host means the
  placeholder leaks verbatim → upstream `401`.

fal spans several hosts, so every host the agent reaches with the key must be
allow-listed and terminated.

## Goals

1. Adding any static-API-key provider is obvious from reading the API's auth
   doc: paste the key, name the env var the doc references, list the host(s).
2. Manual generic providers support **multiple upstream hosts**.
3. fal.ai is a one-click built-in profile (hosts + env var pre-filled).
4. Existing generic providers keep working with no sandbox recreation.

## Non-goals (YAGNI)

- A scheme / auth-prefix field — inert for static keys.
- Per-host path prefixes.
- Dynamic token-grant / OAuth-refresh support for fal.

## Design

### 1. Data model — `right-agent-config::GenericProvider`

- **Remove** `header_name` from the struct. `GenericProvider` has no
  `deny_unknown_fields` and the field was `#[serde(default)]`, so existing
  `agent.yaml` files carrying `header_name:` deserialize fine (the field is
  ignored).
- Replace single `upstream_host: String` with multi-host. Back-compat:
  continue to accept a legacy single `upstream_host`, plus a new
  `upstream_hosts: Vec<String>`; normalize to a non-empty `Vec<String>` on
  read. Validation: at least one host.
- Keep `upstream_path_prefix: Option<String>` as a provider-wide prefix.

Implementation note: use `#[serde(try_from = "GenericProviderRaw")]` where the
raw struct carries optional `upstream_host: Option<String>` and
`upstream_hosts: Option<Vec<String>>`; the `TryFrom` folds them into one
canonical non-empty `Vec<String>` and **fails fast** if zero hosts result
(parity with today's required `upstream_host`).

`upstream_path_prefix` stays a single provider-wide value applied **uniformly
to every host** (default empty = allow all paths). Per-host paths are out of
scope (YAGNI); a user needing heterogeneous host paths leaves the prefix empty
or asks for a built-in profile.

### 2. Profile authoring + built-in `Fal` — `right-openshell::managed_profiles`

- `author_generic_profile` takes a **host slice** and emits one
  `NetworkEndpoint` per host (`protocol: rest`, `access: full`, auto-TLS —
  no deprecated `tls:` field). `header_name`/`auth_style` are fixed internal
  defaults (`Authorization` / `bearer`) for OpenShell profile validity only.
- New `ManagedProfile::Fal` — `Authored`, multi-endpoint — added to the
  `managed_profiles()` registry so it provisions on every `right up`, like
  `Github`.
- Add a `profile_catalog()` entry (`right_openshell::providers`): slug
  `right-fal`, `display_name: "fal.ai"`, `env_var: FAL_KEY`. This yields the
  one-click dashboard type (paste key only).
- **Scope of the built-in profile:** declare only fal's **authenticated API
  hosts** — the hosts that need the credential. Output-media CDN hosts
  (`*.fal.media`) and file-upload targets (e.g. GCS signed-URL PUTs) do **not**
  carry the credential and are **out of v1 scope**: bundling no-auth CDNs into a
  credential-injection profile needlessly terminates them and widens the global
  substitution surface (cross-provider leak, onsails/right-agent#92). Document
  as a known limitation: an agent that must download fal outputs adds those
  hosts to its general allowlist; a follow-up can add non-credential egress.
  Keeping the credential surface tight is the security-first default.
- **Research gate (do before merge):** confirm fal's exact authenticated host
  set and the env-var name from the official fal client source / API reference,
  not web docs alone. Candidates: `fal.run`, `queue.fal.run`,
  `rest.alpha.fal.ai`. Confirm whether the client reads `FAL_KEY` and/or
  `FAL_API_KEY`; if the latter, the catalog declares the canonical one and the
  agent guidance names it.

### 3. Agent-facing guidance — `provider_capabilities::build_usage_hint`

Rewrite the hint to name the env var(s) and instruct the agent to write the
auth header itself per the API docs, e.g.
`-H "Authorization: Key $FAL_KEY"`. The current "gateway substitutes
automatically" wording understates the curl workflow.

### 4. Dashboard UX — `ProvidersView.vue`, `providersViewModel.ts`, `ProviderTypeList.vue`

- Add/Edit generic: **remove the `Header name` field**; turn `Upstream host`
  into a repeatable host list (validate ≥1). Add form microcopy: "The agent
  references the key as `$ENV_VAR` and writes the auth header itself, exactly
  as the API docs say. RightClaw stores the secret and allows the host(s)."
- `ProviderTypeList`: surfaces the catalog's `fal.ai` type.
- Remove the `HEADER_NAME_MICROCOPY` constant; add a pure hosts-list
  validator helper with a direct unit test. Loading/empty/error stay on
  `AsyncState`; grouped lists on `CollapsibleSection` (frontend contract).

### 5. CLI — `right agent providers add`

- Multi-host: repeatable `--upstream-host`. Keep `--header-name` as a
  **hidden, deprecated, accepted-but-ignored** flag for one release so the
  non-interactive automation added in #116 does not break; it no longer affects
  anything. Multi-host MUST be CLI-exposable per the config-exposure rule.

### 6. Internal API + agent.yaml write — `internal_api_providers.rs`

- Create/update requests accept `upstream_hosts`; a single `upstream_host`
  stays accepted for back-compat. Validate **each** host with the existing
  per-host rules (block private/link-local/loopback ranges, warn on plain
  HTTP) — same guard applied N times, no relaxation for multi-host. Drop
  `header_name` branching. The `agent.yaml` writer emits the host list and no
  longer writes a `header_name` line.
- Profile create/update calls the multi-endpoint `author_generic_profile`.
- **Composition confirmation must verify every declared host.**
  `provider_is_composed_with_endpoint` / `wait_for_provider_composed_with_endpoint`
  check a single host/path today; a multi-host update would pass on the
  unchanged first host while later hosts silently failed to compose. Add an
  all-hosts variant (confirm each declared host/path appears in the composed
  `_provider_<name>` rule) and use it for generic create/update and supervisor
  reconcile.

### 7. Upgrade / migration

- Existing single-host + `header_name` providers keep working (back-compat
  deserialize; `header_name` ignored, already inert). They adopt multi-host on
  the next dashboard edit. Profiles live gateway-side and recompose via the
  existing `reconcile_for_sandbox`. `Fal` provisions on the next `right up`.
  **No sandbox recreation.**

### 8. Testing (TDD)

- **Unit:** `author_generic_profile` emits N endpoints for N hosts;
  `GenericProvider` deserializes both legacy single-host and new multi-host;
  new `build_usage_hint` text; viewModel hosts validator; catalog contains
  `right-fal`; `managed_profiles()` contains `Fal`.
- **Live `ci_openshell` (de-risks the root finding):** build on the existing
  harness in `crates/right-openshell/tests/ci_openshell_provider.rs`
  (`poll_sandbox_env`, throwaway providers/fake creds) — do **not** invent new
  infra. A generic provider with an env var on a terminated host; the agent
  runs `curl -H "Authorization: Key $ENV"` against a controllable terminated
  endpoint; assert the upstream received `Authorization: Key <real-secret>`
  (scheme client-written, placeholder substituted) and that a multi-host
  provider composes **all** hosts. `ci_openshell_` test-name prefix,
  `#[ignore = "ci-openshell: ..."]`. If a host-controlled echo endpoint proves
  infeasible under TLS termination, fall back to the established fake-cred /
  effective-policy observation the existing tests already use, and assert
  composition + env injection rather than the echoed header.
- **Final:** `cargo test --workspace`.

## Risks & open items

- **Root finding is doc+code-derived, not yet live-verified.** The live test
  above is the gate; if it reveals OpenShell auto-injects for static creds in
  some path, the "header_name is inert" claim and the drop-the-field decision
  must be revisited before merge. Sequence the live spike early.
- **Existing generic providers re-author their profile once on upgrade**
  (header_name default changes from e.g. `X-Api-Key` to `Authorization`). The
  profile fingerprint diff fires a one-time network-only policy reload — inert
  on the wire, no sandbox recreation. Expected, harmless, worth noting.
- **fal host list** is the only external unknown; gated by the research step.
- **Multi-host widens the terminated-host set** subject to the documented
  global-substitution limitation (#92). No new boundary vs single-host generic
  (operator already chooses hosts); mitigated by identical per-host validation.

## Verification criteria

- Dashboard generic add/edit has no Header-name field and accepts multiple
  hosts; form copy explains the env-var model.
- `fal.ai` appears as a one-click provider type; pasting a key attaches a
  working provider whose hosts are composed into the effective policy.
- A sandboxed agent reaches fal via `curl -H "Authorization: Key $FAL_KEY"`
  and gets a non-401 response (live test).
- Existing generic providers load and function unchanged; no sandbox
  recreation triggered.
- `cargo test --workspace` green.
