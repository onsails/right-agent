# Provider Sharing: Cross-Agent Import / Export

**Date:** 2026-06-15
**Status:** Draft

## Problem

A user who configured a provider on one agent (e.g. a `fal.ai` static-key
provider on `riskoff`, or a custom `generic` provider) must currently
re-enter the same API key by hand to use it on another agent. There is no
way to reuse a credential that already lives on the OpenShell gateway.

The desired flow: if the operator is a trusted dashboard user on **both**
agents, the dashboard should let them copy a provider — credential
included — from one agent to another without retyping the key. This must
work for built-in credentialed providers (`right-fal`, `right-github`,
`anthropic`, …) and for custom `generic` providers.

Two directions are required, both from a single (single-agent) dashboard:

- **Import** — pull a provider from another agent into the current one.
- **Export** — push the current agent's provider out to other agents.

And re-sync: when the source key is later rotated, the operator must be
able to push the new key **over** the existing copy on other agents,
instead of deleting the old copy and importing again.

## Context (current architecture)

Established by code inspection during brainstorming:

- **The dashboard is scoped to one agent.** Each bot process runs its own
  dashboard server with a single `agent_name`. `authenticate_api`
  (`crates/bot/src/telegram/dashboard.rs`) validates Telegram `initData`
  signed by *this* bot's token, then checks the user against *this*
  agent's allowlist; the `{agent}` path segment must equal
  `state.agent_name` or the request is `FORBIDDEN`.
- **The internal API sees every agent.** `internal_api_providers.rs`
  (in the `right` process) holds `agents_dir` (all agents) plus the
  OpenShell gateway client. Its provider handlers are already
  parameterized by agent name — they resolve `agent.yaml`, sandbox name,
  and policy path from `agents_dir/<agent>`.
- **`GetProvider` returns the real credential bytes.** Confirmed by
  `ci_openshell_provider.rs` ("raw GetProvider must expose existing
  credentials for repair echo"). The host-facing wrapper
  `providers::get_provider` deliberately drops them; the raw proto path
  keeps them. So reading a credential back to copy it is feasible.
- **Telegram user ids are global.** The same user id is presented to every
  bot, so an identity proven via one bot's signed `initData` is valid for
  checking another agent's allowlist.
- **One host = one gateway.** The dashboard and internal API are
  host-local, so sharing is within a single host. Cross-host sharing is
  out of scope.
- **Per-agent provider ownership.** Providers are named `<agent>-<slug>`.
  The reconciler attaches anything in `agent.yaml::sandbox::providers` and
  **detaches** any `<agent>-*` provider attached to the sandbox but absent
  from the file. `right agent destroy` cascade-deletes the agent's
  providers.
- **Credential management is dashboard-only.** Per `CLAUDE.md`, providers
  are never created/edited via host CLI or by hand-editing `agent.yaml`.

## Design

### Core decision: copy, not a shared resource

Importing/exporting **copies** the credential into a separate provider
record owned by the destination agent. The platform never attaches one
gateway provider to multiple sandboxes — doing so would break the
`<agent>-*` reconcile/detach invariant and the destroy cascade.

Consequences (accepted):

- Each agent owns an independent provider record with an independent
  credential copy. Rotation, removal, and destroy are per-agent.
- A copy is a point-in-time snapshot. Rotating the source key does **not**
  auto-propagate; the operator re-runs the copy in *overwrite* mode to
  push the new key.

### One primitive, two perspectives

Import and export are the same operation — copy a provider between two
host-local agents — seen from the dashboard's single-agent vantage point:

| Dashboard action | source_agent | dest_agent |
|---|---|---|
| Import | another agent | current agent |
| Export | current agent | another agent |

The dashboard always pins one side to the current agent. Copying between
two *other* agents through the dashboard is not possible.

Both resolve to one internal primitive:

```
provider_copy(actor_user_id, source_agent, source_provider, dest_agent, label?, overwrite)
```

### Overwrite semantics — match key is `env_var`

"The same provider on another agent" is identified by **`env_var`**, which
is unique per agent (existing `EnvVarCollision` invariant). When copying
into a destination:

- env_var **not present** on dest → **create new** `<dest>-<slug>` via the
  existing full create flow with the copied credential.
- env_var **present** on dest → **overwrite in place**: rotate the
  existing dest provider's credential to the source's current value, and
  (for `generic`) re-sync its non-secret config (upstream hosts / path
  prefix). The dest provider's name is **not** changed.

`overwrite` is an explicit boolean on the request and is validated
FAIL-FAST:

- `overwrite=false` but env_var already present → `EnvVarCollision`.
- `overwrite=true` but no matching env_var on dest → error (nothing to
  overwrite).
- `overwrite=true` but the matching dest provider has an incompatible type
  (e.g. source `generic` vs dest built-in) → error; operator must remove
  it first.

No provenance ("imported from riskoff") is stored in v1 — matching by
env_var plus an explicit operator action is sufficient.

### Authorization

The actor (Telegram user id) must be in the allowlist of **both** the
source and the destination agent.

- The dashboard proves the actor's identity via *its* bot's signed
  `initData` and confirms trust in *its own* agent (existing
  `authenticate_api`).
- The internal Unix-socket API (host-only; unreachable from inside any
  sandbox) re-derives trust for the *other* agent by reading its
  `allowlist.yaml` from disk (`right_agent::agent::allowlist::read_file`).
- The dashboard forwards the authenticated `DashboardUser.id` as
  `actor_user_id`. Because the dashboard pins one side to the current
  agent, every copy has the current agent as source or dest, and the
  actor must be independently trusted on the other side.

No new trust boundary is crossed: the bots and the internal API share one
host trust domain (same OS user). The cross-allowlist check is the
meaningful gate for a multi-operator host.

### Credential read-back (the one deliberate secret read)

`provider_copy` reads the source credential from the gateway. This is the
single sanctioned exception to "never persist credentials on the host":

- New `right_openshell::providers::get_provider_credentials(client, name)
  -> HashMap<String, secrecy::SecretString>` exposes the raw credential
  map (the existing public `get_provider` keeps dropping it).
- The value is held transiently in `SecretString`, written straight into
  the destination provider (create or rotate), and never logged, never
  written to `agent.yaml`, never included in any list/detail API response,
  never in backups.

### Component changes

1. **`right_openshell::providers`**
   - `get_provider_credentials(...)` — raw credential read-back as
     `SecretString` map (secrecy discipline; no `tracing` of values).

2. **Internal API — `crates/right/src/internal_api_providers.rs`**
   - `provider_copy(...)` primitive. Reuses the existing create / rotate /
     config-update internals, parameterized by `dest_agent`.
   - `provider_peers(actor_user_id, for_agent)` discovery:
     `discover_agents(agents_dir)` → keep agents where `actor_user_id ∈
     allowlist` and `agent != for_agent` → return each peer's providers
     (`name`, `type`, `env_var`, `label`, `sandbox_mode`,
     `network_policy`). **No credentials.**
   - `assert_trusted(actor_user_id, agent)` helper over
     `allowlist::read_file`.
   - Reuse existing validation: `validate_name`, env_var collision,
     generic-requires-permissive, sandbox-mode checks — applied to the
     **destination** agent.

3. **`InternalClient` — `crates/right-mcp/src/internal_client.rs`**
   - Two new methods carrying `actor_user_id`: `provider_peers` and
     `provider_copy`. There is a single `provider_copy` internal route;
     the dashboard's import and export handlers both call it, differing
     only in which side they pin to the current agent.

4. **Dashboard — `crates/bot/src/telegram/dashboard/providers.rs` + routes**
   - `GET .../providers/peers` → feeds both pickers.
   - `POST .../providers/import { source_agent, source_provider, label?,
     overwrite }` (dest = current agent).
   - `POST .../providers/export { provider, dest_agent, overwrite }`
     (source = current agent; the UI calls once per selected destination
     so per-agent success/failure is visible).
   - Every route passes the authenticated `DashboardUser.id` as
     `actor_user_id`.

5. **Frontend — `ProvidersView.vue` + `providersViewModel.ts`**
   - Add-modal: an "Import from another agent" entry → list peer providers
     → select → collision-aware **Create** vs **Update**.
   - Per-row "Export" action → multi-select destination agents → per-agent
     **Create** vs **Update**.
   - The "collision → create vs update" decision lives in the pure
     `providersViewModel.ts` and is unit-tested directly.

### Data flow (`provider_copy`)

1. Authorize: `actor_user_id ∈ allowlist(source_agent)` **and** `∈
   allowlist(dest_agent)`.
2. Read source: type + generic config + label + env_var from
   `source_agent`'s `agent.yaml`; credential bytes from the gateway via
   `get_provider_credentials`.
3. Resolve dest provider by env_var:
   - **overwrite**: rotate the matching dest provider's credential to the
     source value; for `generic`, also config-update upstream hosts /
     path. (Existing rotate + config-update internals.)
   - **create**: run the full create flow on `dest_agent` with the copied
     credential (author/import generic profile, `CreateProvider`, attach,
     `ensure_provider_policy_loaded`, `wait_for_provider_composed`,
     append to `dest agent.yaml`).
4. The `dest agent.yaml` write goes through `write_merged_rmw`; a running
   destination bot additionally picks up the change via `config_watcher`
   (`ProvidersReload`), which is idempotent against the synchronous
   attach/compose just performed.

### Error handling (all FAIL-FAST with readable messages)

- Destination was never brought up (no sandbox) → attach/compose fails →
  surfaced to the operator.
- Generic provider into a `restrictive` destination → rejected
  (`NetworkPolicyForbidsGeneric`).
- Destination not `openshell` mode → rejected (`SandboxModeNone`).
- `overwrite`/collision mismatch → rejected (see Overwrite semantics).
- Type mismatch on overwrite → rejected.
- Actor not trusted on either side → `UnauthorizedUser`.

### Non-goals (v1)

- No live link / auto-propagating rotation (copy is point-in-time).
- No provenance tracking of where a copy came from.
- No cross-host sharing (single gateway per host).
- No CLI surface — providers stay dashboard-only.
- No sharing of the `claude` login type (reserved for the in-sandbox login
  flow; not a catalog provider).

## Testing

- **Unit**: authorization (actor missing from source / dest allowlist);
  create-vs-overwrite resolution by env_var; explicit-mode validation
  (create+collision, overwrite+no-match, type mismatch); `provider_peers`
  filtering (excludes self, excludes agents where actor is untrusted, no
  credentials in output).
- **Internal-API handler tests** over a temp `agents_dir` with synthetic
  agents and allowlists.
- **Live (`ci_openshell_` prefix, `ci-openshell:` ignore reason)**:
  copy-create into a second agent; copy-overwrite re-syncs a rotated key;
  generic-provider copy composes on the destination.
- **Frontend**: Vue SSR component test for the import/export UI; direct
  unit tests of the pure `providersViewModel.ts` collision/mode logic.

## Verification cadence

Targeted package/module tests during implementation after each red/green
slice. One mandatory final full run from the touched worktree:
`devenv shell -- cargo nextest run --workspace` plus
`devenv shell -- cargo test --doc --workspace`.

## Documentation updates

- **`ARCHITECTURE.md` → Providers**: add the prescriptive invariants
  (cross-agent copy is copy-only / never a shared gateway provider;
  copy requires the actor trusted in both agents; the `env_var` overwrite
  match key; the sanctioned credential read-back rule). Keep within the
  40k budget — prose minimal, walkthrough goes to the satellite.
- **`docs/architecture/providers.md`**: import / export / overwrite
  walkthrough and the `provider_copy` data flow.
- **Security Model**: the credential read-back exception and the
  both-allowlists authorization rule.
