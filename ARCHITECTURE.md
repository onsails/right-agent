# Architecture

## Workspace

Nineteen crates in a Cargo workspace:

| Crate | Path | Role |
|-------|------|------|
| **right-platform-knobs** | `crates/right-platform-knobs/` | UX/prose tunables that should not invalidate platform foundations |
| **right-prompt-safety** | `crates/right-prompt-safety/` | Prompt-injection safety wrappers over `ironclaw_safety` |
| **right-runtime-state** | `crates/right-runtime-state/` | process-compose ports, runtime state JSON, and API-token generation |
| **right-config** | `crates/right-config/` | RIGHT_HOME resolution, global config YAML, agents/backups directory helpers |
| **right-ui** | `crates/right-ui/` | Brand-conformant CLI atoms, blocks, recaps, prompts, and theme detection |
| **right-process** | `crates/right-process/` | Cancel-safe process-group child handling |
| **right-openshell** | `crates/right-openshell/` | OpenShell gRPC/proto, CLI wrappers, sandbox exec, and live-test support |
| **right-platform-store** | `crates/right-platform-store/` | Content-addressed platform-managed sandbox file deployment |
| **right-agent-config** | `crates/right-agent-config/` | Agent configuration DTOs, discovery DTOs, sandbox/memory/STT schema types |
| **right-stt** | `crates/right-stt/` | Host-side STT model cache paths, ffmpeg detection, model download, cache warming |
| **right-db** | `crates/right-db/` | Per-agent SQLite-compatible `data.db` boundary over local Turso: project DB types, migrations, `sql/v*.sql` |
| **right-lifecycle** | `crates/right-lifecycle/` | Learned-skill lifecycle state machine and DB operations over `skill_lifecycle` |
| **right-mcp** | `crates/right-mcp/` | MCP aggregator backend, proxy, reconnect, credentials, token derivation, auth tokens |
| **right-codegen** | `crates/right-codegen/` | Per-agent codegen: settings.json, .mcp.json, prompts, process-compose, cloudflared, sandbox policy, bundled skills |
| **right-dashboard** | `crates/right-dashboard/` | Telegram Mini App dashboard DTOs, auth validation, read models, and static assets |
| **right-memory** | `crates/right-memory/` | Hindsight-resilience layer and retain queue |
| **right-agent** | `crates/right-agent/` | Slim orchestrator: agent discovery, runtime, init, doctor, rebootstrap, cron_spec, tunnel, usage |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |

**Re-export discipline:** The slim `right-agent` does not re-export modules
from `right-db`, `right-mcp`, `right-codegen`, or `right-memory`.
Consumers (CLI, bot, and agent internals) import directly from the source crate.
This keeps the build-cache invariant: an edit inside `right-codegen` rebuilds
`right-codegen` plus its direct consumers, not `right-agent`.

**Crate boundaries:** Phase 4 removed the former shared core crate. New shared
code must go to the most-specific owner crate. Anticipating reuse is not a
reason to create or centralize a shared abstraction; promote on demand, not on
prediction.

Every other crate has a single responsibility (see workspace table).
New code that doesn't fit an existing crate's charter gets its own
crate, not a misfit addition. Default placement for new code is the
most-specific leaf crate.

`right-dashboard` owns Telegram Mini App dashboard DTOs, Telegram `initData`
validation logic, read models, and static asset lookup. `right-bot` owns runtime
route mounting, Telegram menu/button integration, allowlist lookup, and custody
of the bot token used for server-side `initData` validation. The current v1
dashboard API covers overview, activity, knowledge, usage, identity, health,
authenticated learned-skill pin/unpin, and authenticated MCP management.
Explicit health, identity, and knowledge-skill routes may run bounded bot-owned
sandbox probes; overview must use injected runtime state and must not run
doctor or sandbox commands implicitly. Dashboard pin/unpin is the operator
surface for curator-managed learned skills; do not add CLI pinning. Dashboard
MCP routes must go through the internal Unix socket API and must not edit MCP
files or credential storage directly.
Future write routes must call bot-owned control-plane services instead of
directly editing agent files, credentials, or aggregator state.

Global configuration lives in `right-config`: RIGHT_HOME resolution, global
config YAML IO, and agents/backups directory helpers.
Brand-conformant UI lives in `right-ui`; cancel-safe process-group child
handling lives in `right-process`; OpenShell gRPC/proto, CLI wrappers,
sandbox exec, and live-test support live in `right-openshell`; platform-store
deployment lives in `right-platform-store`.
Agent configuration DTOs live in `right-agent-config`; host-side STT cache and
download helpers live in `right-stt`.
`tonic-prost-build` lives in `crates/right-openshell/build.rs`, alongside
the OpenShell `.proto` files it compiles.

`right-platform-knobs` owns UX/prose constants, `right-prompt-safety` owns
memory prompt-safety wrappers, and `right-runtime-state` owns process-compose
runtime-state JSON. Edits in those domains must stay in their owner crates and
must not invalidate global config code.

`right-bot` owns two sibling subtrees: `bot::cc::*` for generic Claude Code
subprocess plumbing (`invocation`, `prompt`, `stream`, `worker_reply`,
`attachments_dto`, `markdown_utils`) and `bot::telegram::*` for
Telegram-specific glue (`handler`, `dispatch`, `filter`, `mention`,
`oauth_callback`, `webhook`, attachment delivery, and chat/session handling).
The `cc/` subtree is generic enough for Stage E to lift into a `right-cc`
crate; Telegram code depends on it for shared output DTOs and HTML helpers.

## Module Map

See: `docs/architecture/modules.md`.

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
section only. Filesystem/landlock changes require sandbox recreation
(see `Upgrade & Migration Model` below).

See: `docs/architecture/sandbox.md` for staging-dir layout, platform-store
deployment, TLS-MITM, and the bot-startup sandbox sequence.

Live OpenShell coverage is CI-explicit: tests that create real sandboxes or
rely on OpenShell CLI file transfer use `#[ignore = "ci-openshell: ..."]`
with a `ci_openshell_` test-name prefix and are called by the workspace-wide
ignored-test filter in `.github/workflows/tests.yml`. Mock gRPC and pure
policy tests remain in the default workspace test path.

### Login Flow (setup-token)

See: `docs/architecture/lifecycle.md` (Login Flow).

### MCP Token Refresh

See: `docs/architecture/mcp.md` (MCP Token Refresh).

### MCP Auth Types

Dashboard MCP management runs URL-first detection, then asks the user to choose
`OAuth`, `Headers`, or `URL as-is`. Detection is advisory; the dashboard choice
is authoritative. Telegram `/mcp` opens the dashboard MCP view and has no
management subcommands.

| auth_type | How token is injected | Selection |
|-----------|----------------------|-----------|
| `oauth` | `Authorization: Bearer` via DynamicAuthClient | User chooses `OAuth`; OAuth AS discovery recommends it |
| `bearer` | `Authorization: Bearer` header | User chooses `Headers` with bearer recommendation/fallback |
| `headers` | Multiple configured HTTP headers | User chooses `Headers`; values are write-only and redacted from list/detail APIs |
| `query_string` | Embedded in URL | User chooses `URL as-is` for a URL containing `?` query params |

`URL as-is` also covers no-auth and loopback development MCP servers. Explicit
registration allows HTTP/HTTPS and warns for plain HTTP; broad private/link-local
ranges remain blocked by default.

MCP OAuth uses Resource Indicators. Right discovers the canonical MCP resource
URI from protected-resource metadata when available, includes it in auth-code
and refresh-token requests, and persists it with the OAuth state. Existing rows
without `oauth_resource` fall back to the canonicalized server URL.

### MCP Aggregator

One shared aggregator process serves all agents on TCP `:8100/mcp` with
per-agent Bearer-token auth. Tool routing rules:

- No `__` prefix → `RightBackend` (built-in tools, unprefixed).
- `rightmeta__` prefix → Aggregator management (read-only: `mcp_list`).
- `{server}__` prefix → `ProxyBackend` (forwarded to upstream MCP).

Internal REST API on Unix socket (`~/.right/run/internal.sock`):
`POST /mcp-add`, `POST /mcp-remove`, `POST /mcp-set-headers`,
`POST /set-token`, `POST /mcp-list`, `POST /mcp-instructions`,
`POST /progress/register`,
`POST /progress/unregister`. MCP management goes through authenticated
Telegram Mini App dashboard routes, which route through this internal Unix
socket API with `InternalClient` (hyper UDS). Agents cannot reach the Unix
socket from inside the sandbox.

Foreground progress uses the built-in `mcp__right__send_progress` tool. The
worker creates a per-invocation MCP config with `X-Right-Invocation`, registers
that invocation with the aggregator, and exposes a bot-local UDS
`POST /progress/send` guarded by a separate send token. Cron, delivery,
reflection, and background-continuation invocations disallow the tool.

Conversation search scope is server-enforced. `mcp__right__thread_search`
searches only the current `(chat_id, effective_thread_id)`.
`mcp__right__chat_search` searches only the current `chat_id`; in DMs this is
only that DM, and in groups this is the whole group across topics. Agents must
never be allowed to pass chat_id, thread_id, user ids, session ids, or a
broader scope to these tools.

See: `docs/architecture/mcp.md` for dispatch detail and rationale.

### Prompting Architecture

Session-bearing `claude -p` invocations get a composite system prompt via
`--system-prompt-file` (the sole prompt mechanism — no `--agent` flag).
Prompt caching is critical — avoid per-message tool calls to read
identity files.

Per-turn skill-learning pipeline (replaces the prior fork-probe classifier):

1. **Anchor capture** (`bot::telegram::worker`): after the foreground assistant
   reply is sent, the worker captures a `ProbeAnchor` (user text, assistant
   text, main session UUID, captured_at, chat/thread, **num_turns,
   total_cost_usd, wall_elapsed_ms, used_skill_receipts**) for downstream
   consumption.

2. **Prefilter** (`bot::learning_prefilter`): a Haiku classifier returns a
   structured three-way decision —
   `Skip{reason}` / `PatchExisting{target_skill, reason}` /
   `CreateNew{topic_hint, reason}`. The prompt embeds per-agent baselines
   (P50/P90/P99 over 14d foreground turns) for `num_turns`, `total_cost_usd`,
   and `wall_elapsed_ms`, plus a one-line-per-skill index summary. Baselines
   are computed on demand by `right_agent::usage::turn_baseline::compute`.

3. **Probe-writer** (`bot::learning_probe_writer`): when the prefilter
   returns non-Skip, the worker forks the main CC session with the decision
   as a directed hint. The writer verifies and may patch, create, or refuse.
   It reports `hint_outcome` (`applied_as_hinted` / `applied_differently` /
   `refused`) back via `mcp__right__skill_learning_finish`.

4. **Curator** (`bot::learning_curator`): per-agent 60s ticker reads state
   from the `curator_state` singleton row in `data.db`. The gate is
   multi-signal: cost spike (today's `learning_probe_writer` cost vs
   `k * 14d P50` with a floor), skill-change count (≥ N skills
   created/patched since last run), or the 168h time fallback. A
   `min_cooldown_hours` floor blocks all triggers including the time
   fallback. Trigger evidence is captured in `last_spike_evidence_json`.

Lifecycle mutable state lives in per-agent `data.db.skill_lifecycle` via
`right-lifecycle`, not `.usage.json`. `skill_learning_events` remains the
append-only audit log for start/finish tool calls, while skill package content
remains under `.claude/skills/<skill_name>/SKILL.md`. Foreground usage is
recorded only from `used_skill_receipts`.

The only learning runtime is the prefilter/probe-writer/curator pipeline. The
old Stage 2 selector/reviewer, learning episode tables, nudge-signal gate, and
review reports have been removed from runtime and schema. Deprecated
`agent.yaml` learning keys are accepted only for upgrade compatibility and
warn at load time.

See `PROMPT_SYSTEM.md` for full documentation.

### Claude Invocation Contract

Every `claude -p` invocation MUST go through `ClaudeInvocation` (defined in
`crates/bot/src/cc/invocation.rs`). Direct construction of `claude_args`
vectors is forbidden — the builder enforces invariant flags at compile time.

**Invariants** (always present for session-bearing invocations, cannot be
omitted):
- `claude -p --dangerously-skip-permissions`
- `--mcp-config <path>` + `--strict-mcp-config` — agents MUST have MCP access
- `--output-format <stream-json|json>` (`--verbose` auto-added for `stream-json` only)
- `--json-schema <schema>` — structured output

The post-turn probe-writer fork IS session-bearing — it forks the main session
(`--fork-session --resume <main>`) so it can preserve `--mcp-config` +
`--strict-mcp-config` and inherit the transcript via prompt cache. Tools are
narrowed at runtime via `--allowedTools Write,Read,Bash,
mcp__right__skill_learning_start,mcp__right__skill_learning_finish`.
Before calling learning MCP tools, background learning writers MUST register an
invocation identity (`ProbeWriter` or `Curator`) and use the resulting
per-invocation MCP config with `X-Right-Invocation`.

The Haiku prefilter and the periodic curator are independent CC invocations.
The prefilter is non-session-bearing (`--tools ""`, JSON schema). The curator
forks a fresh session (no `--resume`) with the curator system prompt and a
narrow tool whitelist.

Deprecated Stage 2 selector/reviewer calls (when `background_review_enabled`
was set) were not session-bearing and intentionally omitted `--mcp-config` /
`--strict-mcp-config`. That path no longer ships; the field is silently ignored
and must not re-enable seed writes or the drain scheduler.

**Optional per-callsite:**
- `--model` — override default model
- `--max-budget-usd` — budget cap (cron jobs)
- `--max-turns` — turn limit
- `--resume` / `--session-id` — session management (worker, delivery)
- `--disallowedTools` — disable CC built-ins that conflict with MCP equivalents

**Adding a new session-bearing `claude -p` callsite:** construct a
`ClaudeInvocation`, set fields, call `.into_args()`, pass result to
`build_prompt_assembly_script()`. Never build args manually. A no-MCP,
no-composite-prompt callsite needs an explicit architecture exception here.

### Reflection Primitive

`crates/bot/src/reflection.rs` exposes
`reflect_on_failure(ctx) -> Result<String, ReflectionError>`. On CC
invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a
human-friendly summary of the failure.

Reflection never reflects on itself. Hindsight `memory_retain` is skipped
for reflection turns. `async_runs.status` gates delivery: `'failed'` routes
to `DELIVERY_INSTRUCTION_FAILURE`; any other status routes to
`DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

See: `docs/architecture/sessions.md` for `ReflectionLimits` (worker vs
cron), usage-event accounting, and label-routing detail.

### Stream Logging

See: `docs/architecture/sessions.md` (Stream Logging).

### Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a
`ScheduleKind` variant. The **`Immediate`** variant (encoded as
`schedule = '@immediate'`) is bot-internal and fires on the next
reconcile tick (≤5s). Immediate jobs default `lock_ttl` to
`IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`) when created without an explicit
TTL; the lock heartbeat is written once at job start and never refreshed,
so a tighter TTL would let the reconciler spawn a duplicate `execute_job`
against the same spec on the next 5-second tick. The TTL is the
duplicate-prevention guard, not a wall-clock execution limit.

Background continuations created by the Telegram worker are `async_runs`
rows with `kind = 'background'`, not cron specs. The worker inserts a
queued row, directly forks Claude with `--resume <main-session>
--fork-session --session-id <run_id>`, then marks the handoff spawned
only after the fork emits its matching `system/init`. Bot startup runs
`background::mark_interrupted_handoffs` against `kind = 'background'`
rows still stuck at `status = 'queued'` and `handoff_state = 'queued'`;
it fails them with pending delivery. It deliberately does not guess at
stale `running` rows without process ownership.

Legacy `@bg:<fork_from-uuid>` rows are not schedulable. Cron loading
skips them so old databases do not block active cron jobs, but no
runtime path creates or executes cron-backed background continuations.

See: `docs/architecture/sessions.md` for the full variant list.

### Per-session mutex on --resume

See: `docs/architecture/sessions.md` (Per-session mutex on --resume).

### Background continuation handoff

Current background handoffs are tracked in `async_runs` as
`kind = 'background'`. The marker injected into foreground turns reads
only those rows for the target chat, and shows running rows plus
finished `success`/`failed` rows whose delivery is still `pending` or
`retryable`. Cron rows are excluded.

Legacy `@bg:<fork_from-uuid>` cron specs are ignored by cron loading.
There is no bot-startup migration or runtime cron scheduling path for
background continuations.

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

See [Upgrade & Migration Model](#upgrade--migration-model) for category definitions.

**Hot-reloadable fields in `agent.yaml`.** Most fields trigger a graceful
restart on change (via `config_watcher`). Two exceptions: `model` and `debug`.
The watcher's smart-diff classifies a `model`/`debug`-only change as
hot-reloadable and stores the new values into `AgentSettings.model` (an
`Arc<ArcSwap<...>>`) and `AgentSettings.debug` (an `Arc<AtomicBool>`)
without restarting. The Telegram `/model` and `/debug` commands exploit this
path — in-flight CC subprocesses keep their old flags; the next invocation
in any chat picks up the new value. Adding more hot-reloadable fields
requires extending the diff in `crates/bot/src/config_watcher.rs::diff_classify`.

### Skill learning loop

The skill-learning pipeline is a per-turn writer plus periodic curator. Two
independent gates run today:

1. **Prefilter + probe-writer gate** (per turn, in worker): runs only when the
   prefilter is enabled, the foreground turn was a Normal prompt mode, and
   today's spend across `right_agent::usage::LEARNING_SOURCES`
   (`learning_prefilter`, `learning_probe_writer`, `learning_curator`) is below
   `LearningConfig.max_daily_budget_usd` (default $1.00). A non-`skip`
   prefilter decision gates and directs the probe-writer fork. The session mutex on
   the main session UUID prevents concurrent `--resume` against the same
   transcript; the writer holds it only until its `system/init` handshake.

2. **Curator gate** (periodic, agent ticker, pure logic in
   `bot::learning_curator::should_run_now`): order is `enabled` → `!paused` →
   `circuit_open_until` (skip if in future) → `min_idle_hours` (skip if any
   chat activity within window) → `min_cooldown_hours` (blocks ALL triggers
   below) → trigger priority **CostSpike > SkillChangeCount > TimeFallback**.
   First-ever runs seed `last_run_at` in `curator_state` and defer (Hermes
   pattern). State (`last_run_at`, `last_run_status`, `consecutive_failures`,
   `circuit_open_until`, `last_spike_evidence_json`) lives in the per-agent
   `curator_state` singleton row.

The old Stage 2 selector/reviewer has been removed from runtime and schema.
Its gate fields (`circuit_failure_threshold`, `circuit_cooldown_minutes`) are
kept as `Option<u32>` in config for backward compatibility and ignored with a
load-time warning.

Adding a new learning-adjacent invocation requires extending
`right_agent::usage::LEARNING_SOURCES` so both the budget gate and the
dashboard `SOURCES` array pick it up; the dashboard test
(`usage_overview_sources_match_learning_sources_constant`) enforces sync via
a dev-dep cross-crate assertion.

Skill lifecycle state lives in `data.db.skill_lifecycle`. It is the source of
truth for active/stale/archived status, `created_by` provenance (foreground /
probe_writer / curator / bundled), usage/patch counters, and the operator pin
flag. The dashboard reads this table for lifecycle overview and is the only
operator pin/unpin surface. Curator transitions read/write DB rows and skip
pinned rows.

### Memory

Two modes, configured per-agent via `memory.provider` in `agent.yaml`:
**Hindsight** (primary, Hindsight Cloud API) and **file** (fallback,
agent-managed `MEMORY.md`). MCP tools `memory_retain` / `memory_recall` /
`memory_reflect` are exposed only in Hindsight mode.

Conversation transcript search is separate from Hindsight. It uses local
Turso FTS indexes over archived Telegram messages and is scoped by the current
foreground invocation.

See: `docs/architecture/memory.md` for auto-retain/recall semantics,
prefetch cache behavior, cron-skip rules, and backgrounded-turn handling.

### Memory Resilience Layer

See: `docs/architecture/memory.md` (Memory Resilience Layer).

### Memory Schema

Tables in per-agent `data.db`: `memories` / `memory_events` (legacy, unused
but retained for migration compat), `telegram_sessions`,
`cron_specs`, `async_runs`, `mcp_servers`, `auth_tokens`, `pending_retains`,
`memory_alerts`, `curator_state` (singleton; `agent_singleton_id` PRIMARY KEY
CHECK = 1), `skill_learning_events`, and `skill_lifecycle`. Run
`sqlite3 data.db .schema` for column-level definitions.

## External Integrations

| System | Protocol | Notes |
|--------|----------|-------|
| process-compose | REST API (TCP :18927) | Health, process start/stop/restart, logs, shutdown |
| Claude Code CLI | Subprocess (`claude -p` via SSH) | Runs inside sandbox, structured JSON output |
| Claude Code CLI | Env var (CLAUDE_CODE_OAUTH_TOKEN) | Auth token from setup-token, injected into claude -p |
| OpenShell | gRPC + mTLS (active gateway endpoint) | Sandbox create/poll/reuse, policy hot-reload, exec, file verification |
| OpenShell | CLI (`openshell sandbox upload/download`) | File transfer (no gRPC equivalent yet) |
| Telegram | teloxide long-polling | CacheMe<Throttle<Bot>> adaptor, per-agent allowlist |
| Cloudflare Tunnel | CLI (`cloudflared`) | Named tunnel, DNS CNAME, credentials file |
| MCP Aggregator | HTTP (:8100/mcp) + Unix socket (internal API) | Aggregates built-in + external MCP backends, per-agent Bearer auth |
| ffmpeg | system | Decode voice/video_note to PCM for whisper-rs | Optional — bot runs without it; voice transcription disabled. doctor warns. |
| ironclaw_safety | crate | Memory-content sanitization (write) and untrusted-content wrapping (read). See `docs/architecture/memory.md`. |

## Runtime isolation — mandatory

All interaction with the running `process-compose` instance MUST go through
`PcClient::from_home(home)`. The `PcClient::new(port)` constructor is
crate-private; external callers cannot construct a client without a `home`.

This guarantees that `right --home <path>` is actually isolated: when a
command is run against a tempdir home with no `state.json`, `from_home`
returns `None` and callers skip PC-touching logic. This property is what
protects tests (which run with a `--home=<tempdir>`) from accidentally hitting
the user's live PC on port 18927 and SIGTERM-ing a same-named process there.

`<home>/run/state.json` carries the port and API token the running PC uses;
it is written by `right_codegen::pipeline` during `right up` and read by every
subsequent command that needs to talk to PC. Older state files without the
`pc_port` field deserialize to `PC_PORT` via `#[serde(default)]`.

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

Per-agent `data.db` is a SQLite-compatible database. Runtime local storage uses
the `turso` crate with `sync` enabled for future Turso Cloud backup work, and
that driver implementation is hidden behind `right-db`.

`right-db` is the only crate that owns local database-driver details. Local
filesystem-backed opens must enable Turso's experimental index-method feature
because conversation and memory search use `CREATE INDEX ... USING fts`, and
must enable Turso's experimental multiprocess-WAL path so bot and MCP
aggregator processes can open the same per-agent `data.db`; this may create
Turso sidecar files such as `data.db-tshm` next to the standard database/WAL
files. Files matching `data.db-*` are disposable runtime sidecars, not durable
backup state; backup and restore flows preserve only the canonical
`VACUUM INTO` snapshot stored as `data.db` in the selected backup directory.
The in-memory test/helper path is the exception: Turso does not support
multiprocess WAL for `:memory:` databases. Other crates must use project-owned
`right_db` types and must not expose raw `turso` connection, transaction, row,
error, value, or parameter types in public APIs.
The runtime database API is async-first: `open_connection`, `open_db`,
`execute`, `query_*`, migrations, and transactions are awaited directly by
callers. Do not add sync facades, runtime `block_on` bridges, or shared-runtime
adapters around `right-db`.
`right-db` may use bundled `rusqlite` only inside locked `migrate: true`
schema bootstrap for legacy FTS5 cleanup; it is not a general runtime database
boundary.

### Migration Ownership

Both the MCP aggregator (`right-mcp-server`) and bot processes run schema bootstrap on per-agent `data.db` via `right_db::open_connection(path, migrate: true)`. This is the only path that may run legacy schema cleanup or database migrations. `right-db` serializes that bootstrap with a per-agent advisory lock file so concurrent startup of MCP and bot processes is safe without relying on process-compose ordering. Under the lock, `right-db` may use bundled `rusqlite` to drop legacy SQLite FTS5 virtual tables and sync triggers before opening the database through Turso, because Turso cannot resolve every old FTS5 schema. Runtime opens with `migrate: false` do not run the scrubber, do not inspect legacy FTS tables, and do not apply migrations. Read-only helpers do not run the scrubber or mutate files. The migration registry (`right_db::migrations::MIGRATIONS`) is the sole place to add new tables.

All pending migrations run inside a single immediate transaction. Rollback is all-or-nothing: a failure at migration N rolls back every prior migration in the same batch and leaves `user_version` unchanged. A concurrent caller that opens the database during a cold-boot batch blocks on that transaction for the full batch duration, not just the next pending version, and may exhaust the 5s `busy_timeout` under WAL. Tests must not assume per-version commit boundaries; see `migration_runner_semantics_rolls_back_all_pending_migrations_on_later_failure`.

### Transaction Rule

Any operation that performs 2+ writes (INSERT, UPDATE, DELETE) MUST use a
single immediate transaction from `Connection::transaction().await`, then
commit explicitly with `Transaction::commit().await` after all writes succeed.
If a caller intentionally aborts a transaction before other connections may
write, it must call `Transaction::rollback().await`; dropping a Turso
transaction only schedules rollback on the next use of that same connection.
Single-statement writes don't need a transaction. Migrations are the sole
exception because the `right-db` migration runner wraps each migration batch.

### Idempotent Migrations

All migrations must be idempotent — safe to re-run if the schema already matches. SQLite-compatible DDL lacks `ADD COLUMN IF NOT EXISTS`, so column additions must check `pragma_table_info` first. Use a Rust migration hook for migrations that need conditional DDL. `CREATE TABLE/INDEX/TRIGGER IF NOT EXISTS` is naturally idempotent.

## Upgrade & Migration Model

Every change that touches codegen, sandbox config, or on-disk state must be
deployable to already-running production agents. Manual migration steps,
`right agent init`, or sandbox recreation are NEVER acceptable as upgrade
paths.

### Codegen categories

Every per-agent codegen output belongs to exactly one category:

| Category | Semantics | Examples |
|---|---|---|
| `Regenerated(BotRestart)` | Unconditional overwrite every bot start. Takes effect on next CC invocation. | settings.json, mcp.json, schemas, system-prompt.md |
| `Regenerated(SandboxPolicyApply)` | Overwrite + `openshell policy set --wait`. Network-only. | policy.yaml (network section) |
| `Regenerated(SandboxRecreate)` | Overwrite + triggers sandbox migration. Filesystem/landlock and other boot-time-only changes. | policy.yaml (filesystem section) |
| `MergedRMW` | Read, merge, write. Preserves unknown fields. | .claude.json, agent.yaml (secret injection) |
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

For sandboxed agents, identity `AgentOwned` files are authoritative in
`/sandbox` once the sandbox exists. Host copies of `IDENTITY.md`, `SOUL.md`,
and `USER.md` are an explicit mirror, not the prompt source. Code that needs a
complete host mirror must call the identity mirror reconciliation helper instead
of assuming a prior user message ran reverse sync.

Cross-agent outputs (process-compose.yaml, agent-tokens.json, cloudflared
config) are all `Regenerated(BotRestart)` — reread on `right up`.

`policy.yaml` mixes a hot-reloadable network section and a recreate-only
filesystem section. It's registered as the stricter `Regenerated(SandboxRecreate)`;
runtime discriminates via `openshell::filesystem_policy_changed`.

### Helper API

`crates/right-codegen/src/contract.rs` provides the only sanctioned writers:

- `write_regenerated(path, content)` — all `Regenerated` outputs except
  `SandboxPolicyApply`.
- `write_regenerated_bytes(path, content)` — byte variant for non-UTF-8
  payloads (bundled skill assets, etc.).
- `write_merged_rmw(path, merge_fn)` — read-modify-write with unknown-field
  preservation.
- `write_agent_owned(path, initial)` — no-op if file exists.
- `write_and_apply_sandbox_policy(sandbox, path, content).await` — the ONLY
  way to update policy for a running sandbox. Writes + applies atomically
  via `openshell policy set --wait`.

Direct `std::fs::write` inside codegen modules is a review-blocking defect.

### Rules for adding a new codegen output

1. Pick a category. Add a `CodegenFile` entry to the matching registry
   (`codegen_registry()` or `crossagent_codegen_registry()`).
2. Use the matching helper. No bare `std::fs::write`.
3. Run `cargo test registry_covers_all_per_agent_writes` to verify the
   registry is complete.
4. If `Regenerated(SandboxRecreate)` — exercise the migration path manually
   and update `Sandbox migration` subsection under Data Flow if the trigger
   condition changed.
5. If the new output is policy-related, apply via
   `write_and_apply_sandbox_policy` only. Adding a new network endpoint is
   fine; adding a new filesystem rule requires `SandboxRecreate` treatment.
6. Never require `right agent init` for existing agents to adopt the
   change. They upgrade via `right restart <agent>`.

### Upgrade flow for a typical codegen change

1. Code change merged.
2. User runs `right restart <agent>` (or the bot restarts naturally via
   process-compose `on_failure`).
3. `run_single_agent_codegen` rewrites every `Regenerated` file.
4. Hot-reload machinery applies per category:
   - `BotRestart`: nothing extra — CC picks up the new file on next invocation.
   - `SandboxPolicyApply`: `write_and_apply_sandbox_policy` hot-reloads via
     `openshell policy set --wait`.
   - `SandboxRecreate`: bot startup compares active vs on-disk policy via
     `filesystem_policy_changed`. On drift, logs a WARN telling the operator
     to run `right agent config <agent>`, which invokes
     `maybe_migrate_sandbox`. No automatic migration — it's disruptive and
     requires operator consent.
5. For `BotRestart` / `SandboxPolicyApply`: zero manual steps.
6. For `SandboxRecreate`: one follow-up command from the operator.

### Non-goals

- Agent-owned content (`AgentOwned` files) — agent property; codegen never
  mutates them.
- OpenShell server upgrades — covered by `OpenShell Integration Conventions`.
- SQLite-compatible schema — handled by `right-db` migrations (see `Local Database Rules`).

### Cross-references

- `AGENTS.md` → `Upgrade-friendly design`, `Never delete sandboxes for
  recovery`, `Self-healing platform` — conventions this model implements.
- Data Flow → `Sandbox migration (filesystem policy change)` — the migration
  flow used by `Regenerated(SandboxRecreate)`.

## Integration Tests Using Live Sandboxes

Any test that needs a live OpenShell sandbox MUST create it via
`right_openshell::test_support::TestSandbox::create("<test-name>")`. The helper:

- Generates a unique `right-test-<name>` sandbox with a minimal hostless `allowed_ips` smoke-test endpoint (`1.1.1.1/32`) on port 443 and `binaries: "**"`.
- Registers the sandbox in `test_cleanup` so sandboxes are deleted even under `panic = "abort"` (the panic hook drains the registry and calls `openshell sandbox delete`).
- Cleans up leftovers from prior SIGKILLed runs via `pkill_test_orphans`.
- Exposes `.exec(&[...])` which goes through gRPC — the project bans the `openshell sandbox exec` CLI from tests.
- Exposes `.name()` for helpers like `upload_file` that take a sandbox name.

Consumers outside `right-agent`'s own unit tests depend on the `test-support` feature:

```toml
[dev-dependencies]
right-openshell = { path = "...", features = ["test-support"] }
```

Rules:

- Never hardcode sandbox names (no `right-foo-test-lifecycle` fixtures).
- Never invoke the `openshell` CLI from tests. Use `TestSandbox::exec` or the gRPC helpers in `right_openshell::openshell`.
- Never add `#[ignore]` to sandbox tests. Dev machines have OpenShell.
- `TestSandbox` holds a `SandboxTestSlot` for its lifetime. Direct tests that bypass `TestSandbox` and call `spawn_sandbox` must acquire `acquire_sandbox_slot()` themselves.
- CI may set `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS` low to throttle only live sandbox creation while preserving normal Cargo test parallelism. Use at least `2` in jobs with a process-lifetime shared sandbox.
- CI may raise `RIGHT_TEST_SANDBOX_READY_TIMEOUT_SECS` and `RIGHT_TEST_SANDBOX_SSH_TIMEOUT_SECS` for cold OpenShell runners; local defaults stay short (`120s` READY, `60s` SSH).

## Security Model

- **Sandbox isolation**: OpenShell (k3s containers) — filesystem + network + TLS policies per agent
- **TLS MITM**: OpenShell proxy terminates and re-signs TLS with per-sandbox CA for L7 inspection
- **Credential isolation**: Host credentials never uploaded to sandbox. Each sandbox authenticates independently via OAuth login flow.
- **Network policy**: Scoped wildcard domain allowlists (`*.anthropic.com`, `*.claude.com`, `*.claude.ai`) or hostless public `allowed_ips` endpoint allowlists + `binaries: "**"`. TLS termination is automatic (OpenShell v0.0.30+).
- **`--dangerously-skip-permissions`**: Always on for all CC invocations. OpenShell policy is the security layer, not CC's permission system.
- **Prompt-injection defense**: `ironclaw_safety::Sanitizer` runs on
  memory writes (Hindsight retain path) and `wrap_external_content`
  frames the `## Memory` section as untrusted data on read. Phase-2
  wrap is the primary defense; phase-1 sanitize is hygiene. See
  `docs/architecture/memory.md`.
- **Chat ID allowlist**: Empty = block all (secure default); per-agent in `agents/<name>/allowlist.yaml`. Legacy `agent.yaml::allowed_chat_ids` is migration input only.
- **Protected MCP**: "right" cannot be removed via the dashboard MCP controls
- **MCP tool restriction**: Agents cannot register/remove external MCP servers — `mcp_add`, `mcp_remove`, `mcp_auth` are not exposed as MCP tools. Only the user can manage servers via the Telegram dashboard MCP view routed through the internal Unix socket API. This prevents sandbox escape via data exfiltration to attacker-controlled MCP endpoints.
- **OAuth CSRF**: Token matching in callback server

## Brand-conformant CLI output

Every user-facing TUI surface in `right` and `right-bot` MUST go through
`right_ui::*` (see `crates/right-ui/src/`). Raw `println!` /
`eprintln!` of user-facing text is a review-blocking defect. Visual
contract, atoms, and theme rules: `docs/brand-guidelines.html` and the
redesign spec at
`docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md`.

Past miss: `cmd_agent_rebootstrap` (`crates/right/src/main.rs`) shipped
with raw `println!` and bare `✓`/`⚠` literals, bypassing the rail and
theme detection. Do not repeat; migrate existing offenders when touched.

## Telegram message UX

Bot-authored Telegram HTML messages MUST escape untrusted text before setting
`ParseMode::Html`. Shared send helpers should preserve effective topic thread
ids. Do not send raw CLI-style prefixes such as `Warning:` or `Failed:` when a
clear user-facing sentence is available.

## OpenShell Integration Conventions

- **Prefer gRPC over CLI**: Use the OpenShell gRPC API (mTLS on the active gateway endpoint) for sandbox operations wherever possible. Resolve the endpoint from `OPENSHELL_GATEWAY_ENDPOINT` or `openshell status`; do not hardcode a gateway port. gRPC is faster, more reliable, and provides structured responses. The CLI (`openshell sandbox upload/download`) is only used for file transfer — no gRPC file transfer API exists yet.
- **gRPC for**: sandbox create/get/delete, readiness polling, exec inside sandbox, policy status, SSH session management.
- **Readiness polling diagnostics**: `wait_for_ready` must preserve the last `GetSandbox` phase/status in timeout errors and treat `SANDBOX_PHASE_ERROR` as terminal. Do not collapse OpenShell status into a bare boolean in wait loops.
- **Sandbox create stdio**: `openshell sandbox create` is a long-running process supervised by gRPC readiness polling. Its stdout/stderr must be inherited or drained concurrently; never leave them piped and unread.
- **SSH remote argv must be quoted centrally**: OpenSSH does not preserve remote argv; it sends one command string to the remote login shell. For remote argv, call `right_openshell::openshell::quote_ssh_remote_args(...)` and pass exactly one argument after the SSH host or `--`. For authored shell scripts, pass exactly one complete script string. Never use `Command::args(...)` or `Vec::join(" ")` for remote argv after the SSH host.
- **CLI for**: file upload/download (SSH+tar under the hood), policy apply (`openshell policy set`).
- **Vendored proto compatibility is load-bearing**: OpenShell v0.0.42 returns sandbox IDs as `Sandbox.metadata.id`; older vendored protos decoded field 1 as top-level `Sandbox.id` and failed with `invalid string value: data is not UTF-8 encoded`. `resolve_sandbox_id` must read `metadata.id`, and `ci_openshell_policy_validates_against_openshell` is the live regression gate.
- **NEVER use CLI for exec**: `openshell sandbox exec` CLI has unreliable argument parsing (positional name vs `--name` flag). Always use gRPC `exec_in_sandbox()` for executing commands inside sandboxes. All callers (sync, platform_store, etc.) must receive a gRPC client.
- **Known CLI bug**: Directory uploads may silently drop small files. Always verify critical files after directory upload, and re-upload individually if missing.

## OpenShell Policy Gotchas

- **Do not emit deprecated `tls:` modes** (OpenShell v0.0.30+). The proxy auto-detects TLS via ClientHello peek and terminates for L7 endpoints. Writing `tls: terminate` or `tls: passthrough` triggers a per-request `WARN` in the sandbox supervisor log and the field is slated for removal. Omit the field for auto-detect; use `tls: skip` only to explicitly disable termination (raw tunnel).
- `binaries: path: "**"` not `"/sandbox/**"`. Claude binary lives at `/usr/local/bin/claude`, not under `/sandbox/`.
- `protocol: rest` and `access: full` are required only for endpoints that intentionally use L7 HTTP policy on terminated plaintext.
- Permissive public internet endpoints are hostless public `allowed_ips` raw tunnels (`tls: skip`, no `protocol`/`access`) on ports 80/443. Do not add L7 REST policy there: OpenShell rejects encoded `/` (`%2F`) request-targets used by scoped npm package metadata.
- Scoped wildcard domains (`*.anthropic.com`) work — the earlier 403 was caused by the binaries restriction, not wildcard matching.
- OpenShell v0.0.37+ rejects TLD/global host wildcards. Permissive public internet policy must use hostless public `allowed_ips` endpoints, not a DNS wildcard.
- CC actively manages `.claude.json` — strips unknown project trust entries on startup. Use `--dangerously-skip-permissions` instead of relying on trust entries.
- `HTTPS_PROXY=http://10.200.0.1:3128` is set automatically inside sandbox. All HTTP/HTTPS goes through the proxy.
- **Host service access from sandbox** (`host.openshell.internal`): requires exact sandbox-resolved `allowed_ips` in the policy endpoint to bypass SSRF protection. OpenShell resolves through the sandbox `/etc/hosts`/DNS view and every resolved private/internal IP must be allowed. Do not hardcode host gateway IPs and do not permanently allow broad private/ULA ranges for Right MCP.
- **Right MCP policy lifecycle**: codegen writes a bootstrap unresolved Right MCP endpoint before sandbox creation. After the sandbox is READY and SSH exec works, resolve `host.openshell.internal` inside that sandbox with `getent ahosts`, generate all unique IPs as IPv4 `/32` and IPv6 `/128`, then hot-apply via `openshell policy set --wait` before any Claude invocation. Bot startup repeats this exact multi-IP hot-apply so backup/restore and host migration self-heal stale `policy.yaml` IPs. `openshell forward`/service exposure is not the Right MCP route; those primitives expose sandbox services outward, not sandbox-to-host aggregator access.
- **Host MCP bind address**: the host-side Right MCP aggregator must bind `0.0.0.0` for sandbox access. `127.0.0.1` is loopback and OpenShell always blocks loopback/link-local/unspecified destinations.
- **Sandbox user-local executables:** OpenShell agents use `/sandbox/.local/bin` as the only platform-supported location for user-installed CLI binaries. Right Agent generates `/sandbox/.right/env.sh`, sources it from `/sandbox/.bashrc`, and sources/falls back to the same environment before sandboxed `claude -p` invocations. npm globals use `NPM_CONFIG_PREFIX=/sandbox/.local`; npm cache uses `NPM_CONFIG_CACHE=/sandbox/.npm`. Do not document or generate `~/bin` as a supported install target.
- **NixOS users**: must add `networking.firewall.trustedInterfaces = [ "docker0" "br-+" ];` to NixOS config. OpenShell runs k3s inside a Docker container on a custom bridge network (`br-XXXXX`), not the default `docker0`. Without this, the NixOS firewall drops traffic from k3s pods to host services. The `+` suffix is iptables wildcard matching all `br-*` interfaces.
- **Filesystem policy changes require sandbox recreation**: `openshell policy set --wait` hot-reloads network policies but does NOT apply filesystem policy changes to running sandboxes. Landlock rules are set at sandbox creation time. To apply filesystem_policy changes, the sandbox must be destroyed and recreated.

## Directory Layout (Runtime)

`~/.right/` is the runtime root (override with `--home`). Critical paths:

- `config.yaml` — global config (tunnel).
- `agents/<name>/` — per-agent state. Key files: `agent.yaml`, `allowlist.yaml`, `policy.yaml`, `data.db`, `.claude/.credentials.json` (symlink to `~/.claude/.credentials.json`, host-only — NOT uploaded to sandbox). Subdirs include `crons/`, `inbox/`, `outbox/`, and `tmp/` for staging during attachment transfer. Sandbox-internal: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl` (CC project history, agent-readable for self-introspection via the `/right-reflect` skill); `/sandbox/.claude/logs/<sid>.log` (CC debug output, only present when `/debug` is on).
- `run/process-compose.yaml`, `run/state.json` (carries `pc_port` + `pc_api_token`), `run/internal.sock` (bot↔aggregator UDS), `run/ssh/<agent>.ssh-config`.
- `backups/<agent>/<YYYYMMDD-HHMM>/` — `sandbox.tar.gz` plus optional `agent.yaml` + `allowlist.yaml` + `data.db` + `policy.yaml` for full backups. `right agent backup` excludes rebuildable sandbox dirs by default (`.cache`, `.venv`, `.npm`, `.uv`); `--include-rebuildable` opts into forensic sandbox archives.
- `logs/<agent>.log.<date>` — per-agent daily log rotation. `mcp-aggregator.log` for the shared aggregator.
- `cache/whisper/ggml-<model>.bin` — STT models (downloaded at `right up`).

## Logging

Bot processes log to stderr + `~/.right/logs/<agent>.log` (daily rotation
via `tracing-appender`). Aggregator logs to stdout +
`~/.right/logs/mcp-aggregator.log`. See: `docs/architecture/sessions.md`
for stream-logging detail.
