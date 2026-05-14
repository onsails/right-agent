# MCP Aggregator and token refresh

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## MCP Token Refresh

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

## MCP Aggregator

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
  - POST /progress/register — register a foreground invocation that may send progress
  - POST /progress/unregister — remove that invocation when the foreground turn ends

Telegram bot uses InternalClient (hyper UDS) to call these endpoints.
Agents cannot reach the Unix socket from inside the sandbox.

## Foreground Progress Tool

`mcp__right__send_progress` is a built-in RightBackend tool routed through the
aggregator. It is available only when the current MCP request carries
`X-Right-Invocation`, which the worker injects by writing a per-invocation MCP
config and uploading it into the sandbox before starting Claude Code.

The bot registers the invocation with the aggregator over
`/progress/register`. The registration stores the bot UDS path and a separate
send token. On tool call, the aggregator validates that the invocation is
active, foreground-only, and not within the 30-second per-invocation rate
limit, then calls the bot's `POST /progress/send` endpoint. Telegram send
failures surface as tool-level `progress_send_failed` errors.

Cron, delivery, reflection, and background-continuation calls pass
`mcp__right__send_progress` via `--disallowedTools`; they have no live
foreground invocation and must use their structured output delivery path.
