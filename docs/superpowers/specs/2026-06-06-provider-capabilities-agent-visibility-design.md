# Provider-capabilities visibility for the agent

**Date:** 2026-06-06
**Status:** Design approved, pending spec review

## Problem

A sandboxed agent sees provider credentials only as opaque placeholder env
vars (e.g. `GITHUB_TOKEN=openshell:resolve:env:v…_GITHUB_TOKEN`). It has no
way to know:

1. that such an env var is a gateway-managed provider credential,
2. which binaries are allowed to "cash in" the placeholder (the gateway
   substitutes the real secret only for requests from the profile's
   `binaries` allowlist hitting a TLS-terminated endpoint), and
3. which hosts the credential is valid for.

The built-in `right-github` profile scopes substitution to `gh`/`git` only.
When the agent reaches `api.github.com` via a non-allowlisted binary
(`curl`, `node` fetch, python), the request falls through to the permissive
`outbound` raw tunnel where no substitution happens — the literal
placeholder is sent and GitHub returns `401 Bad credentials`. The agent
then **misdiagnoses** this as an expired PAT and asks the user to rotate the
token, when in fact `gh`/`git` work and the credential is valid.

### Confirmed diagnosis (agent `right`, sandbox `test-sandbox-20260516-1640`)

- `gh api user` → `200`, returns `right-bot`. Substitution works, PAT valid.
- `git ls-remote https://github.com/onsails/right-agent` → success.
- `curl https://api.github.com/user` → `401`. Binary not in the provider
  rule's `binaries` list → routed via raw `outbound` tunnel → no injection.

The effective policy already contains a correctly composed
`_provider_right_right_github` rule (binaries `gh`/`git`, endpoints
`api.github.com:443`, `github.com:443`). The bug is **agent knowledge**, not
policy composition or credentials.

The binary scoping is intentional and kept: it limits which sandbox
processes can spend the GitHub token (a meaningful supply-chain boundary —
e.g. a malicious `npm postinstall` cannot exfil via GitHub). The fix is to
**teach the agent**, not widen the allowlist.

## Goals

- Give the agent a way to learn, for its own sandbox, which binaries can use
  which credential and on which hosts.
- Give the agent baseline conceptual understanding of the "provider" entity.
- Stop the 401 → "bad credential" misdiagnosis.

## Non-goals (YAGNI)

- Broad policy introspection (full filesystem/network view) — a narrow
  provider-capability tool only. Can be promoted later if a real need
  appears.
- Live credential validation (making a probe request).
- Changing the OpenShell gateway's behavior when an unsubstituted
  placeholder is sent (out of our control).
- Widening the `right-github` `binaries` allowlist.

## Design

Hybrid: a thin pointer + conceptual note in the prompt, with accurate detail
fetched on demand via a new MCP tool.

### 1. New built-in MCP tool `mcp__right__provider_capabilities`

- Read-only, routed to `RightBackend` (unprefixed built-in tool).
- **Scope is server-enforced.** The agent supplies **no arguments** — not a
  sandbox name, not a provider name. Scope resolves from the invocation
  context, exactly like `thread_search` / `forum_topic_list`. The agent
  cannot query another sandbox.
- Returns the list of providers attached to the agent's own sandbox; per
  provider:
  - `display_name` — user-friendly name (e.g. "GitHub"), never raw slug.
  - `env_vars` — names of placeholder env vars actually present in the
    sandbox.
  - `allowed_binaries` — binaries that can use the credential.
  - `endpoint_hosts` — hosts the credential is valid for.
  - `usage_hint` — one line, e.g. "Use `gh`/`git`; the gateway injects auth
    on api.github.com/github.com. Do not put this env var into curl/fetch."
- **Never** returns credential or placeholder *values* — only env-var names
  and non-secret fields.

### 2. Data source — effective policy (fact, not intent)

A read helper lives in `right_openshell::providers`
(`provider_capabilities_for_sandbox(sandbox)`), correlating three live
sources:

- **Effective sandbox policy** over gRPC → parse `_provider_<name>` network
  rules → the `binaries` and `endpoint_hosts` that *actually* govern
  requests right now (catches composition drift).
- **`GetSandboxProviderEnvironment`** → the real placeholder env-var names
  the sandbox currently sees.
- **ListProviders / attachments + profile** → `display_name` and the
  `_provider_<name>` ↔ provider correlation.

Profile operations go through `right_openshell::managed_profiles`; policy
read goes through `right_openshell::openshell`. No new convention bypasses.

Reading the effective policy (not the authored profile) follows the
project's "debuggability over convenience: use the direct observable signal"
rule — composition drift is itself a plausible root cause of such 401s, so
the factual slice is the more valuable one.

### 3. Prompt (thin pointer + concept)

In `OPERATING_INSTRUCTIONS.md`, within the prompt-tier brevity budget
(2–3 sentences):

- **Concept:** providers are gateway-managed credentials; in env they appear
  as opaque placeholders; only specific binaries can use them on specific
  hosts, and the gateway substitutes the secret on the outbound request; do
  not paste a placeholder into arbitrary HTTP clients.
- **Trigger:** on a `401`/`403` from a provider endpoint, call
  `mcp__right__provider_capabilities` before concluding the credential is
  invalid.

Also: the tool's own description, the aggregator `with_instructions()` (per
the MCP convention), and `PROMPT_SYSTEM.md` are kept in sync.

### 4. Aggregator wiring

- Register `provider_capabilities` in the `RightBackend` tool dispatch.
- Update `with_instructions()` to list the tool and its description.
- The tool resolves the agent's sandbox from the same per-invocation context
  used by the other scoped tools — no agent-supplied scope.

## Testing

- **Unit:** parse `_provider_<name>` rules from a sample effective-policy
  payload → correct `binaries` / `endpoint_hosts`; correlation
  provider ↔ rule ↔ env-var.
- **Live `ci_openshell_`:** attach a provider to a `TestSandbox`, call the
  helper, assert the returned binaries/endpoints/env match the composed
  policy. Reuse the generic-provider test harness
  (`ci_openshell_generic_provider.rs`).
- **MCP-level:** the tool scopes from context and rejects/ignores any
  agent-supplied sandbox argument; never emits credential values.

Cadence: targeted package tests during implementation; one
`cargo test --workspace` at the end.

## Security

- Server-enforced scope to the agent's own sandbox; no agent-supplied
  sandbox/provider arguments.
- Zero write surface; read-only.
- Credential/placeholder values never returned — only env-var names and
  non-secret capability metadata.

## Out of scope / explicitly deferred

Broad policy introspection (B2), live credential validation, gateway error
shaping, and any change to the `right-github` binary allowlist.
