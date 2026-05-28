# Providers

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Overview

Providers are typed credential bundles stored on the NVIDIA OpenShell
gateway and attached to sandboxed agents. Each provider has a
gateway-unique name, a type slug (`anthropic`, `openai`, `github`,
`gitlab`, `nvidia`, `codex`, `copilot`, `opencode`, or `generic`), a
credentials map, and an optional non-secret config map. Right Agent
exposes provider management exclusively through the Telegram Mini App
dashboard route `/providers`; credentials never enter `agent.yaml`,
backups, or logs on the host.

The feature is sandbox-only. `sandbox.mode = none` agents cannot
receive provider env vars; the bot rejects `/providers` for them.

Generic providers additionally require `network_policy: permissive`.
Restrictive mode renders only `network_policies.anthropic.endpoints`
(the Anthropic/Claude allowlist) and has no outbound section to extend
with `- domain: <upstream_host>` stanzas for placeholder substitution.
`handle_provider_create` and `handle_provider_config_update` reject
generic operations with `network_policy_forbids_generic` when the agent
is in restrictive mode. Built-in providers are unaffected — they do not
mutate `policy.yaml`.

## Placeholder substitution

At sandbox boot, the OpenShell supervisor calls
`GetSandboxProviderEnvironment` and injects the result as environment
variables on the sandbox supervisor process. The values are opaque
placeholders shaped like `openshell:resolve:env:v<digits>_<NAME>`. Every
process spawned inside the sandbox — including `claude -p` over gRPC
exec and SSH — inherits these env vars at the kernel level.

When the agent makes an HTTPS request through the gateway proxy
(`HTTPS_PROXY=10.200.0.1:3128`, injected at sandbox boot), the proxy
substitutes the placeholder with the real credential before forwarding
upstream. Substitution happens after TLS termination, so the policy
endpoint must use `protocol: rest` (auto-TLS-terminate) or explicit
`tls: terminate`. If the placeholder is sent to a raw-tunnel endpoint
(`tls: skip`), the proxy cannot resolve it and rejects the request with
HTTP 500 — never forwarding the raw placeholder.

## State of truth split

Two stores, both authoritative for different things:

| What                              | Where                                      |
| --------------------------------- | ------------------------------------------ |
| Per-agent list of attached names  | `agent.yaml::sandbox::providers: [...]`    |
| Credential bytes                  | OpenShell gateway (write-once via Right)   |
| Non-secret provider config        | OpenShell gateway                          |
| Sandbox attachment state          | OpenShell gateway (`Sandbox.providers`)    |

`agent.yaml` wins on drift: the reconciler attaches anything in the
file that isn't currently attached, and detaches any extra
`<agent>-*` providers attached to the sandbox but missing from the
file.

## Reconciler walkthrough

Runs at `right up`, after the sandbox is READY and before the bot
starts serving messages.

For each entry in `agent.yaml::sandbox::providers`:

1. `GetProvider` against the gateway.
   - **Ok** → continue.
   - **NotFound** → mark the entry as `Status::Missing` (a "ghost"
     provider). Do not auto-heal: Right does not have the credential
     bytes. The dashboard surfaces these with a *Resolve* action.
2. If not currently attached to the sandbox, call
   `Sandbox.provider.attach`.

Then for each provider currently attached to the sandbox whose name
starts with `<agent>-` but is absent from `agent.yaml`: call
`Sandbox.provider.detach`.

The reconciler returns a `ReconcileReport { attached, detached,
missing }` per agent which is surfaced to the dashboard.

## Policy interaction

Two distinct paths into `policy.yaml`:

**Path A — built-in providers.** Right does not mutate `policy.yaml`.
The OpenShell gateway (v0.0.50+) contributes the profile's endpoints to
the effective sandbox policy automatically when a provider is attached.
Right's `policy.yaml` stays unchanged.

**Path B — generic providers.** Right owns the `policy.yaml` mutation.
On create or upstream-host change:

1. Load current `policy.yaml`.

The new stanza is inserted at the sentinel anchor
`# right-providers: insert-above` emitted inside
`network_policies.outbound.endpoints` by `generate_policy(Permissive)`.
This anchor pins generic provider stanzas to the outbound (permissive)
section; without it, a naive "find first `endpoints:`" heuristic would
land in whichever sub-section appears first under `network_policies:`.

2. Look for an existing `endpoints[]` entry matching `upstream_host`.
   - Absent → append a new stanza: `domain: <host>`, `protocol: rest`,
     `access: full`, optional `path: <prefix>`. Tag with a YAML comment
     `# managed-by: right-providers:<provider-name>` so future strip
     operations can find it.
   - Present with `protocol: rest` → no-op.
   - Present with `tls: skip` → refuse the operation with
     `PolicyConflict { kind: "raw-tunnel" }`. Right does not
     auto-rewrite; the user must resolve the conflict.
   - Present with `tls: terminate` (deprecated but functional) →
     no-op.
3. Write `policy.yaml`.
4. Hot-apply via `openshell policy set --wait`. This is the
   `Regenerated(SandboxPolicyApply)` codegen category — **never**
   `SandboxRecreate`. New endpoints are hot-reloadable.

On remove: if no other generic provider on the same agent uses the
same `upstream_host`, strip the tagged stanza and hot-apply. The strip
is idempotent — if the tag is absent, the policy is returned
unchanged.

## Lifecycle

**Create.** Generic providers run: write policy.yaml (with snapshot)
→ hot-apply → `CreateProvider` → `Sandbox.provider.attach` → write
`agent.yaml`. Built-in providers skip the policy steps. Any failure
triggers ordered rollback: a failed `attach` removes the freshly
created provider; a failed `agent.yaml` write triggers best-effort
detach + delete; a failed policy hot-apply restores the snapshotted
policy.

**Rotate.** `UpdateProvider` only. No sandbox restart. The gateway
issues a new placeholder version; the next outbound request from the
sandbox carries the new placeholder and resolves to the new
credential.

**Edit non-secret config.** Generic providers only. If
`upstream_host` changed, strip the old stanza and append the new one
(with snapshot). Then `UpdateProvider`. Then write `agent.yaml`.

**Remove.** `Sandbox.provider.detach` → `DeleteProvider` → if generic
and no other provider on the agent uses the same host, strip the
policy stanza and hot-apply. Then write `agent.yaml`.

**Ghost (post-restore).** When `agent.yaml` lists a provider that the
gateway doesn't have (typical after backup/restore to a new host),
the reconciler marks the row `Status::Missing`. The dashboard's
*Resolve* action either re-creates the provider with a fresh
credential or strips the entry from `agent.yaml`.

**Cascade on `right agent destroy`.** Before tearing down the sandbox,
Right iterates `agent.yaml::sandbox::providers` and calls
`DeleteProvider` on each. Failures are logged and skipped so destroy
proceeds; orphans clean up on the next `right up`.
