# Architecture

> **Prescriptive only.** Load-bearing rules, contracts, gotchas, reference
> tables. Loaded into every conversation. Descriptive content
> (walkthroughs, narration, data flows) belongs in
> `docs/architecture/*.md`, referenced by plain path. See `AGENTS.md` →
> "Architecture docs split" before editing.

## Workspace

Twenty crates in a Cargo workspace. See: `docs/architecture/modules.md`
for the crate inventory and per-crate ownership detail.

**Re-export discipline:** `right-agent` does not re-export modules from
`right-db`, `right-mcp`, `right-codegen`, or `right-memory`. Consumers
import directly from the source crate. This keeps the build-cache
invariant: an edit inside `right-codegen` rebuilds `right-codegen` plus its
direct consumers, not `right-agent`.

**Crate boundaries:** Phase 4 removed the former shared core crate. New
shared code goes to the most-specific owner crate. Anticipating reuse is
not a reason to create or centralize a shared abstraction; promote on
demand, not on prediction. New code that doesn't fit an existing crate's
charter gets its own crate, not a misfit addition. Default placement is
the most-specific leaf crate.

**Dashboard write contract:** `right-dashboard` future write routes must
call bot-owned control-plane services instead of editing agent files,
credentials, or aggregator state directly. Dashboard MCP routes go through
the internal Unix socket API. Dashboard pin/unpin is the operator surface
for curator-managed learned skills; do not add CLI pinning. Overview must
use injected runtime state and must not run doctor or sandbox commands
implicitly.

## Data Flow

### Agent Lifecycle

See: `docs/architecture/lifecycle.md` (covers `right init`, `right up`,
per-message flow, sandbox migration, `right agent backup`,
`right agent rebootstrap`, `right agent init --from-backup`, and
`right down`).

### Voice transcription

See: `docs/architecture/lifecycle.md` (Voice transcription).

### OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as
long as the agent lives and survive bot restarts.

Policy hot-reload via `openshell policy set --wait` covers the network
section only. Filesystem/landlock changes require sandbox recreation (see
`Upgrade & Migration Model` below).

Live OpenShell coverage is CI-explicit: tests that create real sandboxes
or rely on OpenShell CLI file transfer use `#[ignore = "ci-openshell: ..."]`
with a `ci_openshell_` test-name prefix and are called by the
workspace-wide ignored-test filter in `.github/workflows/tests.yml`. Mock
gRPC and pure policy tests remain in the default workspace test path.

See: `docs/architecture/sandbox.md` for staging-dir layout, platform-store
deployment, TLS-MITM, and the bot-startup sandbox sequence.

### Login Flow (setup-token)

See: `docs/architecture/lifecycle.md` (Login Flow).

### MCP Token Refresh

See: `docs/architecture/mcp.md` (MCP Token Refresh).

### MCP Auth Types

Dashboard MCP management runs URL-first detection, then asks the user to
choose `OAuth`, `Headers`, or `URL as-is`. Detection is advisory; the
dashboard choice is authoritative. Telegram `/mcp` opens the dashboard
MCP view and has no management subcommands.

| auth_type | How token is injected | Selection |
|-----------|----------------------|-----------|
| `oauth` | `Authorization: Bearer` via DynamicAuthClient | User chooses `OAuth`; OAuth AS discovery recommends it |
| `bearer` | `Authorization: Bearer` header | User chooses `Headers` with bearer recommendation/fallback |
| `headers` | Multiple configured HTTP headers | User chooses `Headers`; values are write-only and redacted from list/detail APIs |
| `query_string` | Embedded in URL | User chooses `URL as-is` for a URL containing `?` query params |

`URL as-is` also covers no-auth and loopback development MCP servers.
Explicit registration allows HTTP/HTTPS and warns for plain HTTP; broad
private/link-local ranges remain blocked by default.

MCP OAuth uses Resource Indicators. Right discovers the canonical MCP
resource URI from protected-resource metadata when available, includes it
in auth-code and refresh-token requests, and persists it with the OAuth
state. Existing rows without `oauth_resource` fall back to the
canonicalized server URL.

### MCP Aggregator

One shared aggregator process serves all agents on TCP `:8100/mcp` with
per-agent Bearer-token auth. Tool routing rules:

- No `__` prefix → `RightBackend` (built-in tools, unprefixed).
- `rightmeta__` prefix → Aggregator management (read-only: `mcp_list`).
- `{server}__` prefix → `ProxyBackend` (forwarded to upstream MCP).

Internal REST API on Unix socket (`~/.right/run/internal.sock`) is the
sole MCP-management control plane; dashboard routes proxy through it via
`InternalClient` (hyper UDS). Agents cannot reach the Unix socket from
inside the sandbox.

Foreground progress uses the built-in `mcp__right__send_progress` tool.
Cron, delivery, reflection, and background-continuation invocations
disallow it.

Conversation search scope is server-enforced.
`mcp__right__thread_search` searches only the current
`(chat_id, effective_thread_id)`. `mcp__right__chat_search` searches only
the current `chat_id`; in DMs this is only that DM, and in groups this is
the whole group across topics. Agents must never be allowed to pass
chat_id, thread_id, user ids, session ids, or a broader scope to these
tools.

`mcp__right__forum_topic_list` is scoped the same way: it returns only the
current `chat_id`'s tracked topics, resolved server-side from the invocation —
never agent-supplied. Forum write tools (`forum_topic_create`/`_edit`/`_close`/
`_reopen`) resolve `chat_id` identically and never accept it as an argument; no
delete tool exists.

See: `docs/architecture/mcp.md` for the internal REST surface, progress
register/send wiring, dispatch detail, and rationale.

### Prompting Architecture

Session-bearing `claude -p` invocations get a composite system prompt via
`--system-prompt-file` (the sole prompt mechanism — no `--agent` flag).
Prompt caching is critical — avoid per-message tool calls to read
identity files.

The per-turn skill-learning pipeline (anchor capture → Haiku prefilter →
probe-writer fork → periodic curator) replaces the prior fork-probe
classifier. See `PROMPT_SYSTEM.md` for prompt assembly and
`docs/architecture/learning.md` for the pipeline walkthrough, gate
ordering, and lifecycle storage.

### Claude Invocation Contract

Every `claude -p` invocation MUST go through `ClaudeInvocation` (defined
in `crates/bot/src/cc/invocation.rs`). Direct construction of
`claude_args` vectors is forbidden — the builder enforces invariant flags
at compile time.

**Invariants** (always present for session-bearing invocations, cannot be
omitted):
- `claude -p --dangerously-skip-permissions`
- `--mcp-config <path>` + `--strict-mcp-config` — agents MUST have MCP access
- `--output-format <stream-json|json>` (`--verbose` auto-added for `stream-json` only)
- `--json-schema <schema>` — structured output

**Optional per-callsite:**
- `--model` — override default model
- `--max-budget-usd` — budget cap (cron jobs)
- `--max-turns` — turn limit
- `--resume` / `--session-id` — session management (worker, delivery)
- `--disallowedTools` — disable CC built-ins that conflict with MCP equivalents

**Adding a new session-bearing `claude -p` callsite:** construct a
`ClaudeInvocation`, set fields, call `.into_args()`, pass result to
`build_prompt_assembly_script()`. Never build args manually. A no-MCP,
no-composite-prompt callsite needs an explicit architecture exception
here.

Learning callsites (probe-writer fork, Haiku prefilter, curator) have
specialized contracts — see `docs/architecture/learning.md`.

Idle compaction (`crates/bot/src/idle_compaction.rs`) is another specialized
maintenance callsite: it resumes a session to run `/compact` with **no
`--json-schema` and no `--mcp-config`**, judging success by exit status (the
`/compact` `result` is empty). It runs under the per-session `SessionLocks`
mutex and is never a normal deliverable turn.

### Reflection Primitive

`crates/bot/src/reflection.rs` exposes
`reflect_on_failure(ctx) -> Result<String, ReflectionError>`. On CC
invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a
human-friendly summary of the failure.

Reflection never reflects on itself. Hindsight `memory_retain` is skipped
for reflection turns. `async_runs.status` gates delivery: `'failed'`
routes to `DELIVERY_INSTRUCTION_FAILURE`; any other status routes to
`DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

See: `docs/architecture/sessions.md` for `ReflectionLimits` (worker vs
cron), usage-event accounting, and label-routing detail.

### Stream Logging

See: `docs/architecture/sessions.md` (Stream Logging).

### Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a
`ScheduleKind` variant. The **`Immediate`** variant (encoded as
`schedule = '@immediate'`) is bot-internal and fires on the next reconcile
tick (≤5s). Immediate jobs default `lock_ttl` to
`IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`); the heartbeat is written once at
job start and never refreshed, so a tighter TTL would let the reconciler
spawn a duplicate run against the same spec on the next 5-second tick.
The TTL is the duplicate-prevention guard, not a wall-clock execution
limit.

Background continuations are `async_runs` rows with `kind = 'background'`,
not cron specs. Legacy `@bg:<fork_from-uuid>` rows are not schedulable —
cron loading skips them. No runtime path creates or executes cron-backed
background continuations.

See: `docs/architecture/sessions.md` for the full variant list, the
background handoff lifecycle, and the per-session mutex on `--resume`.

### Configuration Hierarchy

| Scope | File | Source of Truth | Category |
|-------|------|-----------------|----------|
| Global | `~/.right/config.yaml` | Tunnel config | `AgentOwned` (edited by user) |
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, sandbox.name, env | `MergedRMW` |
| Generated | `agents/<name>/.claude/settings.json` | CC behavioral flags (regenerated on bot startup) | `Regenerated(BotRestart)` |
| Generated | `agents/<name>/.claude.json` | Trust, onboarding suppression (read-modify-write) | `MergedRMW` |
| Generated | `agents/<name>/.mcp.json` | MCP server entries (only "right" — externals managed by Aggregator) | `Regenerated(BotRestart)` |
| Agent-owned | `agents/<name>/TOOLS.md` | Agent-owned (created empty on init, then agent-edited) | `AgentOwned` |
| Per-agent | `agents/<name>/policy.yaml` | OpenShell sandbox policy (generated by agent init) | `Regenerated(SandboxRecreate)` |

See [Upgrade & Migration Model](#upgrade--migration-model) for category
definitions.

**Hot-reloadable fields in `agent.yaml`.** Most fields trigger a graceful
restart on change (via `config_watcher`). Three exceptions: `model`,
`debug`, and `sandbox.providers`. The watcher's smart-diff classifies a
`model`/`debug`-only change as hot-reloadable and stores the new values
into `AgentSettings.model` (an `Arc<ArcSwap<...>>`) and
`AgentSettings.debug` (an `Arc<AtomicBool>`) without restarting. A
`sandbox.providers`-only change (Stage B of the diff) is classified
`ProvidersReload`: it applies model/debug in-memory and signals an async
`sandbox_supervisor::hot_reconcile_providers` (provider-aware policy
re-apply via `openshell policy set --wait` + gateway attach/detach
reconcile) instead of restarting. The Telegram `/model` and `/debug`
commands exploit the hot-reload path — in-flight CC subprocesses keep
their old flags; the next invocation in any chat picks up the new value.
Adding more hot-reloadable fields requires extending the two-stage diff in
`crates/bot/src/config_watcher.rs::diff_classify`.

### Skill learning

Adding a new learning-adjacent invocation requires extending
`right_agent::usage::LEARNING_SOURCES` so both the budget gate and the
dashboard `SOURCES` array pick it up; the dashboard test
(`usage_overview_sources_match_learning_sources_constant`) enforces sync
via a dev-dep cross-crate assertion.

The per-turn pipeline runs from two call sites through the shared
`bot::learning_pipeline::run_post_turn`: foreground `Normal` turns and
recurring-cron successes (`ScheduleKind::Recurring`; one-shot cron runs are
excluded). No new `LEARNING_SOURCES` entry — cron learning *is*
`learning_prefilter` + `learning_probe_writer` spend.

Skill lifecycle state lives in `data.db.skill_lifecycle`. Curator
transitions skip pinned rows. The dashboard is the only operator
pin/unpin surface — do not add CLI pinning.

The prefilter and probe-writer skill index is read from inside the sandbox
(`/sandbox/.claude/skills/rightx-*`) via gRPC `exec_in_sandbox`; the host
path is used only for `sandbox: mode: none` agents. A prefilter
skill-index read error returns `Skip`, never an empty index (an empty
index would allow the classifier to recommend creating a skill that
already exists).

Per-skill learning cost and cache are recorded in `data.db.skill_spend`
(kinds `create`/`patch`/`maintain`/`usage`), separate from `usage_events`;
budget-blocked attempts are counted in `data.db.learning_skip`
(`reason='budget'`, `intended_kind` always NULL because the classifier
does not run when budget is exhausted).

See: `docs/architecture/learning.md` for the full pipeline, gate
ordering, spend ledger details, and removed Stage 2 paths.

### Memory

Two modes, configured per-agent via `memory.provider` in `agent.yaml`:
**Hindsight** (primary, Hindsight Cloud API) and **file** (fallback,
agent-managed `MEMORY.md`). MCP tools `memory_retain` / `memory_recall` /
`memory_reflect` are exposed only in Hindsight mode.

Conversation transcript search is separate from Hindsight. It uses local
Turso FTS indexes over archived Telegram messages and is scoped by the
current foreground invocation (see MCP Aggregator above).

See: `docs/architecture/memory.md` for auto-retain/recall semantics,
prefetch cache behavior, cron-skip rules, backgrounded-turn handling, and
the resilience layer.

### Memory Schema

Per-agent `data.db` schema lives in `right-db` migrations
(`right_db::migrations::MIGRATIONS`). Run `sqlite3 data.db .schema` for
column-level definitions.

### Providers

Provider credential management is owned by `right_openshell::providers`.
Credentials never enter `agent.yaml`, backups, or host logs. Per-agent
provider list lives in `agent.yaml::sandbox::providers: [...]`. Gateway
holds the credential bytes. Sandbox sees opaque placeholder env vars
substituted at the proxy on outbound HTTPS.

See: `docs/architecture/providers.md` for the placeholder mechanism,
substitution flow, reconciler walkthrough, and policy interaction.

## External Integrations

See: `docs/architecture/integrations.md` for the protocol inventory.

## Runtime isolation — mandatory

All interaction with the running `process-compose` instance MUST go
through `PcClient::from_home(home)`. The `PcClient::new(port)`
constructor is crate-private; external callers cannot construct a client
without a `home`.

This guarantees that `right --home <path>` is actually isolated: when a
command is run against a tempdir home with no `state.json`, `from_home`
returns `None` and callers skip PC-touching logic. This property is what
protects tests (which run with a `--home=<tempdir>`) from accidentally
hitting the user's live PC on port 18927 and SIGTERM-ing a same-named
process there.

`<home>/run/state.json` carries the port and API token the running PC
uses; it is written by `right_codegen::pipeline` during `right up` and
read by every subsequent command that needs to talk to PC. Older state
files without the `pc_port` field deserialize to `PC_PORT` via
`#[serde(default)]`.

### PC_API_TOKEN authentication

`right up` generates a random API token (`pc_api_token` in `state.json`)
and passes it to process-compose via `PC_API_TOKEN` env var. PcClient
includes it in every request as the `X-PC-Token-Key` header
(process-compose's only supported scheme — does NOT honor
`Authorization: Bearer`).

**When adding new CLI commands that touch PC, never import `PC_PORT`
directly — always resolve through `from_home(home)`.** For "is PC
running?" probes, treat `Ok(None)` as "no — skip or fail with a clear
message pointing at `right up`". `PC_PORT` may still be referenced by
`cmd_up` (passing `--port` to launch PC) and `pipeline.rs` (default into
`state.json`).

## Local Database Rules

Per-agent `data.db` is a SQLite-compatible database. Runtime local
storage uses the `turso` crate with `sync` enabled for future Turso Cloud
backup work, and that driver implementation is hidden behind `right-db`.

`right-db` is the only crate that owns local database-driver details.
Local filesystem-backed opens must enable Turso's experimental
index-method feature (FTS uses `CREATE INDEX ... USING fts`) and the
experimental multiprocess-WAL path so bot and MCP aggregator processes
can open the same per-agent `data.db`; this may create Turso sidecar
files such as `data.db-tshm`. Files matching `data.db-*` are disposable
runtime sidecars, not durable backup state — backup and restore flows
preserve only the canonical `VACUUM INTO` snapshot stored as `data.db`.
The in-memory test/helper path is the exception: Turso does not support
multiprocess WAL for `:memory:`. Other crates must use project-owned
`right_db` types and must not expose raw `turso` connection, transaction,
row, error, value, or parameter types in public APIs.

The runtime database API is async-first: `open_connection`, `open_db`,
`query_*`, migrations, and transactions are awaited directly by callers.
Do not add sync facades, runtime `block_on` bridges, or shared-runtime
adapters around `right-db`.

`right-db` may use bundled `rusqlite` only inside locked `migrate: true`
schema bootstrap for legacy FTS5 cleanup; it is not a general runtime
database boundary.

### Migration Ownership

Both `right-mcp-server` and bot processes run schema bootstrap on
per-agent `data.db` via `right_db::open_connection(path, migrate: true)`.
This is the only path that may run legacy schema cleanup or migrations.
`right-db` serializes that bootstrap with a per-agent advisory lock file
so concurrent startup is safe without relying on process-compose
ordering. Under the lock, `right-db` may use bundled `rusqlite` to drop
legacy SQLite FTS5 virtual tables and sync triggers before opening
through Turso, because Turso cannot resolve every old FTS5 schema.
Runtime opens with `migrate: false` do not run the scrubber, do not
inspect legacy FTS tables, and do not apply migrations. Read-only
helpers do not mutate files. The migration registry
(`right_db::migrations::MIGRATIONS`) is the sole place to add new tables.

All pending migrations run inside a single immediate transaction.
Rollback is all-or-nothing: a failure at migration N rolls back every
prior migration in the same batch and leaves `user_version` unchanged. A
concurrent caller that opens the database during a cold-boot batch
blocks on that transaction for the full batch duration, not just the
next pending version, and may exhaust the 5s `busy_timeout` under WAL.
Tests must not assume per-version commit boundaries; see
`migration_runner_semantics_rolls_back_all_pending_migrations_on_later_failure`.

### Transaction Rule

Any operation that performs 2+ writes (INSERT, UPDATE, DELETE) MUST use a
single immediate transaction from `Connection::transaction().await`,
then commit explicitly with `Transaction::commit().await` after all
writes succeed. If a caller intentionally aborts a transaction before
other connections may write, it must call `Transaction::rollback().await`;
dropping a Turso transaction only schedules rollback on the next use of
that same connection. Single-statement writes don't need a transaction.
Migrations are the sole exception — the migration runner wraps each
batch.

### Idempotent Migrations

All migrations must be idempotent — safe to re-run if the schema already
matches. SQLite-compatible DDL lacks `ADD COLUMN IF NOT EXISTS`, so
column additions must check `pragma_table_info` first. Use a Rust
migration hook for conditional DDL. `CREATE TABLE/INDEX/TRIGGER IF NOT
EXISTS` is naturally idempotent.

## Upgrade & Migration Model

Every change that touches codegen, sandbox config, or on-disk state must
be deployable to already-running production agents. Manual migration
steps, `right agent init`, or sandbox recreation are NEVER acceptable as
upgrade paths.

### Codegen categories

Every per-agent codegen output belongs to exactly one category:

| Category | Semantics | Examples |
|---|---|---|
| `Regenerated(BotRestart)` | Unconditional overwrite every bot start. Takes effect on next CC invocation. | settings.json, mcp.json, schemas, system-prompt.md |
| `Regenerated(SandboxPolicyApply)` | Overwrite + `openshell policy set --wait`. Network-only. | policy.yaml (network section) |
| `Regenerated(SandboxRecreate)` | Overwrite + triggers sandbox migration. Filesystem/landlock and other boot-time-only changes. | policy.yaml (filesystem section) |
| `MergedRMW` | Read, merge, write. Preserves unknown fields. | .claude.json, agent.yaml (secret injection) |
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

Cross-agent outputs (process-compose.yaml, agent-tokens.json,
cloudflared config) are all `Regenerated(BotRestart)` — reread on
`right up`.

### Helper API

`crates/right-codegen/src/contract.rs` provides the only sanctioned
writers:

- `write_regenerated(path, content)` — all `Regenerated` outputs except
  `SandboxPolicyApply`.
- `write_regenerated_bytes(path, content)` — byte variant for non-UTF-8
  payloads.
- `write_merged_rmw(path, merge_fn)` — read-modify-write with
  unknown-field preservation.
- `write_agent_owned(path, initial)` — no-op if file exists.
- `write_and_apply_sandbox_policy(sandbox, path, content).await` — the
  ONLY way to update policy for a running sandbox. Writes + applies
  atomically via `openshell policy set --wait`.

Direct `std::fs::write` inside codegen modules is a review-blocking
defect.

### Rules for adding a new codegen output

1. Pick a category. Add a `CodegenFile` entry to the matching registry
   (`codegen_registry()` or `crossagent_codegen_registry()`).
2. Use the matching helper. No bare `std::fs::write`.
3. Run `cargo test registry_covers_all_per_agent_writes` to verify the
   registry is complete.
4. If `Regenerated(SandboxRecreate)` — exercise the migration path
   manually.
5. If the new output is policy-related, apply via
   `write_and_apply_sandbox_policy` only. Adding a new network endpoint
   is fine; adding a new filesystem rule requires `SandboxRecreate`
   treatment.
6. Never require `right agent init` for existing agents to adopt the
   change. They upgrade via `right restart <agent>`.

See: `docs/architecture/upgrades.md` for the upgrade-flow walkthrough,
identity-mirror semantics, and non-goals.

### Cross-references

- `AGENTS.md` → `Upgrade-friendly design`, `Never delete sandboxes for
  recovery`, `Self-healing platform` — conventions this model implements.
- `docs/architecture/lifecycle.md` → `Sandbox migration (filesystem
  policy change)` — the migration flow used by
  `Regenerated(SandboxRecreate)`.

## Integration Tests Using Live Sandboxes

Any test that needs a live OpenShell sandbox MUST create it via
`right_openshell::test_support::TestSandbox::create("<test-name>")`
(unique sandbox name, automatic cleanup under panic, gRPC-only command
execution). Consumers outside `right-agent`'s own unit tests depend on
the `test-support` feature.

Rules:

- Never hardcode sandbox names.
- Never invoke the `openshell` CLI from tests. Use the `TestSandbox`
  helper or the gRPC helpers in `right_openshell::openshell`.
- Never add `#[ignore]` to sandbox tests. Dev machines have OpenShell.
- `TestSandbox` holds a `SandboxTestSlot` for its lifetime. Direct tests
  that bypass `TestSandbox` and call `spawn_sandbox` must acquire
  `acquire_sandbox_slot()` themselves.
- CI may set `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS` low to throttle only
  live sandbox creation while preserving normal Cargo test parallelism.
  Use at least `2` in jobs with a process-lifetime shared sandbox.
- CI may raise `RIGHT_TEST_SANDBOX_READY_TIMEOUT_SECS` and
  `RIGHT_TEST_SANDBOX_SSH_TIMEOUT_SECS` for cold OpenShell runners;
  local defaults stay short (`120s` READY, `60s` SSH).

## Security Model

- **Sandbox isolation**: OpenShell (k3s containers) — filesystem +
  network + TLS policies per agent.
- **TLS MITM**: OpenShell proxy terminates and re-signs TLS with
  per-sandbox CA for L7 inspection.
- **Credential isolation**: Host credentials never uploaded to sandbox.
  Each sandbox authenticates independently via OAuth login flow.
- **Network policy**: Scoped wildcard domain allowlists
  (`*.anthropic.com`, `*.claude.com`, `*.claude.ai`) or hostless public
  `allowed_ips` endpoint allowlists + `binaries: "**"`. TLS termination
  is automatic (OpenShell v0.0.30+).
- **`--dangerously-skip-permissions`**: Always on for all CC invocations.
  OpenShell policy is the security layer, not CC's permission system.
- **Prompt-injection defense**: `ironclaw_safety::Sanitizer` runs on
  memory writes (Hindsight retain path) and `wrap_external_content`
  frames the `## Memory` section as untrusted data on read. Phase-2 wrap
  is the primary defense; phase-1 sanitize is hygiene. See
  `docs/architecture/memory.md`.
- **Chat ID allowlist**: Empty = block all (secure default); per-agent
  in `agents/<name>/allowlist.yaml`. Legacy
  `agent.yaml::allowed_chat_ids` is migration input only.
- **Protected MCP**: "right" cannot be removed via dashboard MCP controls.
- **MCP tool restriction**: Agents cannot register/remove external MCP
  servers — `mcp_add`, `mcp_remove`, `mcp_auth` are not exposed as MCP
  tools. Only the user can manage servers via the Telegram dashboard MCP
  view routed through the internal Unix socket API. Prevents sandbox
  escape via data exfiltration to attacker-controlled MCP endpoints.
- **Sandboxed CC fails closed**: a sandboxed agent (`resolved_sandbox` set) runs Claude Code only inside its sandbox — every CC-command-construction site MUST call `guard_no_sandboxed_host_exec` (`crates/bot/src/cc/invocation.rs`), which refuses to build a host command when the sandbox connection is absent. On backend outage the worker replies with a diagnosis and skips CC; `SandboxSupervisor` (the sole writer of `SandboxRuntimeHandle` health) retries with backoff — no host fallback ever occurs.
- **OAuth CSRF**: Token matching in callback server.
- **Provider credential isolation**: Provider credential values and
  gateway placeholder values (`openshell:resolve:env:v…_<NAME>`) are
  never logged on the host. Use `secrecy::SecretString` for in-memory
  transport; do not pass credential fields to tracing macros.
  Placeholder substitution is keyed by env-var name on any TLS-terminated
  endpoint, NOT scoped to the owning provider's host — do not rely on
  provider-profile endpoints to confine a credential (OpenShell
  limitation; raw `tls: skip` hosts never substitute, so credentials
  can't reach the open internet). See `docs/architecture/providers.md` and
  onsails/right-agent#92.

## Brand-conformant CLI output

Every user-facing TUI surface in `right` and `right-bot` MUST go through
`right_ui::*` (see `crates/right-ui/src/`). Raw `println!` / `eprintln!`
of user-facing text is a review-blocking defect. Visual contract, atoms,
and theme rules: `docs/brand-guidelines.html` and the redesign spec at
`docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md`.

## Telegram message UX

Bot-authored Telegram HTML messages MUST escape untrusted text before
setting `ParseMode::Html`. Shared send helpers should preserve effective
topic thread ids. Do not send raw CLI-style prefixes such as `Warning:`
or `Failed:` when a clear user-facing sentence is available.

## Dashboard frontend primitives

`right-dashboard` Vue views MUST render loading/empty/error through
`components/AsyncState.vue` (backed by the pure `components/asyncState.ts`
resolver, priority error > loading > empty > content) and MUST render
collapsible grouped lists through `components/CollapsibleSection.vue`. Raw
placeholder text (`'not loaded'`, `'unavailable'`, ad-hoc `v-if="loading"`
Loading lines) in a view is a review-blocking defect — it reintroduces the
loading-flash these primitives exist to prevent. Identity per-file state
labels go through `components/identityLabels.ts`, never raw enum codes.
Component tests use Vue SSR (`@vue/server-renderer` `renderToString`); pure
decision logic is extracted to a `*.ts` helper and unit-tested directly.

RightClaw-owned technical identifiers are `right-*` namespaced; the dashboard
MUST NOT surface raw slugs/prefixes, presenting user-friendly `display_name`
labels instead (e.g. `right-github` renders as "GitHub"). A superseded built-in
profile is kept in the catalog for env-var resolution but filtered from the
offered provider-type list server-side (`HIDDEN_FROM_DASHBOARD` in
`internal_api_providers.rs`), so the UI shows one flat list. Technical precision
lives in the backend; the UI optimizes for user clarity.

## OpenShell Integration Conventions

- **Use gRPC for everything except file transfer and `policy set --wait`**:
  every provider control-plane operation (Create/Get/Update/Delete/List/
  Attach/Detach/ListAttached/GetSandboxProviderEnvironment) goes through
  tonic-generated client stubs from the vendored `openshell.v1` proto.
  CLI remains only for: SSH+tar-backed file upload/download, and
  `openshell policy set --wait` (the policy hot-apply path). Adding a
  new provider operation by CLI is a review-blocking defect.
- **gRPC for**: sandbox create/get/delete, readiness polling, in-sandbox
  command execution, policy status, SSH session management, provider
  CRUD + sandbox attach/detach, Health (version preflight).
- **Readiness polling diagnostics**: `wait_for_ready` must preserve the
  last `GetSandbox` phase/status in timeout errors and treat
  `SANDBOX_PHASE_ERROR` as terminal. Do not collapse OpenShell status
  into a bare boolean in wait loops.
- **Sandbox create stdio**: `openshell sandbox create` stdout/stderr
  must be inherited or drained concurrently; never leave them piped and
  unread.
- **SSH remote argv must be quoted centrally**: For remote argv, call
  `right_openshell::openshell::quote_ssh_remote_args(...)` and pass
  exactly one argument after the SSH host or `--`. For authored shell
  scripts, pass exactly one complete script string. Never use
  `Command::args(...)` or `Vec::join(" ")` for remote argv after the SSH
  host.
- **CLI for**: file upload/download (SSH+tar under the hood), policy
  apply (`openshell policy set`).
- **Vendored proto compatibility is load-bearing**: OpenShell `v0.0.50`
  is the minimum supported version (CLI and gateway).
  `crates/right-openshell/proto/UPSTREAM.md` records the pinned tag and
  fetch date. To bump: run `scripts/vendor-openshell-proto.sh <tag>` and
  `cargo check -p right-openshell` to regenerate stubs. Older protos
  lacked `AttachSandboxProvider` / `DetachSandboxProvider` /
  `ListSandboxProviders` RPCs — the providers feature depends on them.
  `right_openshell::preflight::openshell_preflight` enforces this at
  bot startup; both CLI (`openshell --version`) and gateway (Health
  RPC) must report `>= MIN_OPENSHELL_VERSION`.
  `resolve_sandbox_id` must read `metadata.id`;
  `ci_openshell_policy_validates_against_openshell` is the live
  regression gate.
- **NEVER use the CLI for in-sandbox command execution**: `openshell
  sandbox exec` CLI has unreliable argument parsing. Always use gRPC
  `exec_in_sandbox`.
- **Known CLI bug**: Directory uploads may silently drop small files.
  Always verify critical files after directory upload, and re-upload
  individually if missing.
- **All Provider operations** (Create/Get/Update/Delete/ListProviders,
  sandbox attach/detach, GetSandboxProviderEnvironment,
  ensure_v2_enabled) MUST go through `right_openshell::providers`. Direct
  gRPC or `openshell provider` CLI invocations from other crates are a
  review-blocking defect.

## OpenShell Policy Gotchas

- **Do not emit deprecated `tls:` modes** (OpenShell v0.0.30+). The
  proxy auto-detects TLS via ClientHello peek and terminates for L7
  endpoints. Writing `tls: terminate` or `tls: passthrough` triggers a
  per-request `WARN` and the field is slated for removal. Omit the field
  for auto-detect; use `tls: skip` only to explicitly disable
  termination (raw tunnel).
- `binaries: path: "**"` not `"/sandbox/**"`. Claude lives at
  `/usr/local/bin/claude`.
- `protocol: rest` and `access: full` are required only for endpoints
  that intentionally use L7 HTTP policy on terminated plaintext.
- Permissive public internet endpoints are hostless public `allowed_ips`
  raw tunnels (`tls: skip`, no `protocol`/`access`) on ports 80/443. Do
  not add L7 REST policy there: OpenShell rejects encoded `/` (`%2F`)
  request-targets used by scoped npm package metadata.
- Scoped wildcard domains (`*.anthropic.com`) work.
- OpenShell v0.0.37+ rejects TLD/global host wildcards. Permissive
  public internet policy must use hostless public `allowed_ips`
  endpoints.
- CC actively manages `.claude.json` — strips unknown project trust
  entries on startup. Use `--dangerously-skip-permissions` instead of
  relying on trust entries.
- `HTTPS_PROXY=http://10.200.0.1:3128` is set automatically inside
  sandbox. All HTTP/HTTPS goes through the proxy.
- **Host service access from sandbox** (`host.openshell.internal`):
  requires exact sandbox-resolved `allowed_ips` per policy endpoint.
  Every resolved private/internal IP must be allowed. Do not hardcode
  host gateway IPs and do not permanently allow broad private/ULA ranges
  for Right MCP.
- **Right MCP policy lifecycle**: codegen writes a bootstrap unresolved
  Right MCP endpoint; runtime resolves `host.openshell.internal` inside
  the sandbox, hot-applies all unique IPs as `/32`/`/128` before any
  Claude invocation, and repeats on bot startup so backup/restore and
  host migration self-heal stale IPs. See `docs/architecture/sandbox.md`
  for the resolution sequence. `openshell forward` is not the Right MCP
  route.
- **Host MCP bind address**: the host-side Right MCP aggregator must
  bind `0.0.0.0` for sandbox access. OpenShell always blocks
  loopback/link-local/unspecified destinations.
- **Sandbox user-local executables**: `/sandbox/.local/bin` is the only
  platform-supported install target (sourced via `/sandbox/.right/env.sh`
  from `/sandbox/.bashrc` and before sandboxed `claude -p`). npm uses
  `NPM_CONFIG_PREFIX=/sandbox/.local` + `NPM_CONFIG_CACHE=/sandbox/.npm`.
  Do not document or generate `~/bin`.
- **NixOS users**: must add `networking.firewall.trustedInterfaces = [
  "docker0" "br-+" ];` — OpenShell runs k3s on a custom Docker bridge,
  and without this the firewall drops k3s-pod-to-host traffic.
- **Filesystem policy changes require sandbox recreation**: `openshell
  policy set --wait` hot-reloads network policies but does NOT apply
  filesystem policy changes to running sandboxes. Landlock rules are set
  at sandbox creation time.

## Directory Layout & Logging

See: `docs/architecture/layout.md` for the `~/.right/` inventory and
logging destinations.
