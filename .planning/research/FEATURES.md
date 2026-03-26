# Feature Research: Per-Agent SQLite Memory (v2.3)

**Domain:** AI agent memory system — per-agent structured memory with skill API + CLI inspection
**Researched:** 2026-03-26
**Confidence:** HIGH (primary operations from IronClaw source, OpenClaw docs, Anthropic memory tool spec, rusqlite crate verified; IronClaw internal tool signatures inferred from deepwiki + GitHub search results — LOW confidence on exact param names)

---

## Context: What Already Exists

Each agent dir (`~/.rightclaw/agents/<name>/`) has a `MEMORY.md` flat file — Claude's native auto-memory. CC loads the first 200 lines at session start. Works well for key facts, but:

- No structure (flat markdown, no query capability)
- No provenance (who stored what and when)
- CC's own write policy (implicit, not agent-controlled)
- No CLI inspection without reading raw markdown
- No search (full file load only)
- No expiry or soft-delete

v2.3 adds a parallel structured SQLite store. MEMORY.md stays untouched — SQLite is additive, not a replacement.

---

## IronClaw Comparison

**IronClaw** (`github.com/nearai/ironclaw`) is the closest competitor: Rust, agent memory, OpenClaw-compatible, security-focused. Key differences:

| Aspect | IronClaw | RightClaw v2.3 |
|--------|----------|----------------|
| DB backend | PostgreSQL + pgvector | SQLite (bundled rusqlite) |
| Memory tools | `memory_search`, `memory_write`, `memory_read`, `memory_tree` | `/remember`, `/recall`, `/forget` (slash commands + model-invocable) |
| Vector search | pgvector (requires psql server) | FTS5 keyword search (no embedding dependency) |
| Isolation | Per-workspace PostgreSQL DB | Per-agent .sqlite file in agent HOME |
| Ops overhead | psql server required | Zero — bundled SQLite, single file |
| Skill format | SKILL.md + tool calls | SKILL.md built-in (same as rightskills) |
| Provenance | Not documented in public sources | `stored_by` + `source_tool` + timestamps (designed in) |

IronClaw chose PostgreSQL for vector search at scale. For RightClaw's per-agent, local-first model, SQLite is correct — same decision OpenClaw and ZeroClaw made. No embedding dependency keeps the install size and startup time minimal. FTS5 full-text search is sufficient for the "recall a fact" use case.

---

## The Four Core Operations (Industry Consensus)

The field has converged on four primitive operations. Every production memory system implements these. Missing any = incomplete.

| Operation | What It Does | Industry Term |
|-----------|--------------|---------------|
| **store** | Write a new memory entry with timestamp + provenance | ADD, memorize, `memory_write` |
| **recall** | Read by key or retrieve recent entries | READ, `memory_read` |
| **search** | Query by content (keyword or semantic) | SEARCH, `memory_search` |
| **forget** | Soft-delete or hard-delete an entry | DELETE, `memory_forget` |

Anthropic's own memory tool uses: `view`, `create`, `str_replace`, `insert`, `delete`, `rename` — more filesystem-flavored but maps to the same primitives. IronClaw exposes: `memory_search`, `memory_write`, `memory_read`, `memory_tree`. Agent Zero uses: recall, memorize, consolidate.

---

## Feature Landscape

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| `store` — write a named memory entry | Any memory system must write | LOW | `key + value + optional tags`; auto-timestamps; `stored_by` = agent name from `RC_AGENT_NAME` env |
| `recall` — read entry by key | Symmetry with store; exact lookup | LOW | Returns single row or error; include created_at, updated_at, stored_by |
| `search` — full-text query across all entries | Find memories without knowing exact key | MEDIUM | SQLite FTS5 on `key` + `value` columns; returns ranked results |
| `forget` — delete an entry by key | Memory without forgetting = noise accumulation (Mem0, MemoryBank lesson) | LOW | Soft-delete: set `deleted_at`, keep row for audit trail; hard-delete not exposed to agent |
| Timestamps on every entry | Provenance minimum — when was this stored | LOW | `created_at` + `updated_at` + `deleted_at` columns, SQLite `CURRENT_TIMESTAMP` defaults |
| Provenance: who stored it | Multi-tool agents need to know source | LOW | `stored_by TEXT` column — agent name from env var `RC_AGENT_NAME`; `source_tool TEXT` — slash command name or `auto` |
| Per-agent isolation | Each agent has separate memory space | LOW | DB file lives at `$AGENT_HOME/memory.db`; no shared DB across agents |
| `rightclaw memory list` CLI | Operators need to inspect agent memory | LOW | Read-only: list all entries (key, created_at, stored_by, preview of value) |
| `rightclaw memory search <query>` CLI | FTS query from outside an agent session | LOW | Same FTS5 query path as skill; output to stdout |
| `rightclaw memory delete <key>` CLI | Operators can remove bad/stale entries | LOW | Hard-delete from CLI (bypasses soft-delete — operators have direct DB access) |
| Built-in skill (`/memory`) | Memory operations from inside CC session | MEDIUM | SKILL.md with store/recall/search/forget slash commands; installed as built-in on every `rightclaw up` |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Audit trail: full row history never deleted | GDPR right-to-be-forgotten is a hard problem; RightClaw can offer it correctly via soft-delete — entries "forgotten" remain in DB with `deleted_at` set, invisible to agent search but auditable | LOW | Soft-delete pattern: `WHERE deleted_at IS NULL` on all agent queries; CLI sees all including deleted |
| `tags` column for categorical recall | `/recall tag:credential-used` retrieves all entries with that tag — useful for tracking what the agent has tried | LOW-MEDIUM | `tags TEXT` JSON array stored as JSON string; FTS5 indexes it |
| `rightclaw memory export <agent>` | Dump agent memory to JSON or markdown for backup/migration | LOW | Simple `SELECT * FROM memories` formatted output |
| Update-in-place with history | When agent stores same key twice, track `updated_at` rather than creating duplicate — avoids bloat; Mem0's ADD/UPDATE/NOOP pattern | LOW | `INSERT OR REPLACE` or `ON CONFLICT(key) DO UPDATE SET value=..., updated_at=CURRENT_TIMESTAMP` |
| `rightclaw memory stats` | Show entry count, oldest/newest entry, size on disk — useful for long-running agents | LOW | Single aggregation query |
| WAL mode by default | Concurrent reads during agent session don't block CLI inspection writes | LOW | `PRAGMA journal_mode=WAL` on DB open; standard best practice (IronClaw and OpenClaw both do this) |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Vector/semantic search (embeddings) | "Find memories similar to X" without exact keywords | Requires embedding model — adds llama.cpp or OpenAI API dep, 200MB+ model download, or network call. Defeats local-first, zero-ops goal. OpenClaw needs FTS fallback when embeddings fail anyway. | FTS5 BM25 search is sufficient for named facts. Tag-based recall (`tag:domain`) covers categorical retrieval. Add semantic search in v2.4+ if demand exists |
| Cross-agent memory sharing | "Agent A and B share a common knowledge base" | Violates per-agent isolation — the core RightClaw security model. Shared memory = shared trust boundary = one compromised agent can pollute all agents. Out of scope per PROJECT.md. | MCP memory server (PROJECT.md future item). Each agent maintains its own store. |
| Memory expiry / TTL | "Forget things older than N days" | Destroys audit trail — primary compliance and forensics value of the log. Also: deciding what to expire requires a policy that's hard to get right (Mem0 uses LLM-driven merge, not TTL). | Soft-delete preserves record. CLI `memory delete` for manual cleanup. If storage grows large, `memory export` + truncate is user-controlled. |
| LLM-driven memory consolidation | "Let Claude decide what to keep, merge, and discard" (Mem0 pattern) | Requires additional API calls on every store; adds latency and cost; consolidation bugs can silently corrupt memory. Overkill for v2.3. | Simple key-based store. Agent is responsible for key names. If it stores the same key twice, `updated_at` tracks the change. Consolidation is v2.4+. |
| Automatic memory flush (CC-triggered) | "CC auto-stores important facts at session end" | Session-end hook pattern is unreliable in CC (SessionStart hooks have known "ToolUseContext required" bug; session end is harder). Race conditions with process-compose restarts. | Explicit `/remember key value` by agent. Agent decides what's worth persisting. More reliable, better auditability. |
| Per-memory encryption | "Encrypt sensitive values in DB" | SQLite doesn't have column-level encryption in standard builds. SQLCipher adds build complexity. For v2.3, sandbox (bubblewrap/Seatbelt) already restricts DB access — CC can't read outside agent dir. | Agent HOME isolation is the security layer. If field-level encryption is needed, `env:` passthrough for sensitive values is the right pattern. SQLCipher in v2.4+ |

---

## Feature Dependencies

```
[SQLite DB file at $AGENT_HOME/memory.db]
    └──required before──> [/memory skill can store/recall]
    └──required before──> [rightclaw memory CLI can read]
    └──required before──> [audit trail exists]

[rightclaw-core crate: db module]
    └──required before──> [rightclaw memory CLI (rightclaw-cli crate)]
    └──required before──> [DB creation in rightclaw up]

[DB created on rightclaw up]
    └──depends on──> [rightclaw-core db module]
    └──touches──> [cmd_up.rs: create memory.db with schema if absent]

[/memory built-in skill (SKILL.md)]
    └──depends on──> [DB file exists (created by rightclaw up)]
    └──depends on──> [RC_AGENT_NAME env var (already set by shell wrapper)]
    └──installed via──> [install_builtin_skills() in cmd_up.rs]

[rightclaw memory list|search|delete CLI]
    └──depends on──> [rightclaw-core db module]
    └──depends on──> [agent name arg to locate agent HOME]
    └──independent of──> [/memory skill]

[Soft-delete (deleted_at column)]
    └──required before──> [audit trail is meaningful]
    └──enables──> [CLI hard-delete bypass (operator sees all rows)]
```

### Dependency Notes

- **DB module must be in `rightclaw-core`**, not `rightclaw-cli`. Both the CLI and the skill codegen path need to share the schema. Putting it in `rightclaw-cli` would require the skill to depend on the CLI crate — wrong direction.
- **`RC_AGENT_NAME` env var is already set** by the shell wrapper (identity env vars). No new env var work needed for provenance tracking.
- **DB creation is in `cmd_up`** — on every `rightclaw up`, create `memory.db` with the full schema if the file does not exist. `CREATE TABLE IF NOT EXISTS` is idempotent. Never drop or migrate on up — migrations are a separate concern.
- **FTS5 virtual table** requires the `fts5` feature in rusqlite (not enabled by default; must add to Cargo features). The `bundled` feature includes FTS5.
- **skill depends on DB file existing** — if an agent runs a session before `rightclaw up` creates the DB, the skill will fail. The skill should check for DB existence and emit a clear error rather than crashing silently.

---

## Schema Design

### Primary `memories` Table

```sql
CREATE TABLE IF NOT EXISTS memories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL UNIQUE,
    value       TEXT NOT NULL,
    tags        TEXT NOT NULL DEFAULT '[]',  -- JSON array string
    stored_by   TEXT NOT NULL,               -- agent name (RC_AGENT_NAME)
    source_tool TEXT NOT NULL DEFAULT 'manual',  -- slash command or 'auto'
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    deleted_at  TEXT                         -- NULL = active; set = soft-deleted
);
```

### FTS5 Virtual Table

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    key,
    value,
    tags,
    content='memories',
    content_rowid='id'
);
```

Trigger to keep FTS5 in sync:

```sql
CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, key, value, tags) VALUES (new.id, new.key, new.value, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags) VALUES ('delete', old.id, old.key, old.value, old.tags);
    INSERT INTO memories_fts(rowid, key, value, tags) VALUES (new.id, new.key, new.value, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags) VALUES ('delete', old.id, old.key, old.value, old.tags);
END;
```

### PRAGMA Settings on Open

```sql
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
```

WAL mode: concurrent reads from CLI while agent session is active. Foreign keys: not used in v2.3 but good hygiene.

---

## Skill API Design

### /memory skill — Slash Command Surface

The skill is invoked as a CC slash command. Claude calls it; the skill instructions tell Claude how to form the arguments.

| Command | Syntax | What It Does |
|---------|--------|--------------|
| `/remember` | `/remember <key> <value>` | Store or update a memory. Key is a short identifier (snake_case). Value is the content. |
| `/recall` | `/recall <key>` | Retrieve a specific memory by exact key. |
| `/search` | `/search <query>` | FTS5 full-text search across key, value, tags. Returns ranked results. |
| `/forget` | `/forget <key>` | Soft-delete a memory. Sets `deleted_at`. Agent search will no longer find it. |

Implementation approach: The SKILL.md instructions tell Claude to invoke a shell script (in `skills/memory/bin/`) that wraps `sqlite3` CLI or is a compiled helper. The shell script handles DB access; Claude passes arguments as positional params.

### CLI Subcommand Surface

`rightclaw memory <agent> <subcommand>`:

| Subcommand | Args | Output |
|------------|------|--------|
| `list` | `[--all]` (include deleted) | Table: key, created_at, stored_by, value preview |
| `search` | `<query>` | FTS5 results: key, relevance, value preview |
| `delete` | `<key>` | Hard-delete (removes row, removes from FTS5); requires confirmation |
| `export` | `[--format json|md]` | Full dump to stdout |
| `stats` | — | Count, oldest/newest, DB size |

### Provenance Fields

Every entry records:

- `stored_by` — the agent's name (from `RC_AGENT_NAME` env var, already set by shell wrapper)
- `source_tool` — which tool invoked the write: `remember`, `auto`, `import`, etc.
- `created_at` — ISO 8601 UTC timestamp, set at insert, never changed
- `updated_at` — ISO 8601 UTC timestamp, updated on every write to same key
- `deleted_at` — ISO 8601 UTC timestamp if soft-deleted; NULL if active

This is the minimum audit trail. No cryptographic hashing needed for v2.3 — the audit value comes from temporal provenance, not tamper-evidence. Hash-based audit (as used by some enterprise systems) is v2.4+ if compliance requirements emerge.

---

## MVP Definition

### Launch With (v2.3)

- [ ] **SQLite DB creation on `rightclaw up`** — `memory.db` created with schema in each agent HOME if absent. `PRAGMA journal_mode=WAL`. Idempotent (`CREATE TABLE IF NOT EXISTS`). No migration on startup.
- [ ] **`/memory` built-in skill** — SKILL.md with store, recall, search, forget. Shell script helper in `skills/memory/bin/mem.sh` that wraps sqlite3. Installed as built-in by `install_builtin_skills()`. Provenance from `RC_AGENT_NAME`.
- [ ] **`rightclaw memory list <agent>`** — Read `memory.db` from agent HOME. Table output. `--all` flag shows soft-deleted entries.
- [ ] **`rightclaw memory search <agent> <query>`** — FTS5 query, stdout output.
- [ ] **`rightclaw memory delete <agent> <key>`** — Hard-delete with confirmation prompt.
- [ ] **Soft-delete semantics** — Agent `forget` sets `deleted_at`; agent `search`/`recall` filter `WHERE deleted_at IS NULL`; CLI `list` shows all by default, `list --all` shows deleted entries too.
- [ ] **Timestamps + provenance on every write** — `created_at`, `updated_at`, `stored_by`, `source_tool` populated on every insert/update.

### Add After Validation (v2.3.x)

- [ ] **`rightclaw memory export <agent>`** — JSON dump for backup/migration. Trigger: operators want to migrate agents or audit memory externally.
- [ ] **`rightclaw memory stats <agent>`** — Count, oldest, newest, DB size. Low effort, useful for long-running agents.
- [ ] **Tags support in search** — `/search tag:credential-used`. FTS5 already indexes tags column; this is an argument parsing enhancement in the skill.

### Future Consideration (v2.4+)

- [ ] **Vector/semantic search** — FTS5 covers v2.3. Add sqlite-vec extension + local embeddings if agents outgrow keyword search.
- [ ] **SQLCipher encryption** — DB at rest encryption. Blocked on build complexity; sandbox provides sufficient isolation for v2.3.
- [ ] **LLM-driven consolidation** — Mem0-style ADD/UPDATE/NOOP pipeline. High value but adds API calls on every store. Design-heavy.
- [ ] **Cross-agent MCP memory server** — PROJECT.md explicit future item. Requires MCP protocol design; out of scope until then.

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| SQLite DB creation on up | HIGH (everything depends on it) | LOW (schema + rusqlite bundled) | P1 |
| /memory skill (store/recall/search/forget) | HIGH (agent usability — primary interface) | MEDIUM (SKILL.md + shell script + FTS5) | P1 |
| Timestamps + provenance | HIGH (audit trail is the stated goal) | LOW (column defaults, no extra logic) | P1 |
| rightclaw memory list/search | HIGH (CLI inspection is the stated goal) | LOW (read-only SQL queries) | P1 |
| rightclaw memory delete | MEDIUM (operator cleanup) | LOW (hard-delete + confirmation) | P1 |
| Soft-delete semantics | HIGH (audit trail integrity) | LOW (deleted_at column + WHERE filter) | P1 |
| Tags column | MEDIUM (categorical recall) | LOW (JSON column, FTS5 indexed) | P2 |
| export subcommand | LOW (nice for power users) | LOW (SELECT * + format) | P2 |
| stats subcommand | LOW (informational) | LOW (aggregate query) | P2 |
| Vector search | HIGH long-term | HIGH (embedding dep, build complexity) | P3 |
| SQLCipher | MEDIUM (compliance) | HIGH (build complexity) | P3 |

---

## Competitor Feature Analysis

| Feature | IronClaw | OpenClaw | RightClaw v2.3 |
|---------|----------|----------|----------------|
| DB backend | PostgreSQL + pgvector | SQLite (sqlite-vec + FTS5) | SQLite (FTS5, no vector) |
| Memory tools | `memory_search`, `memory_write`, `memory_read`, `memory_tree` | `memory_search`, `memory_get` | `/remember`, `/recall`, `/search`, `/forget` |
| Semantic search | Yes (pgvector) | Yes (sqlite-vec + embeddings) | No (FTS5 only) — v2.4 |
| Provenance | Not documented | Not documented | stored_by + source_tool + timestamps |
| Soft-delete / audit trail | Not documented | Not documented | Yes — designed in |
| CLI inspection | Not documented | Not documented | Yes — `rightclaw memory` subcommand |
| Ops overhead | psql server required | zero | zero |
| Per-agent isolation | Per-workspace DB | MEMORY.md per agent | Per-agent .sqlite file |
| WAL mode | n/a (postgres) | Yes | Yes (PRAGMA on open) |
| Tags / categorical recall | Not documented | Not documented | tags column + FTS5 |

---

## Existing Skill Infrastructure (Reuse)

The built-in skill install mechanism already exists (`install_builtin_skills()` in `cmd_up.rs`). v2.3 adds one more skill to the same function:

- Source dir: `skills/memory/` in repo
- Install path: `$AGENT_HOME/.claude/skills/memory/SKILL.md`
- Binary helper: `skills/memory/bin/mem.sh` (sqlite3 wrapper)
- Constant: `SKILL_MEMORY` alongside `SKILL_RIGHTSKILLS`

The `RC_AGENT_NAME` env var is already exported by the shell wrapper (identity env vars block). No new env var infrastructure needed.

---

## Sources

- [IronClaw GitHub — nearai/ironclaw](https://github.com/nearai/ironclaw) — Rust, PostgreSQL, memory_search/write/read/tree tools (MEDIUM confidence — tool names from search result summaries, not source code direct read)
- [deepwiki.com/nearai/ironclaw](https://deepwiki.com/nearai/ironclaw) — memory system architecture (MEDIUM confidence)
- [Memory & Search OpenClaw deepwiki](https://deepwiki.com/openclaw/openclaw/3.4.3-memory-and-search) — SQLite FTS5 + sqlite-vec, hybrid search (MEDIUM confidence)
- [OpenClaw memory docs](https://docs.openclaw.ai/concepts/memory) — memory_search, memory_get tools (MEDIUM confidence)
- [Letta agent memory blog](https://www.letta.com/blog/agent-memory) — MemGPT tiered memory model: core/recall/archival (HIGH confidence — official Letta source)
- [Mem0 overview — AI Agent Memory Systems in 2026](https://yogeshyadav.medium.com/ai-agent-memory-systems-in-2026-mem0-zep-hindsight-memvid-and-everything-in-between-compared-96e35b818da8) — ADD/UPDATE/DELETE/NOOP consolidation pattern (MEDIUM confidence)
- [rusqlite crates.io v0.38.0](https://crates.io/crates/rusqlite) — current version, bundled feature, FTS5 support (HIGH confidence)
- [Rusqlite Rust Guide 2025](https://generalistprogrammer.com/tutorials/rusqlite-rust-crate-guide) — bundled feature, schema patterns (MEDIUM confidence)
- [Red Gate: Database Design for Audit Logging](https://www.red-gate.com/blog/database-design-for-audit-logging/) — audit trail schema best practices (HIGH confidence)
- [Why Your AI Agent's Memory Is Broken — SQLite](https://gerus-lab.hashnode.dev/why-your-ai-agents-memory-is-broken-and-how-to-fix-it-with-sqlite) — WAL mode, FTS5 for agent memory (MEDIUM confidence)
- [Anthropic Memory Tool docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool) — official memory_20250818 tool API (HIGH confidence)
- [IronClaw "Why Postgres?" issue #19](https://github.com/nearai/ironclaw/issues/19) — confirms IronClaw chose PostgreSQL over SQLite (HIGH confidence — primary source)
- [OpenClaw vs IronClaw comparison](https://clawchemy.xyz/blog/openclaw-vs-ironclaw-which-ai-agent-framework-is-best) — feature matrix context (LOW confidence — single blog source)
- [sqlite-memory GitHub (sqliteai)](https://github.com/sqliteai/sqlite-memory) — FTS5 + vector hybrid, markdown-aware chunking pattern (MEDIUM confidence)

---
*Feature research for: RightClaw v2.3 Memory System milestone*
*Researched: 2026-03-26*
