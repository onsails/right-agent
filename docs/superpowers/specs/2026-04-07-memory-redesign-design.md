# Memory System Redesign: CC Native Memory + Structured Records

**Date:** 2026-04-07
**Status:** Draft

## Problem

RightClaw agents have SQLite-backed memory tools (`store`, `recall`, `search`, `forget`) but:

1. **No memory in system prompt.** Agents wake up with zero context unless they actively call tools. IronClaw solves this by injecting a curated MEMORY.md into the system prompt every session.
2. **No behavioral guidance.** AGENTS.md has no instructions about when to store or recall. Tools exist but agents have no protocol for using them.
3. **Reinventing CC's memory.** Claude Code already has a battle-tested auto-memory system (`~/.claude/memory/MEMORY.md`) that auto-saves important context and injects it into every session. We disable it (`autoMemoryEnabled: false`) and replace it with our MCP tools — which are worse at the fuzzy "remember what matters" job.
4. **Confusing tool names.** `store`/`recall`/`search`/`forget` sound like general-purpose memory, creating ambiguity about when to use them vs. just relying on conversation context.

## Design

Split memory into two complementary systems with clear boundaries:

| System | Owns | Storage | Injection |
|--------|------|---------|-----------|
| **CC native memory** | Conversation continuity — preferences, decisions, context | `~/.claude/memory/MEMORY.md` | Auto-injected into system prompt every session |
| **right MCP records** | Structured data — tagged entries, cron results, audit trail | SQLite `memory.db` (FTS5/BM25) | On-demand via MCP tool calls |

### Change 1: Enable CC native memory

In `crates/rightclaw/src/codegen/settings.rs`, flip `autoMemoryEnabled` from `false` to `true`.

CC's auto-memory will:
- Observe conversations and auto-save important context
- Load saved context into the system prompt on every new session
- Handle curation internally (CC has its own heuristics)

No additional prompt engineering needed — CC's memory system has its own built-in behavioral rules.

### Change 2: Rename MCP tools

Rename the four memory tools to signal "structured records, not diary":

| Old name | New name | New description |
|----------|----------|-----------------|
| `store` | `store_record` | "Store a tagged record. Content is scanned for prompt injection. Use for structured data (cron results, audit entries, explicit facts) — not for general conversation context. Returns record ID." |
| `recall` | `query_records` | "Look up records by tag or keyword. Returns matching active records." |
| `search` | `search_records` | "Full-text search records using FTS5. Returns BM25-ranked results." |
| `forget` | `delete_record` | "Soft-delete a record by ID. Entry is excluded from queries but preserved in audit log." |

Rename in both servers:
- `crates/rightclaw-cli/src/memory_server.rs` — function names, `#[tool]` descriptions
- `crates/rightclaw-cli/src/memory_server_http.rs` — function names, `#[tool]` descriptions

Rename param structs to match:
- `StoreParams` → `StoreRecordParams`
- `RecallParams` → `QueryRecordsParams`
- `SearchParams` → `SearchRecordsParams`
- `ForgetParams` → `DeleteRecordParams`

Update schemars descriptions on param fields:
- `StoreRecordParams.content`: "Content to store as a record"
- `StoreRecordParams.tags`: "Comma-separated tags for categorization"
- `QueryRecordsParams.query`: "Tag or keyword to search by"
- `SearchRecordsParams.query`: "Full-text search query"
- `DeleteRecordParams.id`: "Record ID to soft-delete"

Update response messages:
- `"stored memory id={id}"` → `"stored record id={id}"`
- `"forgot memory id={id}"` → `"deleted record id={id}"`
- `"memory id={id} not found or already deleted"` → `"record id={id} not found or already deleted"`

The `source_tool` value written to SQLite changes from `"mcp:store"` to `"mcp:store_record"`.

### Change 3: Update AGENTS.md prompt instructions

Add a `## Memory` section to both `templates/right/AGENTS.md` and `identity/AGENTS.md`. Place it **before** `## MCP Management`.

```markdown
## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).
```

Also update the `## MCP Management` section in both files: the tool list (`mcp_add`, `mcp_remove`, etc.) stays unchanged — those are MCP management tools, not record tools.

### Change 4: Update settings test

In `crates/rightclaw/src/codegen/settings_tests.rs`, the `generates_behavioral_flags` test asserts `autoMemoryEnabled` is `false`. Update to `true`.

## Files Modified

| File | Change |
|------|--------|
| `crates/rightclaw/src/codegen/settings.rs` | `autoMemoryEnabled: false` → `true` |
| `crates/rightclaw/src/codegen/settings_tests.rs` | Update assertion to `true` |
| `crates/rightclaw-cli/src/memory_server.rs` | Rename functions, param structs, descriptions, response messages |
| `crates/rightclaw-cli/src/memory_server_http.rs` | Same renames (imports + function names + descriptions) |
| `templates/right/AGENTS.md` | Add `## Memory` section |
| `identity/AGENTS.md` | Add `## Memory` section |

## Not Changed

- **SQLite schema** — `memories` table stays as-is. "Records" is a prompt-facing rename, not a schema change.
- **Underlying store functions** — `store_memory`, `recall_memories`, `search_memories`, `forget_memory` in `crates/rightclaw/src/memory/store.rs` keep their names. They're internal API, not agent-facing.
- **`PROTECTED_MCP_SERVER`** — stays `"right"`.
- **Prompt injection guard** — unchanged, still scans `store_record` content.
- **ARCHITECTURE.md** — will reference "right MCP" for records; CC memory is external to rightclaw architecture.

## Risks

1. **CC memory quality in sandbox.** CC's auto-memory is designed for developer CLI usage. Inside an OpenShell sandbox with Telegram as the only channel, its heuristics may not be well-tuned. Mitigation: monitor early and adjust if needed.
2. **CLAUDE_CONFIG_DIR per agent.** Each agent sandbox has its own `CLAUDE_CONFIG_DIR`. CC memory is already isolated per agent — no cross-contamination risk.
3. **Tool rename breaks existing agents.** Agents with stored cron specs or skills referencing `store`/`recall` by name will break. Mitigation: this is a breaking change, acknowledged. `rightclaw up` regenerates all configs.
