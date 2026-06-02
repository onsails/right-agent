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
with `- host: <upstream_host>` stanzas for placeholder substitution.
`handle_provider_create` and `handle_provider_config_update` reject
generic operations with `network_policy_forbids_generic` when the agent
is in restrictive mode. Built-in providers are unaffected — they do not
mutate `policy.yaml`.

## Placeholder substitution

At sandbox boot, the OpenShell supervisor calls
`GetSandboxProviderEnvironment` and injects the result as environment
variables on the sandbox supervisor process. The values are opaque
placeholders shaped like `openshell:resolve:env:v<fingerprint>_<NAME>`,
where `<fingerprint>` is derived from the provider credential inputs.
Every process spawned inside the sandbox — including `claude -p` over
gRPC exec and SSH — inherits these env vars at the kernel level. The
sandbox only ever sees the placeholder, never the raw credential;
`GetSandboxProviderEnvironment` returns the resolved value but is for the
trusted host/supervisor, not the sandbox.

Attaching or detaching a provider, and rotating a credential, all
propagate to a **running** sandbox without a restart: the placeholder set
(and, on rotation, the `<fingerprint>`) updates for newly-spawned
processes. Propagation is eventually-consistent, not instantaneous —
empirically sub-second for an attach and several seconds for a rotation —
so code/tests that read the env immediately after the gateway call must
poll, not read once (see `ci_openshell_provider.rs::poll_sandbox_env`).

When the agent makes an HTTPS request through the gateway proxy
(`HTTPS_PROXY=10.200.0.1:3128`, injected at sandbox boot), the proxy
substitutes the placeholder with the real credential before forwarding
upstream. Substitution happens **after TLS termination**, so the request
must hit a TLS-terminated L7 endpoint (`protocol: rest`, auto-detected —
do not write the deprecated `tls: terminate`). If the request is instead
handled by a raw-tunnel endpoint (`tls: skip`), the proxy never
terminates TLS and never inspects the request, so it forwards the bytes
**verbatim**: the opaque placeholder string reaches the upstream and the
API rejects it (typically `401`). The real credential is never exposed —
only the meaningless `openshell:resolve:env:...` token leaks. (Empirically
confirmed on a throwaway sandbox: the upstream cert was the real CA, not
the per-sandbox OpenShell CA, and the placeholder was echoed back
unchanged.)

**Substitution is keyed by env-var name, not by host.** The proxy
resolves a placeholder on any TLS-terminated endpoint using the combined
set of the sandbox's attached-provider credentials — it does **not** check
that the destination host belongs to the provider that owns the
credential. So a placeholder for provider A is substituted even if it
appears in a request to provider B's terminated host (verified live with
two throwaway providers + fake creds). Raw-tunnel (`tls: skip`) hosts
never substitute (above), so credentials cannot reach the open internet,
but cross-provider delivery among an agent's own terminated hosts is
possible. Do not rely on provider-profile endpoints to confine a
credential. This is a documented OpenShell limitation (see the
[providers-v2 docs](https://docs.nvidia.com/openshell/sandboxes/providers-v2)
— endpoint-scoped credential injection is roadmap, not implemented);
tracked in onsails/right-agent#92.

**Endpoint ordering is load-bearing.** OpenShell evaluates
`network_policies.outbound.endpoints` in order. In permissive mode the
hostless `tls: skip` catch-all (ports 443/80, broad `allowed_ips`) would,
if it appeared first, IP-match and raw-tunnel every provider host —
stranding the placeholder exactly as above. Provider host L7 endpoints
are therefore emitted **before** the catch-all: the
`# right-providers: insert-above` anchor sits at the **top** of the
endpoints list in `permissive_endpoints()`
(`crates/right-codegen/src/policy.rs`), so appended stanzas precede the
catch-all and win the match. IP carve-out alone does not help — removing
the covering range while the catch-all stays first turns the leak into a
CONNECT 403. `permissive_provider_endpoint_precedes_tls_skip_catch_all`
enforces this.

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
`# right-providers: insert-above` emitted at the **top** of
`network_policies.outbound.endpoints` by `generate_policy(Permissive)`.
The anchor's position is load-bearing twice over: it pins generic
provider stanzas to the outbound (permissive) section (without it, a
naive "find first `endpoints:`" heuristic would land in whichever
sub-section appears first under `network_policies:`), and it places them
*before* the hostless `tls: skip` catch-all so the proxy TLS-terminates
and substitutes (see "Endpoint ordering is load-bearing" above).

2. Look for an existing `endpoints[]` entry matching `upstream_host`.
   - Absent → append a new stanza: `host: <host>`, `port: 443`,
     `protocol: rest`, `access: full`, optional `path: <prefix>`
     (OpenShell rejects `domain:` as an unknown endpoint field). Tag with
     a YAML comment
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

### Durability across full regen

The Path-B on-add mutation above patches a single stanza onto the live
policy, but a full `policy.yaml` regeneration (bot start, `right
restart`, host reboot, the supervisor recovery loop, or a
`config_watcher` restart) rebuilds the file from scratch. To stop
generic-provider stanzas from being wiped on every regen — which strands
the credential placeholder on a raw tunnel and surfaces as an upstream
401 — **every** full regen MUST fold providers back in via
`right_codegen::policy::apply_provider_stanzas(&generate_policy(...),
providers)`. Callsites: `sandbox_supervisor::bring_up_sandbox`,
`right_codegen::pipeline::run_single_agent_codegen`, and the
`right/src/main.rs` init helpers (`write_bootstrap_right_mcp_policy`,
`apply_exact_right_mcp_policy_for_sandbox`). `apply_provider_stanzas`
is a no-op on a restrictive (anchorless) policy and idempotent on an
already-folded one. The network policy is thus reconstructable from
`agent.yaml` alone.

Every regen callsite renders through
`right_codegen::policy::generate_provider_aware_policy(...)` (the single
`generate_policy` + `apply_provider_stanzas` composition) rather than
calling `generate_policy` bare.

A `sandbox.providers`-only edit to `agent.yaml` no longer forces a
restart: `config_watcher` classifies it `ProvidersReload` and signals
`sandbox_supervisor::hot_reconcile_providers`, which re-renders the
provider-aware policy with resolved host IPs, hot-applies it (`openshell
policy set --wait`), and reconciles gateway attach/detach. The lib.rs
consumer retries the hot path with bounded backoff. There is no periodic
provider reconcile, so if it still fails the *live* sandbox policy stays
stale until the next bot restart or sandbox bring-up — re-edit
`sandbox.providers` or restart to retry. The *on-disk* policy is always
correct (every full regen folds providers back in), so a restart fully
self-heals.

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

## Managed profiles (RightClaw-owned)

`right_openshell::managed_profiles` owns a small set of gateway provider
profiles whose names carry the `right-*` ownership prefix and are
maintained entirely by the platform — never created, edited, or deleted by
the user.

**`right-github`** is the first managed profile and the single GitHub
provider users add. It is derived from the `github` built-in profile by
copying its endpoints and credential set, then setting every endpoint's
`access` preset to **`full`** (rename id/display to `right-github` /
"GitHub"). Deriving — rather than authoring — keeps it drift-proof: the
managed profile is always re-derived from whatever the live `github`
profile contains at the time `ensure_profiles` runs, so it tracks any
upstream change to the base endpoints.

**Why `access: full`.** Once credential injection forces TLS termination
on the GitHub endpoints, `access` and `rules` are method-level and
mutually exclusive. The `github` built-in ships `access: read-only`, which
blocks every non-GET/HEAD method — including git push (a POST to
`git-receive-pack`), rejected with a gateway 403. The `full` preset
permits all methods, unblocking fetch _and_ push, without hand-authoring
allow-all `rules` (the two are exclusive — `derive` clears `rules` and
sets the preset).

**One provider, not a toggle.** Providers are a credential-injection
mechanism, not a read/write boundary — and on the recommended `permissive`
network policy a read-only distinction is not a real security boundary
anyway (the catch-all raw tunnel already carries non-provider traffic).
So there is no read/write toggle: `right-github` is full-access, and the
read-only built-in `github` is kept in the catalog (so already-provisioned
`github` providers still resolve their `GITHUB_TOKEN` env var) but hidden
from the dashboard add-list via `HIDDEN_FROM_DASHBOARD` in
`internal_api_providers.rs`. Credential confinement (a provider token must
only reach that provider's hosts) is **not** host-scoped at runtime — an
accepted OpenShell limitation tracked in `onsails/right-agent#92`; this
profile does not change that posture.

**`ensure_profiles` reconcile.** Called once per gateway at `right up`,
before the reconciler attaches providers to agent sandboxes.

- If the base `github` profile does not exist on this gateway,
  `ensure_profiles` logs a warning and returns `EnsureOutcome::Skipped` for
  `right-github` — a non-fatal per-profile skip, never an abort of
  `right up`. Real gRPC errors still propagate (FAIL FAST).
- The fingerprint used for idempotency includes both `access` and `rules`;
  a profile still on the old `read-only` preset is detected as drift and
  re-imported, while an already-correct profile is left untouched.
- No auto-GC in v1: if `right-github` is present on the gateway but no
  agent is using it, it stays. Cleanup is a manual operator action.

**Path A purity.** Like all built-in providers, `right-github` follows
Path A: the gateway contributes its endpoints to the effective sandbox policy
automatically on attach. `policy.yaml` for each agent is never touched.
Git LFS is a separate sandbox-tooling concern and is out of scope for this
subsystem.
