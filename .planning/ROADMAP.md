# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2026-03-27)
- ✅ **v2.4 Sandbox Telegram Fix** - Phase 20 (shipped 2026-03-28)
- ✅ **v2.5 RightCron Reliability** - Phase 21 (shipped 2026-03-31)
- ✅ **v3.0 Teloxide Bot Runtime** - Phases 22-28.2 (shipped 2026-04-01)
- ✅ **v3.1 Sandbox Fix & Verification** - Phases 29-31 (shipped 2026-04-03)
- ✅ **v3.2 MCP & Tunnel** - Phases 38-41 (shipped 2026-04-05)
- 🚧 **v3.3 MCP Self-Management** - Phase 1 (in progress)

## Phases

<details>
<summary>✅ v1.0 Core Runtime (Phases 1-4) - SHIPPED 2026-03-23</summary>

See [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.0 Native Sandbox (Phases 5-7) - SHIPPED 2026-03-24</summary>

See [milestones/v2.0-ROADMAP.md](milestones/v2.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.1 Headless Agent Isolation (Phases 8-10) - SHIPPED 2026-03-25</summary>

See [milestones/v2.1-ROADMAP.md](milestones/v2.1-ROADMAP.md)

</details>

<details>
<summary>✅ v2.2 Skills Registry (Phases 11-15) - SHIPPED 2026-03-26</summary>

See [milestones/v2.2-ROADMAP.md](milestones/v2.2-ROADMAP.md)

</details>

<details>
<summary>✅ v2.3 Memory System (Phases 16-19) — SHIPPED 2026-03-27</summary>

See [milestones/v2.3-ROADMAP.md](milestones/v2.3-ROADMAP.md)

</details>

<details>
<summary>✅ v2.4 Sandbox Telegram Fix (Phase 20) — SHIPPED 2026-03-28</summary>

See [milestones/v2.4-ROADMAP.md](milestones/v2.4-ROADMAP.md)

</details>

<details>
<summary>✅ v2.5 RightCron Reliability (Phase 21) — SHIPPED 2026-03-31</summary>

See [milestones/v2.5-ROADMAP.md](milestones/v2.5-ROADMAP.md)

</details>

<details>
<summary>✅ v3.0 Teloxide Bot Runtime (Phases 22-28.2) — SHIPPED 2026-04-01</summary>

See [milestones/v3.0-ROADMAP.md](milestones/v3.0-ROADMAP.md)

</details>

<details>
<summary>✅ v3.1 Sandbox Fix & Verification (Phases 29-31) — SHIPPED 2026-04-03</summary>

See [milestones/v3.1-ROADMAP.md](milestones/v3.1-ROADMAP.md)

</details>

<details>
<summary>✅ v3.2 MCP & Tunnel (Phases 38-41) — SHIPPED 2026-04-05</summary>

See [milestones/v3.2-ROADMAP.md](milestones/v3.2-ROADMAP.md)

</details>

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1-4. Core Runtime | v1.0 | ✓ | Complete | 2026-03-23 |
| 5-7. Native Sandbox | v2.0 | ✓ | Complete | 2026-03-24 |
| 8-10. Headless Agent Isolation | v2.1 | ✓ | Complete | 2026-03-25 |
| 11-15. Skills Registry | v2.2 | ✓ | Complete | 2026-03-26 |
| 16-19. Memory System | v2.3 | ✓ | Complete | 2026-03-27 |
| 20. Sandbox Telegram Fix | v2.4 | ✓ | Complete | 2026-03-28 |
| 21. RightCron Reliability | v2.5 | ✓ | Complete | 2026-03-31 |
| 22-28.2. Teloxide Bot Runtime | v3.0 | ✓ | Complete | 2026-04-01 |
| 29-31. Sandbox Fix & Verification | v3.1 | ✓ | Complete | 2026-04-03 |
| 38-41. MCP & Tunnel | v3.2 | ✓ | Complete | 2026-04-05 |
| 1. MCP management tools | v3.3 | 0/2 | Not started | - |

### 🚧 v3.3 MCP Self-Management (In Progress)

### Phase 1: MCP management tools in rightmemory server

**Goal:** Add mcp_add, mcp_remove, mcp_list, mcp_auth tools to the rightmemory MCP server so agents can self-manage their MCP connections.
**Requirements**: MCP-TOOL-01, MCP-TOOL-02, MCP-TOOL-03, MCP-TOOL-04, MCP-TOOL-05, MCP-NF-01, MCP-NF-02
**Depends on:** —
**Plans:** 2 plans

Plans:
- [x] 01-01-PLAN.md — Extend MemoryServer struct with rightclaw_home/agent_dir fields, inject RC_RIGHTCLAW_HOME into mcp_config, extract tests to separate file
- [x] 01-02-PLAN.md — Add mcp_add, mcp_remove, mcp_list, mcp_auth tools to MemoryServer with full tests
