---
name: rightmcp
description: >-
  Finds and adds MCP servers for this RightClaw agent. Searches for OAuth-capable
  endpoints first (Claude Code / Codex integration docs), falls back to API-key
  endpoints. All management goes through the user's Telegram commands — the agent
  never handles credentials directly. Use when the user asks to add, connect,
  or set up an MCP server or integration.
version: 0.1.0
---

@known-endpoints.yaml

# /rightmcp -- Add MCP Server

## When to Activate

Activate this skill when:
- The user asks to add, connect, or set up an MCP server or integration
- The user asks about connecting a third-party service via MCP
- The user names a specific service and wants it added (e.g. "add Composio", "connect Linear")

## Architecture

You have NO direct MCP management access. All management goes through the user's
Telegram commands. Here's what happens behind the scenes:

- The **RightClaw MCP Aggregator** proxies all MCP traffic, stores tokens, and
  refreshes OAuth automatically. You never see or handle credentials.
- `/mcp add <name> <url>` auto-detects authentication:
  1. Tries OAuth AS discovery on the URL — if found, registers and tells user to `/mcp auth`
  2. Detects query-string auth (key embedded in URL) — registers as-is
  3. For other URLs — detects auth type, asks user for token in Telegram if needed
- `/mcp auth <name>` — starts browser-based OAuth flow
- `/mcp remove <name>` — unregisters a server (`right` is protected)
- `/mcp list` — shows all servers with status

## Procedure

### Step 1: Check current servers

Call `mcp__right__mcp_list()` to see what's already registered. If the requested service is
already connected, tell the user and stop.

### Step 2: Check known endpoints

Check `known-endpoints.yaml` for the requested service. If a match exists,
skip web search entirely and go straight to Step 3 with the URL from the file.

### Step 3: Search for OAuth endpoint FIRST

If the service is NOT in known endpoints, search the web.
Your first search query MUST target Claude Code or Codex integration docs.
These describe OAuth-capable MCP endpoints that work with `/mcp auth`.

Use these search queries (in order, stop when you find a URL):
1. `"<service> MCP server Claude Code"`
2. `"<service> MCP Claude Desktop config"`
3. `"<service> MCP endpoint OAuth"`

Look for streamable HTTP or SSE URLs like:
- `https://mcp.service.dev/sse`
- `https://mcp.service.dev/mcp`
- `https://service.com/v1/mcp`

**DO NOT** use URLs from your training data. Only use URLs found in search results.

### Step 4: If OAuth URL found

Give the user the Telegram command:
```
/mcp add <name> <url>
```
The bot detects OAuth automatically and will prompt for `/mcp auth <name>`.

### Step 5: If no OAuth endpoint found

Search more broadly for any MCP endpoint:
1. `"<service> MCP server URL"`
2. Check the service's official docs for MCP/API integration pages

If you find an API-key URL (token in query string or requires header), give:
```
/mcp add <name> <url>
```
The bot will determine the auth method and ask the user for credentials if needed.

### Step 6: If no MCP endpoint found

Tell the user the service may not have MCP support yet. Suggest:
- Checking the service's docs or integrations page directly
- Looking for community MCP servers on GitHub

## Constraints

- **NEVER** ask the user for API keys or tokens — the bot handles credential collection
- **NEVER** guess or fabricate URLs from training data — only use URLs from known-endpoints.yaml or search results
- **NEVER** attempt to call internal MCP management APIs — they don't exist as agent tools
- **ALWAYS** check known-endpoints.yaml first, then search the web if no match — do not rely on prior knowledge of MCP endpoints
- **ALWAYS** call `mcp__right__mcp_list()` first to check existing servers
