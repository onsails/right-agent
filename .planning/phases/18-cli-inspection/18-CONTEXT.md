# Phase 18: CLI Inspection - Context

**Gathered:** 2026-03-26
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 18 delivers `rightclaw memory` — a nested subcommand group with four operator-facing operations (`list`, `search`, `delete`, `stats`) that inspect any agent's SQLite memory database from the terminal without entering an agent session. No MCP layer, no skill changes.

</domain>

<decisions>
## Implementation Decisions

### Subcommand Structure
- **D-01:** Nested subcommand: `Commands::Memory { command: MemoryCommands }` mirroring the existing `Commands::Config { command: ConfigCommands }` pattern. All four ops appear under `rightclaw memory --help`.
  - `rightclaw memory list <agent>`
  - `rightclaw memory search <agent> <query>`
  - `rightclaw memory delete <agent> <id>`
  - `rightclaw memory stats <agent>`

### Output Format
- **D-02:** Default output is plain columnar text (fixed-width, `println!`). `--json` flag on `list`, `search`, and `stats` emits newline-delimited JSON for scripting/piping. Consistent with `rightclaw list` and `rightclaw status` output style.
  - JSON keys match `MemoryEntry` field names: `id`, `content`, `tags`, `stored_by`, `source_tool`, `created_at`, `importance`
  - `stats` JSON: `{ "agent": str, "db_size_bytes": u64, "total_entries": u64, "oldest": str|null, "newest": str|null }`

### Pagination
- **D-03:** `--limit N` (default **10**) + `--offset N` (default 0) on both `list` and `search`. When result count equals limit, print a footer:
  - `10 of 127 entries shown  (--offset 10 for next page)`
  - When `--json` is active, omit footer (consumer handles pagination).

### Hard-Delete Semantics
- **D-04:** `delete` removes the `memories` row only (`DELETE FROM memories WHERE id = ?`). `memory_events` rows for that id are **preserved** — the audit trail remains intact. The entry simply stops appearing in `list`/`search`.
- **D-05:** `delete` always prompts confirmation: display truncated entry content + `stored_by`, then `Hard-delete this entry? [y/N]:`. Default is No (Enter cancels). No `--force` flag in v2.3 (can be added later if scripting need emerges).

### Agent Resolution
- **D-06:** Agent path = `$RIGHTCLAW_HOME/agents/<agent>`. If the directory does not exist → fatal miette error: `"agent '{name}' not found at {path}"`. If `memory.db` is absent → fatal: `"no memory database for agent '{name}' — run \`rightclaw up\` first"`.

### New Store Function Required
- **D-07:** `hard_delete_memory(conn: &Connection, id: i64) -> Result<(), MemoryError>` must be added to `crates/rightclaw/src/memory/store.rs`. Returns `MemoryError::NotFound(id)` if row does not exist. Called only by the CLI — not exposed via MCP.

### Claude's Discretion
- Column widths and truncation for `content` in list output (suggested: truncate at 60 chars with `…`).
- Whether `stats` shows size in bytes, KB, or auto-scales (e.g. `4.2 KB`).
- Exact SQL for `stats` (total active entries, oldest/newest `created_at` from non-deleted rows).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase Foundation
- `.planning/phases/16-db-foundation/16-CONTEXT.md` — schema topology, MemoryError type, open_connection API
- `.planning/phases/17-memory-skill/17-CONTEXT.md` — MCP server pattern, store function signatures

### Existing Store Functions (reuse directly)
- `crates/rightclaw/src/memory/store.rs` — `recall_memories`, `search_memories`, `forget_memory`, `MemoryEntry` struct
- `crates/rightclaw/src/memory/mod.rs` — `open_connection(agent_path)` signature

### CLI Patterns to Follow
- `crates/rightclaw-cli/src/main.rs` — `Commands::Config` nested subcommand pattern (lines ~24-82); `cmd_list` / `cmd_doctor` for plain println! output style
- `crates/rightclaw/src/memory/error.rs` — `MemoryError` variants (add nothing — reuse `NotFound`)

### Requirements
- `.planning/REQUIREMENTS.md` — CLI-01 through CLI-04 (this phase)

No external specs — requirements fully captured in decisions above.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `open_connection(agent_path: &Path)` in `memory/mod.rs` — opens WAL-mode DB with migrations; returns live `Connection`
- `search_memories(conn, query)` — FTS5 BM25, returns `Vec<MemoryEntry>` (CLI-02 reuses this directly)
- `recall_memories(conn, query)` — LIKE fallback, also returns `Vec<MemoryEntry>` (not needed for CLI — `list` uses SELECT without filter)
- `MemoryEntry { id, content, tags, stored_by, source_tool, created_at, importance }` — output struct for list/search
- `MemoryError::NotFound(i64)` — already defined, reuse for `hard_delete_memory`

### Established Patterns
- Nested subcommand: `Commands::Config { command: ConfigCommands }` + match arm dispatch — copy this for `Commands::Memory { command: MemoryCommands }`
- Fatal error format: `miette::miette!("agent '{name}' not found at {path}")` — matches existing `cmd_up` style
- `rightclaw list` uses `println!` with manual column formatting — same approach for memory list/search
- Confirmation prompt: no existing `[y/N]` pattern in codebase — implement with `std::io::stdin().read_line()`

### Integration Points
- `Commands` enum in `main.rs` — add `Memory { command: MemoryCommands }` variant + `MemoryCommands` enum
- `crates/rightclaw/src/memory/store.rs` — add `hard_delete_memory()` function
- `crates/rightclaw/src/memory/mod.rs` — re-export `hard_delete_memory` alongside existing store functions
- No new workspace crates needed — all code stays in `rightclaw` (library) and `rightclaw-cli` (binary)

</code_context>

<specifics>
## Specific Notes

- Default limit is **10** (user-specified). MCP skill uses 50 — CLI intentionally uses a tighter default for terminal readability.
- `delete` has no `--force` flag in v2.3. If scripting need emerges, add in v2.4.
- `list` uses a direct `SELECT ... FROM memories WHERE deleted_at IS NULL ORDER BY created_at DESC LIMIT ? OFFSET ?` — does NOT call `recall_memories` (which takes a query string). New store function `list_memories(conn, limit, offset)` needed.

</specifics>

<deferred>
## Deferred Ideas

- `--force`/`-f` flag on `delete` for scripting/CI use — v2.4 candidate
- `rightclaw memory export <agent>` JSON/CSV dump — already listed as MEM-F04 in REQUIREMENTS.md
- Vector/semantic search via sqlite-vec — MEM-F01 in REQUIREMENTS.md
- Memory eviction policy (expires_at, importance threshold) — MEM-F03

</deferred>

---

*Phase: 18-cli-inspection*
*Context gathered: 2026-03-26*
