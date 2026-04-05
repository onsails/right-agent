# Phase 1: MCP management tools in rightmemory server - Discussion Log (Assumptions Mode)

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the analysis.

**Date:** 2026-04-05
**Phase:** 01-mcp-management-tools-in-rightmemory-server
**Mode:** assumptions
**Areas analyzed:** MCP Tool Infrastructure, Agent Directory Context, mcp_auth OAuth Scope, Secret Hygiene

## Assumptions Presented

### MCP Tool Infrastructure
| Assumption | Confidence | Evidence |
|------------|-----------|----------|
| Add 4 tools to existing MemoryServer via #[tool] macros in memory_server.rs | Confident | memory_server.rs #[tool_router] pattern; MCP-TOOL-05 requires existing server |

### Agent Directory Context
| Assumption | Confidence | Evidence |
|------------|-----------|----------|
| agent_dir from $HOME, .claude.json path = $HOME/.claude.json, project key via canonicalize() | Likely | memory_server.rs:301-305 HOME env; handler.rs:483-487 canonicalize pattern |

### mcp_auth OAuth Scope
| Assumption | Confidence | Evidence |
|------------|-----------|----------|
| mcp_auth only returns auth URL; callback lives in bot via cloudflared | Likely | handler.rs:280-460 two-phase flow; memory server has no HTTP listener |

### Secret Hygiene
| Assumption | Confidence | Evidence |
|------------|-----------|----------|
| mcp_list uses mcp_auth_status() from detect.rs — no token fields in output | Confident | detect.rs:7-64 ServerStatus/AuthState structure; handler.rs:239 existing pattern |

## Corrections Made

No corrections — all assumptions confirmed.
