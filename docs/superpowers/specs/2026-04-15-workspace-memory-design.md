# Workspace Memory System

## Problem

Agents have no persistent behavioral memory. Two issues exposed this:

1. **Composio misdiagnosis**: Agent reported "no write permissions" when the actual error was wrong parameter format (`input` vs `arguments`). After fixing, the agent had no way to remember the correct format for future sessions.
2. **No memory disambiguation**: The system prompt vaguely says "persistent memory" without distinguishing behavioral memory (changes how the agent acts) from data storage (facts to look up later).

Claude Code's built-in auto-memory exists but is non-functional: `--system-prompt-file` replaces the default system prompt which contains auto-memory instructions. The agent never receives them.

## Design

Replace the current flat `memories` table and `store_record`/`query_records`/`search_records`/`delete_record` MCP tools with a database-backed virtual filesystem modeled after IronClaw's workspace memory.

### Core Concepts

**Workspace** — a virtual filesystem stored in SQLite. Each document has a path (e.g., `MEMORY.md`, `daily/2026-04-15.md`, `projects/alpha/notes.md`). No actual files on disk — paths are values in a `path` column.

**Prompt-injected documents** — `MEMORY.md` and today/yesterday daily logs are assembled into a composite file and uploaded to sandbox before every `claude -p` invocation. The agent sees them in its system prompt without making tool calls.

**Arbitrary paths** — agents store any structured data at arbitrary paths (e.g., `trackers/github-releases.json`, `events/paris/2026-04-15.json`). These are only accessible via `memory_search` and `memory_read`, not prompt-injected.

**Chunking** — documents over 800 words are split into chunks with 15% overlap for better FTS accuracy. Short documents = 1 chunk = full content.

**Versioning** — replace operations auto-create a version snapshot. Daily logs skip versioning (configured via folder `.config`).

## Schema (v14 migration)

```sql
CREATE TABLE workspace_documents (
    id TEXT PRIMARY KEY,                -- UUID
    path TEXT NOT NULL UNIQUE,
    content TEXT NOT NULL DEFAULT '',
    metadata TEXT NOT NULL DEFAULT '{}', -- JSON: {skip_versioning, skip_indexing, hygiene, schema}
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE workspace_document_versions (
    id TEXT PRIMARY KEY,                -- UUID
    document_id TEXT NOT NULL REFERENCES workspace_documents(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,          -- SHA-256 of content
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    changed_by TEXT,
    UNIQUE(document_id, version)
);

CREATE TABLE workspace_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL REFERENCES workspace_documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB,                     -- reserved for future vector search
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(document_id, chunk_index)
);

CREATE VIRTUAL TABLE workspace_chunks_fts USING fts5(
    content,
    content='workspace_chunks',
    content_rowid='id'
);

-- FTS sync triggers (INSERT/DELETE/UPDATE on workspace_chunks)
```

Drop old `memories` table, `memories_fts` virtual table, and `memory_events` table.

## MCP Tools

Four tools on the `right` backend. Old tools (`store_record`, `query_records`, `search_records`, `delete_record`) are removed entirely.

### memory_write

| param | type | default | description |
|-------|------|---------|-------------|
| `content` | string | `""` | Content to write |
| `target` | string | null | Shortcut: `"memory"` → `MEMORY.md`, `"daily_log"` → `daily/YYYY-MM-DD.md` |
| `path` | string | null | Direct workspace path (used when target is null) |
| `append` | bool | `true` | Append (true) or replace (false) |
| `old_string` | string | null | Patch mode: find this exact string |
| `new_string` | string | null | Patch mode: replace with this |
| `replace_all` | bool | `false` | Patch mode: all occurrences |

**Target resolution:**
- `"memory"` → path `MEMORY.md`, append separator `\n\n`
- `"daily_log"` → path `daily/YYYY-MM-DD.md`, always append, prefix `[HH:MM:SS] `, ignore `append` flag
- null → use `path` param directly

**On write:**
1. Prompt injection guard (reject if detected)
2. If replace: create version snapshot of current content (unless `skip_versioning` in folder .config)
3. Write/append content to document (upsert — create if not exists)
4. Re-chunk document, update FTS index

**Patch mode** (when `old_string` present):
- `new_string` required
- Document must exist and contain `old_string`
- Creates version before patching
- Returns `{status: "patched", path, replacements, content_length}`

**Validation:**
- Either `target` or `path` must be provided (not both null)
- Absolute filesystem paths rejected (`/Users/...`, `~/...`)
- Empty `old_string` rejected

### memory_read

| param | type | default | description |
|-------|------|---------|-------------|
| `path` | string | required | Workspace path |
| `version` | int | null | Specific version (null = latest) |
| `list_versions` | bool | `false` | Return version list instead of content |

**Return formats:**
- Normal: `{path, content, word_count, updated_at}`
- Version: `{path, version, content, content_hash, created_at, changed_by}`
- List versions: `[{version, content_hash, created_at, changed_by}, ...]` (up to 50)

### memory_search

| param | type | default | description |
|-------|------|---------|-------------|
| `query` | string | required | Search query |
| `limit` | int | 5 (max 20) | Max results |

FTS5 on chunks, BM25 ranking. Returns `[{content, score, path, document_id}]`.

When embedding provider is added later, this becomes hybrid FTS + vector with RRF fusion. The interface stays the same.

### memory_tree

| param | type | default | description |
|-------|------|---------|-------------|
| `path` | string | `""` | Root path |
| `depth` | int | 1 (max 10) | Tree depth |

Returns JSON array of strings (files) and objects (directories with children). Built from `path` column by splitting on `/`.

## Composite Memory (Prompt Injection)

### Assembly

Before every `claude -p` invocation (worker, cron, delivery), the host-side code:

1. Opens `data.db`, reads `workspace_documents` for paths: `MEMORY.md`, `daily/{today}.md`, `daily/{yesterday}.md`
2. Assembles composite markdown:

```markdown
## Long-Term Memory

{MEMORY.md content, truncated to 200 lines}

## Today's Notes

{daily/YYYY-MM-DD.md content}

## Yesterday's Notes

{daily/YYYY-MM-DD.md content}
```

3. Empty sections are omitted entirely (no header if no content)
4. If all three are empty/missing, no composite file is produced — prompt assembly skips the `cat`
5. If MEMORY.md exceeds 200 lines, truncate and append: `\n\n[Truncated — {total} lines total. Curate with memory_write to keep under 200.]`

### Upload

- Sandbox mode: upload to `/platform/composite-memory.md.{sha256}` (content-addressed, like other platform files). Symlink from a stable path if needed.
- No-sandbox mode: write to `{agent_dir}/.claude/composite-memory.md`

### Prompt assembly

`build_prompt_assembly_script()` adds a `cat` for the composite memory file after identity files, before MCP instructions:

```
Operating Instructions → IDENTITY.md → SOUL.md → USER.md → AGENTS.md → TOOLS.md → composite-memory.md → MCP Instructions
```

### Timezone

Daily log paths use UTC by default. Configurable per-agent via `agent.yaml` `timezone` field (future enhancement — not in this spec).

## Skill

Built-in skill `rightmemory`, installed to `.claude/skills/rightmemory/` via `codegen/skills.rs`.

Contents teach the agent:
- `memory_write(target: "memory")` for behavioral learning (tool formats, user preferences, mistakes to avoid)
- `memory_write(target: "daily_log")` for session notes
- `memory_write(path: "...")` for structured data (trackers, events, project notes)
- `memory_search` before answering questions about prior work
- `memory_read` for specific documents
- `memory_tree` to explore workspace
- MEMORY.md is in system prompt — brevity matters, curate periodically
- Conventions: `projects/{name}/` for project data, `trackers/` for state tracking

Listed in OPERATING_INSTRUCTIONS.md under Core Skills as `/rightmemory`.

## Hygiene

### Folder .config documents

Workspace paths starting with a directory can have a `.config` document that applies to all documents in that directory:

```json
{"hygiene": {"enabled": true, "retention_days": 30}, "skip_versioning": true}
```

### Seed .config documents

Created at agent init:
- `daily/.config` — `{"hygiene": {"enabled": true, "retention_days": 30}, "skip_versioning": true}`

### Hygiene runner

Runs in the bot process on each background sync cycle (every 5 min):
1. Find all `.config` documents with `hygiene.enabled: true`
2. For each: delete documents in that directory older than `retention_days`
3. `.config` documents themselves are never deleted

### Seed documents

Created at agent init (`init_agent()` or first bot startup):
- `MEMORY.md` — empty content
- `daily/.config` — hygiene config as above

## Files Changed

### New files
| File | Purpose |
|------|---------|
| `crates/rightclaw/src/memory/sql/v14_workspace.sql` | Schema DDL |
| `crates/rightclaw/src/memory/workspace.rs` | Workspace CRUD, chunking, versioning, hygiene, composite assembly |
| `crates/rightclaw/src/memory/workspace_tests.rs` | Tests |
| `skills/rightmemory/SKILL.md` | Memory management skill |

### Modified files
| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/migrations.rs` | Add v14 migration, drop `memories`/`memories_fts`/`memory_events` |
| `crates/rightclaw/src/memory/mod.rs` | Export workspace module |
| `crates/rightclaw-cli/src/right_backend.rs` | Replace 4 old tools with 4 new tools, update `with_instructions()` |
| `crates/rightclaw-cli/src/aggregator.rs` | Update `with_instructions()` |
| `crates/bot/src/telegram/worker.rs` | Composite memory assembly + upload before `claude -p` |
| `crates/bot/src/telegram/prompt.rs` | Add composite-memory.md to prompt assembly script |
| `crates/bot/src/cron.rs` | Composite memory assembly before cron `claude -p` |
| `crates/bot/src/cron_delivery.rs` | Composite memory assembly before delivery `claude -p` |
| `crates/bot/src/sync.rs` | Add hygiene runner to sync cycle |
| `crates/rightclaw/src/init.rs` | Seed MEMORY.md and daily/.config |
| `crates/rightclaw/src/memory/guard.rs` | Extend to all workspace writes |
| `crates/rightclaw/src/codegen/skills.rs` | Install rightmemory skill |
| `templates/right/prompt/OPERATING_INSTRUCTIONS.md` | Replace Memory section, add /rightmemory to Core Skills |
| `crates/rightclaw/src/codegen/agent_def.rs` | Update "persistent memory" wording |
| `ARCHITECTURE.md` | Update Memory Schema section |
| `PROMPT_SYSTEM.md` | Update tool references |

### Deleted files
| File | Reason |
|------|--------|
| `crates/rightclaw/src/memory/sql/v1_schema.sql` | Old memories table (migration still referenced but table dropped in v14) |
| `crates/rightclaw-cli/src/memory_server.rs` | Deprecated MCP stdio server |

### Deleted code (within files)
| Location | What |
|----------|------|
| `right_backend.rs` | `store_record`, `query_records`, `search_records`, `delete_record` handlers |
| `right_backend.rs` | `StoreRecordParams`, `QueryRecordParams`, etc. structs |
| `memory/store.rs` | `store_memory`, `recall_memories`, `search_memories`, `forget_memory` functions (keep `save_auth_token`, `get_auth_token`, `delete_auth_token` — move to separate module if needed) |

## Testing

### Unit tests (workspace.rs)

**Basic CRUD:**
- write + read roundtrip
- append mode (content concatenation)
- replace mode (full overwrite)
- patch mode (old_string/new_string)
- patch mode replace_all
- patch mode: old_string not found → error
- delete document
- read non-existent path → error
- write creates document if not exists (upsert)

**Targets:**
- `target: "memory"` resolves to `MEMORY.md`
- `target: "daily_log"` resolves to `daily/YYYY-MM-DD.md` with `[HH:MM:SS]` prefix
- `target: "daily_log"` always appends regardless of `append` flag
- `target: "memory"` append uses `\n\n` separator
- invalid target with no path → error

**Versioning:**
- replace creates version with correct version number
- sequential replaces increment version
- list_versions returns correct history
- read specific version returns correct content
- content_hash is SHA-256
- skip_versioning: true in folder .config → no versions created

**Chunking:**
- document < 800 words → 1 chunk
- document exactly 800 words → 1 chunk
- document 801 words → multiple chunks with 15% overlap
- re-chunk on update (old chunks deleted, new created)
- FTS indexes chunk content

**Search:**
- FTS finds content across documents
- FTS finds content within chunks of large documents
- BM25 ranking (more relevant results first)
- limit parameter respected
- no results → empty array
- search returns document path

**Tree:**
- empty workspace → seed documents only
- nested paths build correct tree structure
- depth=1 shows only top level
- depth=2 shows children
- path parameter scopes to subdirectory

**Guard:**
- prompt injection in content → rejected
- normal content → accepted
- guard applies to all workspace writes (not just MEMORY.md)

### Composite memory tests

**Happy path:**
- MEMORY.md + today + yesterday → all three sections
- Only MEMORY.md → one section
- Only today's daily → one section

**Edge cases — empty/missing:**
- MEMORY.md does not exist → no Long-Term Memory section
- MEMORY.md exists but empty → no Long-Term Memory section
- Today's daily does not exist → no Today's Notes section
- Yesterday's daily does not exist → no Yesterday's Notes section
- Both dailies missing → only Long-Term Memory (if exists)
- All three missing → no composite file produced, prompt assembly skips cat
- First session ever (fresh agent) → no composite file, agent functions normally

**Truncation:**
- MEMORY.md exactly 200 lines → no truncation
- MEMORY.md 201 lines → truncated to 200 + warning line
- MEMORY.md 1000 lines → truncated to 200 + warning line with "1000 lines total"

**Timezone:**
- UTC date boundary: daily log path uses correct date
- Daily log written at 23:59 UTC, composite assembled at 00:01 UTC next day → yesterday's log appears

### Hygiene tests

- Document older than retention_days → deleted
- Document exactly at retention boundary → deleted
- Document within retention → preserved
- .config document itself → never deleted
- Directory without .config → no cleanup
- hygiene.enabled: false → no cleanup

### Integration tests

- Full flow: memory_write via MCP → composite assembled → uploaded to sandbox → visible in system prompt
- Cron job writes to memory → next chat session sees it in prompt
- Agent writes daily_log → same session continues normally → next session sees it in yesterday/today
