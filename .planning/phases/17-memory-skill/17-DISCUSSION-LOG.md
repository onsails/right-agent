# Phase 17: Memory Skill - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-26
**Phase:** 17-memory-skill
**Areas discussed:** MCP vs SKILL.md, multi-agent isolation, MCP library, .mcp.json codegen, tool API shape, SEC-01 injection scanning

---

## MCP vs SKILL.md (discovered pre-discussion)

| Option | Description | Selected |
|--------|-------------|----------|
| SKILL.md (bash + sqlite3) | Slash commands via system prompt injection, sqlite3 CLI | |
| MCP server (stdio) | Structured tool calls, rusqlite directly | ✓ |

**User's choice:** MCP server
**Notes:** User initiated this change before gray area discussion. Reasoning: structured output, no sqlite3 binary dep in sandbox, no shell escaping risk. User immediately flagged multi-agent consideration — each agent spawns own server process via per-agent .mcp.json, uses $HOME/memory.db. Stdio eliminates port conflicts.

---

## MCP Library

| Option | Description | Selected |
|--------|-------------|----------|
| rmcp (official) | modelcontextprotocol/rust-sdk, handles protocol | ✓ |
| Manual JSON-RPC | ~100 lines serde_json, no dep | |
| Research first | Evaluate before deciding | |

**User's choice:** rmcp (official Rust SDK)
**Notes:** rmcp 1.3.0 confirmed via research. Critical finding: tracing_subscriber must use stderr writer or JSON-RPC stream is corrupted.

---

## .mcp.json Codegen

| Option | Description | Selected |
|--------|-------------|----------|
| Merge (read+inject+write) | Preserves existing MCP servers | ✓ |
| Generate only if absent | Skips agents with existing .mcp.json | |

**User's choice:** Merge
**Notes:** Follow generate_telegram_channel_config pattern.

---

## Tool API Shape

| Option | Description | Selected |
|--------|-------------|----------|
| store / recall / search / forget | Short verb-only | ✓ |
| remember / recall / search / forget | remember aligns with slash cmd | |
| memory_store / memory_recall / ... | Prefixed, unambiguous | |

**User's choice:** store / recall / search / forget

---

## SEC-01: Injection Scanning

| Option | Description | Selected |
|--------|-------------|----------|
| Reject on known patterns | Hardcoded list, str::contains | |
| No scanning | Trust the agent | |
| Research first | Find Rust libs + patterns | ✓ |

**User's choice:** Research first — also find ready Rust libraries
**Notes:** Research confirmed no viable Rust crates (only candidate: 27 downloads). Decision: str::contains() on ~15 multi-word phrases from OWASP LLM01:2025. Conservative list avoids false positives by using multi-word injection-specific phrases only. Single words explicitly excluded.

---

## Claude's Discretion

- SQL query implementation for recall (tag match) and search (FTS5) — standard rusqlite pattern
- Error message text for injection rejection
- schemars JsonSchema derive structs layout

## Deferred Ideas

- Slash commands (/remember etc.) as aliases — possible v2.4 addition on top of MCP tools
- SKILL-05 (install rightmemory SKILL.md) — obsolete, replaced by .mcp.json codegen
