# Phase 33: Auth Detection — Context

## Domain

Surface MCP OAuth auth state per agent via `rightclaw mcp status`, and emit a non-fatal warning
during `rightclaw up` when any agent has unauthenticated or expired MCP servers.

## Canonical Refs

- `crates/rightclaw/src/mcp/credentials.rs` — `read_credential`, `mcp_oauth_key`, `CredentialToken` (Phase 32)
- `crates/rightclaw/src/codegen/mcp_config.rs` — `.mcp.json` generation, server entry schema
- `crates/rightclaw-cli/src/main.rs` — `Commands` enum, `MemoryCommands` pattern to mirror for `McpCommands`
- `crates/rightclaw/src/agent/discovery.rs` — agent discovery, `AgentDef` structure
- `.planning/REQUIREMENTS.md` — DETECT-01, DETECT-02

## Decisions

### 1. Expiry states: 3 only — defer "expiring soon" to Phase 35

Auth state has exactly 3 values: `present` / `missing` / `expired`.

- `missing` — key absent from `~/.claude/.credentials.json`
- `expired` — key present, `expires_at > 0`, and `expires_at < now_unix`
- `present` — key present and not expired (`expires_at == 0` OR `expires_at >= now_unix`)

No "expiring soon" state in this phase. That threshold policy belongs in Phase 35 (token refresh).
Adding it here would partially implement refresh semantics without the refresh logic — defer cleanly.

### 2. Table layout: grouped by agent

`rightclaw mcp status` output groups servers under their agent:

```
right:
  notion    missing
  linear    present

scout:
  notion    expired
```

`--agent <name>` filters to a single agent (shows one group). No flat/columnar layout.
If an agent has no HTTP/SSE servers, skip it silently (nothing to show).

### 3. Warning in `rightclaw up`: specific enumeration

When any agent has missing or expired OAuth tokens, emit one Warn log line that names each
agent+server pair:

```
[WARN] MCP auth required: right/notion (missing), scout/notion (expired)
```

No generic "run mcp status" redirect. Operators launching agents need to know which agent/server
to fix without running a second command. The list is bounded (agents × servers is small).

### 4. Server filter: `url` field presence = OAuth candidate

A server entry in `.mcp.json` is an OAuth candidate if and only if it has a `url` field.
Servers without `url` (stdio transport — command + args only, e.g. `rightmemory`) are skipped
silently. No hardcoded name blocklist.

This is the correct semantic boundary: HTTP/SSE servers authenticate via OAuth; stdio servers
are local processes and never need it.

## Deferred Ideas

None surfaced during discussion.

## Implementation Notes for Researcher/Planner

- **Credentials path**: host user's `~/.claude/.credentials.json` — not agent-local. Each agent
  dir has a symlink `agent/.claude/.credentials.json → ~/.claude/.credentials.json`, but for
  `mcp status` and `up` warn, read the host credentials file directly (same file either way).
- **`McpCommands`**: Add `Mcp { subcommand: McpCommands }` to `Commands` enum, mirroring the
  `Memory` pattern. Start with just `Status { agent: Option<String> }`.
- **No new library deps expected** — detection is `read_credential` + timestamp comparison +
  table formatting (already doing similar in `cmd_memory_list`).
- **`rightclaw up` warn**: happens after agent discovery, before process-compose launch. Collect
  all (agent, server, state) tuples where state != present, then emit single Warn if non-empty.
