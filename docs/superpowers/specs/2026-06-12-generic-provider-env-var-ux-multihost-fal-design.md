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

Implementation note (verify in plan): a custom deserialize or a normalizer
that folds `upstream_host` + `upstream_hosts` into one canonical
`Vec<String>`, so the rest of the code reads a single list.

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
- **Research gate (do before merge):** confirm fal's exact host set and the
  env-var name from the official fal client source / API reference, not web
  docs alone. Candidates: `fal.run`, `queue.fal.run`, `rest.alpha.fal.ai`,
  output media (`*.fal.media` / `v3.fal.media`). Confirm whether the client
  reads `FAL_KEY` and/or `FAL_API_KEY`.

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

- Multi-host: repeatable `--upstream-host` (or CSV). Remove `--header-name`
  (inert). Multi-host MUST be CLI-exposable per the config-exposure rule.

### 6. Internal API + agent.yaml write — `internal_api_providers.rs`

- Create/update requests accept `upstream_hosts`; a single `upstream_host`
  stays accepted for back-compat. Validate each host; drop `header_name`
  branching. The `agent.yaml` writer emits the host list and no longer writes
  a `header_name` line.
- Profile create/update calls the multi-endpoint `author_generic_profile`.

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
- **Live `ci_openshell` (de-risks the root finding):** a generic provider with
  an env var on a terminated host; the agent runs
  `curl -H "Authorization: Key $ENV"`; assert substitution fires and the
  upstream receives the real key. Confirms the scheme is client-side and
  multi-host composition works. `ci_openshell_` test-name prefix,
  `#[ignore = "ci-openshell: ..."]`.
- **Final:** `cargo test --workspace`.

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
