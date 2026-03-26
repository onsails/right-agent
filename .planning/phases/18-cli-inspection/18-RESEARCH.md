# Phase 18: CLI Inspection - Research

**Researched:** 2026-03-26
**Domain:** Rust CLI subcommand extension, SQLite query patterns, terminal output formatting
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Nested subcommand `Commands::Memory { command: MemoryCommands }` mirroring `Commands::Config`. All four ops: `list`, `search`, `delete`, `stats`.
- **D-02:** Default output is plain columnar `println!`. `--json` flag on `list`, `search`, `stats` emits newline-delimited JSON. JSON keys match `MemoryEntry` field names. `stats` JSON: `{ "agent": str, "db_size_bytes": u64, "total_entries": u64, "oldest": str|null, "newest": str|null }`.
- **D-03:** `--limit N` (default 10) + `--offset N` (default 0) on `list` and `search`. Footer when count == limit: `10 of 127 entries shown  (--offset 10 for next page)`. Omit footer when `--json`.
- **D-04:** `delete` removes `memories` row only. `memory_events` rows preserved (audit trail intact).
- **D-05:** `delete` always prompts confirmation with truncated content + `stored_by`. `Hard-delete this entry? [y/N]:`. Default No. No `--force` in v2.3.
- **D-06:** Agent path = `$RIGHTCLAW_HOME/agents/<agent>`. Missing dir → fatal miette: `"agent '{name}' not found at {path}"`. Missing `memory.db` → fatal: `"no memory database for agent '{name}' — run \`rightclaw up\` first"`.
- **D-07:** `hard_delete_memory(conn: &Connection, id: i64) -> Result<(), MemoryError>` added to `store.rs`. Returns `MemoryError::NotFound(id)` if row absent. CLI-only, not MCP-exposed.

### Claude's Discretion
- Column widths and truncation for `content` in list output (suggested: truncate at 60 chars with `…`).
- Whether `stats` shows size in bytes, KB, or auto-scales (e.g. `4.2 KB`).
- Exact SQL for `stats` (total active entries, oldest/newest `created_at` from non-deleted rows).

### Deferred Ideas (OUT OF SCOPE)
- `--force`/`-f` flag on `delete` for scripting/CI — v2.4 candidate
- `rightclaw memory export <agent>` JSON/CSV dump — MEM-F04
- Vector/semantic search via sqlite-vec — MEM-F01
- Memory eviction policy (expires_at, importance threshold) — MEM-F03
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CLI-01 | `rightclaw memory list <agent>` shows paginated memory table with timestamps | New `list_memories(conn, limit, offset)` store fn + table formatting in cmd_memory_list |
| CLI-02 | `rightclaw memory search <agent> <query>` uses same FTS5 index as skill | Reuse existing `search_memories()` + apply limit/offset at SQL level |
| CLI-03 | `rightclaw memory delete <agent> <id>` hard-deletes entry (operator bypass of soft-delete) | New `hard_delete_memory()` store fn; confirmation prompt via stdin read_line |
| CLI-04 | `rightclaw memory stats <agent>` shows DB size, entry count, oldest/newest entry | SQLite `page_count * page_size` pragma for size; COUNT/MIN/MAX on non-deleted rows |
</phase_requirements>

## Summary

Phase 18 extends the existing CLI with a `rightclaw memory` subcommand group. All building blocks already exist in the codebase — the phase is assembly, not invention. The `Commands::Config` nested subcommand pattern is the direct template. Store functions (`search_memories`, `forget_memory`, `MemoryEntry`) are reused directly; two new store functions (`list_memories`, `hard_delete_memory`) fill the remaining gaps. No new crates needed.

The only non-trivial design area is output formatting: columnar terminal tables with truncation and a pagination footer. The codebase currently uses raw `println!` with manual `{:<N}` format strings (see `cmd_status`, `cmd_list`). That pattern is the standard here. For `--json`, `serde_json::to_string` on existing `MemoryEntry` fields is sufficient — no new serialization types needed.

The confirmation prompt for `delete` has no existing precedent in the codebase. The `prompt_telegram_user_id` function in main.rs shows the correct pattern: `std::io::stdin().read_line()` with flush before print.

**Primary recommendation:** Copy `Commands::Config` pattern verbatim. Add `MemoryCommands` enum, two new store functions, four `cmd_memory_*` functions in main.rs. Keep all code in existing crates — no new workspace crates.

## Standard Stack

### Core (all already in Cargo.toml — zero new dependencies)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | workspace | SQLite queries for list/search/delete/stats | Already powering memory store |
| serde_json | workspace | `--json` output serialization | Already a workspace dep |
| clap | workspace | `MemoryCommands` enum via `#[derive(Subcommand)]` | Already powering all CLI parsing |
| miette | workspace | Fatal error formatting for bad agent/DB path | Project standard for user-facing errors |
| thiserror | workspace | `MemoryError` already covers all error cases | Already defined in error.rs |

**No new `cargo add` commands needed for this phase.**

### New Store Functions Required

| Function | File | Signature |
|----------|------|-----------|
| `list_memories` | `crates/rightclaw/src/memory/store.rs` | `fn list_memories(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<MemoryEntry>, MemoryError>` |
| `hard_delete_memory` | `crates/rightclaw/src/memory/store.rs` | `fn hard_delete_memory(conn: &Connection, id: i64) -> Result<(), MemoryError>` |

Both must be re-exported from `crates/rightclaw/src/memory/mod.rs` alongside existing store exports.

## Architecture Patterns

### Subcommand Structure (copy from Commands::Config)

```rust
// In Commands enum — same pattern as Config:
Memory {
    #[command(subcommand)]
    command: MemoryCommands,
},

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Show paginated memory table
    List {
        agent: String,
        #[arg(long, default_value = "10")] limit: i64,
        #[arg(long, default_value = "0")]  offset: i64,
        #[arg(long)]                       json: bool,
    },
    /// Full-text search memories (FTS5 BM25)
    Search {
        agent: String,
        query: String,
        #[arg(long, default_value = "10")] limit: i64,
        #[arg(long, default_value = "0")]  offset: i64,
        #[arg(long)]                       json: bool,
    },
    /// Hard-delete a memory entry (operator bypass)
    Delete {
        agent: String,
        id: i64,
    },
    /// Show database stats
    Stats {
        agent: String,
        #[arg(long)] json: bool,
    },
}
```

Match arm dispatch mirrors the Config pattern:

```rust
Commands::Memory { command } => match command {
    MemoryCommands::List { agent, limit, offset, json } =>
        cmd_memory_list(&home, &agent, limit, offset, json),
    MemoryCommands::Search { agent, query, limit, offset, json } =>
        cmd_memory_search(&home, &agent, &query, limit, offset, json),
    MemoryCommands::Delete { agent, id } =>
        cmd_memory_delete(&home, &agent, id),
    MemoryCommands::Stats { agent, json } =>
        cmd_memory_stats(&home, &agent, json),
},
```

### Agent Path Resolution (reusable helper)

All four commands need the same resolution logic. Extract to a private function:

```rust
fn resolve_agent_db(home: &Path, agent: &str) -> miette::Result<rusqlite::Connection> {
    let agent_path = home.join("agents").join(agent);
    if !agent_path.exists() {
        return Err(miette::miette!(
            "agent '{}' not found at {}",
            agent, agent_path.display()
        ));
    }
    let db_path = agent_path.join("memory.db");
    if !db_path.exists() {
        return Err(miette::miette!(
            "no memory database for agent '{}' — run `rightclaw up` first",
            agent
        ));
    }
    rightclaw::memory::open_connection(&agent_path)
        .map_err(|e| miette::miette!("failed to open memory.db for '{}': {e:#}", agent))
}
```

### list_memories SQL

```rust
// New store function — no injection guard needed (read-only, no user data written)
pub fn list_memories(
    conn: &Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, content, tags, stored_by, source_tool, created_at, importance \
         FROM memories \
         WHERE deleted_at IS NULL \
         ORDER BY created_at DESC \
         LIMIT ?1 OFFSET ?2",
    )?;
    // ... query_map same as recall_memories
}
```

### hard_delete_memory SQL

```rust
// Pattern mirrors forget_memory but uses DELETE instead of UPDATE
pub fn hard_delete_memory(conn: &Connection, id: i64) -> Result<(), MemoryError> {
    // Verify row exists (deleted or not — operator can hard-delete soft-deleted rows)
    let exists: bool = conn
        .query_row("SELECT 1 FROM memories WHERE id = ?1", [id], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    if !exists {
        return Err(MemoryError::NotFound(id));
    }
    conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
    Ok(())
}
```

Note: `memory_events` rows are NOT deleted (D-04). The TRIGGER on `memories_ad` will attempt to update `memories_fts` — this is correct behavior (FTS index must stay consistent). The `memories_ad` trigger fires on DELETE and removes the FTS entry.

### stats SQL

```rust
// Three queries in sequence:
// 1. DB file size (filesystem)
let db_size = std::fs::metadata(db_path)?.len();

// 2. Entry count + oldest/newest (single query)
conn.query_row(
    "SELECT count(*), min(created_at), max(created_at) \
     FROM memories WHERE deleted_at IS NULL",
    [],
    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?)),
)?
```

Use `std::fs::metadata(agent_path.join("memory.db"))?.len()` for size — no SQLite pragma needed. SQLite `page_count * page_size` gives allocated pages, not actual file size; filesystem metadata is simpler and accurate.

### Output Formatting Patterns

**Columnar table (list/search):**

```rust
// Header
println!("{:<6} {:<60} {:<20} {}", "ID", "CONTENT", "STORED_BY", "CREATED_AT");
// Row with truncation
let truncated = if entry.content.len() > 60 {
    format!("{}…", &entry.content[..59])
} else {
    entry.content.clone()
};
println!("{:<6} {:<60} {:<20} {}", entry.id, truncated, stored_by, entry.created_at);
```

Use `entry.content.chars().count()` for char-safe truncation (content may include multi-byte UTF-8).

**Pagination footer (text mode only):**

```rust
if results.len() as i64 == limit {
    let total: i64 = conn.query_row(
        "SELECT count(*) FROM memories WHERE deleted_at IS NULL",
        [], |r| r.get(0)
    )?;
    println!("\n{} of {} entries shown  (--offset {} for next page)",
        limit, total, offset + limit);
}
```

**Size auto-scaling (stats, Claude's discretion):**

```rust
fn format_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{} B", bytes) }
    else if bytes < 1_048_576 { format!("{:.1} KB", bytes as f64 / 1024.0) }
    else { format!("{:.1} MB", bytes as f64 / 1_048_576.0) }
}
```

**Confirmation prompt (delete):**

```rust
// Pattern from prompt_telegram_user_id in main.rs
use std::io::{self, Write};
// Display truncated entry first
println!("  content:   {}", truncate(&entry.content, 60));
println!("  stored_by: {}", entry.stored_by.as_deref().unwrap_or("(unknown)"));
print!("Hard-delete this entry? [y/N]: ");
io::stdout().flush().map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
let mut input = String::new();
io::stdin().read_line(&mut input)
    .map_err(|e| miette::miette!("failed to read input: {e}"))?;
if input.trim().to_lowercase() != "y" {
    println!("Aborted.");
    return Ok(());
}
```

### --json Output Pattern

```rust
if json {
    for entry in &entries {
        // serde_json::json! macro for inline structs, or derive Serialize on MemoryEntry
        println!("{}", serde_json::json!({
            "id": entry.id,
            "content": entry.content,
            "tags": entry.tags,
            "stored_by": entry.stored_by,
            "source_tool": entry.source_tool,
            "created_at": entry.created_at,
            "importance": entry.importance,
        }));
    }
    return Ok(());
}
```

`MemoryEntry` currently derives `Debug, Clone` only. Two options:
1. Add `#[derive(serde::Serialize)]` to `MemoryEntry` and call `serde_json::to_string` — cleaner, requires adding `serde` feature to `MemoryEntry` derive.
2. Use `serde_json::json!` macro inline — no struct change needed.

**Recommendation:** Add `#[derive(serde::Serialize)]` to `MemoryEntry`. `serde` is already a workspace dep. This is cleaner and avoids manual field listing in json macro.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| FTS5 search pagination | Custom cursor logic | SQL `LIMIT ?1 OFFSET ?2` appended to existing `search_memories` query |
| DB file size | SQLite page_count pragma | `std::fs::metadata().len()` — simpler and accurate |
| JSON output | Custom serializer | `serde_json::json!` macro or derive `Serialize` on `MemoryEntry` |
| Progress/spinner | Any TUI library | Not needed — commands complete in milliseconds on local SQLite |

**Key insight:** This is a read-mostly local SQLite CLI. Everything is synchronous and fast. No async needed — all four commands are `fn` not `async fn`.

## Common Pitfalls

### Pitfall 1: UTF-8 truncation panic
**What goes wrong:** `&entry.content[..59]` panics on multi-byte character boundary.
**Why it happens:** Rust string slicing is byte-indexed; memory content may contain non-ASCII.
**How to avoid:** Use `entry.content.chars().take(59).collect::<String>()` for safe truncation.
**Warning signs:** Test with content containing emoji or CJK characters.

### Pitfall 2: FTS5 query syntax errors surface as rusqlite errors
**What goes wrong:** User passes `search_memories` a query like `foo*bar` that violates FTS5 syntax — returns `Err(MemoryError::Sqlite(...))` instead of a helpful message.
**Why it happens:** FTS5 MATCH fails at SQL execution time.
**How to avoid:** Map `MemoryError::Sqlite` in the CLI command to a user-friendly miette error that suggests valid FTS5 syntax.

### Pitfall 3: hard_delete triggers FTS5 update (expected, not a bug)
**What goes wrong:** Developer sees FTS index UPDATE after DELETE and thinks something is wrong.
**Why it happens:** `memories_ad` trigger fires on DELETE and removes the FTS entry — this is correct schema behavior from Phase 16.
**How to avoid:** Document this in `hard_delete_memory` doc comment. Do not suppress the trigger.

### Pitfall 4: delete on soft-deleted row
**What goes wrong:** `hard_delete_memory` called on an id that has `deleted_at IS NOT NULL`. The row exists in the DB — the function should succeed (operator intent is to remove it entirely).
**Why it happens:** D-04 says "removes the memories row only" — no restriction to non-deleted rows.
**How to avoid:** Check `SELECT 1 FROM memories WHERE id = ?1` (no `deleted_at IS NULL` filter). Any existing row can be hard-deleted.

### Pitfall 5: search with limit+offset requires adding LIMIT/OFFSET to search_memories
**What goes wrong:** Existing `search_memories()` has hardcoded `LIMIT 50` — cannot be paginated.
**Why it happens:** CLI needs limit/offset control; MCP skill uses fixed 50.
**How to avoid:** Either add a new `search_memories_paged(conn, query, limit, offset)` function, or add optional limit/offset parameters. **Recommended:** new function `search_memories_paged` — keeps existing MCP behavior unchanged (don't break SKILL-03).

### Pitfall 6: Confirmation prompt on non-TTY stdin
**What goes wrong:** If `rightclaw memory delete` is called in a pipe (e.g., shell script), `read_line` returns empty immediately and the default `N` cancels silently.
**Why it happens:** D-05 explicitly has no `--force` for v2.3. This is intentional behavior.
**How to avoid:** No action needed — silent cancel on non-TTY is the correct v2.3 behavior.

## Code Examples

### Existing Confirmation Prompt Pattern (from main.rs)

```rust
// Source: crates/rightclaw-cli/src/main.rs — prompt_telegram_user_id()
use std::io::{self, Write};
print!("Prompt text: ");
io::stdout().flush().map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
let mut input = String::new();
io::stdin()
    .read_line(&mut input)
    .map_err(|e| miette::miette!("failed to read input: {e}"))?;
let answer = input.trim();
```

### Existing Columnar Output Pattern (from main.rs cmd_status)

```rust
// Source: crates/rightclaw-cli/src/main.rs — cmd_status()
println!("{:<20} {:<12} {:<10} UPTIME", "NAME", "STATUS", "PID");
for p in &processes {
    println!("{:<20} {:<12} {:<10} {}", p.name, p.status, p.pid, p.system_time);
}
```

### Existing Fatal Error Pattern

```rust
// Source: crates/rightclaw-cli/src/main.rs — cmd_up() agent not found
return Err(miette::miette!(
    "agent '{}' not found. Available agents: {}",
    name, available.join(", ")
));
```

### Existing Store Function Pattern (for new list_memories)

```rust
// Source: crates/rightclaw/src/memory/store.rs — recall_memories()
pub fn recall_memories(conn: &Connection, query: &str) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare("SELECT id, content, tags, stored_by, source_tool, created_at, importance FROM memories WHERE deleted_at IS NULL AND (...) ORDER BY created_at DESC LIMIT 50")?;
    let entries = stmt.query_map([query], |row| Ok(MemoryEntry { ... }))?.collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}
```

## State of the Art

| Old Approach | Current Approach | Impact |
|--------------|------------------|--------|
| `serde_yaml` (archived 2024) | `serde-saphyr` | No impact on this phase — no YAML involved |
| `forget_memory` (soft-delete only) | `hard_delete_memory` (new, operator hard-delete) | CLI-03 new function, audit trail preserved |

**No deprecated patterns in scope for this phase.**

## Open Questions

1. **`search_memories_paged` vs modifying `search_memories`**
   - What we know: existing `search_memories` has hardcoded `LIMIT 50`; CLI needs `LIMIT ?1 OFFSET ?2`
   - What's unclear: whether to add params to existing function (changing MCP behavior) or add a new function
   - Recommendation: add `search_memories_paged(conn, query, limit, offset)` as a new function. Keep existing `search_memories` unchanged to avoid breaking MCP skill. Both functions can share the same SQL with different parameter binding.

2. **`MemoryEntry` Serialize derive**
   - What we know: `MemoryEntry` derives `Debug, Clone` only; `serde_json::json!` macro works without it
   - What's unclear: whether to touch the `MemoryEntry` struct definition
   - Recommendation: add `#[derive(serde::Serialize)]` — it's the cleaner path and `serde` is already in scope for the crate.

## Sources

### Primary (HIGH confidence)
- Direct code inspection: `crates/rightclaw-cli/src/main.rs` — Commands enum, ConfigCommands pattern, output style, confirmation prompt pattern
- Direct code inspection: `crates/rightclaw/src/memory/store.rs` — MemoryEntry, store function signatures, rusqlite query_map pattern
- Direct code inspection: `crates/rightclaw/src/memory/mod.rs` — open_connection API
- Direct code inspection: `crates/rightclaw/src/memory/error.rs` — MemoryError variants
- Direct code inspection: `crates/rightclaw/src/memory/sql/v1_schema.sql` — schema topology, FTS5 triggers, memories_ad behavior on DELETE
- Direct code inspection: `crates/rightclaw/src/memory/store_tests.rs` — testing patterns for store functions

### Secondary (MEDIUM confidence)
- SQLite docs: `DELETE FROM table` fires AFTER DELETE triggers — confirmed via v1_schema.sql `memories_ad` trigger structure

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, all patterns read directly from source
- Architecture: HIGH — direct copy of verified existing patterns, SQL straightforward
- Pitfalls: HIGH — derived from schema inspection and code reading, not speculation

**Research date:** 2026-03-26
**Valid until:** 2026-04-25 (stable stack, no external services)
