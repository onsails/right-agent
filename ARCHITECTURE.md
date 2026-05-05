# Architecture

## Workspace

Three crates in a Cargo workspace:

| Crate | Path | Role |
|-------|------|------|
| **right-agent** | `crates/right-agent/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, OpenShell |
| **right** | `crates/right/` | CLI binary (`right`) + MCP Aggregator (HTTP) |
| **right-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |

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

### Login Flow (setup-token)

See: `docs/architecture/lifecycle.md` (Login Flow).

### MCP Token Refresh

See: `docs/architecture/mcp.md` (MCP Token Refresh).

### MCP Auth Types

Four auth methods supported (detected automatically by `/mcp add`):

| auth_type | How token is injected | Detection |
|-----------|----------------------|-----------|
| `oauth` | `Authorization: Bearer` via DynamicAuthClient | OAuth AS discovery (RFC 9728/8414/OIDC) |
| `bearer` | `Authorization: Bearer` header | Haiku classification or fallback for private URLs |
| `header` | Custom header (e.g. `X-Api-Key`) | Haiku classification |
| `query_string` | Embedded in URL | URL contains `?` query string |

### MCP Aggregator

One shared aggregator process serves all agents on TCP `:8100/mcp` with
per-agent Bearer-token auth. Tool routing rules:

- No `__` prefix → `RightBackend` (built-in tools, unprefixed).
- `rightmeta__` prefix → Aggregator management (read-only: `mcp_list`).
- `{server}__` prefix → `ProxyBackend` (forwarded to upstream MCP).

Internal REST API on Unix socket (`~/.right/run/internal.sock`):
`POST /mcp-add`, `POST /mcp-remove`, `POST /set-token`, `POST /mcp-list`,
`POST /mcp-instructions`. Telegram bot uses `InternalClient` (hyper UDS).
Agents cannot reach the Unix socket from inside the sandbox.

See: `docs/architecture/mcp.md` for dispatch detail and rationale.

### Prompting Architecture

Every `claude -p` invocation gets a composite system prompt via
`--system-prompt-file` (the sole prompt mechanism — no `--agent` flag).
Prompt caching is critical — avoid per-message tool calls to read
identity files.

See `PROMPT_SYSTEM.md` for full documentation.

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

`crates/bot/src/reflection.rs` exposes
`reflect_on_failure(ctx) -> Result<String, ReflectionError>`. On CC
invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a
human-friendly summary of the failure.

Reflection never reflects on itself. Hindsight `memory_retain` is skipped
for reflection turns. `cron_runs.status` gates delivery: `'failed'` routes
to `DELIVERY_INSTRUCTION_FAILURE`; any other status routes to
`DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

See: `docs/architecture/sessions.md` for `ReflectionLimits` (worker vs
cron), usage-event accounting, and label-routing detail.

### Stream Logging

See: `docs/architecture/sessions.md` (Stream Logging).

### Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a
`ScheduleKind` variant. The **`Immediate`** variant (encoded as
`schedule = '@immediate'`) is bot-internal — used for
background-continuation jobs and fired on the next reconcile tick (≤5s).
`insert_immediate_cron` defaults `lock_ttl` to
`IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`); the lock heartbeat is written once
at job start and never refreshed, so a tighter TTL would let the
reconciler spawn a duplicate `execute_job` against the same spec on the
next 5-second tick. The TTL is the duplicate-prevention guard, not a
wall-clock execution limit.

See: `docs/architecture/sessions.md` for the full variant list.

### Per-session mutex on --resume

See: `docs/architecture/sessions.md` (Per-session mutex on --resume).

### Background continuation: X-FORK-FROM convention

A background continuation cron job is identified by its prompt starting with
`X-FORK-FROM: <main_session_id>\n`. `cron::execute_job` strips this header,
sets `ClaudeInvocation::resume_session_id` and `fork_session = true`, and
passes the body as the user message. The forked session inherits the main
session's full history; the body is a short SYSTEM_NOTICE asking the agent to
finish answering the user's most recent message.

This convention avoids a `cron_specs` schema migration. It is bot-internal —
no agent or user is expected to construct prompts with this prefix. The parser
refuses to run a job whose `schedule_kind` is anything other than `Immediate`
even if the prefix is present — agents cannot hijack `--resume` by crafting
prompts. Invalid UUIDs in the header are also refused (the run is marked
`failed`, the lock file is removed, and `execute_job` returns).

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

### Memory

Two modes, configured per-agent via `memory.provider` in `agent.yaml`:
**Hindsight** (primary, Hindsight Cloud API) and **file** (fallback,
agent-managed `MEMORY.md`). MCP tools `memory_retain` / `memory_recall` /
`memory_reflect` are exposed only in Hindsight mode.

See: `docs/architecture/memory.md` for auto-retain/recall semantics,
prefetch cache behavior, cron-skip rules, and backgrounded-turn handling.

### Memory Resilience Layer

See: `docs/architecture/memory.md` (Memory Resilience Layer).

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
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

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

## Brand-conformant CLI output

Every user-facing TUI surface in `right` and `right-bot` MUST go through
`right_agent::ui::*` (see `crates/right-agent/src/ui/`). Raw `println!` /
`eprintln!` of user-facing text is a review-blocking defect. Visual
contract, atoms, and theme rules: `docs/brand-guidelines.html` and the
redesign spec at
`docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md`.

Past miss: `cmd_agent_rebootstrap` (`crates/right/src/main.rs`) shipped
with raw `println!` and bare `✓`/`⚠` literals, bypassing the rail and
theme detection. Do not repeat; migrate existing offenders when touched.

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

Bot processes log to stderr + `~/.right/logs/<agent>.log` (daily rotation
via `tracing-appender`). Aggregator logs to stdout +
`~/.right/logs/mcp-aggregator.log`. See: `docs/architecture/sessions.md`
for stream-logging detail.
