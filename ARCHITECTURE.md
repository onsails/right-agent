# Architecture

## Workspace

Three crates in a Cargo workspace:

| Crate | Path | Role |
|-------|------|------|
| **rightclaw** | `crates/rightclaw/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, OpenShell |
| **rightclaw-cli** | `crates/rightclaw-cli/` | CLI binary (`rightclaw`) + MCP Aggregator (HTTP) |
| **rightclaw-bot** | `crates/bot/` | Telegram bot runtime (teloxide) + cron engine + login flow |

## Module Map

### rightclaw (core)

```
src/
├── agent/
│   ├── discovery.rs    # Scan agents/ dir: agent presence detected by agent.yaml, validate names, parse config
│   └── types.rs        # AgentDef, AgentConfig, RestartPolicy, SandboxOverrides
├── config/
│   └── config.rs       # GlobalConfig (tunnel), RIGHTCLAW_HOME resolution
├── codegen/
│   ├── agent_def.rs    # System prompt generation, compiled-in constants (OPERATING_INSTRUCTIONS, BOOTSTRAP_INSTRUCTIONS), JSON schemas
│   ├── claude_json.rs  # .claude.json — trust (/sandbox + agent dir), onboarding, credential symlinks
│   ├── settings.rs     # .claude/settings.json — behavioral flags
│   ├── mcp_config.rs   # .mcp.json — only right entry; externals managed by Aggregator
│   ├── policy.rs       # OpenShell policy.yaml — network/filesystem/TLS sandbox rules
│   ├── process_compose.rs  # process-compose.yaml via minijinja
│   ├── tools.rs        # Generate TOOLS.md per agent
│   ├── mcp_instructions.rs  # Generate MCP instructions markdown from SQLite mcp_servers cache
│   ├── skills.rs       # Install built-in skills to .claude/skills/
│   ├── telegram.rs     # Telegram-specific codegen helpers
│   └── cloudflared.rs  # Tunnel entry generation
├── memory/
│   ├── store.rs        # SQLite: store, recall, search (FTS5/BM25), forget
│   ├── migrations.rs   # Schema versioning (rusqlite_migration)
│   ├── guard.rs        # Prompt injection detection
│   └── error.rs        # MemoryError types
├── runtime/
│   ├── state.rs        # RuntimeState persistence (JSON)
│   ├── pc_client.rs    # process-compose REST API client (port 18927)
│   └── deps.rs         # Binary availability checks
├── mcp/
│   ├── credentials.rs  # OAuth token persistence + SQLite server registry
│   ├── internal_client.rs  # Hyper UDS client for bot→aggregator IPC
│   ├── oauth.rs        # OAuth flow initiation
│   ├── proxy.rs        # ProxyBackend, DynamicAuthClient, BackendStatus
│   └── refresh.rs      # Token refresh scheduler (runs in Aggregator process)
├── openshell.rs        # gRPC mTLS — sandbox create/poll/exec, CLI wrappers for upload/download, staging dir, file verification
├── doctor.rs           # Diagnostic checks (deps, structure, MCP, sandbox, tunnel)
├── init.rs             # rightclaw init workflow + init_agent() for per-agent init
└── error.rs            # AgentError (miette diagnostics)
```

### rightclaw-cli

```
src/
├── main.rs               # CLI dispatcher
├── aggregator.rs         # MCP Aggregator: Aggregator + ToolDispatcher + BackendRegistry
├── right_backend.rs      # RightBackend: 13 built-in tools (memory, cron, mcp_list, bootstrap)
├── internal_api.rs       # Internal REST API on Unix socket (mcp-add, mcp-remove, set-token, mcp-instructions)
└── memory_server.rs      # MCP stdio server (CLI-only, deprecated)
```

### rightclaw-bot

```
src/
├── lib.rs              # Entry: resolve agent dir, open data.db, sandbox lifecycle, start teloxide
├── telegram/
│   ├── prompt.rs       # Shared prompt assembly: build_prompt_assembly_script, shell helpers
│   ├── attachments.rs  # Attachment extraction, download/upload, send, cleanup, YAML formatting
│   ├── mod.rs          # Token resolution (env > file > yaml)
│   ├── bot.rs          # Bot adaptor: CacheMe<Throttle<Bot>>
│   ├── dispatch.rs     # Long-polling dispatcher setup + dptree DI
│   ├── handler.rs      # /start, /reset, /mcp, /doctor + text→worker routing + auth code interception
│   ├── worker.rs       # Per-session CC invocation, auth error detection, reply parsing
│   ├── session.rs      # telegram_sessions table (chat_id, thread_id, uuid)
│   ├── filter.rs       # Allowed chat ID enforcement
│   └── oauth_callback.rs  # Axum OAuth redirect server
├── login.rs            # Token-based Claude login flow — setup-token request, DB persistence, env var injection
├── sync.rs             # Background file sync: settings, schema, skills, .claude.json verification
├── cron.rs             # Cron engine: load specs, lock check, invoke CC with system prompt, persist results
├── cron_delivery.rs    # Delivery poll loop: idle detection, dedup, CC session delivery (haiku), cleanup
└── error.rs            # BotError types
```

## Data Flow

### Agent Lifecycle

```
rightclaw init
  ├─ Create ~/.rightclaw/agents/<name>/ with template files
  ├─ Write AGENTS.md, BOOTSTRAP.md, agent.yaml
  │   (IDENTITY.md, SOUL.md, USER.md created later by bootstrap CC session)
  ├─ Generate .claude/settings.json, .claude.json
  ├─ Symlink credentials from ~/.claude/
  ├─ Detect Telegram token, cloudflared tunnel
  └─ Write ~/.rightclaw/config.yaml

rightclaw agent init <name>
  ├─ Interactive wizard: sandbox mode, network policy, telegram, model
  ├─ Create ~/.rightclaw/agents/<name>/ with template files
  ├─ Write AGENTS.md, BOOTSTRAP.md, agent.yaml
  │   (IDENTITY.md, SOUL.md, USER.md created later by bootstrap CC session)
  ├─ Write sandbox config to agent.yaml (mode + policy_file)
  ├─ If mode=openshell: generate policy.yaml in agent dir
  ├─ Generate .claude/settings.json, .claude.json
  └─ Symlink credentials from ~/.claude/

rightclaw up [--agents x,y] [--detach] [--no-sandbox]
  ├─ Discover agents from agents/ directory
  ├─ Per agent: resolve secret for token map (generate if missing)
  ├─ Generate agent-tokens.json
  ├─ Generate process-compose.yaml (minijinja)
  ├─ Generate cloudflared config (if tunnel)
  └─ Launch process-compose (TUI or detached)

rightclaw bot --agent <name>  (spawned by process-compose)
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
  ├─ Initial sync (blocking): deploy platform files to /platform/ (content-addressed + symlinks)
  ├─ Start background sync task (every 5 min — re-deploys /platform/, GC stale entries)
  ├─ Start cron engine, OAuth callback server, refresh scheduler
  └─ Start teloxide long-polling dispatcher

Per message:
  ├─ Extract text + attachments from Telegram message
  ├─ Check if token request waiting for auth token → forward to intercept slot
  ├─ Route to worker task via DashMap<(chat_id, thread_id), Sender>
  ├─ Worker: debounce 500ms → download attachments → upload to sandbox inbox
  ├─ Format input: single text → raw string, multi/attachments → YAML
  ├─ Pipe input to claude -p via stdin (SSH or direct)
  ├─ Parse reply JSON with typed attachments
  ├─ Send text reply to Telegram
  ├─ Download outbound attachments from sandbox outbox → send to Telegram
  └─ Periodic cleanup: hourly, configurable retention (default 7 days)

Config change (rightclaw agent config):
  ├─ Writes agent.yaml
  ├─ config_watcher detects change (2s debounce)
  ├─ Bot exits with code 2
  ├─ process-compose restarts bot (on_failure policy)
  └─ Bot re-runs per-agent codegen with new config → applies fresh policy

rightclaw down
  └─ POST /project/stop to process-compose REST API
```

### OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. Survive bot restarts.

```
Bot startup:
  ├─ gRPC GetSandbox → exists?
  │   ├─ YES: apply_policy (hot-reload via openshell policy set --wait)
  │   └─ NO: prepare_staging_dir → spawn_sandbox → wait_for_ready
  ├─ generate_ssh_config (on every startup, host-side file)
  ├─ initial_sync (blocking — before teloxide starts)
  │   ├─ Deploy platform files to /platform/ (content-addressed + symlinks)
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, re-deploys /platform/, GC stale entries)

Sandbox network:
  ├─ HTTP CONNECT proxy at 10.200.0.1:3128 (set via HTTPS_PROXY env)
  ├─ TLS MITM: proxy terminates TLS, re-signs with per-sandbox CA
  │   └─ Sandbox trusts CA via /etc/openshell-tls/ca-bundle.pem
  ├─ Policy controls which domains are allowed (wildcards supported)
  └─ tls: terminate REQUIRED on all HTTPS endpoints (OpenShell v0.0.23)

Staging dir (minimal bootstrap — platform files deployed via /platform/ during initial_sync):
  ├─ .claude/settings.json    — CC behavioral flags
  ├─ .claude/reply-schema.json — structured output schema
  ├─ .claude.json              — trust + onboarding
  └─ mcp.json                  — MCP server entries
  EXCLUDED: skills (deployed to /platform/), credentials, plugins

Platform store (/platform/ inside sandbox):
  ├─ Content-addressed files: settings.json.<hash>, reply-schema.json.<hash>, ...
  ├─ Content-addressed skill dirs: skills/rightmcp.<hash>/, skills/rightcron.<hash>/
  ├─ Symlinked from /sandbox/.claude/ → /platform/
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

Internal REST API on Unix socket (~/.rightclaw/run/internal.sock):
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
(IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md).
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

### Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. Each event is written to a per-session
NDJSON log at `~/.rightclaw/logs/streams/<session-uuid>.ndjson`.

When `show_thinking: true` (default), a live thinking message in Telegram shows
the last 5 events (tool calls, text) with turn counter and cost. Updated every 2s
via `editMessageText`. Stays in chat after completion.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 2.0 for cron,
per-message from agent.yaml). Cron jobs disable `Agent` tool to prevent budget waste on
subagent branches. Process timeout (600s) is a safety net only.

### Configuration Hierarchy

| Scope | File | Source of Truth |
|-------|------|-----------------|
| Global | `~/.rightclaw/config.yaml` | Tunnel config |
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, env |
| Generated | `agents/<name>/.claude/settings.json` | CC behavioral flags (regenerated on bot startup) |
| Generated | `agents/<name>/.claude.json` | Trust, onboarding suppression (read-modify-write) |
| Generated | `agents/<name>/.mcp.json` | MCP server entries (only "right" — externals managed by Aggregator) |
| Per-agent | `agents/<name>/AGENTS.md` | Per-agent config (subagents, routing, skills) |
| Agent-owned | `agents/<name>/TOOLS.md` | Agent-owned (created empty on init, then agent-edited) |
| Per-agent | `agents/<name>/policy.yaml` | OpenShell sandbox policy (generated by agent init) |

### Memory Schema (SQLite)

```
memories        (id, content, tags, stored_by, source_tool, created_at, deleted_at, importance)
memory_events   (memory_id, event_type, actor, timestamp)
memories_fts    (FTS5 virtual table — BM25 ranking)
telegram_sessions (chat_id, effective_thread_id, session_uuid, created_at)
cron_specs      (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at)
cron_runs       (id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at)
mcp_servers     (name, url, instructions, auth_type, auth_header, auth_token, refresh_token, token_endpoint, client_id, client_secret, expires_at, created_at)
auth_tokens     (token, created_at)
```

## Key Types

```rust
AgentDef        // Discovered agent: name, path, identity, config, optional files
AgentConfig     // From agent.yaml: restart, model, telegram, SandboxConfig, env
GlobalConfig    // From config.yaml: tunnel
RuntimeState    // Persisted JSON: agents, socket_path, started_at
MemoryEntry     // SQLite row: id, content, tags, stored_by, importance
WorkerContext   // Per-session: chat_id, thread_id, agent_dir, bot, db, ssh config, pc_port, auth state
ProcessInfo     // From process-compose API: name, status, pid, exit_code
LoginEvent      // Token request→async: Done, Error
```

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

## SQLite Rules

### Migration Ownership

Only the MCP aggregator (`right-mcp-server`) runs schema migrations via `open_connection(path, migrate: true)`. All other processes (bots, CLI commands, runtime code) open the database with `migrate: false`. Bot processes declare `depends_on: right-mcp-server: condition: process_started` in process-compose to ensure the aggregator migrates before bots start.

### Transaction Rule

Any operation that performs 2+ writes (INSERT, UPDATE, DELETE) MUST wrap them in a single `conn.unchecked_transaction()`. Single-statement writes don't need a transaction. Migrations are the sole exception (handled by `rusqlite_migration` internally).

Use `unchecked_transaction()` (takes `&self`) rather than `transaction()` (takes `&mut self`) since most callsites hold `&Connection`.

### Idempotent Migrations

All migrations must be idempotent — safe to re-run if the schema already matches. SQLite lacks `ADD COLUMN IF NOT EXISTS`, so column additions must check `pragma_table_info` first. Use `M::up_with_hook()` for migrations that need conditional DDL. `CREATE TABLE/INDEX/TRIGGER IF NOT EXISTS` is naturally idempotent.

## Security Model

- **Sandbox isolation**: OpenShell (k3s containers) — filesystem + network + TLS policies per agent
- **TLS MITM**: OpenShell proxy terminates and re-signs TLS with per-sandbox CA for L7 inspection
- **Credential isolation**: Host credentials never uploaded to sandbox. Each sandbox authenticates independently via OAuth login flow.
- **Network policy**: Wildcard domain allowlists (*.anthropic.com, *.claude.com, *.claude.ai) + `tls: terminate` + `binaries: "**"`
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

- `tls: terminate` is **required** on all HTTPS endpoints (OpenShell v0.0.23). Without it, proxy attempts raw L4 tunnel which fails with "Connection reset by peer" during TLS handshake.
- `binaries: path: "**"` not `"/sandbox/**"`. Claude binary lives at `/usr/local/bin/claude`, not under `/sandbox/`.
- `protocol: rest` and `access: full` required when `tls: terminate` is set.
- Wildcard domains (`*.anthropic.com`) work — the earlier 403 was caused by the binaries restriction, not wildcard matching.
- CC actively manages `.claude.json` — strips unknown project trust entries on startup. Use `--dangerously-skip-permissions` instead of relying on trust entries.
- `HTTPS_PROXY=http://10.200.0.1:3128` is set automatically inside sandbox. All HTTP/HTTPS goes through the proxy.
- **Host service access from sandbox** (`host.docker.internal`): requires `allowed_ips: ["172.16.0.0/12"]` in the policy endpoint to bypass SSRF protection. Server must bind `0.0.0.0` (not `127.0.0.1` — loopback is always blocked). Plain HTTP works without `tls: terminate`.
- **NixOS users**: must add `networking.firewall.trustedInterfaces = [ "docker0" "br-+" ];` to NixOS config. OpenShell runs k3s inside a Docker container on a custom bridge network (`br-XXXXX`), not the default `docker0`. Without this, the NixOS firewall drops traffic from k3s pods to host services. The `+` suffix is iptables wildcard matching all `br-*` interfaces.
- **Filesystem policy changes require sandbox recreation**: `openshell policy set --wait` hot-reloads network policies but does NOT apply filesystem policy changes to running sandboxes. Landlock rules are set at sandbox creation time. To apply filesystem_policy changes, the sandbox must be destroyed and recreated.

## Directory Layout (Runtime)

```
~/.rightclaw/
├── config.yaml
├── agents/<name>/
│   ├── AGENTS.md, BOOTSTRAP.md, agent.yaml
│   ├── IDENTITY.md, SOUL.md, USER.md  # created by bootstrap CC session, not init
│   ├── TOOLS.md                       # agent-owned (created empty on init, then agent-edited)
│   ├── policy.yaml          # OpenShell sandbox policy (openshell agents only)
│   ├── data.db            # SQLite: memories, sessions, cron, MCP servers + auth state
│   ├── oauth-callback.sock
│   ├── crons/*.yaml
│   ├── inbox/          # Received Telegram attachments (no-sandbox mode)
│   ├── outbox/         # CC-generated files for Telegram (no-sandbox mode)
│   ├── tmp/inbox/      # Temporary download before sandbox upload
│   ├── tmp/outbox/     # Temporary download from sandbox for sending
│   ├── staging/           # Prepared on sandbox creation only
│   └── .claude/
│       ├── settings.json
│       ├── .mcp.json
│       ├── .credentials.json → ~/.claude/.credentials.json  (host-only, NOT uploaded to sandbox)
│       ├── reply-schema.json
│       ├── bootstrap-schema.json      # generated per agent by bot on startup
│       └── skills/{rightskills,rightcron}/
├── run/
│   ├── process-compose.yaml
│   ├── ssh/<agent>.ssh-config
│   ├── internal.sock         # Unix socket for bot→aggregator IPC
│   └── runtime-state.json
├── logs/
│   └── <agent>.log.<date>   # Per-agent daily log rotation
└── scripts/
    └── cloudflared-start.sh
```

## Logging

Bot processes write to both stderr (process-compose TUI) and `~/.rightclaw/logs/<agent>.log` (daily rotation via `tracing-appender`). MCP Aggregator writes to both stdout (colored) and `~/.rightclaw/logs/mcp-aggregator.log` (daily rotation, no ANSI). Login flow has step-by-step INFO-level logging for debuggability.
