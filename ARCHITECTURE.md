# Architecture

## Workspace

Three crates in a Cargo workspace:

| Crate | Path | Role |
|-------|------|------|
| **rightclaw** | `crates/rightclaw/` | Core library — agent discovery, codegen, config, memory, runtime, MCP, OpenShell |
| **rightclaw-cli** | `crates/rightclaw-cli/` | CLI binary (`rightclaw`) + memory MCP server (stdio/HTTP) |
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
│   ├── mcp_config.rs   # .mcp.json — right + external MCP entries
│   ├── policy.rs       # OpenShell policy.yaml — network/filesystem/TLS sandbox rules
│   ├── process_compose.rs  # process-compose.yaml via minijinja
│   ├── tools.rs        # Generate TOOLS.md per agent
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
│   ├── credentials.rs  # OAuth token persistence
│   ├── detect.rs       # MCP server detection from .claude.json
│   ├── oauth.rs        # OAuth flow initiation
│   └── refresh.rs      # Token refresh scheduler
├── openshell.rs        # gRPC mTLS — sandbox create/poll/exec, CLI wrappers for upload/download, staging dir, file verification
├── doctor.rs           # Diagnostic checks (deps, structure, MCP, sandbox, tunnel)
├── init.rs             # rightclaw init workflow + init_agent() for per-agent init
└── error.rs            # AgentError (miette diagnostics)
```

### rightclaw-cli

```
src/
├── main.rs               # CLI dispatcher (clap): init, agent init, list, doctor, up, down, status, restart, attach, pair, config, memory, mcp, bot, memory-server
├── memory_server.rs      # MCP stdio server — store/recall/search/forget + cron + mcp management
└── memory_server_http.rs # Optional HTTP transport for MCP
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
├── cron.rs             # Cron engine: load specs, lock check, invoke CC, notify
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
  ├─ Per agent:
  │   ├─ Parse agent.yaml → AgentConfig
  │   ├─ Codegen: system-prompt.md, <name>.md + <name>-bootstrap.md (@ file references), TOOLS.md, settings.json, .claude.json, .mcp.json, bootstrap-schema.json, reply-schema.json
  │   ├─ Validate policy.yaml exists (for openshell agents)
  │   └─ Install built-in skills (rightskills, cronsync)
  ├─ Generate process-compose.yaml (minijinja)
  └─ Launch process-compose (TUI or detached)

rightclaw bot --agent <name>  (spawned by process-compose)
  ├─ Resolve token, open memory.db
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
  ├─ .claude/skills/{rightskills,cronsync}/ — builtin skills only
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
OAuth callback → write Bearer to agent_dir/.mcp.json → send RefreshEntry
  → refresh scheduler sets timer (expires_at - 10 min)
  → on timer: POST refresh_token to token_endpoint
  → update Bearer in .mcp.json → upload to sandbox
```

`/mcp add` and `/mcp remove` also trigger immediate upload of `.mcp.json` to sandbox.

### Prompting Architecture

Every `claude -p` invocation gets a **composite system prompt** assembled from identity files.
No `--agent` flag — `--system-prompt-file` is the sole prompt mechanism.

In sandbox mode, the composite is assembled **inside the sandbox** (single SSH command,
`cat` + `claude -p`). In no-sandbox mode, assembled on host from local files.

Prompt caching is critical — avoid per-message tool calls to read identity files.

See PROMPT_SYSTEM.md for full documentation.

### Configuration Hierarchy

| Scope | File | Source of Truth |
|-------|------|-----------------|
| Global | `~/.rightclaw/config.yaml` | Tunnel config |
| Per-agent | `agents/<name>/agent.yaml` | Restart, model, telegram, sandbox overrides, env |
| Generated | `agents/<name>/.claude/settings.json` | CC behavioral flags (regenerated on `up`) |
| Generated | `agents/<name>/.claude.json` | Trust, onboarding suppression (read-modify-write) |
| Generated | `agents/<name>/.mcp.json` | MCP server entries (right + external) |
| Generated | `agents/<name>/TOOLS.md` | Tool reference for agent (regenerated on `up`) |
| Per-agent | `agents/<name>/policy.yaml` | OpenShell sandbox policy (generated by agent init) |

### Memory Schema (SQLite)

```
memories        (id, content, tags, stored_by, source_tool, created_at, deleted_at, importance)
memory_events   (memory_id, event_type, actor, timestamp)
memories_fts    (FTS5 virtual table — BM25 ranking)
telegram_sessions (chat_id, effective_thread_id, session_uuid, created_at)
cron_runs       (job_name, run_id, started_at, completed_at, exit_code, log_path)
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
| MCP | stdio + optional HTTP | right built-in, external via OAuth |

## Security Model

- **Sandbox isolation**: OpenShell (k3s containers) — filesystem + network + TLS policies per agent
- **TLS MITM**: OpenShell proxy terminates and re-signs TLS with per-sandbox CA for L7 inspection
- **Credential isolation**: Host credentials never uploaded to sandbox. Each sandbox authenticates independently via OAuth login flow.
- **Network policy**: Wildcard domain allowlists (*.anthropic.com, *.claude.com, *.claude.ai) + `tls: terminate` + `binaries: "**"`
- **`--dangerously-skip-permissions`**: Always on for all CC invocations. OpenShell policy is the security layer, not CC's permission system.
- **Prompt injection detection**: Pattern matching in memory guard before SQLite insert
- **Chat ID allowlist**: Empty = block all (secure default); per-agent in agent.yaml
- **Protected MCP**: "right" cannot be removed via `/mcp remove`
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
│   ├── TOOLS.md                       # generated by rightclaw up
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
│       ├── bootstrap-schema.json      # generated per agent by rightclaw up
│       ├── agents/<name>.md           # main agent def (@ file references)
│       ├── agents/<name>-bootstrap.md # bootstrap agent def (@ file references)
│       └── skills/{rightskills,cronsync}/
├── run/
│   ├── process-compose.yaml
│   ├── ssh/<agent>.ssh-config
│   └── runtime-state.json
├── logs/
│   └── <agent>.log.<date>   # Per-agent daily log rotation
└── scripts/
    └── cloudflared-start.sh
```

## Logging

Bot processes write to both stderr (process-compose TUI) and `~/.rightclaw/logs/<agent>.log` (daily rotation via `tracing-appender`). Login flow has step-by-step INFO-level logging for debuggability.
