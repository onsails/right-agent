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

### Agent Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as
long as the agent lives, run detached, and survive bot restarts.

Egress policy and initial resources are create-time. Provider values rotate
live; a missing binding may be added by SDK-managed restart, which preserves
the persistent sandbox filesystem.

Live-microVM coverage is CI-explicit: tests that boot a real sandbox use
`#[ignore = "ci-msb: ..."]` with a `ci_msb_` test-name prefix (enforced by
`crates/right/tests/ci_ignored_contract.rs`) and run in the opt-in
`sandbox` job in `.github/workflows/tests.yml`, which needs a runner
exposing `/dev/kvm`. Everything that does not need a live VM stays in the
default workspace test path.

See: `docs/architecture/sandbox.md` for the bring-up sequence, staging-dir
layout, platform-store deployment, egress/secret model, and graceful
degrade.

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
disallow it. `mcp__right__send_message` is foreground-only, capped at 20
calls/turn, with chat/thread scope server-resolved from the registered
invocation — never from agent args (same rule as `send_progress`);
attachment paths MUST be under `/sandbox/outbox/`.

Conversation transcript scope is server-enforced.
`mcp__right__thread_search` searches only the current
`(chat_id, effective_thread_id)`. `mcp__right__chat_search` searches only
the current `chat_id` (a DM only that DM; a group the whole group across
topics). `mcp__right__get_messages_by_id` is scoped to the same
chat/thread pair; agents supply only `message_ids`, and ids outside scope
or not archived are absent.
Agents must never be allowed to pass chat_id, thread_id, user ids, session
ids, or a broader scope to these tools.
`mcp__right__thread_focus_set` writes agent focus for the current
conversation scope, never agent args.

`mcp__right__forum_topic_list` returns only the current `chat_id`'s tracked
topics, resolved server-side. Forum write tools (`forum_topic_create`/`_edit`/
`_close`/`_reopen`) resolve `chat_id` identically, never from agent args; no
delete tool exists.

`mcp__right__channel_read`/`channel_post` are the exception:
they accept an agent-supplied `channel` id, valid only against `kind:
channel` allowlist entries, validated on both aggregator and bot sides.
`channel_list` takes no parameters.

`mcp__right__cron_trigger` resolves the origin chat for a `then` follow-up from
the foreground invocation's conversation scope; agents never pass it.

See: `docs/architecture/mcp.md` for the REST surface and dispatch detail.

### Prompting Architecture

Session-bearing `claude -p` invocations get a composite system prompt via
`--system-prompt-file` (the sole prompt mechanism — no `--agent` flag).
Prompt caching is critical — avoid per-message tool calls to read
identity files. Foreground worker prompts MUST use per-session prompt-file
paths because their `## Current Conversation` block is session-scoped; other
session-bearing composite callers may omit chat context and use their existing
prompt paths. The system prompt contains only stable/base prompt content, mode
instructions, identity files, TOOLS, optional foreground-worker chat context,
MCP instructions, optional foreground-worker operator focus, and file-mode
`MEMORY.md`. Hindsight recall, agent-saved focus, memory-status markers, and
repair notices MUST be prepended to stdin/user message instead; agent-saved
focus is sanitized and externally wrapped, not system prompt content.

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
for reflection turns.

The worker aborts after 3 consecutive structured-output schema rejections
and routes to reflection (`FailureKind::StructuredOutputLoop`); reflection
runs on a separate path and never triggers the detector.

A successful turn with `content: null` but undelivered assistant text gets
one repair resume; a second null confirms intentional silence (see
`docs/architecture/sessions.md`).

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

`policy.yaml` is gone: egress is a typed value on the sandbox spec, applied
at create time, so there is no generated policy file and no policy-apply
category.

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
`sandbox_supervisor::hot_reconcile_providers`, which revokes obsolete
Right-managed bindings live, rotates existing bindings, and restart-adds a
supported missing binding without deleting the sandbox. The Telegram `/model`
and `/debug` commands exploit the hot-reload path — in-flight CC subprocesses
keep their old flags; the next invocation in any chat picks up the new value.
Adding more hot-reloadable fields requires extending the two-stage diff in
`crates/bot/src/config_watcher.rs::diff_classify`.

### Skill learning

Adding a new learning-adjacent invocation requires extending
`right_agent::usage::LEARNING_SOURCES` so both the budget gate and the
dashboard `SOURCES` array pick it up (enforced by a dev-dep cross-crate
assertion in the dashboard tests).

The per-turn pipeline runs from two call sites through the shared
`bot::learning_pipeline::run_post_turn`: foreground `Normal` turns and
recurring-cron successes (`ScheduleKind::Recurring`; one-shot cron runs are
excluded). No new `LEARNING_SOURCES` entry — cron learning *is*
`learning_prefilter` + `learning_probe_writer` spend.

The agent may also author/patch `rightx-*` skills inline mid-turn (foreground,
and cron via the learning-capable `Cron` invocation kind); the async probe is
skipped on any turn that did so. Inline `foreground`/`cron` provenance skills
are curator-auto-managed like `probe_writer`/`curator`.

Skill lifecycle state lives in `data.db.skill_lifecycle`. Curator
transitions skip pinned rows. The dashboard is the only operator
pin/unpin surface — do not add CLI pinning.

The prefilter and probe-writer skill index is read from inside the sandbox
(`/sandbox/.claude/skills/rightx-*`) via a guest exec — the only source;
there is no host fallback. A prefilter skill-index read error returns
`Skip`, never an empty index — empty would let the classifier recommend
creating a skill that already exists.

Per-skill learning cost and cache are recorded in `data.db.skill_spend`
(kinds `create`/`patch`/`maintain`/`usage`), separate from `usage_events`;
budget-blocked attempts are counted in `data.db.learning_skip`
(`reason='budget'`, `intended_kind` always NULL — classifier doesn't run).

`cron_skill_links` is the only reliable cron→skill provenance carrier
(`created_by` can't express it); link MCP tools resolve scope server-side,
multi-row writes share one transaction, deletion cascades via `delete_spec`.
See `docs/architecture/learning.md` for pipeline, spend ledger, and cron↔skill linking.

### Memory

Two modes, configured per-agent via `memory.provider` in `agent.yaml`:
**Hindsight** (primary, Hindsight Cloud API) and **file** (fallback,
agent-managed `MEMORY.md`). MCP tools `memory_retain` / `memory_recall` /
`memory_reflect` are exposed only in Hindsight mode.

Conversation transcript search is separate from Hindsight. It uses local
Turso FTS indexes over archived Telegram messages and is scoped by the
current foreground invocation (see MCP Aggregator above).

See: `docs/architecture/memory.md` for auto-retain/recall semantics,
prompt placement, prefetch cache behavior, cron-skip rules,
backgrounded-turn handling, and the resilience layer.

### Memory Schema

Per-agent `data.db` schema lives in `right-db` migrations
(`right_db::migrations::MIGRATIONS`). Run `sqlite3 data.db .schema` for
column-level definitions.

### Providers

Provider credential management is owned by `right_providers::ProviderStore`
(`~/.right/providers.db`, SQLite mode 0600). No credential value ever
crosses a store read API: `ProviderRecord` carries no credential field and
`source_ref_binding` is the only reader. Per-agent provider definitions live
in `agent.yaml::sandbox::providers: [...]`; ownership is a `providers.db`
column (`owner_agent` + `provider_borrows`), never the `agent.yaml`
`shared_from` field (legacy migration input only; the internal API writers
never emit it).

Provider health is the tri-state `ready` / `needs-value` / `error`. The
OpenShell-era composition machinery (`ensure_v2_enabled`,
`wait_for_provider_composed*`, effective-policy reads) is deleted along
with the gateway itself; the internal provider API
(`internal_api_providers.rs`) talks only to the store. Rotation and
config-update fail fast on a record whose built-in slug no longer resolves
(`unknown_builtin_slug`, HTTP 500); the list view marks such a row `error`
instead of aborting.

Cross-agent SHARING (`provider_share`/`_unshare`) attaches a borrowed
reference pointing at the true owner (actor trusted in **both**); no secret
is read back or copied. A borrowed record is read-only for its borrower
(409 `borrowed_read_only`); owner deletion re-homes the record to a
surviving borrower. Dashboard mutations serialize per agent via
`provider_lock` (validate name first; share locks the DEST agent only).

See: `docs/architecture/providers.md` for the source-ref secret mechanism,
store row model, cross-agent sharing, and the dashboard surface.

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

`right-db` opens every per-agent `data.db` through Turso. Pre-v34 SQLite
FTS5 cleanup is no longer supported in-process: the bundled-`rusqlite`
scrubber was removed (onsails/right-agent#79) after deployed databases
soaked past migration v34. A database still carrying legacy SQLite FTS5
virtual tables that Turso cannot open now fails the open instead of being
scrubbed.

### Migration Ownership

Both `right-mcp-server` and bot processes run schema bootstrap on
per-agent `data.db` via `right_db::open_connection(path, migrate: true)`.
This is the only path that may run migrations. `right-db` serializes that
bootstrap with a per-agent advisory lock file so concurrent startup is
safe without relying on process-compose ordering. Migration v34 drops any
remaining legacy SQLite FTS5 triggers/virtual tables and creates the Turso
FTS indexes; it stays idempotent for any database Turso can already open.
Runtime opens with `migrate: false` do not apply migrations. Read-only
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
| `MergedRMW` | Read, merge, write. Preserves unknown fields. | .claude.json, agent.yaml (secret injection) |
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

There is no sandbox-policy or sandbox-recreate codegen category: egress is a
create-time sandbox-spec property, while provider bindings reconcile through
the runtime rather than a generated file.

Cross-agent outputs (process-compose.yaml, agent-tokens.json,
cloudflared config) are all `Regenerated(BotRestart)` — reread on
`right up`.

### Helper API

`crates/right-codegen/src/contract.rs` provides the only sanctioned
writers:

- `write_regenerated(path, content)` — all `Regenerated` outputs.
- `write_regenerated_bytes(path, content)` — byte variant for non-UTF-8
  payloads.
- `write_merged_rmw(path, merge_fn)` — read-modify-write with
  unknown-field preservation.
- `write_agent_owned(path, initial)` — no-op if file exists.

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

A live Agent Sandbox is a booted microVM, so every test that needs one is
`#[ignore = "ci-msb: ..."]` with a `ci_msb_` name (the marker/prefix contract
is enforced by `crates/right/tests/ci_ignored_contract.rs`). Run them with
`cargo nextest run -p <crate> --run-ignored all`.

Helpers live in `crates/right-sandbox/tests/common/mod.rs`:
`ensure_runtime_installed()` behind a cross-process install lock,
`acquire_vm_slot()` for the host-global concurrent-microVM cap
(`RT_MSB_MAX_CONCURRENT_VMS`), and `SandboxGuard`, which owns a unique name
and deletes the sandbox on drop — including during a panicking unwind.

Rules:

- Never hardcode a sandbox name, in a live test *or* a unit test. The
  microsandbox catalog is host-global with no `--home` isolation, so a fixed
  name is a unit test that can delete a developer's real microVM. Derive one
  per run from the PID and clock.
- Never leak a microVM: acquire it through `SandboxGuard`, or destroy it in a
  drop guard of your own.
- Acquire a VM slot before booting; probe VMs are GiB-scale and several
  worktrees may run probes at once.

## Security Model

- **Sandbox isolation**: microsandbox microVM — hardware-isolated guest
  with its own kernel, per-agent network egress policy.
- **TLS interception**: a bypass deny-list (ADR-0003). Declaring any
  provider secret enables interception on the intercepted ports for every
  destination *except* `right_sandbox::TLS_BYPASS_HOSTS`, which always
  carries the Anthropic hosts — so Claude's own path is never intercepted
  and the guest needs no CA configuration.
- **Credential isolation**: Host credentials are never uploaded to the
  sandbox. Each sandbox authenticates independently via the OAuth login
  flow, and provider credentials reach it as source-ref bindings whose
  value is substituted on the wire, never written into the guest.
- **Network policy**: `permissive` (open egress) or `restrictive` (a
  domain-suffix allow list: `anthropic.com`, `claude.com`, `claude.ai`,
  `storage.googleapis.com`, plus the always-open host destination group).
  Egress is create-time only — changing it needs a sandbox recreate.
- **`--dangerously-skip-permissions`**: Always on for all CC invocations.
  The microVM boundary is the security layer, not CC's permission system.
- **Prompt-injection defense**: `ironclaw_safety::Sanitizer` runs on
  memory writes (Hindsight retain path) and `wrap_external_content`
  frames the `## Long-Term Memory` section as untrusted data on read. Phase-2 wrap
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
- **Sandboxed CC fails closed**: a sandboxed agent runs Claude Code only inside its microVM — every CC-command-construction site MUST call `guard_no_sandboxed_host_exec` (`crates/bot/src/cc/invocation.rs`), which refuses to build a host command when the sandbox handle is absent. On backend outage the worker replies with a diagnosis and skips CC; `SandboxSupervisor` (the sole writer of `SandboxRuntimeHandle` health) retries with backoff — no host fallback ever occurs. The supervisor MUST verify reported sandbox failures by reading the real sandbox phase through the SDK, not by runtime reachability alone.
- **OAuth CSRF**: Token matching in callback server.
- **Provider credential isolation**: Provider credential values and the
  guest-visible placeholder values (`$MSB_<NAME>`) are never logged on the
  host. Use `secrecy::SecretString` for in-memory transport; do not pass
  credential fields to tracing macros. Substitution is keyed by env-var
  name on any intercepted endpoint, NOT scoped to the owning provider's
  host — do not rely on a provider's declared hosts to confine a
  credential. Bypassed (non-intercepted) hosts never substitute, so
  credentials cannot reach them. See `docs/architecture/providers.md` and
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

## Agent Sandbox Conventions

- **One SDK boundary**: `crates/right-sandbox` is the only crate that may
  depend on the microsandbox SDK, and no raw SDK type appears in its
  public API. A direct `microsandbox::` use anywhere else is a
  review-blocking defect.
- **One spec builder**: every writer that creates an agent's sandbox —
  the bot's supervisor and the `right` CLI — MUST go through
  `right_sandbox::agent_sandbox_spec`. Egress and secret structure are
  create-time only, so two builders drifting would silently produce
  sandboxes with different network reach, failing only at turn time.
- **One name resolver**: every lifecycle path (bring-up, destroy,
  rebootstrap, migrate) MUST resolve through
  `right_sandbox::resolve_sandbox_name`. Two call sites disagreeing about
  an agent's sandbox name is how a destroy orphans a microVM.
- **Readiness polling diagnostics**: `wait_ready` must preserve the last
  observed phase in timeout errors and treat a terminal phase as an
  immediate error. Do not collapse the phase into a bare boolean in wait
  loops.
- **Runtime install is implicit**: `ensure_runtime_installed` runs at bot
  startup and installs the SDK's own runtime; there is no PATH dependency
  and no operator install step. `diagnose_host` is the hypervisor
  preflight, and a hypervisor-less host degrades with a diagnosis rather
  than crashing.
- **SDK version**: `right_sandbox::PINNED_SDK_VERSION` records the exact
  SDK this crate is built against. The SDK pins its own `msb` runtime and
  `libkrunfw`, so Right maintains no separate version floor. Bumping it
  requires the `ci_msb_` contract suite to pass first.
- **Exec takes argv, never a shell string**: build an `ExecRequest` with
  a program and arguments. Guest stdin above the protocol frame cap MUST
  go through `ChunkedStdin`; a single oversized write tears the exec
  session down.
- **Provider secrets are source refs**: a `SecretBinding` names the
  guest-visible env var (holding a placeholder), the host env var the
  value is read from, and the hosts allowed to receive it. A credential
  value never enters a spec, the sandbox's durable config, or the guest.
- **Provider binding convergence**: existing provider values rotate live through
  `SandboxHandle::apply_secret`; missing bindings use the SDK's restart-backed
  modify path. Obsolete Right-managed bindings MUST be explicitly removed live
  before dashboard remove/unshare reports success. Final removal MUST persist
  TLS-off; SDK 0.6.10 leaves the active VM TLS-on but credential-free until its
  next start. Reconciliation MUST NOT remove sandbox secrets whose env-var
  identity was never declared by this agent's providers.
- **Provider binding identity is unique per agent**: two declared providers
  MUST NOT bind the same guest env var; reconciliation fails before sandbox
  mutation rather than ambiguously letting one record replace/revoke another.

## Sandbox Gotchas

- **Egress entries are domain suffixes, not globs**: `anthropic.com`
  already covers `*.anthropic.com`. Writing `*.anthropic.com` is wrong.
- **TLS interception is a deny-list, not an allow-list**: declaring any
  secret intercepts every destination on the intercepted ports except
  `TLS_BYPASS_HOSTS`. Adding a host to that list removes interception
  from it — treat the list as security-relevant inventory (ADR-0003).
- **Substitution is per-SNI host-gated**: the SDK substitutes a placeholder
  only on connections whose SNI matches its binding's `allowed_hosts`;
  cross-provider substitution is impossible (#92, OpenShell-era, closed).
- CC actively manages `.claude.json` — it strips unknown project trust
  entries on startup. Use `--dangerously-skip-permissions` instead of
  relying on trust entries.
- **Host MCP bind address**: the host-side Right MCP aggregator binds
  `127.0.0.1`, not `0.0.0.0`. The guest reaches it through the
  `host.microsandbox.internal` alias, which resolves to a host loopback
  address, so binding every interface would expose the aggregator to the
  local network for no benefit.
- **Sandbox executable PATH**: `/sandbox/.local/bin` is the supported guest
  install target and intentionally precedes the root-owned `/opt/right/bin`
  host-staged fallback. Both come from `/sandbox/.right/env.sh` before
  sandboxed `claude -p`; npm uses `NPM_CONFIG_PREFIX=/sandbox/.local` +
  `NPM_CONFIG_CACHE=/sandbox/.npm`. Do not document or generate `~/bin`.
- **Every network change needs a recreate**: there is no policy hot-apply.
  Egress is applied by the SDK at create, so an `agent.yaml`
  `network_policy` change takes effect only on a recreated sandbox.

## Directory Layout & Logging

See: `docs/architecture/layout.md` for the `~/.right/` inventory and
logging destinations.
