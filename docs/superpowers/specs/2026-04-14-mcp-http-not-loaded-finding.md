# Finding: CC does not load HTTP MCP servers from --mcp-config

## Date
2026-04-14

## Problem
Claude Code (`claude -p`) does not load MCP tools from HTTP MCP servers
specified via `--mcp-config` or `.mcp.json`. The aggregator at
`http://host.docker.internal:8100/mcp` is reachable (verified via curl
through OpenShell proxy), but CC never registers its tools.

## Evidence

1. `--mcp-config /sandbox/mcp.json --strict-mcp-config` with valid
   `{"mcpServers":{"right":{"type":"http","url":"http://...","headers":{...}}}}`
   → CC starts, but `mcp__right__*` tools are absent. ToolSearch returns
   `total_deferred_tools: 7` (only CC built-in deferred tools).

2. Same config as inline JSON string via `--mcp-config '{...}'` → same result.

3. `.mcp.json` in project root (without `--strict-mcp-config`) → CC loads
   cloud MCP servers (Blockscout, Crypto.com — all HTTPS) but NOT our HTTP server.

4. `curl` from sandbox through OpenShell proxy reaches aggregator fine
   (401 without auth, 405 with auth — expected for GET on MCP endpoint).

## Likely cause

CC refuses to connect to plain `http://` MCP servers. All working MCP
servers in tests use `https://`. The sandbox sets `HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY` to `http://10.200.0.1:3128` — CC routes all traffic through
OpenShell proxy. The proxy passes HTTP through (verified), but CC itself
may reject non-TLS MCP connections.

## Sandbox proxy environment
```
HTTP_PROXY=http://10.200.0.1:3128
HTTPS_PROXY=http://10.200.0.1:3128
ALL_PROXY=http://10.200.0.1:3128
no_proxy=127.0.0.1,localhost,::1
```

`host.docker.internal` is NOT in `no_proxy`.

## Options to investigate

1. **Add HTTPS to aggregator** — self-signed cert, CC trusts sandbox CA
   (`/etc/openshell-tls/ca-bundle.pem`). Most aligned with sandbox security model.

2. **Add `host.docker.internal` to `no_proxy`** — bypasses proxy for
   aggregator traffic. May not help if CC rejects HTTP regardless of proxy.

3. **CC env var or config to allow HTTP MCP** — check if CC has a flag
   to permit plain HTTP MCP connections (e.g. for local development).

4. **Tunnel aggregator through cloudflared** — gives HTTPS endpoint but
   adds latency and complexity.

## What works now

- Bot Telegram commands (`/mcp add`, `/mcp auth`, `/mcp list`) work correctly
- Aggregator connects to upstream MCP servers (Composio OAuth flow works)
- Token refresh works
- The only broken link: CC inside sandbox → aggregator HTTP MCP connection
