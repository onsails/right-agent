# Providers

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Overview

Providers are typed credential bundles stored on the NVIDIA OpenShell
gateway and attached to sandboxed agents. Each provider has a
gateway-unique name, a gateway type, a credentials map, and an optional
non-secret config map. Non-generic providers use explicit profile slugs,
including upstream slugs such as `anthropic`, `openai`, or `gitlab` and
Right-managed slugs such as `right-github`; generic providers are
displayed as `generic` in Right's dashboard and `agent.yaml`, but the
gateway provider `type` is the Right-authored profile ID
(`right-provider-*`). Right Agent
exposes provider management exclusively through the Telegram Mini App
dashboard route `/providers`; credentials never enter `agent.yaml`,
backups, or logs on the host.

The feature is sandbox-only. `sandbox.mode = none` agents cannot
receive provider env vars; the bot rejects `/providers` for them.

Generic providers additionally require `network_policy: permissive`.
Right authors OpenShell provider profiles for generic upstream hosts and
relies on those profile endpoints being composed into the sandbox's
outbound policy for placeholder substitution. Restrictive mode has not
been validated for generic provider-profile composition and is rejected
up-front.
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

**Endpoint ordering is load-bearing.** OpenShell evaluates the effective
`network_policies.outbound.endpoints` in order. In permissive mode the
hostless `tls: skip` catch-all (ports 443/80, broad `allowed_ips`) would,
if it appeared before provider-profile L7 endpoints, IP-match and
raw-tunnel every provider host — stranding the placeholder exactly as
above. Right's generated `policy.yaml` is intentionally provider-free:
there is no `# right-providers: insert-above` anchor and no folded
provider stanza. Ordering now comes from OpenShell's provider-profile
composition, and Right forces that composition to reload after profile or
attachment changes by reapplying the base policy with `openshell policy
set --wait`. That command is only a reload trigger: OpenShell can no-op
when the policy hash is unchanged, so its return is not the composition
success signal.

That composition is itself gated by the gateway-global runtime setting
`providers_v2_enabled`, which fresh OpenShell gateways default to `false`.
While off, the gateway silently skips merging provider-profile endpoints —
the placeholder env var is still injected, but the proxy denies CONNECT to
the upstream because the terminated endpoint never appears in the effective
policy. Right guarantees the flag through two funnels:
`right_openshell::providers::reconcile_for_sandbox` for supervisor bring-up
and hot-reconcile, and the dashboard create/config-update handlers for
explicit user mutations. Both call
`right_openshell::providers::ensure_v2_enabled` (a global `UpdateConfig`
upsert) before attaching or recomposing a provider. The flag persists in
the gateway's settings store, so a long-lived dev gateway that was enabled
once keeps working — which is why missed enablement tends to surface only
on fresh gateways.

`openshell::wait_for_provider_composed` is the success signal after the
reload. It polls the sandbox's **effective** policy (`get_effective_policy`,
backed by `GetSandboxConfig` — the policy the in-sandbox supervisor pulls)
and requires the composed `_provider_<name>` rule to appear. This is
deliberately *not* the stored policy revision (`get_active_policy` /
`GetSandboxPolicyStatus`): authored generic provider rules are merged only
into the effective policy and never appear in the stored revision, so reading
the stored revision would time out for every generic provider even though
substitution works. If a future attach path misses the v2 enable step, this
wait times out and fails loudly instead of silently letting users discover
upstream 401/CONNECT failures later.
Generic provider create/config-update and supervisor reconciles use the
stricter endpoint-aware variant, requiring the composed rule to contain the
expected upstream host/path so a stale pre-update rule cannot pass.

## State of truth split

Two stores, both authoritative for different things:

| What                              | Where                                      |
| --------------------------------- | ------------------------------------------ |
| Per-agent list of attached names  | `agent.yaml::sandbox::providers: [...]`    |
| Credential bytes                  | OpenShell gateway (write-once via Right)   |
| Non-secret provider config        | OpenShell gateway                          |
| Managed/generic provider profiles | OpenShell gateway                          |
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
2. If the provider exists with the legacy generic gateway type
   `type = "generic"`, call `UpdateProvider` with
   `type = right_openshell::managed_profiles::generic_provider_profile_id(name)`,
   empty credentials, and empty config. This preserves the gateway-held
   credential bytes while moving already-deployed agents to the current
   provider-profile shape. Built-in providers and already-migrated generic
   providers are left unchanged.
3. If not currently attached to the sandbox, call
   `Sandbox.provider.attach`.

Then for each provider currently attached to the sandbox whose name
starts with `<agent>-` but is absent from `agent.yaml`: call
`Sandbox.provider.detach`.

The reconciler returns a `ReconcileReport { attached, detached,
repaired, missing, errors }` per agent which is surfaced in logs and to
callers.

## Policy interaction

Right no longer folds provider endpoints into `policy.yaml`. Every
generation callsite renders the provider-free base policy with
`right_codegen::policy::generate_policy(...)`; the policy tests assert
that permissive policy output contains no `right-providers` anchor or
managed provider stanza.

Built-in providers use OpenShell's existing profiles. Generic providers
use Right-authored OpenShell profiles whose IDs are derived from the
gateway provider name (`right-provider-...`). The gateway provider's
`type` is the profile ID, while the dashboard and `agent.yaml` continue
to expose the provider as `generic`.

On create or upstream-host/config change, Right authors/imports the
generic provider profile, creates or updates the gateway provider against
that profile ID, and calls `ensure_provider_policy_loaded(sandbox,
policy_path)`. That helper reapplies the current base `policy.yaml` with
`openshell policy set --wait`; it does not write provider stanzas. The
reload is required because OpenShell provider-profile composition is not
fully loaded by attach/import alone on the observed v0.0.56 behavior, but
composition is confirmed only by effective-policy polling (`GetSandboxConfig`)
for the composed provider rule. This scope covers built-in and generic
providers; both rely on OpenShell profile composition.
Built-in creates skip profile authoring but still attach, reload
composition, and wait for their composed provider rule before `agent.yaml`
is updated.

On remove, Right detaches and deletes the gateway provider, removes the
`agent.yaml` row, and reloads provider-profile composition with
`ensure_provider_policy_loaded`. Generic providers additionally run the
legacy folded-policy strip path; new composition-based policies have no
tagged stanza, so this is normally a no-op. It exists to clean up
already-deployed policies that still contain
`# managed-by: right-providers:<provider-name>` stanzas.
Folding stays removed; the legacy strip path is cleanup-only.

A `sandbox.providers`-only edit to `agent.yaml` no longer forces a
restart: `config_watcher` classifies it `ProvidersReload` and signals
`sandbox_supervisor::hot_reconcile_providers`, which ensures generic
profiles exist, reconciles gateway attach/detach, and reloads
provider-profile composition with `openshell policy set --wait`, then
relies on active-policy composition checks where a provider attach must be
confirmed. The lib.rs consumer retries the hot path with bounded backoff.
There is no periodic provider reconcile, so persistent failure can leave
the live sandbox's attachment/composition state stale until the next bot
restart or sandbox bring-up — re-edit `sandbox.providers` or restart to
retry.

## Lifecycle

**Create.** Generic providers run: author/import profile →
`CreateProvider` with the profile ID as gateway type →
`Sandbox.provider.attach` → `ensure_provider_policy_loaded` →
endpoint-aware `wait_for_provider_composed` → write `agent.yaml`. Built-in
providers skip the profile-authoring step and use rule-presence
composition confirmation. Any failure triggers ordered rollback: a failed
`attach` removes the freshly created provider; a failed policy-load,
composition-confirmation, or `agent.yaml` write triggers best-effort detach
and delete, then a rollback reload.

**Rotate.** `UpdateProvider` only. No sandbox restart. The gateway
issues a new placeholder version; the next outbound request from the
sandbox carries the new placeholder and resolves to the new
credential.

**Edit non-secret config.** Generic providers only. Re-author/import the
profile, `UpdateProvider`, `ensure_provider_policy_loaded`,
`wait_for_provider_composed` with the expected endpoint host/path, then
write `agent.yaml`. A failed profile import, gateway update, policy-load,
composition confirmation, or YAML write triggers rollback of the gateway
provider/profile as applicable plus a rollback reload. The `env_var` is
stable after creation unless a credential is supplied through a
rotate/update path that can update gateway credentials consistently.

**Remove.** `Sandbox.provider.detach` → `DeleteProvider` → remove the
`agent.yaml` row → reload provider-profile composition. Generic providers
also run legacy folded-policy cleanup.

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

**`right-provider-*`** profiles are per-generic-provider authored
profiles. The stable profile ID is derived from the gateway provider name
with a sanitized slug plus hash suffix so two provider names that
normalize to the same slug still get distinct profile IDs. Each profile
contains the generic provider's L7 endpoint (`host`, port 443,
`protocol: rest`, optional path prefix), credential env-var shape, and a
`binaries` entry with `path: "**"`. Without the binary wildcard, OpenShell
does not match sandbox commands to the provider profile and CONNECT can be
blocked before placeholder substitution. Right imports these profiles
before create/update and at `right up` for already-configured agents.

**Provider-profile purity.** Like all built-in providers, `right-github`
relies on the gateway to contribute its endpoints to the effective sandbox
policy automatically on attach. `policy.yaml` for each agent is never
touched. Git LFS is a separate sandbox-tooling concern and is out of scope
for this subsystem.
