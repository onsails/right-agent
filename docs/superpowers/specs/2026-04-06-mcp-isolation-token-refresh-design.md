# OpenShell Sandbox + HTTP MCP + OAuth Token Refresh

**Date:** 2026-04-06
**Status:** Draft
**Scope:** Security overhaul — replace CC native sandbox with OpenShell, convert rightmemory to HTTP MCP server

## Problem

### 1. Sandbox isolation is broken

CC native sandbox (`settings.json`) has fundamental flaws:

- **Symlink escape:** `.credentials.json` symlink lets CC resolve the real `~/.claude/` and read host's `.claude.json` with OAuth tokens for other services
- **Write/Edit bypass:** CC's Write/Edit tools run in-process (`fs.writeFileSync`), not through bwrap. `allowWrite` restrictions don't apply ([#29048](https://github.com/anthropics/claude-code/issues/29048))
- **Self-modification:** Agent can rewrite `.claude/settings.json` to disable its own sandbox. `denyWrite` blocks Write/Edit tools but `allowRead`/`denyRead` semantics make it impossible to protect files inside an `allowRead` region — allow takes precedence over deny for reads
- **Plugins symlink:** `plugins/` symlink provides another path back to host. Unnecessary — rightclaw is the Telegram bot, not CC's plugin system

### 2. MCP OAuth tokens expire silently

Token refresh was deleted in v3.2 under the assumption CC handles it. But agents run via `claude -p` (one-shot) — no long-running process to refresh. Tokens expire after `expires_in` (typically 3600s) and MCP servers silently stop working.

### 3. Rightmemory stdio MCP is incompatible with container sandbox

Rightmemory runs as a stdio subprocess of CC. Inside an OpenShell container, the rightclaw binary isn't available. Uploading it is fragile and wasteful.

## Solution

### 1. OpenShell sandbox for all agents

Each agent runs inside an OpenShell sandbox — a Docker container with Landlock filesystem enforcement, seccomp syscall filtering, network namespace isolation, and HTTP CONNECT proxy.

**Why OpenShell over CC native sandbox:**
- Kernel-level enforcement (Landlock) — CC tools can't bypass it
- Network isolation — default-deny, no host filesystem access
- Container filesystem — agent can't see or modify host files
- Policy is immutable from inside the sandbox

**Previous blocker (resolved):** OpenShell sandbox couldn't do CC OAuth because `.claude.json`/`.credentials.json` lived on host. Fix: `openshell upload` copies these files into the container. CC sees them as local files, OAuth works, no symlink back to host.

#### 1.1 Sandbox lifecycle

```
rightclaw up:
  for each agent:
    1. Generate agent files on host (staging area)
    2. openshell sandbox create --policy policy.yaml --name rightclaw-{agent} -- sleep infinity
    3. openshell upload: .claude.json, .credentials.json, IDENTITY.md, SOUL.md, etc.
    4. openshell upload: .mcp.json (with rightmemory + external MCP Bearer tokens)
    5. openshell sandbox exec: claude -p (per telegram message, via bot)

rightclaw down:
  for each agent:
    openshell sandbox delete rightclaw-{agent}
```

#### 1.2 Policy.yaml

```yaml
version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
  read_write:
    - /tmp
    - /sandbox        # agent working directory

landlock:
  compatibility: hard_requirement

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
  anthropic_api:
    endpoints:
      - host: "api.anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "statsig.anthropic.com"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"

  rightmemory:
    endpoints:
      - host: "host.docker.internal"
        port: ${RIGHTMEMORY_PORT}
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"

  # Per-agent MCP server policies added dynamically
  # e.g. notion, linear — only their specific domains
```

#### 1.3 CC invocation inside sandbox

```bash
openshell sandbox exec rightclaw-{agent} -- \
  claude -p \
    --dangerously-skip-permissions \
    --output-format json \
    --session-id {uuid} \
    -- "{user_message}"
```

No `--strict-mcp-config` needed — sandbox is fully isolated, CC reads `.mcp.json` from `/sandbox/` inside container. No host `.claude.json` to leak from.

No `settings.json` sandbox config needed — OpenShell is the enforcement layer.

### 2. Rightmemory as HTTP MCP server

Convert rightmemory from stdio to HTTP MCP server running on the host.

#### 2.1 Architecture

```
Host:
  rightmemory HTTP server (localhost:PORT)
    ├── /mcp  (MCP protocol endpoint)
    └── per-agent auth via Bearer token

Sandbox (agent "brain"):
  .mcp.json:
    rightmemory:
      type: http
      url: http://host.docker.internal:PORT/mcp
      headers:
        Authorization: Bearer <agent-token>
```

#### 2.2 Per-agent authentication

- `rightclaw up` generates a random secret per agent (`rand::thread_rng`, 32 bytes, base64url)
- Secret = Bearer token (no derivation needed — each token is unique and random)
- Token → agent name mapping stored in-memory in rightmemory server
- Token written to agent's `.mcp.json` as `Authorization: Bearer <token>`
- `.mcp.json` uploaded into sandbox — agent can read it but can't escape container

**Security property:** Agent can only access its own memory DB. Even if it reads its Bearer token from `.mcp.json`, it can't forge another agent's token.

#### 2.3 Rightmemory server process

Launched by process-compose alongside bot processes. Single process, all agents.

```yaml
# in process-compose.yaml
rightmemory:
  command: rightclaw memory-server --http --port ${PORT} --agents-dir ~/.rightclaw/agents
  availability:
    restart: on_failure
```

### 3. MCP config — all in `.mcp.json`

All MCP servers (rightmemory + external HTTP servers with OAuth tokens) are written to a staging `.mcp.json` on the host, then uploaded into the sandbox.

```json
{
  "mcpServers": {
    "rightmemory": {
      "type": "http",
      "url": "http://host.docker.internal:8100/mcp",
      "headers": {
        "Authorization": "Bearer <agent-secret>"
      }
    },
    "notion": {
      "type": "http",
      "url": "https://mcp.notion.com/mcp",
      "headers": {
        "Authorization": "Bearer <oauth-access-token>"
      }
    }
  }
}
```

#### 3.1 Staging area on host

```
~/.rightclaw/agents/<name>/
├── staging/                       # files to upload into sandbox
│   ├── .claude.json               # trust + onboarding (generated)
│   ├── .claude/.credentials.json  # copied from host (Anthropic OAuth)
│   ├── .claude/settings.json      # minimal: skipDangerousModePermissionPrompt only
│   ├── .mcp.json                  # all MCP servers + Bearer tokens
│   ├── IDENTITY.md
│   ├── SOUL.md
│   ├── AGENTS.md
│   └── agent.yaml
├── oauth-state.json               # refresh tokens (never uploaded)
├── agent-secret.key               # rightmemory Bearer token (never uploaded)
└── oauth-callback.sock
```

`oauth-state.json` and `agent-secret.key` stay on host — never enter the sandbox.

### 4. Migrate credential functions to `.mcp.json`

**`crates/rightclaw/src/mcp/credentials.rs`:**

Rewrite to flat `.mcp.json` structure. Remove `agent_path_key` parameter (no `projects` nesting).

| Current | New | Change |
|---|---|---|
| `add_http_server_to_claude_json()` | `add_http_server()` | Flat `mcpServers` in `.mcp.json` |
| `remove_http_server_from_claude_json()` | `remove_http_server()` | Same |
| `list_http_servers_from_claude_json()` | `list_http_servers()` | Same |
| `set_server_header()` | `set_server_header()` | Operates on `.mcp.json` |

**`crates/bot/src/telegram/oauth_callback.rs`:**

- `OAuthCallbackState.claude_json_path` → `OAuthCallbackState.mcp_json_path`
- `OAuthCallbackState.agent_path_key` removed
- After writing Bearer token, persist OAuth state + re-upload `.mcp.json` into sandbox

### 5. OAuth token refresh — background tokio task

#### 5.1 Persistent OAuth state

After successful token exchange, persist to `~/.rightclaw/agents/<name>/oauth-state.json`:

```json
{
  "servers": {
    "notion": {
      "refresh_token": "<refresh_token>",
      "token_endpoint": "https://accounts.notion.com/oauth/token",
      "client_id": "...",
      "client_secret": null,
      "expires_at": "2026-04-06T21:00:00Z",
      "server_url": "https://mcp.notion.com/mcp"
    }
  }
}
```

This file **never enters the sandbox** — refresh tokens stay on host only.

#### 5.2 Refresh scheduler

Tokio task spawned alongside bot's dispatch loop:

1. On startup: load `oauth-state.json`, schedule refresh for each server
2. Refresh fires at `expires_at - 10 minutes`
3. Refresh flow:
   - POST to `token_endpoint` with `grant_type=refresh_token`
   - Success: update Bearer in staging `.mcp.json`, re-upload into sandbox, update `oauth-state.json`
   - Failure: retry 3x with exponential backoff (30s, 60s, 120s), then Telegram notification
4. If `refresh_token` is null: skip scheduling, log warning

#### 5.3 Integration with OAuth callback

`complete_oauth_flow()` persists to `oauth-state.json` and sends `RefreshEntry` via `tokio::sync::mpsc` channel to refresh scheduler.

### 6. Delete legacy code

| Code | Action |
|---|---|
| `create_plugins_symlink()` | Delete entirely |
| `create_credential_symlink()` | Replace with file copy to staging dir |
| `generate_settings()` (sandbox settings) | Simplify — only `skipDangerousModePermissionPrompt` + `autoMemoryEnabled: false` |
| `settings_tests.rs` | Rewrite for minimal settings |
| `telegram_token_file` in agent.yaml | Remove — token passed via `RC_TELEGRAM_TOKEN` env var only |
| `.claude/channels/telegram/.env` | Delete — leftover from CC Telegram plugin era |

## File changes summary

| File | Action |
|---|---|
| `crates/rightclaw/src/mcp/credentials.rs` | Rewrite: `.claude.json` → `.mcp.json`, flat structure |
| `crates/rightclaw/src/mcp/refresh.rs` | **New:** OAuth state persistence + refresh scheduler |
| `crates/rightclaw/src/mcp/mod.rs` | Add `pub mod refresh;` |
| `crates/rightclaw/src/codegen/claude_json.rs` | Delete `create_plugins_symlink()`, change `create_credential_symlink()` to copy |
| `crates/rightclaw/src/codegen/settings.rs` | Simplify: only `skipDangerousModePermissionPrompt` + `autoMemoryEnabled` |
| `crates/rightclaw/src/codegen/settings_tests.rs` | Rewrite for minimal settings |
| `crates/rightclaw/src/codegen/sandbox.rs` | **New:** OpenShell sandbox create/delete/upload/exec |
| `crates/rightclaw/src/codegen/policy.rs` | **New:** Generate policy.yaml from agent config |
| `crates/rightclaw-cli/src/main.rs` | Rewrite `cmd_up()`/`cmd_down()` for OpenShell lifecycle |
| `crates/rightclaw-cli/src/memory_server.rs` | Rewrite: stdio → HTTP (axum), add Bearer auth + agent routing |
| `crates/bot/src/telegram/oauth_callback.rs` | Use `.mcp.json`, persist OAuth state, re-upload to sandbox |
| `crates/bot/src/telegram/worker.rs` | `invoke_cc()` → `openshell sandbox exec` instead of direct `claude -p` |
| `crates/bot/src/telegram/dispatch.rs` | Spawn refresh scheduler task |
| `templates/process-compose.yaml.j2` | Add rightmemory HTTP server process, remove `RC_TELEGRAM_TOKEN_FILE` |
| `templates/right/policy.yaml` | **New:** Default OpenShell policy |

## Testing

- **credentials.rs:** Rewrite tests for flat `.mcp.json` structure
- **refresh.rs:** OAuth state serde, refresh timing, retry logic, mpsc integration
- **sandbox.rs:** OpenShell create/upload/exec/delete (integration tests with real OpenShell)
- **policy.rs:** Policy generation from agent config
- **memory_server.rs:** HTTP MCP endpoint, Bearer auth, agent isolation (agent A can't access agent B's data)
- **worker.rs:** Verify `openshell sandbox exec` command construction
- **oauth_callback.rs:** OAuth state persisted, `.mcp.json` re-uploaded after token exchange

## Risks

- **OpenShell stability:** Alpha software. Mitigated: we used it before (phases 2-3), architecture is proven, subscription issue resolved.
- **Docker dependency:** OpenShell runs inside Docker. Agents need Docker daemon running. Mitigated: doctor check for Docker + OpenShell at startup.
- **`host.docker.internal` availability:** Not available on all Docker setups (Linux needs `--add-host`). Mitigated: detect platform, fall back to host IP.
- **Upload latency:** `openshell upload` before each `claude -p` adds overhead for `.mcp.json` re-upload after token refresh. Mitigated: upload is tar-over-SSH, small files (<1KB), negligible.
- **Providers without `refresh_token`:** Some OAuth providers don't return refresh tokens. Scheduler skips these with a warning. Manual re-auth required on expiry.

## Migration from v3.x

- `rightclaw init` regenerates agent layout with staging dir
- `rightclaw doctor` checks for OpenShell + Docker instead of bubblewrap
- First `rightclaw up` after upgrade creates OpenShell sandboxes, previous CC sandbox settings are ignored
- `.mcp.json` format unchanged (already standard), content migrated from `.claude.json`
