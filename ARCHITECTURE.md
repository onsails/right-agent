# Architecture

## Workspace

Three crates in a Cargo workspace:

| Crate | Path | Role |
|-------|------|------|
| **right-agent** | `crates/right-agent/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, OpenShell |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |

## Module Map

### right-agent (core)

- `agent/` — agent discovery (presence detected by `agent.yaml`) and types (`AgentDef`, `AgentConfig`, `RestartPolicy`).
- `config/` — `GlobalConfig` (tunnel) and `RIGHT_HOME` resolution.
- `codegen/` — per-agent and cross-agent code generation: settings, `.claude.json`, `.mcp.json`, policy, process-compose, TOOLS.md, MCP instructions, bundled skills, cloudflared. The helper API in `codegen/contract.rs` is the only sanctioned writer (see Upgrade & Migration Model).
- `memory/` — Hindsight Cloud client (`hindsight.rs`), composite memory in file or Hindsight mode (`composite.rs`), schema migrations, prompt-injection guard. `store.rs` is legacy SQLite memory retained for migration compat.
- `runtime/` — `RuntimeState` JSON persistence, process-compose REST client, dependency checks.
- `mcp/` — OAuth credentials, internal UDS client (bot→aggregator), OAuth flow, proxy backend, token refresh scheduler.
- Single-file modules: `openshell.rs` (gRPC mTLS + CLI wrappers), `stt.rs` (whisper model cache + ffmpeg), `doctor.rs`, `init.rs`, `error.rs`.

### right (CLI)

- `main.rs` — CLI dispatcher.
- `aggregator.rs` — MCP Aggregator (Aggregator + ToolDispatcher + BackendRegistry).
- `right_backend.rs` — built-in MCP tools (memory, cron, mcp_list, bootstrap).
- `internal_api.rs` — internal REST API on Unix socket.
- `memory_server.rs` — deprecated CLI-only MCP stdio server.

### right-bot

- `lib.rs` — entry: resolve agent dir, open `data.db`, sandbox lifecycle, start teloxide.
- `telegram/` — bot adaptor, dispatcher, handler, per-session worker, session table, chat-ID filter, OAuth callback server, prompt assembly, attachments (with STT integration), `invocation.rs` (`ClaudeInvocation` builder — see Claude Invocation Contract).
- `login.rs` — token-based Claude login flow (setup-token, env var injection).
- `sync.rs` — background platform-store sync to `/sandbox/.platform/`.
- `cron.rs`, `cron_delivery.rs` — cron engine and delivery loop (resumes main session so cron results land in agent context).
- `reflection.rs` — `reflect_on_failure` primitive (see Reflection Primitive).
- `stt/` — host-side voice/video_note transcription (ffmpeg + whisper-rs + Russian markers).
- `error.rs` — `BotError` types.

## Data Flow

### Agent Lifecycle

```
right init  /  right agent init <name>
  ├─ `agent init` runs an interactive wizard (sandbox mode, network policy,
  │   telegram, chat IDs, stt, memory) and writes sandbox config + policy.yaml
  │   to the agent dir. `init` skips the wizard and also writes
  │   ~/.right/config.yaml + detects Telegram token / cloudflared tunnel.
  ├─ Create ~/.right/agents/<name>/ with template files
  ├─ Write AGENTS.md, BOOTSTRAP.md, agent.yaml
  │   (IDENTITY.md, SOUL.md, USER.md created later by bootstrap CC session)
  ├─ Generate .claude/settings.json, .claude.json
  └─ Symlink credentials from ~/.claude/

right up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config (if tunnel)
  └─ Launch process-compose (TUI or detached)

right bot --agent <name>  (spawned by process-compose)
  ├─ Resolve token, open data.db
  ├─ Per-agent codegen:
  │   ├─ settings.json, schemas
  │   ├─ .claude.json, credentials symlink, mcp.json
  │   ├─ TOOLS.md, skills install, policy.yaml
  │   └─ data.db init, git init, secret generation
  ├─ Clear Telegram webhook, verify bot identity
  ├─ Sandbox lifecycle:
  │   ├─ Check if sandbox exists via gRPC → reuse with policy hot-reload
  │   ├─ Or create new: prepare staging dir, spawn sandbox, wait for READY
  │   └─ Generate SSH config for sandbox exec
  ├─ Initial sync (blocking): deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  ├─ Start background sync task (every 5 min — re-deploys /sandbox/.platform/, GC stale entries)
  ├─ Start cron engine, OAuth callback server, refresh scheduler
  └─ Start teloxide long-polling dispatcher

Per message:
  ├─ Extract text + attachments from Telegram message
  ├─ Check if token request waiting for auth token → forward to intercept slot
  ├─ Route to worker task via DashMap<(chat_id, thread_id), Sender>
  ├─ Worker: debounce 500ms → download attachments → upload to sandbox inbox
  ├─ Format input: single text → raw string, multi/attachments → YAML
  ├─ Pipe input to claude -p via stdin (SSH or direct)
  │   ├─ First message: --session-id <uuid> (new session)
  │   ├─ Subsequent: --resume <root_session_id> (persistent session)
  │   └─ Sessions persist across messages — agent retains full CC context
  ├─ Parse reply JSON with typed attachments
  ├─ Send text reply to Telegram
  ├─ Download outbound attachments from sandbox outbox → send to Telegram
  └─ Periodic cleanup: hourly, configurable retention (default 7 days)

Config change (right agent config):
  ├─ Writes agent.yaml
  ├─ Detects filesystem policy change via gRPC GetSandboxPolicyStatus
  │   ├─ Network-only change: config_watcher → bot restart → hot-reload
  │   └─ Filesystem change: sandbox migration (below)
  ├─ config_watcher detects change (2s debounce)
  ├─ Bot exits with code 2
  ├─ process-compose restarts bot (on_failure policy)
  └─ Bot re-runs per-agent codegen with new config → applies fresh policy

Sandbox migration (filesystem policy change):
  ├─ Backup sandbox-only (SSH tar czpf)
  ├─ Create new sandbox right-<agent>-<YYYYMMDD-HHMM> with new policy
  ├─ Wait for READY + SSH ready
  ├─ Restore files via SSH tar xzpf
  ├─ Write sandbox.name to agent.yaml
  ├─ Delete old sandbox (best-effort)
  └─ config_watcher restarts bot → picks up new sandbox

right agent backup <name> [--sandbox-only]
  ├─ Sandbox mode: SSH tar /sandbox/ → sandbox.tar.gz
  ├─ No-sandbox mode: tar agent dir → sandbox.tar.gz
  ├─ Full mode: + agent.yaml, policy.yaml, VACUUM INTO data.db
  └─ Stored at ~/.right/backups/<agent>/<YYYYMMDD-HHMM>/

right agent init <name> --from-backup <path>
  ├─ Validate: agent must not exist, backup has sandbox.tar.gz + agent.yaml
  ├─ Restore config files to new agent dir
  ├─ Create new sandbox with timestamped name
  ├─ Restore sandbox files via SSH tar
  ├─ Write sandbox.name to agent.yaml
  └─ Run codegen + initial sync

right down
  └─ POST /project/stop to process-compose REST API
```

### Voice transcription

`voice` and `video_note` Telegram attachments are transcribed on the host
inside `download_attachments` when `agent.yaml`'s `stt.enabled` is true and
ffmpeg is present. The transcript is wrapped in a Russian marker
(`[Пользователь надиктовал...]` / `[Пользователь записал кружок...]`) and
prepended to the user-message text. The original audio file is dropped on
the host — it never reaches the sandbox.

Models live at `~/.right/cache/whisper/ggml-<model>.bin` and are
downloaded at `right up` (skipped if ffmpeg is missing). Default model
is `small`; per-agent override via `agent.yaml`:

    stt:
      enabled: true
      model: small   # tiny | base | small | medium | large-v3

When ffmpeg is missing or the model file is absent, the bot still runs;
voice messages produce an error marker that the agent relays to the user.

### OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as long as the agent lives and survive bot restarts.

```
Bot startup:
  ├─ gRPC GetSandbox → exists?
  │   ├─ YES: apply_policy (hot-reload via openshell policy set --wait)
  │   └─ NO: prepare_staging_dir → spawn_sandbox → wait_for_ready
  ├─ generate_ssh_config (on every startup, host-side file)
  ├─ initial_sync (blocking — before teloxide starts)
  │   ├─ Deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, re-deploys /sandbox/.platform/, GC stale entries)

Sandbox network:
  ├─ HTTP CONNECT proxy at 10.200.0.1:3128 (set via HTTPS_PROXY env)
  ├─ TLS MITM: proxy auto-detects TLS (ClientHello peek) and terminates
  │   unconditionally for credential injection (OpenShell v0.0.30+)
  │   └─ Sandbox trusts CA via /etc/openshell-tls/ca-bundle.pem
  └─ Policy controls which domains are allowed (wildcards supported)

Staging dir (minimal bootstrap — platform files deployed via /sandbox/.platform/ during initial_sync):
  ├─ .claude/settings.json    — CC behavioral flags
  ├─ .claude/reply-schema.json — structured output schema
  ├─ .claude.json              — trust + onboarding
  └─ mcp.json                  — MCP server entries
  EXCLUDED: skills (deployed to /sandbox/.platform/), credentials, plugins

Platform store (/sandbox/.platform/ inside sandbox):
  ├─ Content-addressed files: settings.json.<hash>, reply-schema.json.<hash>, ...
  ├─ Content-addressed skill dirs: skills/rightmcp.<hash>/, skills/rightcron.<hash>/
  ├─ Symlinked from /sandbox/.claude/ → /sandbox/.platform/
  ├─ Read-only (chmod a-w after deploy)
  └─ GC removes stale entries after each sync cycle
```

### Login Flow (setup-token)

When `claude -p` returns 403/401 (auth error):

```
1. is_auth_error() detects auth failure in CC JSON output
2. spawn_token_request() — tokio task:
   ├─ Send "Claude needs authentication" notification to Telegram
   ├─ Send setup-token instructions to Telegram
   ├─ Delete stale token from auth_tokens table (if any)
   ├─ Create oneshot channel, store sender in auth_code_tx intercept slot
   ├─ Wait for token from Telegram (5-min timeout)
   ├─ Telegram handler intercepts next message as token
   ├─ Save token to auth_tokens table in data.db
   └─ Send "Token saved" confirmation to Telegram
3. On next claude -p: load token from auth_tokens, inject as
   CLAUDE_CODE_OAUTH_TOKEN env var (sandbox: export in shell script,
   no-sandbox: cmd.env())
4. On error/timeout: notify user, reset auth_watcher_active flag
```

### MCP Token Refresh

```
OAuth callback (bot) → POST /set-token to Aggregator (Unix socket)
  → Aggregator updates DynamicAuthClient.token in-memory
  → Aggregator saves to mcp_servers SQLite table (auth_token, expires_at, etc.)
  → Aggregator starts refresh timer (expires_at - 10 min)
  → on timer: POST refresh_token to token_endpoint
  → update DynamicAuthClient.token in-memory
  → save refreshed token to SQLite (db_update_oauth_token)
  → no .mcp.json writes, no sandbox uploads
```

### MCP Auth Types

Four auth methods supported (detected automatically by `/mcp add`):

| auth_type | How token is injected | Detection |
|-----------|----------------------|-----------|
| `oauth` | `Authorization: Bearer` via DynamicAuthClient | OAuth AS discovery (RFC 9728/8414/OIDC) |
| `bearer` | `Authorization: Bearer` header | Haiku classification or fallback for private URLs |
| `header` | Custom header (e.g. `X-Api-Key`) | Haiku classification |
| `query_string` | Embedded in URL | URL contains `?` query string |

### MCP Aggregator

The Aggregator replaces HttpMemoryServer as the MCP endpoint. One shared process
serves all agents on TCP :8100/mcp with per-agent Bearer token authentication.

Tool routing:
  - No `__` prefix → RightBackend (built-in tools, unprefixed)
  - `rightmeta__` prefix → Aggregator management (read-only: mcp_list)
  - `{server}__` prefix → ProxyBackend (forwarded to upstream MCP)

Internal REST API on Unix socket (~/.right/run/internal.sock):
  - POST /mcp-add — register external MCP server
  - POST /mcp-remove — remove external MCP server
  - POST /set-token — deliver OAuth tokens after authentication
  - POST /mcp-list — list MCP servers with status
  - POST /mcp-instructions — fetch MCP server instructions markdown

Telegram bot uses InternalClient (hyper UDS) to call these endpoints.
Agents cannot reach the Unix socket from inside the sandbox.

### Prompting Architecture

Every `claude -p` invocation gets a **composite system prompt** assembled from
compiled-in constants (operating instructions, bootstrap) and agent-owned files
(IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md, MCP instructions, composite-memory).
No `--agent` flag — `--system-prompt-file` is the sole prompt mechanism.

A single `build_prompt_assembly_script()` generates a parameterized shell script
(root_path=/sandbox for OpenShell, root_path=agent_dir for no-sandbox) that assembles
the composite. Sandbox: executed via SSH. No-sandbox: executed via `bash -c`.

Prompt caching is critical — avoid per-message tool calls to read identity files.

See PROMPT_SYSTEM.md for full documentation.

### Claude Invocation Contract

Every `claude -p` invocation MUST go through `ClaudeInvocation` (defined in
`crates/bot/src/telegram/invocation.rs`). Direct construction of `claude_args`
vectors is forbidden — the builder enforces invariant flags at compile time.

**Invariants** (always present, cannot be omitted):
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

**Adding a new `claude -p` callsite:** construct a `ClaudeInvocation`, set fields,
call `.into_args()`, pass result to `build_prompt_assembly_script()`. Never build
args manually.

### Reflection Primitive

`crates/bot/src/reflection.rs` exposes `reflect_on_failure(ctx) -> Result<String, ReflectionError>`.
On CC invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a human-friendly
summary of the failure instead of the raw ring-buffer dump.

- Worker uses `ReflectionLimits::WORKER` (3 turns, $0.20, 90s process timeout).
  Reflection reply is sent to Telegram directly; on reflection failure, the
  caller falls back to the raw error message.
- Cron uses `ReflectionLimits::CRON` (5 turns, $0.40, 180s process timeout).
  Reflection reply is stored in `cron_runs.notify_json`; `cron_delivery` picks
  it up and relays using `DELIVERY_INSTRUCTION_FAILURE` (non-verbatim — agent
  may rephrase lightly, must preserve facts).
- `usage_events` rows for reflection use `source = "reflection"`, discriminated
  by `chat_id` (worker parent) vs `job_name` (cron parent). `/usage` shows them
  on a separate "🧠 Reflection" line per window.
- Reflection never reflects on itself. Hindsight `memory_retain` is skipped for
  reflection turns.
- `cron_runs.status` gates delivery: `'failed'` routes to
  `DELIVERY_INSTRUCTION_FAILURE`, any other status (currently `'success'`)
  routes to `DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

### Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. For cron jobs, stdout is tee'd into
an NDJSON log inside the sandbox at `/sandbox/crons/logs/{job_name}-{run_id}.ndjson`
(agents can read these directly via `Read`). Per-job retention keeps the last 10 logs.
Worker sessions do not write stream logs.

When `show_thinking: true` (default), a live thinking message in Telegram shows
the last 5 events (tool calls, text) with turn counter and cost. Updated every 2s
via `editMessageText`. Stays in chat after completion.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 2.0 for cron,
per-message from agent.yaml). Cron jobs disable `Agent` tool to prevent budget waste on
subagent branches. Process timeout (600s) is a safety net only.

### Configuration Hierarchy

| Scope | File | Source of Truth | Category |
|-------|------|-----------------|----------|
| Global | `~/.right/config.yaml` | Tunnel config | `AgentOwned` (edited by user) |
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, sandbox.name, env | `MergedRMW` |
| Generated | `agents/<name>/.claude/settings.json` | CC behavioral flags (regenerated on bot startup) | `Regenerated(BotRestart)` |
| Generated | `agents/<name>/.claude.json` | Trust, onboarding suppression (read-modify-write) | `MergedRMW` |
| Generated | `agents/<name>/.mcp.json` | MCP server entries (only "right" — externals managed by Aggregator) | `Regenerated(BotRestart)` |
| Per-agent | `agents/<name>/AGENTS.md` | Per-agent config (subagents, routing, skills) | `AgentOwned` |
| Agent-owned | `agents/<name>/TOOLS.md` | Agent-owned (created empty on init, then agent-edited) | `AgentOwned` |
| Per-agent | `agents/<name>/policy.yaml` | OpenShell sandbox policy (generated by agent init) | `Regenerated(SandboxRecreate)` |

See [Upgrade & Migration Model](#upgrade--migration-model) for category definitions.

### Memory

Two modes, configured per-agent via `memory.provider` in agent.yaml:

**File mode (default):** Agent manages `MEMORY.md` via CC Edit/Write.
Bot injects file contents into system prompt (truncated to 200 lines).
No MCP memory tools.

**Hindsight mode (optional):** Hindsight Cloud API (`api.hindsight.vectorize.io`),
one bank per agent. Three MCP tools exposed via aggregator:
`memory_retain`, `memory_recall`, `memory_reflect`. Prefetch cache is in-memory
(lost on restart → blocking recall on first interaction).

Auto-retain after each turn: content formatted as JSON role/content/timestamp
array, `document_id` = CC session UUID (same as `--resume`), `update_mode:
"append"` so only new content triggers LLM extraction (O(n) vs O(n²) for
full-session replace). Tags: `["chat:<chat_id>"]` for per-chat scoping.

Auto-recall before each `claude -p`: query truncated to 800 chars, tags
`["chat:<chat_id>"]` with `tags_match: "any"` (returns per-chat + global untagged
memories). Prefetch uses same parameters.

**Cron jobs skip memory:** Cron and delivery sessions perform no auto-recall
or auto-retain. Cron prompts are static instructions — recall results would be
irrelevant and corrupt user memory representations (same approach as hermes-agent
`skip_memory=True`). Crons can call `memory_recall` and `memory_retain` MCP tools
explicitly when needed.

The legacy `store_record` / `query_records` / `search_records` / `delete_record`
tools are removed from the surface; their backing tables (`memories`,
`memories_fts`, `memory_events`) are retained for migration compat.

### Memory Resilience Layer

`memory::resilient::ResilientHindsight` wraps `HindsightClient` with:
- per-process circuit breaker (closed→open after 5 fails in 30s; 30s initial
  open with doubling backoff to a 10 min cap; 1h hard open on Auth)
- classified retries (Transient/RateLimited yes; Auth/Client/Malformed no)
- SQLite-backed `pending_retains` queue (1000-row cap, 24h age cap)
- `watch::Sender<MemoryStatus>` signalling Healthy/Degraded/AuthFailed

The bot runs a single drain task (30s interval, batch 20, stop on first
non-Client failure). The aggregator shares the same SQLite queue via the
per-agent `data.db`; it enqueues on failure but never drains.

Telegram alerts (`memory_alerts` table, 24h dedup, 1h startup cleanup) fire
on:
- `AuthFailed` transition
- >20 `Client`-kind drops in a 1h rolling window (`client_flood`)

Doctor checks queue size (500/900 row thresholds), oldest-row age (1h/12h
thresholds), and long-standing (>24h) alerts.

### Memory Schema (SQLite)

Tables in per-agent `data.db`: `memories` / `memory_events` / `memories_fts`
(legacy, unused but retained for migration compat), `telegram_sessions`,
`cron_specs`, `cron_runs`, `mcp_servers`, `auth_tokens`, `pending_retains`,
`memory_alerts`. Run `sqlite3 data.db .schema` for column-level definitions.

## External Integrations

| System | Protocol | Notes |
|--------|----------|-------|
| process-compose | REST API (TCP :18927) | Health, process start/stop/restart, logs, shutdown |
| Claude Code CLI | Subprocess (`claude -p` via SSH) | Runs inside sandbox, structured JSON output |
| Claude Code CLI | Env var (CLAUDE_CODE_OAUTH_TOKEN) | Auth token from setup-token, injected into claude -p |
| OpenShell | gRPC + mTLS (:8080) | Sandbox create/poll/reuse, policy hot-reload, exec, file verification |
| OpenShell | CLI (`openshell sandbox upload/download`) | File transfer (no gRPC equivalent yet) |
| Telegram | teloxide long-polling | CacheMe<Throttle<Bot>> adaptor, per-agent allowlist |
| Cloudflare Tunnel | CLI (`cloudflared`) | Named tunnel, DNS CNAME, credentials file |
| MCP Aggregator | HTTP (:8100/mcp) + Unix socket (internal API) | Aggregates built-in + external MCP backends, per-agent Bearer auth |
| ffmpeg | system | Decode voice/video_note to PCM for whisper-rs | Optional — bot runs without it; voice transcription disabled. doctor warns. |

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
it is written by `codegen::pipeline` during `right up` and read by every
subsequent command that needs to talk to PC. Older state files without the
`pc_port` field deserialize to `PC_PORT` via `#[serde(default)]`.

### PC_API_TOKEN authentication

`right up` generates a random bearer token (`pc_api_token` in
`state.json`) and passes it to process-compose via `PC_API_TOKEN` env var.
PcClient reads the token from state.json and includes it in every request as
`Authorization: Bearer <token>`. Process-compose rejects unauthenticated
requests when this env var is set.

This prevents any stray HTTP caller (tests, debugging tools, browser
extensions) from accidentally stopping or restarting production bots by
hitting `localhost:18927`.

**When adding new CLI commands that touch PC, never import `PC_PORT` directly —
always resolve through `from_home(home)`.** For "is PC running?" probes,
treat `Ok(None)` as "no — skip or fail with a clear message pointing at
`right up`". `PC_PORT` may still be referenced in two places: by
`cmd_up` when passing `--port` to launch PC, and by `pipeline.rs` when
writing the default into `state.json`. Both are the same constant by
construction.

## SQLite Rules

### Migration Ownership

Both the MCP aggregator (`right-mcp-server`) and bot processes run schema migrations on per-agent `data.db` via `open_connection(path, migrate: true)`. Migrations are idempotent — concurrent callers are safe (WAL mode + busy_timeout). CLI commands and other processes open with `migrate: false`. Bot processes still declare `depends_on: right-mcp-server` for MCP readiness, but no longer depend on it for schema migrations.

### Transaction Rule

Any operation that performs 2+ writes (INSERT, UPDATE, DELETE) MUST wrap them in a single `conn.unchecked_transaction()`. Single-statement writes don't need a transaction. Migrations are the sole exception (handled by `rusqlite_migration` internally).

Use `unchecked_transaction()` (takes `&self`) rather than `transaction()` (takes `&mut self`) since most callsites hold `&Connection`.

### Idempotent Migrations

All migrations must be idempotent — safe to re-run if the schema already matches. SQLite lacks `ADD COLUMN IF NOT EXISTS`, so column additions must check `pragma_table_info` first. Use `M::up_with_hook()` for migrations that need conditional DDL. `CREATE TABLE/INDEX/TRIGGER IF NOT EXISTS` is naturally idempotent.

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
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, AGENTS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

Cross-agent outputs (process-compose.yaml, agent-tokens.json, cloudflared
config) are all `Regenerated(BotRestart)` — reread on `right up`.

`policy.yaml` mixes a hot-reloadable network section and a recreate-only
filesystem section. It's registered as the stricter `Regenerated(SandboxRecreate)`;
runtime discriminates via `openshell::filesystem_policy_changed`.

### Helper API

`crates/right-agent/src/codegen/contract.rs` provides the only sanctioned writers:

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
- SQLite schema — handled by `rusqlite_migration` (see `SQLite Rules`).

### Cross-references

- `CLAUDE.md` → `Upgrade-friendly design`, `Never delete sandboxes for
  recovery`, `Self-healing platform` — conventions this model implements.
- Data Flow → `Sandbox migration (filesystem policy change)` — the migration
  flow used by `Regenerated(SandboxRecreate)`.

## Integration Tests Using Live Sandboxes

Any test that needs a live OpenShell sandbox MUST create it via
`right_agent::test_support::TestSandbox::create("<test-name>")`. The helper:

- Generates a unique `right-test-<name>` sandbox with a minimal permissive policy (wildcard `"**.*"` host on port 443, `binaries: "**"`).
- Registers the sandbox in `test_cleanup` so sandboxes are deleted even under `panic = "abort"` (the panic hook drains the registry and calls `openshell sandbox delete`).
- Cleans up leftovers from prior SIGKILLed runs via `pkill_test_orphans`.
- Exposes `.exec(&[...])` which goes through gRPC — the project bans the `openshell sandbox exec` CLI from tests.
- Exposes `.name()` for helpers like `upload_file` that take a sandbox name.

Consumers outside `right-agent`'s own unit tests depend on the `test-support` feature:

```toml
[dev-dependencies]
right-agent = { path = "...", features = ["test-support"] }
```

Rules:

- Never hardcode sandbox names (no `right-foo-test-lifecycle` fixtures).
- Never invoke the `openshell` CLI from tests. Use `TestSandbox::exec` or the gRPC helpers in `crate::openshell`.
- Never add `#[ignore]` to sandbox tests. Dev machines have OpenShell.
- Parallel caps (`SandboxTestSlot` / `acquire_sandbox_slot`) are unchanged — tests that create multiple sandboxes should still acquire a slot.

## Security Model

- **Sandbox isolation**: OpenShell (k3s containers) — filesystem + network + TLS policies per agent
- **TLS MITM**: OpenShell proxy terminates and re-signs TLS with per-sandbox CA for L7 inspection
- **Credential isolation**: Host credentials never uploaded to sandbox. Each sandbox authenticates independently via OAuth login flow.
- **Network policy**: Wildcard domain allowlists (*.anthropic.com, *.claude.com, *.claude.ai) + `binaries: "**"`. TLS termination is automatic (OpenShell v0.0.30+).
- **`--dangerously-skip-permissions`**: Always on for all CC invocations. OpenShell policy is the security layer, not CC's permission system.
- **Prompt injection detection**: Pattern matching in memory guard before SQLite insert
- **Chat ID allowlist**: Empty = block all (secure default); per-agent in agent.yaml
- **Protected MCP**: "right" cannot be removed via `/mcp remove`
- **MCP tool restriction**: Agents cannot register/remove external MCP servers — `mcp_add`, `mcp_remove`, `mcp_auth` are not exposed as MCP tools. Only the user can manage servers via Telegram `/mcp` commands routed through the internal Unix socket API. This prevents sandbox escape via data exfiltration to attacker-controlled MCP endpoints.
- **OAuth CSRF**: Token matching in callback server

## OpenShell Integration Conventions

- **Prefer gRPC over CLI**: Use the OpenShell gRPC API (mTLS on :8080) for sandbox operations wherever possible. gRPC is faster, more reliable, and provides structured responses. The CLI (`openshell sandbox upload/download`) is only used for file transfer — no gRPC file transfer API exists yet.
- **gRPC for**: sandbox create/get/delete, readiness polling, exec inside sandbox, policy status, SSH session management.
- **CLI for**: file upload/download (SSH+tar under the hood), policy apply (`openshell policy set`).
- **NEVER use CLI for exec**: `openshell sandbox exec` CLI has unreliable argument parsing (positional name vs `--name` flag). Always use gRPC `exec_in_sandbox()` for executing commands inside sandboxes. All callers (sync, platform_store, etc.) must receive a gRPC client.
- **Known CLI bug**: Directory uploads may silently drop small files. Always verify critical files after directory upload, and re-upload individually if missing.

## OpenShell Policy Gotchas

- **Do not emit `tls:` field** (OpenShell v0.0.30+). The proxy auto-detects TLS via ClientHello peek and terminates unconditionally for credential injection. Writing `tls: terminate` or `tls: passthrough` triggers a per-request `WARN` in the sandbox supervisor log and the field is slated for removal. Omit the field for auto-detect; use `tls: skip` only to explicitly disable termination (raw tunnel).
- `binaries: path: "**"` not `"/sandbox/**"`. Claude binary lives at `/usr/local/bin/claude`, not under `/sandbox/`.
- `protocol: rest` and `access: full` are required for HTTPS endpoints so the proxy applies L7 policy on the terminated plaintext.
- Wildcard domains (`*.anthropic.com`) work — the earlier 403 was caused by the binaries restriction, not wildcard matching.
- CC actively manages `.claude.json` — strips unknown project trust entries on startup. Use `--dangerously-skip-permissions` instead of relying on trust entries.
- `HTTPS_PROXY=http://10.200.0.1:3128` is set automatically inside sandbox. All HTTP/HTTPS goes through the proxy.
- **Host service access from sandbox** (`host.openshell.internal`): requires `allowed_ips` in the policy endpoint to bypass SSRF protection. Server must bind `0.0.0.0` (not `127.0.0.1` — loopback is always blocked). Plain HTTP just works (no TLS to terminate). Prefer `host.openshell.internal` over `host.docker.internal` — both resolve to the same IP, but the OpenShell hostname is guaranteed available in all sandboxes regardless of Docker setup.
- **NixOS users**: must add `networking.firewall.trustedInterfaces = [ "docker0" "br-+" ];` to NixOS config. OpenShell runs k3s inside a Docker container on a custom bridge network (`br-XXXXX`), not the default `docker0`. Without this, the NixOS firewall drops traffic from k3s pods to host services. The `+` suffix is iptables wildcard matching all `br-*` interfaces.
- **Filesystem policy changes require sandbox recreation**: `openshell policy set --wait` hot-reloads network policies but does NOT apply filesystem policy changes to running sandboxes. Landlock rules are set at sandbox creation time. To apply filesystem_policy changes, the sandbox must be destroyed and recreated.

## Directory Layout (Runtime)

`~/.right/` is the runtime root (override with `--home`). Critical paths:

- `config.yaml` — global config (tunnel).
- `agents/<name>/` — per-agent state. Key files: `agent.yaml`, `policy.yaml`, `data.db`, `.claude/.credentials.json` (symlink to `~/.claude/.credentials.json`, host-only — NOT uploaded to sandbox). Subdirs include `crons/`, `inbox/`, `outbox/`, and `tmp/` for staging during attachment transfer.
- `run/process-compose.yaml`, `run/state.json` (carries `pc_port` + `pc_api_token`), `run/internal.sock` (bot↔aggregator UDS), `run/ssh/<agent>.ssh-config`.
- `backups/<agent>/<YYYYMMDD-HHMM>/` — `sandbox.tar.gz` plus optional `agent.yaml` + `data.db` + `policy.yaml` for full backups.
- `logs/<agent>.log.<date>` — per-agent daily log rotation. `mcp-aggregator.log` for the shared aggregator.
- `cache/whisper/ggml-<model>.bin` — STT models (downloaded at `right up`).

## Logging

Bot processes write to both stderr (process-compose TUI) and `~/.right/logs/<agent>.log` (daily rotation via `tracing-appender`). MCP Aggregator writes to both stdout (colored) and `~/.right/logs/mcp-aggregator.log` (daily rotation, no ANSI). Login flow has step-by-step INFO-level logging for debuggability.
