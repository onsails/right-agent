# Providers

> **Authority note (microsandbox migration).** The internal provider API and
> dashboard handlers run on `right_providers::ProviderStore`
> (`~/.right/providers.db`, SQLite 0600). The OpenShell gateway, its CRUD,
> profile-composition confirmation, and `wait_for_provider_composed*` flows
> are deleted. What that means:
>
> - **Authority.** Records and credentials live in `providers.db`; ownership
>   is the `owner_agent` column plus `provider_borrows` rows, never
>   `agent.yaml`'s `shared_from` (legacy migration input only).
> - **Wire shapes.** `ProviderView.status` is the tri-state
>   `{kind: ready|needs_value|error}` (error carries `message`); the
>   `composed: bool|null` field and the `healthy|missing|gateway_error|
>   unknown_builtin` statuses are gone. Error→HTTP mapping is unchanged
>   (409 `borrowed_read_only`/`copy_conflict`, 422
>   `source_credential_unreadable`, 500 `unknown_builtin_slug`, 403
>   `unauthorized`, …).
> - **Sharing.** `/provider-share` inserts a borrow row pointing at the true
>   owner and appends a definition-only entry to the destination's
>   agent.yaml; no secret is read back or copied. Owner deletion re-homes
>   the record to a surviving borrower.
> - **Redaction.** Store read APIs structurally carry no credential;
>   `ProviderStore::source_ref_binding` is the only reader and returns a
>   redacted binding whose value is consumed only by the sandbox apply path's
>   scoped, zeroizing SDK resolver. No process environment mutation occurs.
>

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Overview

Providers are typed credential bundles stored in `~/.right/providers.db` and
bound into an agent's sandbox as source-reference secrets. Each provider has
an agent-unique name, a type (a built-in catalog slug such as `anthropic`,
`openai`, `gitlab`, or the Right-managed `right-github`; or `generic`), a
credential, and — for generic providers — a declared env var and upstream
host list. Right Agent exposes provider management exclusively through the
Telegram Mini App dashboard route `/providers`; credentials never enter
`agent.yaml`, backups, or logs on the host.

A provider with no allowed hosts is rejected: its credential could never be
substituted, so accepting it would promise an injection that cannot happen.

## Placeholder substitution

A provider reaches the guest as a `right_sandbox::SecretBinding`, built by
`ProviderStore::source_ref_binding`. It carries:

- `env_var` — the guest-visible environment variable.
- `placeholder` — what the guest actually sees in that variable
  (`$MSB_<ENV_VAR>` by default). Stable across rotation.
- `source_env_var` — a durable owner-and-record-scoped source identity. It is
  persisted instead of plaintext and does not refer to process environment.
- `allowed_hosts` — exact hosts (`api.example.com`) or suffix wildcards
  (`*.example.com`) permitted to receive the substituted value.
- `inject_query` — opt-in query-parameter substitution. Headers and
  basic-auth are on by default; body injection is never enabled.
- a private `SecretString` credential, redacted from `Debug`, installed in the
  SDK's scoped resolver only while create/start/apply runs.

The SDK persists the placeholder and source identity, so no secret material is
at rest in the sandbox's durable config and the guest never holds a credential.
Substitution happens on the intercepted connection: the agent writes the auth
header exactly as the API documents, using the placeholder, and only the
placeholder token is replaced.

**TLS interception is a bypass deny-list (ADR-0003).** Declaring any secret
enables interception for every destination on the intercepted ports *except*
`right_sandbox::TLS_BYPASS_HOSTS`, which always carries the Anthropic hosts —
so Claude's own traffic is never intercepted and the guest needs no CA
configuration. A request to a bypassed host is forwarded verbatim, which
means the opaque placeholder reaches the upstream and the API rejects it
(typically `401`). The real credential is never exposed.

**Substitution is keyed by env-var name, not by the owning provider.** A
placeholder is resolved on any intercepted destination in the binding's
allowed-host set; nothing checks that the destination belongs to the provider
that owns the credential. Do not rely on a provider's declared hosts to
confine a credential across an agent's own providers. Tracked in
onsails/right-agent#92.

Because the SDK keys substitution by guest env var, an agent may declare at
most one provider per env var. Reconciliation rejects duplicate identities
(for example `github` plus `right-github`) before mutating the sandbox; otherwise
removing either record could ambiguously revoke the survivor.

**Secret structure is normally create-time, with SDK-managed upgrade and
revocation paths.** For an existing binding, `SandboxHandle::apply_secret`
computes host additions and removals, shrinks removed destinations while the
old credential is live, rotates, and widens additions only after rotation
succeeds. A rotation failure therefore cannot expose the old credential to a
new destination. If a provider was `NeedsValue` when the sandbox was created,
its first usable value adds the missing binding through
`modify().secret(...).restart().apply()`. That stop/persist/start cycle keeps
the same sandbox and preserves its writable filesystem. Removing a binding uses
`modify().remove_secret(name).apply()` and revokes substitution live in the
runtime while deleting the entry from durable config. Removing the final secret
disables TLS interception in desired config; the already-running VM keeps it
enabled (with no bindings or credentials) until its next start because SDK
0.6.10 has no live TLS toggle. microsandbox 0.6.10 cannot express query-parameter
injection on a new binding through `modify`; Right fails that addition rather
than silently installing weaker semantics.

## State of truth split

`providers.db` is the single authority. `agent.yaml` carries definitions
only:

| What                             | Where                                           |
| -------------------------------- | ----------------------------------------------- |
| Per-agent list of declared names | `agent.yaml::sandbox::providers: [...]`         |
| Credential bytes                 | `providers.db` (`providers.credential`, 0600)   |
| Non-secret provider config       | `providers.db`                                  |
| Ownership / borrowing            | `providers.owner_agent` + `provider_borrows`    |
| Live guest bindings              | sandbox config; live-rotated, added, or revoked |

`agent.yaml` wins on drift for *which* providers an agent declares; the store
wins on everything about a provider. `shared_from` in `agent.yaml` is legacy
migration input only — the internal API writers never emit it.

## Reconciler walkthrough

For each declared provider, `ProviderStore::source_ref_binding` resolves the
owning row and returns a redacted binding with an owner-scoped source identity.
The credential is installed into the vendored SDK's scoped resolver only for
the create/start/apply call; the guard removes and zeroizes it afterward.

- At create, ready bindings go into `agent_sandbox_spec`; `NeedsValue` records
  are deliberately skipped so migrated agents can start and receive their
  first value from the dashboard.
- On every bot bring-up, after the sandbox reports ready and before it is
  published Ready or synced, each ready binding goes through
  `SandboxHandle::apply_secret`. Existing bindings converge with the safe
  shrink→rotate→widen order, while missing non-query bindings are added with
  the SDK-managed restart path. `NeedsValue` records remain skipped, and any
  apply error fails bring-up.
- Config hot-reconcile verifies that obsolete live entries have both a known
  provider env var and a Right-minted hashed source identity before removal.
- Dashboard create, remove, share, borrow, and watcher/startup/recovery
  reconciliation take the destination agent's cross-process advisory provider
  lock before touching its sandbox. The lock path is under Right's home and is
  keyed by agent identity only; credentials never enter the path.
- Owner rotate and generic config-update enumerate the owner plus every
  borrower from `providers.db`, then resolve each holder's current config and
  sandbox under that destination lock. Success means every holder converged;
  post-commit failure names failed agents without including credentials.

The bot's process-local mutex serializes its config publication with recovery,
while the per-agent advisory file lock is authoritative across bot, aggregator,
watcher, startup, CLI, and cross-agent destination processes. A missing
declaration/store record, unavailable sandbox, or failed SDK apply returns an
error instead of falsely reporting that durable state is live everywhere.

A record whose built-in slug no longer resolves fails fast
(`unknown_builtin_slug`, HTTP 500) on rotation and config-update; the list
view marks such a row `error` rather than aborting the whole listing.

## Network-policy interaction

There is no generated policy file and no provider stanzas. An agent's
network stance is the `network_policy` field in `agent.yaml`, translated to a
`right_sandbox::Egress` value at sandbox create:

- `permissive` — open egress.
- `restrictive` — the domain-suffix allow list in
  `right_sandbox::agent::RESTRICTIVE_EGRESS_ALLOW`, plus the always-open host
  destination group.

Egress is create-time only, so a `network_policy` change needs a sandbox
recreate. Secret substitution is independent of egress: it happens on the
intercepted connection and is governed by the binding's `allowed_hosts` and
the TLS bypass list, not by the egress allow list.

A `sandbox.providers`-only edit to `agent.yaml` does not force a restart:
`config_watcher` classifies it `ProvidersReload` and signals
`sandbox_supervisor::hot_reconcile_providers`. The lib.rs consumer retries
the hot path with bounded backoff. Startup always runs reconciliation after the
sandbox reports ready. Hot reconcile uses the previously accepted config to
remove declarations absent from the new config; dashboard remove/unshare revoke
synchronously. A durable rotation self-heals on the next bot start.

## Lifecycle

**Create.** The internal API validates and stores the provider, then appends
its definition-only entry to `agent.yaml`. Bot bring-up resolves ready records
from `ProviderStore` into source-ref bindings. Migrated `NeedsValue` records
stay declared but unbound until their first dashboard rotation.

**Rotate.** The owner bot verifies durable state, then asks the aggregator to
write the new credential to `providers.db`. It enumerates every holder from the
store and applies the named provider using each holder's own config and sandbox
under that destination's advisory lock. The dashboard returns success only
after owner and borrower sandboxes converge; a post-commit failure names the
failed agents without exposing credential material.

**Edit non-secret config.** Generic-provider endpoint changes update the
store's non-secret definition, then fan out to every holder through the same
per-destination locked convergence path.

**Share/borrow.** Both mutations preflight the destination sandbox, commit the
borrow reference and destination `agent.yaml`, then directly apply the binding
to the destination before success. Cross-agent share does not assume another
bot process's watcher has converged the live sandbox.

**Remove/unshare.** The bot first preflights the target and live sandbox. The
internal API rewrites `agent.yaml` while retaining its exact original, removes
the store row/reference, and restores YAML if the store mutation fails so retry
sees the original declaration. Before HTTP success the bot explicitly revokes
the obsolete binding from the runtime and durable sandbox config.

**NeedsValue after migration.** The definition exists in both `agent.yaml` and
`providers.db`, but the store refuses to produce a binding until a real
credential is rotated in. That rotate immediately adds the binding to the
existing sandbox without deleting it.

**Cascade on `right agent destroy`.** Before tearing down the sandbox, Right
cascades the agent's provider rows in the store: owned records are removed
(re-homing to a surviving borrower if one exists), borrowed records are
unshared. Nothing is left orphaned in `providers.db`.

## Built-in catalog (RightClaw-owned)

`right_providers::catalog` is the compile-time built-in catalog: for each
slug, the env var the credential binds to, the display name, the category
rendered by `/provider-types`, the `allowed_hosts` the credential may be
substituted into, and whether query-parameter injection is enabled.

A superseded slug stays in the catalog with `hidden: true` so existing
records still resolve their env var, but `offered_catalog()` filters it out
of the dashboard's add-list. Today only `github` is hidden.

**`right-github`** is the single GitHub provider users add. The retired
gateway derived it from the `github` profile to open every HTTP method
(`access: full`), because the read-only base blocked git push — a POST to
`git-receive-pack`. Substitution has no method dimension now, so the two
entries differ only in visibility: `right-github` is offered, `github` is
kept for already-provisioned records.

**One provider, not a toggle.** Providers are a credential-injection
mechanism, not a read/write boundary, so there is no read/write toggle.
Credential confinement (a token reaching only its own provider's hosts) is
**not** enforced at runtime — substitution is keyed by env-var name, an
accepted limitation tracked in `onsails/right-agent#92`.

**`right-fal`** covers fal.ai's authenticated API hosts only (`fal.run`,
`queue.fal.run`, `rest.fal.ai`). Output-media CDN and upload targets are
intentionally outside the binding until their credential and network
behavior are verified.

**`generic`** is the escape hatch: its env var and hosts come from the
record's `GenericSpec`, never from the catalog. `claude` is a reserved slug
(the in-sandbox Claude Code login flow owns it) and is always rejected.

The catalog's slugs, env vars, display names, categories, and order are a
dashboard contract; `catalog_tests.rs` pins them.

## Cross-agent provider sharing (multi-attach)

A trusted operator can SHARE one provider account across several host-local
agents: the *same* `providers.db` record is bound into each agent's sandbox.
The secret stays in the store and is never read back into a second record.

**Why not copy.** The previous design copied the credential by reading it
back from the gateway, which **redacted** stored secrets — it returned the
literal string `"REDACTED"` — so the copy wrote `"REDACTED"` as the
destination credential and the proxy substituted it verbatim on egress,
yielding an upstream `401`. Copy-by-readback was unfixable and is retired.
Borrowing is the supported replacement: `provider_borrows` rows point at the
true owner, and `source_ref_binding` follows the borrow to read the owner's
credential at bind time.

**Naming & ownership.** A record's NAME no longer encodes its owner. New
records use an agent-agnostic `{type-slug}-{short-uuid}` id (e.g. `fal-a1b2c3`,
from `new_record_name`); existing `{agent}-{slug}` records keep their names.
`validate_name` accepts both forms. Ownership is store data — the
`owner_agent` column plus a `provider_borrows` row per borrower. Both agents'
`agent.yaml` simply declare the same record id:

```yaml
# owner (riskoff)               # borrower (right) — SAME record id
sandbox:                        sandbox:
  providers:                      providers:
    - name: fal-a1b2c3              - name: fal-a1b2c3

```

`ProviderEntry::is_owned()` / `is_borrowed()` drive dashboard read-only
state, rotation rights, reconcile, and the destroy cascade. Ownership is
authoritative in the store (`owner_agent` + `provider_borrows`);
`agent.yaml`'s `shared_from` is legacy migration input that the internal API
writers never emit. The borrower's entry carries the non-secret
`type`/`label`/`generic` so its binding names the right env var and hosts;
only the credential stays store-side.

**Discovery.** `provider_peers(actor_user_id, for_agent)` enumerates
host-local agents (excluding `for_agent`) where the actor is trusted,
returning each peer's providers (name, type, env_var, label, generic)
— never credentials. `build_peers` skips an unreadable peer `agent.yaml` with
a warning.

**Authorization.** The actor MUST be trusted in the allowlist
(`allowlist.yaml` `users[].id`) of BOTH agents. The dashboard proves identity
+ own-agent trust via `authenticate_api`; the host-only internal Unix-socket
API re-checks the other agent from disk (`require_trusted`; secure default =
deny). `actor_user_id` always comes from the authenticated user. The
dashboard "Share with…" action pins `owner_agent = current agent`,
`dest_agent = selected peer` (push). The mirror-image "Borrow…" action
(`handle_borrow` → same `provider_share` call) pins `owner_agent = selected
peer`, `dest_agent = current agent` (pull). Direction is purely which dashboard
the operator starts from; the backend re-checks the identical both-sides trust
either way, so push and pull carry equal privilege. Borrow candidates come from
`provider_peers` (the destination already receives each trusted peer's provider
names, no secrets) and the UI pre-blocks any name the current agent already
holds — the same collision `plan_share` rejects.

**Share.** `handle_provider_share` (pure guard `plan_share`: reject self,
reject a dest that already declares the record) resolves the owner's entry,
builds the borrowed entry (re-sharing a borrowed record points `shared_from`
at the *true* owner, not the intermediary), then: `ensure_v2` → attach the
existing record to the dest sandbox → policy-load → confirm composition
(endpoint-exact for generic, name-only for built-in) → append the borrowed
entry to the dest `agent.yaml` **last**. Any post-attach failure rolls back
via `rollback_shared_attachment`, which **detaches only** — it MUST NOT
`delete_provider`, because the record belongs to the owner and may serve
other sandboxes.

**Unshare.** `handle_provider_unshare` (pure guard `plan_unshare`: reject
unsharing an OWNED record) detaches the record from the borrower's sandbox and
removes the borrowed `agent.yaml` entry, then reloads policy. It NEVER
`delete_provider`s — the owner keeps the record. Borrowed providers render
read-only in the dashboard (no rotate/remove/edit/config; a single "Unshare"
action + a "Shared from {owner}" label).

**Reconcile.** The supervisor resolves ALL declared names (owned + borrowed)
into secret bindings; the declared list is the source of truth, not the name
prefix. A borrowed entry resolves through its borrow row to the owner's
credential, so the borrower never holds a second copy.

**Lifecycle (refcount).** On `agent destroy`, the store cascades per record:
an owned record whose borrowers still reference it is **re-homed** to a
surviving borrower rather than deleted; an owned record with no borrowers is
removed; a borrowed record is unshared. The cascade runs inside the store, so
`providers.db` is never left with an orphaned row.

**Connected rotation.** Dashboard share/borrow applies the owner's current
credential to the destination before success; later destination reconcile
resolves the owner's current credential from the shared store. No copy is made.

**Redaction guard (retained).** Even with copy retired,
`check_source_credential_readable` + the `SourceCredentialUnreadable` (422)
error remain (`#[allow(dead_code)]`) as defense-in-depth: any future host-side
read-back caller must reject a redacted/empty value before writing it.

Backend: `internal_api_providers::{handle_provider_share, handle_provider_unshare,
plan_share, plan_unshare, handle_provider_peers, build_peers, require_trusted}`;
`right_providers::ProviderStore` (borrow rows, destroy cascade,
`source_ref_binding`). Dashboard routes:
`/dashboard/{agent}/api/v1/providers/{peers,share,borrow,unshare}` plus
`DELETE /dashboard/{agent}/api/v1/providers/{provider_name}`.
