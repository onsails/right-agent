# Providers — design

**Status:** spec. Implementation plan is a separate doc.

**Scope:** A new "Providers" feature in Right Agent that wraps NVIDIA OpenShell's
provider system. Lets the user attach typed credential bundles (Anthropic,
OpenAI, GitHub, GitLab, etc., plus a `generic` escape hatch) to a sandboxed
agent. Credentials live on the OpenShell gateway, never on disk in Right. The
sandbox sees opaque placeholder env vars; the gateway proxy substitutes the real
value on outbound HTTPS. Works only for `sandbox.mode = openshell` agents.

**Out of scope:** the in-sandbox `claude login` OAuth flow keeps working as
today; providers do not replace or compete with it. The `claude`-typed OpenShell
provider profile is intentionally not exposed by Right.

## Motivation

Today every Right Agent that wants to talk to an external API (other than
Claude) either bakes credentials into a tool's MCP config or has the user paste
keys interactively into the chat. Both are bad: the first leaks secrets through
agent.yaml and backups; the second has no recovery path and no central rotation.

OpenShell ≥ v0.0.30 with `providers_v2_enabled` solves the underlying problem:
credentials live gateway-side, the sandbox only sees placeholder tokens, the
gateway proxy substitutes the real value on egress over TLS-terminated HTTPS.
The substitution mechanism is opaque to the agent — any HTTPS call hitting the
right hostname is automatically authenticated. The platform piece this leaves
to Right Agent is *management*: creating, rotating, attaching, deleting
providers on behalf of the user, and keeping per-agent state consistent.

## Naming

The feature is called **Providers** throughout Right Agent — same name as the
underlying OpenShell concept. No "accounts" terminology anywhere in user-facing
copy.

## Constraints

- Sandboxed agents only. `sandbox.mode = none` agents have no
  `GetSandboxProviderEnvironment` path and the bot rejects `/providers` with a
  one-line explanation.
- One provider belongs to exactly one agent — no sharing. Enforced by name
  prefix: every Right-managed gateway provider is named `<agent>-<label>` (or
  `<agent>-<type>` when the label equals the type slug). Cross-agent reuse is
  rejected by the internal API.
- `providers_v2_enabled` must be `true` on the gateway. Right ensures this at
  `right up`; failure to enable is fatal for `right up` only when at least one
  agent has a non-empty providers list.
- Credentials never enter agent.yaml, never enter logs, never enter backups,
  never leave the gateway in plaintext to the host. `GetSandboxProviderEnvironment`
  returns placeholders (e.g.
  `openshell:resolve:env:v17329906524197465519_RIGHT_PROBE_TOKEN`); Right may
  display these for diagnostics but must not log them.
- Built-in `claude` provider type is hidden by Right. The in-sandbox `claude
  login` OAuth flow remains the Claude credential path.

## Architecture

```
┌──────────────────────┐     dashboard UI
│  Telegram Mini App   │     /agent/{name}/providers route
│  ProvidersPage       │
└──────────┬───────────┘
           │ HTTPS (dashboard)
           ▼
┌──────────────────────┐     crates/bot/src/telegram/dashboard/providers.rs
│  bot dashboard       │     - GET    list
│  handlers            │     - POST   create
└──────────┬───────────┘     - POST   rotate
           │                 - POST   config-update (generic only)
           │                 - DELETE remove
           ▼
┌──────────────────────┐
│ InternalClient (UDS) │
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐     crates/right/src/internal_api.rs
│ internal_api routes  │     /provider-list, /provider-create, /provider-rotate,
└──────────┬───────────┘     /provider-config-update, /provider-remove, /provider-types
           │
           ▼
┌──────────────────────┐     NEW: crates/right-openshell/src/providers.rs
│  right_openshell::   │     thin Rust wrappers over OpenShell Provider gRPC
│      providers       │     + ensure_v2_enabled + list_profiles cache
└──────────┬───────────┘
           │ mTLS gRPC
           ▼
┌──────────────────────┐
│   OpenShell gateway  │     authoritative store: credentials, config,
│                      │     endpoint discovery, sandbox attachment
└──────────┬───────────┘
           │ env-var injection at sandbox boot
           │ + placeholder → real header substitution on egress
           ▼
        sandbox
```

**State-of-truth split.**

- `agent.yaml::sandbox::providers: [{ name, type, label?, generic? }]` — per-agent
  declarative list. No secret values. `MergedRMW` codegen category.
- OpenShell gateway — credential bytes, non-secret config, endpoint discovery,
  attachment to a specific sandbox.

The agent.yaml list is the per-agent source of truth. The gateway holds the
credential. Reconciliation makes the gateway match agent.yaml at `right up`
and on every dashboard mutation.

**Layering rules.**

- The Provider gRPC client lives only in `right_openshell::providers`. Other
  crates must use that module — no parallel client construction.
- The internal API on the Unix socket is the only writer to
  `agent.yaml::sandbox::providers` and the only caller of the gateway provider
  RPCs from a write path. Dashboard handlers proxy via `InternalClient`. Agents
  inside the sandbox cannot reach the socket.
- Dashboard handlers in `crates/bot/src/telegram/dashboard/providers.rs` mirror
  the shape of the existing `mcp.rs` handlers (same internal-client pattern,
  same error response helper, same TLS-only origin policy).

## Data model

`agent.yaml` schema additions (existing struct: `SandboxConfig`):

```yaml
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: right-myagent
  providers:                          # NEW
    - name: myagent-anthropic         # gateway-unique, prefix-namespaced
      type: anthropic                 # one of nine built-in slugs or "generic"
      label: ~                        # optional. Default = type slug
      # generic-only block, omitted for built-in providers:
      generic:
        env_var: MY_API_TOKEN
        header_name: "X-Service-Token"   # default "Authorization" with "Bearer " prefix
        upstream_host: api.example.com   # added to policy as protocol: rest
        upstream_path_prefix: /v1         # optional
```

**Provider types Right exposes (9):**

Eight built-in slugs — `anthropic`, `codex`, `copilot`, `github`, `gitlab`,
`nvidia`, `openai`, `opencode` — plus `generic`. The ninth built-in OpenShell
slug, `claude`, is intentionally hidden.

**Naming rules** (enforced by internal API):

- `name` must equal `<agent>-<slug>` where slug is the label or the type.
- Slug pattern `[a-z][a-z0-9-]{0,31}`. Total name ≤ 64 chars.
- Two providers with the same `name` on one agent: 409.
- Two providers with the same env var name on one agent: 409 (would shadow
  each other in the env block).
- A provider whose name does not match the agent's prefix: 400.

**`ProviderView` (response DTO, never contains secret values):**

```rust
struct ProviderView {
    name: String,
    type_: String,
    label: Option<String>,
    env_var: String,
    generic: Option<GenericView>,
    updated_at: Option<DateTime<Utc>>,
    status: ProviderStatus, // Healthy | Missing (ghost) | GatewayError(String)
}

struct GenericView {
    header_name: String,
    upstream_host: String,
    upstream_path_prefix: Option<String>,
}
```

## Mutation → reconciliation matrix

Every dashboard mutation runs the gateway operation synchronously and only
writes `agent.yaml` after gateway success. No deferred background reconcile.

| Dashboard action     | Internal route             | Gateway calls (in order)                       | agent.yaml          | Sandbox effect                      |
| -------------------- | -------------------------- | ---------------------------------------------- | ------------------- | ----------------------------------- |
| Create + attach      | `POST /provider-create`    | `CreateProvider` → `Sandbox.provider.attach`   | append to providers | new env var live on next proxy reload |
| Rotate credential    | `POST /provider-rotate`    | `UpdateProvider`                               | unchanged           | env var value rotates without sandbox restart |
| Edit non-secret config | `POST /provider-config-update` | `UpdateProvider` (+ policy reconcile if host changed) | update `generic.*`  | same as rotate                      |
| Remove               | `DELETE /provider-remove`  | `Sandbox.provider.detach` → `DeleteProvider`   | remove entry        | env var unset on next proxy reload  |

A separate `detach-but-keep` action is **not** exposed in the UI; with no
sharing, a detached provider is an orphan.

**Reconciliation on `right up`** (idempotent, defensive):

- For each sandboxed agent, walk `providers` list:
  - `GetProvider` for each entry. Missing → mark as `Status::Missing` (ghost).
    Do not auto-heal: we don't have the credential bytes.
  - Check the sandbox's current attachment set. Attach any name in
    agent.yaml that isn't currently attached. Detach any `<agent>-*` provider
    attached to the sandbox that isn't in agent.yaml (agent.yaml wins on drift).
- Result: a `ReconcileReport` per agent surfaced to the dashboard.

**Cascade on `right agent destroy`:**

- Before tearing down the sandbox, call `DeleteProvider` for every name in
  `providers`. Orphan-free on the gateway.

**`providers_v2_enabled`:**

- `right up` calls `ensure_v2_enabled` once during startup.
- `GetGatewayConfig` → if `providers_v2_enabled == true`, no-op.
- Otherwise `UpdateGatewayConfig` with the flag set.
- If the update fails AND any agent has a non-empty `providers` list, `right up`
  exits with a fatal error pointing the operator at the gateway docs.
- If no agent uses providers, the failure is logged as a warning and startup
  continues.

## Dashboard UX

**Bot command:** `/providers`. Opens the Mini App at
`/agent/{name}/providers`. For `sandbox.mode = none`, the bot replies with a
single Telegram message *"Providers are only available for sandboxed agents.
This agent runs in host mode."* and does not open the dashboard.

**List view.** Rows showing type badge, gateway name, env var (and for
generic, upstream host + header). `[+ Add]` button. Per-row actions: *Rotate*
and a `⋯` menu with *Edit non-secret config* (generic only) and *Delete*.

**Add — built-in type.** Two screens:

1. Type grid showing nine built-in cards. Each card surfaces the env var the
   profile will inject (looked up from `openshell provider list-profiles`,
   cached at bot startup).
2. Single password field for the credential, optional label field (used only
   when the user wants a second provider of the same type on this agent).

**Add — generic type.** Pick *Generic* from the type grid, then fill the form:

- Label (required, slug, unique within agent)
- Env var name (required, `[A-Z_][A-Z0-9_]*`, ≤ 64 chars)
- Credential value (password)
- Upstream host (required, e.g. `api.example.com`)
- Header name (default `Authorization`; when the user enters anything else,
  e.g. `X-Service-Token`, the value injected is the raw placeholder; when left
  as `Authorization` the value is `Bearer <placeholder>`)
- Upstream path prefix (optional)

Submitting a generic provider triggers a policy.yaml reconcile and hot-apply
(see [Policy interaction](#policy-interaction) below) before the gateway calls.

**Rotate.** A single password field, no confirmation. Server: `UpdateProvider`
only.

**Delete.** Confirmation dialog naming the provider and warning about loss of
access. Server: `provider.detach` → `DeleteProvider` → policy reconcile (strip
endpoint if no other provider on this agent uses the same host) → agent.yaml
write.

**Error UX.**

- Gateway unreachable → red banner. Mutations disabled.
- `providers_v2_enabled` not on → red banner *"Provider v2 not enabled on the
  gateway. Run `right up`."* Mutations disabled.
- *Ghost* providers (in agent.yaml, missing on gateway): row in muted style
  with a *Resolve* menu offering *Re-create with new credential* or *Remove from
  agent.yaml*. Surfaces typical post-backup-restore state.

## Internal API surface

New routes in `crates/right/src/internal_api.rs`. Bodies are JSON; the agent
name travels in the body (no path params). Auth: socket file ownership (same
as existing routes).

| Method   | Path                       | Body                                                            | Result          |
| -------- | -------------------------- | --------------------------------------------------------------- | --------------- |
| `POST`   | `/provider-list`           | `{ agent }`                                                     | `[ProviderView]`|
| `POST`   | `/provider-create`         | `{ agent, type, label?, credential, generic? }`                 | `ProviderView`  |
| `POST`   | `/provider-rotate`         | `{ agent, name, credential }`                                   | `ProviderView`  |
| `POST`   | `/provider-config-update`  | `{ agent, name, generic: { env_var?, header_name?, upstream_host?, upstream_path_prefix? } }` | `ProviderView` |
| `DELETE` | `/provider-remove`         | `{ agent, name }`                                               | `{ removed: true }` |
| `POST`   | `/provider-types`          | `{}`                                                            | `[ProviderProfile]` |

**Error taxonomy** (single enum, HTTP-mapped):

```rust
enum ProviderApiError {
    NotFound { name: String },                       // 404
    NameCollision { name: String },                  // 409
    EnvVarCollision { env_var: String },             // 409
    InvalidName { name: String, reason: String },    // 400
    InvalidEnvVar { env_var: String },               // 400
    SandboxModeNone,                                 // 400
    V2NotEnabled,                                    // 503
    PolicyConflict { host: String, kind: String },   // 409
    Gateway(String),                                 // 502 — anything from OpenShell gRPC
    AgentYamlWrite(String),                          // 500 — agent.yaml write failed AFTER gateway change
}
```

**Rollback rule.** When `AgentYamlWrite` triggers, the handler attempts a
best-effort gateway rollback: re-call `DeleteProvider` for a freshly created
provider, or restore the prior credential for a failed rotation. If the
rollback also fails, the response includes a *"Provider exists on gateway but
agent.yaml is out of sync; will reconcile on next `right up`"* hint and the
startup reconciler resolves it (detaches the orphan or surfaces it as a ghost).

**Concurrency.** Per-`(agent, provider_name)` mutex on the bot side serializes
mutations to the same provider. Different providers proceed in parallel. This
avoids surfacing OpenShell conflict errors from racing dashboard clicks.

## gRPC wrappers

New module: **`crates/right-openshell/src/providers.rs`**. Sole owner of the
Provider gRPC client. Exposed API (sketch):

```rust
pub async fn ensure_v2_enabled(endpoint: &GatewayEndpoint) -> Result<bool, ProviderError>;
pub async fn list_profiles(endpoint: &GatewayEndpoint) -> Result<Vec<ProviderProfile>, ProviderError>;
pub async fn create_provider(endpoint: &GatewayEndpoint, spec: ProviderSpec) -> Result<Provider, ProviderError>;
pub async fn get_provider(endpoint: &GatewayEndpoint, name: &str) -> Result<Provider, ProviderError>;
pub async fn update_provider(endpoint: &GatewayEndpoint, spec: ProviderSpec) -> Result<Provider, ProviderError>;
pub async fn delete_provider(endpoint: &GatewayEndpoint, name: &str) -> Result<(), ProviderError>;
pub async fn list_providers_by_prefix(endpoint: &GatewayEndpoint, prefix: &str) -> Result<Vec<Provider>, ProviderError>;
pub async fn attach_to_sandbox(endpoint: &GatewayEndpoint, sandbox_id: &str, name: &str) -> Result<(), ProviderError>;
pub async fn detach_from_sandbox(endpoint: &GatewayEndpoint, sandbox_id: &str, name: &str) -> Result<(), ProviderError>;
pub async fn get_sandbox_provider_environment(endpoint: &GatewayEndpoint, sandbox_id: &str)
    -> Result<HashMap<String, String>, ProviderError>;  // values are placeholders — NEVER LOG
```

**Cache.** `list_profiles` is called once at bot startup, stored in an
`Arc<Vec<ProviderProfile>>` surfaced by `/provider-types`. Gateway unreachable
at startup is logged; the dashboard renders an error banner until the next
successful fetch.

**`get_sandbox_provider_environment`.** Used only for diagnostics. Values are
opaque placeholders, but the function carries a `// SAFETY: do not log` comment
and the dashboard handler explicitly filters values out of its response body.

**Sandbox-create wiring.** `spawn_sandbox` (in
`crates/right-openshell/src/openshell.rs`) gains a `providers: &[String]`
parameter. When non-empty, the function emits `--provider <name>` for each
entry before `--policy`. Existing callers in agent-init and sandbox migration
pass the value of `agent.yaml::sandbox::providers`.

**Backup / restore.** The `providers` field travels in agent.yaml backups. On
restore to a different host, the gateway side is empty. The startup reconciler
flags each entry as `Status::Missing`; the dashboard's *Resolve* action lets
the user re-enter the credential and re-create the gateway provider.

## Policy interaction

Two paths into `policy.yaml` mutation.

**Path A — built-in providers.** Right does **not** mutate policy.yaml. The
built-in provider profiles ship their own endpoint discovery; with v2 enabled,
OpenShell contributes those endpoints to the effective sandbox policy at
attach time. Right's policy.yaml stays unchanged.

**Path B — generic providers.** Right owns the policy mutation. On
`provider-create` and `provider-config-update` (when `upstream_host` changes),
the flow is:

1. Load current `policy.yaml`.
2. Find `endpoints[]` entry matching `upstream_host`:
   - **Absent.** Append a new endpoint: `domain: <host>`, `protocol: rest`,
     `access: full`, `path: <prefix>` (if set). Tag with YAML comment
     `# managed-by: right-providers:<provider-name>`.
   - **Present, `protocol: rest`.** No-op.
   - **Present, `tls: skip` (raw tunnel).** Refuse the operation with
     `PolicyConflict { host, kind: "raw-tunnel" }`. Do not auto-rewrite.
   - **Present, `tls: terminate` (deprecated but functional).** No-op.
3. Write policy.yaml.
4. Hot-apply via `openshell policy set --wait`. This is the
   `Regenerated(SandboxPolicyApply)` codegen category — **not**
   `SandboxRecreate`. New endpoints are hot-reloadable.

On `provider-remove`, the inverse: if no other generic provider on this agent
uses the same `upstream_host`, strip the tagged stanza and hot-apply.

**Atomicity of the create-generic sequence.**

1. write policy.yaml (snapshot prior content)
2. `openshell policy set --wait`
3. `CreateProvider`
4. `Sandbox.provider.attach`
5. write agent.yaml

Failure at 2 → restore policy.yaml from snapshot, return error. Failure at 3
or 4 → strip the new stanza, hot-apply again, return the gateway error.
Failure at 5 → enter the [Rollback rule](#rollback-rule) flow.

**No filesystem-policy mutation.** Providers only touch the network section.
Landlock rules are not provider-relevant. No `SandboxRecreate` paths ever fire
from provider operations.

## Upgrade safety

- Existing agents with no `providers` field in agent.yaml: the field defaults
  to empty. No behavior change.
- The field belongs to the `MergedRMW` category; backup/restore preserves it.
- `right agent rebootstrap` carries the field through.
- `right agent init --from-backup` restoring across hosts: providers travel as
  metadata; users re-enter credentials via the dashboard's *Resolve* flow.
- New `--provider` CLI flag exposure on `right agent config`: **not** added.
  Providers are bot-managed (operational concern), the same exception
  documented in AGENTS.md for `/mcp` and `/model`.

## Logging discipline

- Credential values: never logged. The handler that receives a `credential`
  field in a request body must not pass that field to any tracing macro. A
  newtype `RedactedSecret(String)` with a `Debug` impl that prints `"[redacted]"`
  guards this in the request DTOs.
- Placeholder values (e.g.
  `openshell:resolve:env:v17329906524197465519_RIGHT_PROBE_TOKEN`): never
  logged. Operators reading logs would mistake them for leaked secrets; they
  aren't, but the principle stands.
- Provider *names*, types, and labels: log freely (operational visibility).
- Env var *names* and upstream hosts: log freely.

## Testing

Cadence per AGENTS.md: TDD for new behavior, narrow test commands during
iteration, full workspace test before finishing each worktree.

**Unit tests:**

- `providers.rs` spec → RPC mapping against a mock gRPC server.
- Name validation, env-var validation, collision rejection in `internal_api`.
- Policy mutation: append/no-op/strip cases against fixture policy.yaml files.
- `sandbox.mode = none` rejection at internal_api boundary.

**Live OpenShell tests** (`ci-openshell: ...`, prefix `ci_openshell_provider_*`):

- `ci_openshell_provider_create_attach_env_visible`: create + attach a generic
  provider, exec `printenv` inside the sandbox, assert env var is set to a
  placeholder.
- `ci_openshell_provider_rotate_no_restart`: create, rotate, observe new
  placeholder value visible without sandbox restart.
- `ci_openshell_provider_detach_removes_env`: detach, observe env var absent
  on next reload.
- `ci_openshell_provider_v2_flip`: starting from `providers_v2_enabled=false`,
  `ensure_v2_enabled` flips it to true; idempotent on second call.
- `ci_openshell_provider_policy_hot_apply`: generic provider create appends a
  TLS-terminated endpoint to policy.yaml; `openshell policy set --wait`
  succeeds; no sandbox recreation.
- `ci_openshell_provider_raw_tunnel_conflict`: pre-seed policy with a raw
  tunnel for the host; assert generic create returns `PolicyConflict`.
- `ci_openshell_provider_destroy_cascade`: agent destroy deletes all
  `<agent>-*` providers from the gateway.

**Reconciler tests:**

- `reconciler_detects_ghost_providers`: agent.yaml lists a provider; gateway
  returns `NotFound`; reconciler reports `Status::Missing`.
- `reconciler_cleans_drift_attachments`: gateway has an attached `<agent>-*`
  not in agent.yaml; reconciler detaches it.
- `reconciler_attaches_missing`: agent.yaml lists provider that exists on
  gateway but isn't attached; reconciler attaches.

**Negative:**

- Provider operations against `sandbox.mode = none` agent rejected with
  `SandboxModeNone`.
- Provider name with wrong prefix rejected with `InvalidName`.
- `claude` as a type slug rejected with `InvalidName`.

## ARCHITECTURE.md updates

This feature adds three review-blocking rules. Each is brief (≤3 sentences) and
satisfies the "Rule / Enforcement / Brevity" tests in AGENTS.md.

1. **Provider gRPC ownership.** Add to the existing "OpenShell Integration
   Conventions" section: *"All Provider RPCs (CreateProvider, GetProvider,
   UpdateProvider, DeleteProvider, ListProviders, attach/detach,
   GetSandboxProviderEnvironment, ensure_v2_enabled) MUST go through
   `right_openshell::providers`. Direct gRPC client construction for provider
   operations is a review-blocking defect."*

2. **Provider management surface.** Add to the existing "Conventions" section:
   *"Provider management goes through the Telegram Mini App dashboard opened by
   `/providers`. Never create or edit gateway providers via host CLI or
   agent.yaml directly — the dashboard is the control plane."*

3. **Credential logging.** Add to "Security Model": *"Provider credential
   values and placeholder values
   (`openshell:resolve:env:v…_<NAME>`) are never logged. Use `RedactedSecret`
   for in-memory transport; do not pass credential fields to tracing macros."*

Descriptive content — the placeholder mechanism, the substitution flow, the
policy gotchas, the reconciler walkthrough — goes into a new satellite
`docs/architecture/providers.md`, referenced by plain path from ARCHITECTURE.md.

## Non-goals

- Sharing one provider across multiple agents.
- A CLI surface for provider management.
- Replacing the in-sandbox `claude login` flow.
- A `claude`-typed provider exposed through Right.
- Provider history / rotation audit trail (the gateway has `updated_at`; we
  surface it but don't keep our own log).
- Background re-attach worker. All reconciliation is synchronous on mutation
  or eager at `right up`.

## Open questions

- **OpenShell `Settings` RPC name for `providers_v2_enabled`.** We rely on
  `GetGatewayConfig` / `UpdateGatewayConfig` to flip the flag — proto field name
  may differ. The plan must verify against the current proto and surface any
  mismatch as a single small adjustment to `ensure_v2_enabled`.
- **`generic` profile endpoint discovery shape.** The OpenShell docs say v2
  contributes endpoints to the sandbox policy, but the exact field on the
  `Provider` message that carries discovery for generic providers is not
  spelled out in the available docs. If contribution is profile-only (i.e.
  `generic` doesn't contribute), Path B (Right writes policy.yaml) is the only
  option and this design is unaffected. If `generic` *does* contribute
  endpoints from a Provider-level `discovery` field, we may be able to skip
  Path B entirely. The plan must determine this with a quick probe and pick
  whichever produces less code.
- **Profile cache refresh.** Right caches `list_profiles` at bot startup; new
  built-in types added by OpenShell upgrades won't appear until the next bot
  restart. Acceptable for v1. If profile churn happens often, add a refresh
  endpoint or TTL later.
