# Right HTTP MCP: Host-Side Server + Agent Secrets + Rename

**Date:** 2026-04-07
**Status:** Approved

## Problem

Three issues with the current rightmemory MCP setup:

1. **MCP tools unavailable in sandbox.** `mcp.json` has a stdio entry with `"command": "/host/path/to/rightclaw"` — a host-side binary path. CC inside the OpenShell sandbox can't launch it. The rightmemory MCP server never starts, so its tools (`mcp_add`, `mcp_remove`, `store`, `recall`, etc.) are never available to the agent.

2. **Inconsistent transport.** `generate_mcp_config()` (stdio) is used during `rightclaw up`, but `generate_mcp_config_http()` and `run_memory_server_http()` exist as dead code — never wired into bot startup or process-compose.

3. **Naming.** "rightmemory" is verbose. Should be "right" — it's the rightclaw MCP server, not just memory.

## Solution

### Change 1: Per-agent secret in `agent.yaml`

`rightclaw init` generates a persistent 32-byte random secret per agent:

```yaml
# agent.yaml
secret: "base64url-random-32-bytes-here"
restart: on_failure
```

- Stored as base64url string (no padding), 43 characters.
- Existing agents without `secret` get one auto-generated on next `rightclaw up`.
- Bearer token for the HTTP MCP server is derived deterministically: `HMAC-SHA256(secret, "right-mcp")` → base64url.
- Future services derive different tokens from the same secret using different labels.

### Change 2: Shared HTTP MCP server in process-compose

One `rightclaw memory-server-http` process serves all agents, managed by process-compose.

**Startup flow:**
1. `rightclaw up` discovers agents, reads/generates secrets.
2. Derives Bearer tokens: `HMAC-SHA256(agent.secret, "right-mcp")` per agent.
3. Writes token map to `~/.rightclaw/run/agent-tokens.json`:
   ```json
   {"right": "derived-token-aaa", "brain": "derived-token-bbb"}
   ```
4. Adds `right-mcp-server` process to process-compose.yaml:
   ```yaml
   right-mcp-server:
     command: /path/to/rightclaw memory-server-http --port 8100 --token-map /home/user/.rightclaw/run/agent-tokens.json
     environment:
       - RC_RIGHTCLAW_HOME=/home/user/.rightclaw
     availability:
       restart: always
   ```
5. Bot processes depend on `right-mcp-server`.

**Token map file:** `~/.rightclaw/run/agent-tokens.json` — maps agent name → Bearer token. Read by `run_memory_server_http()` on startup. The `AgentTokenMap` middleware validates the Bearer token and injects `AgentInfo` into request extensions.

**Port:** 8100 (hardcoded default, can be overridden via `--port`).

**Bind address:** `0.0.0.0:8100` — required for sandbox access via `host.docker.internal`. Loopback (`127.0.0.1`) is always blocked from sandbox by OpenShell SSRF protection.

### Change 3: `mcp.json` generation — HTTP for both modes

`rightclaw up` calls `generate_mcp_config_http()` instead of `generate_mcp_config()` for all agents.

**Sandbox mode** (`mcp.json` uploaded to sandbox):
```json
{
  "mcpServers": {
    "right": {
      "type": "http",
      "url": "http://host.docker.internal:8100/mcp",
      "headers": {
        "Authorization": "Bearer <derived-token>"
      }
    }
  }
}
```

**Direct mode** (`--no-sandbox`, `mcp.json` in agent dir):
```json
{
  "mcpServers": {
    "right": {
      "type": "http",
      "url": "http://127.0.0.1:8100/mcp",
      "headers": {
        "Authorization": "Bearer <derived-token>"
      }
    }
  }
}
```

Both use the same Bearer token derived from the agent's secret.

### Change 4: Sandbox policy update

`generate_policy()` updates the `right` (formerly `rightmemory`) network policy entry:

```yaml
right:
  endpoints:
    - host: "host.docker.internal"
      port: 8100
      allowed_ips:
        - "172.16.0.0/12"
      protocol: rest
      access: full
  binaries:
    - path: "**"
```

Key changes from previous policy:
- `allowed_ips: ["172.16.0.0/12"]` — required to bypass OpenShell SSRF protection for private IPs.
- No `tls: terminate` — plain HTTP works, proxy forwards directly.
- Renamed from `rightmemory:` to `right:`.

### Change 5: Rename `rightmemory` → `right`

All occurrences across the codebase (~63):

| Location | Change |
|----------|--------|
| `crates/rightclaw/src/mcp/mod.rs` | `PROTECTED_MCP_SERVER = "right"` |
| `crates/rightclaw/src/codegen/mcp_config.rs` | Server key `"right"` in JSON |
| `crates/rightclaw/src/codegen/policy.rs` | Policy section name `right:` |
| `crates/rightclaw-cli/src/memory_server.rs` | Tool descriptions |
| `crates/rightclaw-cli/src/memory_server_http.rs` | Tool descriptions, log messages |
| `crates/bot/src/sync.rs` | Comment |
| All test files | Assertions, test data |

### Change 6: CLI subcommand for HTTP server

Add `Commands::MemoryServerHttp` to the CLI:

```
rightclaw memory-server-http --port 8100 --token-map /path/to/agent-tokens.json
```

Reads `RC_RIGHTCLAW_HOME` from env (or `--rightclaw-home` flag). Opens per-agent `memory.db` lazily on first request (existing behavior in `run_memory_server_http`).

## Token Derivation

```
agent.yaml: secret = random_bytes(32) |> base64url_no_pad
Bearer token = HMAC-SHA256(base64url_decode(secret), "right-mcp") |> base64url_no_pad
```

The same secret can derive tokens for future services:
- `HMAC-SHA256(secret, "right-mcp")` — MCP server auth
- `HMAC-SHA256(secret, "right-cron")` — future cron API auth
- etc.

## NixOS Prerequisite

On NixOS, the firewall must trust Docker bridge interfaces for `host.docker.internal` to work:

```nix
networking.firewall.trustedInterfaces = [ "docker0" "br-+" ];
```

Without this, OpenShell's k3s proxy cannot reach host services. See ARCHITECTURE.md for details.

## Out of Scope

- Removing `generate_mcp_config()` (stdio variant) — keep for potential future use
- Changes to `refresh.rs` or `oauth_callback.rs` — OAuth token refresh is orthogonal
- Changes to `login.rs` — orthogonal concern
- MCP config filename constant extraction (`mcp.json` literals) — deferred

## Expected Impact

- **rightmemory MCP tools available in sandbox** — `store`, `recall`, `search`, `forget`, `mcp_add`, `mcp_remove`, `mcp_list`, `mcp_auth`, `cron_list_runs`, `cron_show_run`
- **Consistent behavior** — same HTTP transport for sandbox and `--no-sandbox`
- **Stable auth** — tokens derived from persistent secret, survive restarts
- **Cleaner naming** — "right" instead of "rightmemory"

## Files Changed

| File | Change |
|------|--------|
| `crates/rightclaw/src/agent/types.rs` | Add `secret: Option<String>` to `AgentConfig` |
| `crates/rightclaw/src/codegen/mcp_config.rs` | Rename `rightmemory` → `right`, update `generate_mcp_config_http()` |
| `crates/rightclaw/src/codegen/policy.rs` | Rename section, add `allowed_ips` |
| `crates/rightclaw/src/codegen/process_compose.rs` | Add `right-mcp-server` process |
| `crates/rightclaw/src/mcp/mod.rs` | `PROTECTED_MCP_SERVER = "right"`, add token derivation fn |
| `crates/rightclaw-cli/src/main.rs` | Add `MemoryServerHttp` command, switch to `generate_mcp_config_http()`, generate token map |
| `crates/rightclaw-cli/src/memory_server_http.rs` | Accept `--token-map` file path, rename in descriptions |
| `crates/rightclaw-cli/src/memory_server.rs` | Rename in descriptions |
| `crates/bot/src/sync.rs` | Rename in comments |
| `crates/bot/src/telegram/worker.rs` | No change (already passes `--mcp-config`) |
| `crates/rightclaw/src/mcp/detect.rs` | Rename in tests |
| `crates/rightclaw/src/mcp/credentials.rs` | Rename in tests |
| All test files | Update assertions and test data |
