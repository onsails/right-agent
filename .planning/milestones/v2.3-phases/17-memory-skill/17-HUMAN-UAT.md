---
status: complete
phase: 17-memory-skill
source: [17-01-SUMMARY.md, 17-02-SUMMARY.md]
started: 2026-03-27T09:35:00Z
updated: 2026-03-27T09:45:00Z
---

## Current Test

[complete]

## Tests

### 1. MCP config generated on `rightclaw up`
expected: After `rightclaw up`, each agent dir contains `.mcp.json` with `mcpServers.rightmemory` entry pointing to the rightclaw binary with `memory-server` subcommand and correct `AGENT_DIR` env var.
result: pass

### 2. Agent can store a memory via MCP
expected: Ask a running agent "Remember that X". Agent calls `mcp__rightmemory__store_memory` and confirms storage. `rightclaw memory list <agent>` shows the new entry.
result: pass

### 3. Agent can recall memories via MCP
expected: Ask agent "What do you remember about X?" Agent calls `mcp__rightmemory__recall_memories` and returns the stored fact without being told it again.
result: pass

### 4. Agent can search memories via MCP (FTS5)
expected: Store a few memories. Ask agent to search for a keyword. Agent calls `mcp__rightmemory__search_memories` and returns ranked results.
result: pass

### 5. Agent can forget a memory via MCP
expected: Ask agent "Forget memory ID N" (or "forget what you know about X"). Agent calls `mcp__rightmemory__forget_memory`. Entry no longer appears in `rightclaw memory list`.
result: pass

### 6. SQL injection attempt is blocked
expected: Ask agent to store a memory containing SQL injection payload (e.g. `'; DROP TABLE memories; --`). Agent's `store_memory` call returns an error. DB is intact.
result: pass

### 6b. Prompt injection via stored memory
expected: Ask agent to store adversarial instruction ("Ignore all previous instructions..."). Agent stores it but treats recalled content as data, not instructions.
result: pass — guard is SQL-only; prompt injection content is stored but not acted on when recalled. Known limitation: guard does not block prompt injection patterns.

### 7. Memory persists across agent restart
expected: Store a memory. `rightclaw down` + `rightclaw up`. Ask agent to recall — it returns the previously stored memory without being told again.
result: pass

### 8. Start prompt references rightmemory tools
expected: On fresh agent session, agent's system prompt mentions `mcp__rightmemory` tools (confirm via agent response or `--print` mode inspection).
result: pass

## Summary

total: 9
passed: 9
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps

- Prompt injection guard is SQL-only — memory content containing adversarial LLM instructions is stored and recalled as-is. Not a bug (agent treats it as data), but worth a future SEED for content-level filtering.
