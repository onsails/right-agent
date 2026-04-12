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
│   ├── agent_def.rs    # Two agent def .md files per agent using @ file references: <name>.md (main) and <name>-bootstrap.md
│   ├── claude_json.rs  # .claude.json — trust (/sandbox + agent dir), onboarding, credential symlinks
│   ├── settings.rs     # .claude/settings.json — behavioral flags
│   ├── mcp_config.rs   # .mcp.json — only right entry; externals managed by Aggregator
│   ├── policy.rs       # OpenShell policy.yaml — network/filesystem/TLS sandbox rules
│   ├── process_compose.rs  # process-compose.yaml via minijinja
│   ├── tools.rs        # Generate TOOLS.md per agent
│   ├── mcp_instructions.rs  # Generate MCP_INSTRUCTIONS.md from SQLite mcp_servers cache
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
├── internal_api.rs       # Internal REST API on Unix socket (mcp-add, mcp-remove, set-token)
└── memory_server.rs      # MCP stdio server (CLI-only, deprecated)
```

### rightclaw-bot

```
src/
├── lib.rs              # Entry: resolve agent dir, open memory.db, sandbox lifecycle, start teloxide
├── telegram/
│   ├── attachments.rs  # Attachment extraction, download/upload, send, cleanup, YAML formatting
│   ├── mod.rs          # Token resolution (env > file > yaml)
│   ├── bot.rs          # Bot adaptor: CacheMe<Throttle<Bot>>
│   ├── dispatch.rs     # Long-polling dispatcher setup + dptree DI
│   ├── handler.rs      # /start, /reset, /mcp, /doctor + text→worker routing + auth code interception
│   ├── worker.rs       # Per-session CC invocation, auth error detection, reply parsing
│   ├── session.rs      # telegram_sessions table (chat_id, thread_id, uuid)
│   ├── filter.rs       # Allowed chat ID enforcement
│   └── oauth_callback.rs  # Axum OAuth redirect server
├── login.rs            # PTY-driven Claude login flow (expectrl) — menu navigation, URL extraction, code submission
├── sync.rs             # Background file sync: settings, schema, skills, .claude.json verification
├── cron.rs             # Cron engine: load specs from cron_specs table, lock check, invoke CC, persist results
├── cron_delivery.rs    # Delivery poll loop: idle detection, dedup, CC session delivery, cleanup
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
  ├─ Resolve token, open memory.db
  ├─ Per-agent codegen:
  │   ├─ settings.json, agent defs, system-prompt, schemas
  │   ├─ .claude.json, credentials symlink, mcp.json
  │   ├─ TOOLS.md, skills install, policy.yaml
  │   └─ memory.db init, git init, secret generation
  ├─ Clear Telegram webhook, verify bot identity
  ├─ Sandbox lifecycle:
  │   ├─ Check if sandbox exists via gRPC → reuse with policy hot-reload
  │   ├─ Or create new: prepare staging dir, spawn sandbox, wait for READY
  │   └─ Generate SSH config for sandbox exec
  ├─ Initial sync: upload settings.json, .claude.json, skills to sandbox (blocking)
  ├─ Start background sync task (every 5 min)
  ├─ Start cron engine, OAuth callback server, refresh scheduler
  └─ Start teloxide long-polling dispatcher

Per message:
  ├─ Extract text + attachments from Telegram message
  ├─ Check if login flow waiting for auth code → forward to PTY
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
  │   ├─ Upload settings.json, reply-schema.json, builtin skills
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, same operations)

Sandbox network:
  ├─ HTTP CONNECT proxy at 10.200.0.1:3128 (set via HTTPS_PROXY env)
  ├─ TLS MITM: proxy terminates TLS, re-signs with per-sandbox CA
  │   └─ Sandbox trusts CA via /etc/openshell-tls/ca-bundle.pem
  ├─ Policy controls which domains are allowed (wildcards supported)
  └─ tls: terminate REQUIRED on all HTTPS endpoints (OpenShell v0.0.23)

Staging dir (for initial upload only):
  ├─ .claude/settings.json    — CC behavioral flags
  ├─ .claude/reply-schema.json — structured output schema
  ├─ .claude/agents/<name>.md  — agent definition
  ├─ .claude/skills/{rightskills,rightcron}/ — builtin skills only
  ├─ .claude.json              — trust + onboarding
  └─ .mcp.json                 — MCP server entries
  EXCLUDED: .credentials.json (sandbox gets own via login), plugins/, shell-snapshots/
```

### Login Flow (PTY-driven)

When `claude -p` returns 403/401 (no credentials in sandbox):

```
1. is_auth_error() detects auth failure in CC JSON output
2. spawn_auth_watcher() — tokio task:
   ├─ Spawns blocking thread with expectrl PTY session
   ├─ SSH -t into sandbox: claude --dangerously-skip-permissions -- /login
   ├─ PTY width set to 500 cols (prevent URL line-wrapping)
   ├─ Wait for "Select login method" menu → send \r (Claude subscription)
   ├─ Wait for "Browser didn't open" → extract OAuth URL
   ├─ Send URL to Telegram
   ├─ Wait for "Paste" prompt → send WaitingForCode event
   ├─ Telegram handler intercepts next message as auth code
   ├─ Send code + \r to PTY
   └─ Wait for success/error from CC
3. On success: notify user in Telegram
4. On error/timeout: notify user, reset auth_watcher_active flag
```

### MCP Token Refresh

```
OAuth callback (bot) → POST /set-token to Aggregator (Unix socket)
  → Aggregator updates DynamicAuthClient.token in-memory
  → Aggregator saves to oauth-state.json
  → Aggregator starts refresh timer (expires_at - 10 min)
  → on timer: POST refresh_token to token_endpoint
  → update DynamicAuthClient.token in-memory
  → save to oauth-state.json
  → no .mcp.json writes, no sandbox uploads
```

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

Telegram bot uses InternalClient (hyper UDS) to call these endpoints.
Agents cannot reach the Unix socket from inside the sandbox.

### Prompting Architecture

Every `claude -p` invocation gets a **composite system prompt** assembled from identity files.
No `--agent` flag — `--system-prompt-file` is the sole prompt mechanism.

In sandbox mode, the composite is assembled **inside the sandbox** (single SSH command,
`cat` + `claude -p`). In no-sandbox mode, assembled on host from local files.

Prompt caching is critical — avoid per-message tool calls to read identity files.

See PROMPT_SYSTEM.md for full documentation.

### Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. Each event is written to a per-session
NDJSON log at `~/.rightclaw/logs/streams/<session-uuid>.ndjson`.

When `show_thinking: true` (default), a live thinking message in Telegram shows
the last 5 events (tool calls, text) with turn counter and cost. Updated every 2s
via `editMessageText`. Stays in chat after completion.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 1.0)
from agent.yaml. Process timeout (600s) is a safety net only.

### Configuration Hierarchy

| Scope | File | Source of Truth |
|-------|------|-----------------|
| Global | `~/.rightclaw/config.yaml` | Tunnel config |
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, env |
| Generated | `agents/<name>/.claude/settings.json` | CC behavioral flags (regenerated on bot startup) |
| Generated | `agents/<name>/.claude.json` | Trust, onboarding suppression (read-modify-write) |
| Generated | `agents/<name>/.mcp.json` | MCP server entries (only "right" — externals managed by Aggregator) |
| Agent-owned | `agents/<name>/TOOLS.md` | Agent-owned (created empty on init, then agent-edited) |
| Generated | `agents/<name>/MCP_INSTRUCTIONS.md` | Generated by Aggregator from SQLite mcp_servers cache |
| Per-agent | `agents/<name>/policy.yaml` | OpenShell sandbox policy (generated by agent init) |

### Memory Schema (SQLite)

```
memories        (id, content, tags, stored_by, source_tool, created_at, deleted_at, importance)
memory_events   (memory_id, event_type, actor, timestamp)
memories_fts    (FTS5 virtual table — BM25 ranking)
telegram_sessions (chat_id, effective_thread_id, session_uuid, created_at)
cron_specs      (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at)
cron_runs       (id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at)
mcp_servers     (name, url, instructions TEXT, created_at)
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
LoginEvent      // PTY→async: Url, WaitingForCode, Done, Error
```

## External Integrations

| System | Protocol | Notes |
|--------|----------|-------|
| process-compose | REST API (TCP :18927) | Health, process start/stop/restart, logs, shutdown |
| Claude Code CLI | Subprocess (`claude -p` via SSH) | Runs inside sandbox, structured JSON output |
| Claude Code CLI | PTY (expectrl via SSH) | Interactive login flow — menu navigation, OAuth |
| OpenShell | gRPC + mTLS (:8080) | Sandbox create/poll/reuse, policy hot-reload, exec, file verification |
| OpenShell | CLI (`openshell sandbox upload/download`) | File transfer (no gRPC equivalent yet) |
| Telegram | teloxide long-polling | CacheMe<Throttle<Bot>> adaptor, per-agent allowlist |
| Cloudflare Tunnel | CLI (`cloudflared`) | Named tunnel, DNS CNAME, credentials file |
| MCP Aggregator | HTTP (:8100/mcp) + Unix socket (internal API) | Aggregates built-in + external MCP backends, per-agent Bearer auth |

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

## Directory Layout (Runtime)

```
~/.rightclaw/
├── config.yaml
├── agents/<name>/
│   ├── AGENTS.md, BOOTSTRAP.md, agent.yaml
│   ├── IDENTITY.md, SOUL.md, USER.md  # created by bootstrap CC session, not init
│   ├── TOOLS.md                       # agent-owned (created empty on init, then agent-edited)
│   ├── MCP_INSTRUCTIONS.md            # generated by Aggregator from SQLite mcp_servers cache
│   ├── policy.yaml          # OpenShell sandbox policy (openshell agents only)
│   ├── memory.db
│   ├── oauth-state.json
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
│       ├── agents/<name>.md           # main agent def (@ file references)
│       ├── agents/<name>-bootstrap.md # bootstrap agent def (@ file references)
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

Bot processes write to both stderr (process-compose TUI) and `~/.rightclaw/logs/<agent>.log` (daily rotation via `tracing-appender`). Login flow has step-by-step INFO-level logging for debuggability.
