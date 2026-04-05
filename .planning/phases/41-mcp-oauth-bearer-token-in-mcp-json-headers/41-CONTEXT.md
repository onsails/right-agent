---
phase: 41
name: MCP OAuth Bearer token in .mcp.json headers
created: 2026-04-05
---

# Phase 41 Context

## Problem

After successful OAuth token exchange via `/mcp auth`, the bot writes credentials to `~/.claude/.credentials.json` using a proprietary key derivation (`mcp_oauth_key()` in `credentials.rs`). However, CC uses a different key derivation to look up credentials, so it can never find the tokens we wrote. Result: `/mcp list` shows `✅ present` but CC agents can't actually use the MCP server.

## Root Cause

`mcp_oauth_key()` hashes a JSON object `{"type":"sse","url":"...","headers":{}}` but CC uses a different input format. Reverse-engineering CC's key derivation is fragile and will break on CC updates.

## Solution

Stop writing to `.credentials.json`. Instead, write the OAuth Bearer token directly into `.mcp.json` as an `Authorization` header:

```json
{
  "mcpServers": {
    "notion": {
      "url": "https://mcp.notion.com/mcp",
      "headers": {
        "Authorization": "Bearer <access_token>"
      }
    }
  }
}
```

CC passes headers to MCP servers as-is — no credential lookup needed. Standard OAuth, no proprietary key derivation.

## Scope

### Must Change
1. **`oauth_callback.rs`** — After token exchange, write Bearer token to `.mcp.json` headers instead of `.credentials.json`
2. **`refresh.rs`** — Token refresh updates `.mcp.json` headers instead of `.credentials.json`
3. **`detect.rs`** — `mcp_auth_status()` checks for `Authorization` header in `.mcp.json` instead of looking up `.credentials.json`
4. **`handler.rs`** — `/mcp auth` flow writes token to `.mcp.json` headers on success
5. **`credentials.rs`** — Remove `mcp_oauth_key()`, `write_credential()`, `read_credential()` (no longer needed)
6. **`lib.rs` (bot)** — Startup MCP auth check reads `.mcp.json` headers instead of credentials file

### Must NOT Change
- OAuth discovery (RFC 9728/8414) — works correctly
- PKCE generation — works correctly
- Token exchange — works correctly
- `.mcp.json` structure for stdio servers (command/args) — unchanged
- `/mcp add` / `/mcp remove` — unchanged

### Also Remove (unnecessary after this change)
- Agent restart after OAuth callback (CC uses `claude -p` per message, no persistent session to restart)
- `mcp_oauth_key()` and credential file rotation logic

## Key Files
- `crates/rightclaw/src/mcp/credentials.rs` — gut most of it
- `crates/rightclaw/src/mcp/detect.rs` — change from credentials lookup to header check
- `crates/rightclaw/src/mcp/refresh.rs` — change write target from credentials to .mcp.json
- `crates/bot/src/telegram/oauth_callback.rs` — change write target
- `crates/bot/src/telegram/handler.rs` — /mcp auth flow
- `crates/bot/src/lib.rs` — startup auth check

## Constraints
- `.mcp.json` is also read by `generate_mcp_config()` in codegen — must not break that
- Token refresh must be atomic (don't corrupt .mcp.json mid-write)
- Keep the refresh scheduler running — tokens expire
