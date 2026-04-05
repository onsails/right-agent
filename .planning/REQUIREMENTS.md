# Requirements: RightClaw v3.3 MCP Self-Management

**Defined:** 2026-04-05
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own sandbox configuration and identity, orchestrated by a single CLI command.

## v3.3 Requirements

### MCP-TOOL — MCP Management via MCP Server Tools

- [ ] **MCP-TOOL-01**: Agent can call `mcp_add(name, url)` MCP tool to add an HTTP MCP server to its `.claude.json` with `type: http`
- [ ] **MCP-TOOL-02**: Agent can call `mcp_remove(name)` MCP tool to remove a server from `.claude.json` (rightmemory is protected and cannot be removed)
- [ ] **MCP-TOOL-03**: Agent can call `mcp_list()` MCP tool to see all configured MCP servers with source (`.claude.json` or `.mcp.json`) and auth state (present/missing)
- [ ] **MCP-TOOL-04**: Agent can call `mcp_auth(server_name)` MCP tool to initiate OAuth flow — returns auth URL; after user completes browser auth, Bearer token is written to `.claude.json` headers
- [ ] **MCP-TOOL-05**: All MCP management tools are exposed via the existing rightmemory MCP server (stdio transport, already launched by process-compose)

### MCP-NF — Non-Functional

- [ ] **MCP-NF-01**: MCP tools must not expose secrets (tokens, refresh tokens) in return values — only confirmation messages and status
- [ ] **MCP-NF-02**: `mcp_auth` works headless — returns URL for user to click, cloudflared tunnel callback handles token exchange

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| MCP-TOOL-01 | TBD | Pending |
| MCP-TOOL-02 | TBD | Pending |
| MCP-TOOL-03 | TBD | Pending |
| MCP-TOOL-04 | TBD | Pending |
| MCP-TOOL-05 | TBD | Pending |
| MCP-NF-01 | TBD | Pending |
| MCP-NF-02 | TBD | Pending |

**Coverage:**
- v3.3 requirements: 7 total (MCP-TOOL×5 + MCP-NF×2)
- Mapped to phases: 0
- Unmapped: 7

---
*Requirements defined: 2026-04-05*
