# Project Research Summary

**Project:** RightClaw v2.3 Memory System
**Domain:** Per-agent SQLite-backed persistent memory in Rust async CLI
**Researched:** 2026-03-26
**Confidence:** HIGH

## Executive Summary

RightClaw v2.3 adds structured, queryable memory to each agent as a complement to the existing flat `MEMORY.md` file. The key insight from research: these are two strictly separate systems serving different purposes — `MEMORY.md` is human-authored and auto-injected into the CC system prompt; SQLite is agent-managed episodic memory accessed only via explicit skill calls. This boundary is not just good design — it is a security requirement. The MINJA attack (NeurIPS 2025) demonstrated 95%+ injection success rates against memory-backed agents by poisoning the retrieval path. Collapsing the boundary (writing SQLite entries into MEMORY.md, or auto-injecting recall at session start) directly enables this attack class. IronClaw chose PostgreSQL for vector search at scale; that choice confirms SQLite is correct for RightClaw's per-agent, local-first, zero-ops model — they explicitly rejected SQLite via GitHub issue #19.

The recommended stack is a synchronous `MemoryStore` in `rightclaw-core` backed by `rusqlite 0.39` with the `bundled` feature and `rusqlite_migration 2.5` for schema versioning via SQLite `user_version` pragma. The skill pattern is a SKILL.md instructing Claude Code to call `sqlite3` bash commands directly — no MCP, no custom binary, consistent with `rightskills` and `rightcron`. `tokio-rusqlite` was evaluated and rejected: `cmd_memory` is a synchronous CLI operation and the `cmd_up` scaffold loop is already effectively sync; async bridging overhead adds complexity with no benefit for a single-writer embedded database. `sqlx` is rejected due to `libsqlite3-sys` semver conflict and compile-time query checking being broken for runtime-dynamic paths.

The critical risks resolve to two rules enforced from day one: (1) append-only schema with `BEFORE UPDATE` and `BEFORE DELETE` triggers set to `RAISE(ABORT, ...)` on the events table, plus WAL mode + `busy_timeout=5000ms` on every connection open; (2) the memory skill NEVER writes to `MEMORY.md`. Every other pitfall — migration lock-in, cross-agent leakage, silent write failure — is a configuration error on one of these two axes.

## Key Findings

### Recommended Stack

The SQLite driver stack is `rusqlite 0.39` (bundles SQLite 3.51.1) + `rusqlite_migration 2.5`. Note: STACK.md recommended `rusqlite 0.38` + `rusqlite_migration 2.4` + `tokio-rusqlite 0.7`. ARCHITECTURE.md (written from direct codebase inspection, same date) supersedes this with `0.39` + `2.5` + sync-only. ARCHITECTURE.md is authoritative.

**Core technologies:**
- `rusqlite 0.39` (features = ["bundled"]): SQLite driver — canonical Rust binding, thin wrapper, full control; bundled feature eliminates system SQLite version variability; correct for a single-binary system tool that ships via `install.sh`
- `rusqlite_migration 2.5`: Schema migrations — `user_version` pragma tracking (no extra table), embedded SQL strings in Rust, idempotent `to_latest()` safe to call on every startup; 2.5 required for rusqlite 0.39 compatibility (2.4.x breaks on 0.38 statement caching change)
- `sqlite3` (system binary, accessed from skill): Skill-facing access — SKILL.md instructs CC to run `sqlite3 "$HOME/memory.db"` bash commands directly; `$HOME` resolves to agent dir because shell wrapper already sets `export HOME=<agent_path>`

**Do not add:**
- `sqlx`: Conflict hazard with `libsqlite3-sys` (both link the same C library), compile-time query checks broken for dynamic runtime paths, async connection pool irrelevant for SQLite
- `tokio-rusqlite`: Unnecessary — `MemoryStore` is sync; no executor starvation risk when DB calls live in the scaffold loop outside async task tree
- `diesel`, `sea-orm`: ORM overhead wrong for a simple K/V + search store
- `refinery`: heavier than needed; rusqlite_migration is lighter and SQLite-only

### Expected Features

The field has converged on four core memory primitives. Every production system implements these; missing any results in an incomplete system.

**Must have (table stakes):**
- `store` — write a named memory entry (key + value + provenance); slash command `/remember key value`; upsert on same key via `ON CONFLICT(key) DO UPDATE` tracking `updated_at`
- `recall` — read by exact key; slash command `/recall key`; returns row including timestamps and provenance
- `search` — full-text query across key, value, tags; slash command `/search query`; `LIKE '%query%'` in v2.3 (FTS5 in v2.4)
- `forget` — soft-delete by key; slash command `/forget key`; inserts a forget event in `memory_events`, NEVER hard-deletes
- Timestamps + provenance on every entry: `created_at`, `updated_at`, `stored_by` (from `RC_AGENT_NAME`, already set by shell wrapper), `source_tool`
- Per-agent isolation: DB at `~/.rightclaw/agents/<name>/memory.db`, no shared state
- `rightclaw memory list|search|delete|vacuum` CLI subcommands for operator inspection
- Soft-delete semantics: agent queries filter `WHERE deleted_at IS NULL`; CLI `--all` flag shows deleted entries

**Should have (competitive):**
- `tags` column for categorical recall
- `rightclaw memory export` — JSON/markdown dump
- `rightclaw memory stats` — entry count, DB size, oldest/newest (makes bloat visible)
- WAL mode by default — concurrent CLI reads while agent session is active
- Append-only audit trail from day 1 (see pitfalls — this is a must-have by v2.3, not a differentiator)

**Defer (v2.4+):**
- FTS5 virtual table + sync triggers — schema should include them in V1 (retrofitting is expensive), but skill can use LIKE in v2.3
- Vector/semantic search — requires embedding model, defeats zero-ops goal
- SQLCipher encryption — sandbox already provides isolation
- LLM-driven consolidation (Mem0 pattern) — adds API call latency on every store
- Cross-agent MCP memory server — explicit PROJECT.md future item
- `expires_at`/`importance` eviction logic — columns should exist in V1 schema, logic in v2.4

**Critical anti-feature:** MEMORY.md is not part of the memory system. The skill must never write to it.

### Architecture Approach

The new `memory/` module lives in `rightclaw-core` because both `cmd_up` (scaffold loop) and `rightclaw-cli` (memory subcommand) need the DB layer — putting it in `rightclaw-cli` would create a wrong-direction dependency. The module is four files (`mod.rs`, `migrations.rs`, `store.rs`, `error.rs`). The skill is installed as a built-in alongside `rightskills` and `rightcron` via `include_str!` at compile time. DB creation is step 10 in `cmd_up`'s per-agent scaffold loop (after `install_builtin_skills`), idempotent on every `rightclaw up`.

**Major components:**
1. `rightclaw-core/src/memory/` — DB open/init, WAL pragma, migrations, synchronous `MemoryStore` struct with insert/list/search/delete/vacuum
2. `skills/rightmemory/SKILL.md` — embedded skill; instructs CC to call `sqlite3 "$HOME/memory.db"` for store/recall/search/forget; compiled in via `include_str!`; NEVER writes to MEMORY.md
3. `rightclaw-cli: Commands::Memory` + `cmd_memory()` — `rightclaw memory list|search|delete|vacuum [--agent name]`; synchronous; uses `MemoryStore` from core
4. `cmd_up` scaffold loop step 10 — calls `memory::open_db(&agent.path)` idempotently on every `rightclaw up`

**DB location:** `~/.rightclaw/agents/<name>/memory.db` — flat in agent root, not inside `.claude/` (scaffold-managed directory that settings.json regenerates on every `up`).

**Schema note:** Two tables — `memories` (current state, key is UNIQUE) and `memory_events` (append-only event log). `forget` inserts a forget event; it never DELETEs from `memories`. Plain `LIKE '%query%'` for search in v2.3; FTS5 virtual table should be in V1 schema even if unused in v2.3 to avoid a costly later migration.

### Critical Pitfalls

1. **Memory entries injected into system prompt via MEMORY.md** — The skill must never write to `MEMORY.md`; `rightclaw up` must never read SQLite and append to `MEMORY.md`. NeurIPS 2025 MINJA attack: 95%+ injection success via poisoned memory retrieval. Enforce in code, document in SKILL.md. Run `store` 10 times and verify `MEMORY.md` is unchanged.

2. **Mutable audit trail** — A schema with `updated_at` and hard-delete `forget` has no real audit trail. Use an append-only `memory_events` table with `BEFORE UPDATE` and `BEFORE DELETE` triggers that `RAISE(ABORT, 'memory_events is append-only')`. `forget` inserts an event row with `event_type='forget'`, never DELETEs.

3. **Missing WAL + busy_timeout** — Default `busy_timeout=0` means `SQLITE_BUSY` on any contention — and the skill fails silently (CC may interpret non-zero exit as "nothing found"). Set `PRAGMA journal_mode=WAL` and `PRAGMA busy_timeout=5000` on every connection open. Treat `SQLITE_BUSY` after timeout as a hard error with user-visible message.

4. **No migration strategy from day one** — First schema is always wrong; there is always a v2.4 (FTS5 queries, eviction logic, embedding column). Use `rusqlite_migration` from the first commit; every connection open calls `MIGRATIONS.to_latest(&mut conn)`. Once a migration is shipped, it is immutable — new changes are new migration entries.

5. **Cross-agent DB path derivation** — If DB path defaults to `~/.rightclaw/memory.db` instead of `~/.rightclaw/agents/<name>/memory.db`, all agents share one store — complete isolation failure. DB path must always be `agent_dir.join("memory.db")`. Skill uses `$HOME/memory.db` which is correct because `$HOME` is remapped to agent dir by the shell wrapper.

## Implications for Roadmap

Three implementation phases, each a prerequisite for the next.

### Phase 1: DB Foundation + Skill

**Rationale:** Everything depends on the DB module. Schema decisions (append-only audit, eviction columns, FTS5 virtual table presence) are extremely costly to change after data exists in production agent DBs. Get schema right first. Security requirements (MEMORY.md boundary, append-only triggers, WAL + busy_timeout) must be in this phase, not added later.

**Delivers:** `rightclaw-core/src/memory/` module (4 files); schema with `memories` + `memory_events` (append-only) + FTS5 virtual table + sync triggers + immutability triggers + `expires_at`/`importance` eviction columns; `open_db()` called from `cmd_up` scaffold loop; `skills/rightmemory/SKILL.md` installed as built-in; WAL + busy_timeout on every connection open.

**Addresses from FEATURES.md:** store, recall, search, forget slash commands; timestamps + provenance; soft-delete semantics; per-agent isolation; MEMORY.md boundary enforced in code.

**Avoids from PITFALLS.md:** Pitfalls #1 (MEMORY.md injection), #2 (no migrations), #3 (file locking/WAL), #5 (dual-truth MEMORY.md/SQLite), #6 (mutable audit trail), #7 (silent write failure), #9 (cross-agent path derivation).

**Research flag:** Standard patterns — rusqlite migration setup, WAL pragma, append-only schema with triggers all have well-documented precedents. No research phase needed.

**Must pass before Phase 2:**
- WAL + busy_timeout verified via PRAGMA queries in tests
- Per-agent isolation test: two agents, unique memories, no cross-read
- MEMORY.md untouched after 10 skill `store` calls
- `UPDATE memory_events SET content='x'` raises ABORT (trigger test)
- Migration idempotency: two `rightclaw up` calls on fresh agent, `user_version` correct, no re-apply

### Phase 2: CLI Inspection Commands

**Rationale:** CLI (`rightclaw memory list/search/delete/vacuum`) depends on the DB module from Phase 1. CLI write operations (delete, vacuum) can contend with running agents — contention behavior must be designed against a working DB with real WAL behavior.

**Delivers:** `Commands::Memory` in `rightclaw-cli`; `cmd_memory()` synchronous function; table output for list/search; hard-delete with confirmation prompt; vacuum; `--agent name` flag; `rightclaw memory stats` showing entry count + DB size on disk.

**Uses from STACK.md:** `rusqlite` via `rightclaw-core`; no new dependencies.

**Implements from FEATURES.md:** `rightclaw memory list/search/delete` (P1 operators), `stats` (P2), `export` (P2 — can defer to v2.3.x).

**Avoids from PITFALLS.md:** Pitfall #8 (CLI vs. agent contention) — CLI uses `BEGIN IMMEDIATE` short transactions; fails after busy_timeout with clear message "Agent is currently writing memory. Try again in a moment."

**Research flag:** Standard patterns — clap derive subcommand, table printing. No research needed.

### Phase 3: Doctor Integration + Injection Scanning

**Rationale:** Operational checks and injection scanning are additive hardening on top of a working memory system. Doctor checks depend on Phase 1 (DB layer); injection scanning requires a working `store` path from Phase 1.

**Delivers:** `rightclaw doctor` checks for DB existence + schema version + WAL file cleanup + `memory.db` file permissions (0600); injection scanning on `store` (scan for imperative injection patterns before write, reject with error message on match); `rightclaw memory stats` DB size warning at configurable threshold.

**Addresses from FEATURES.md:** eviction column logic wire-up (columns from Phase 1 schema get logic here); `rightclaw memory stats` size warning.

**Avoids from PITFALLS.md:** Pitfalls #1 (injection scanning), #4 (memory bloat visibility), #10 (migration checksum mismatch detection in doctor).

**Research flag:** Injection scanning pattern needs research before implementing — practical Rust implementation (what regex patterns, Unicode homoglyph handling, false-positive thresholds) is sparse in existing research. Run `/gsd:research-phase` for injection scanning before Phase 3.

### Phase Ordering Rationale

- Schema decisions are irreversible once production data exists. Phase 1 nails the schema — append-only events, FTS5 virtual table presence, eviction columns — even if the logic using those columns ships later.
- Skill (Phase 1) and CLI (Phase 2) share the same `MemoryStore` — no circular dependency, clean build order.
- Doctor integration (Phase 3) is additive and non-blocking — agents function without it, but operators are blind without it.
- This order directly mirrors the dependency graph in FEATURES.md: `[DB module] → [skill + CLI] → [hardening]`.

### Research Flags

Needs research before implementation:
- **Phase 3 (injection scanning):** What patterns to detect, false-positive rate, whether to reject-on-match or sanitize-and-store. MINJA NeurIPS 2025 paper identifies the attack class but practical Rust implementation patterns are not documented in current research.

Standard patterns (skip research phase):
- **Phase 1:** rusqlite migrations, WAL setup, append-only schema with triggers — all well-documented with official sources
- **Phase 2:** CLI subcommand with clap derive, table output — established codebase pattern

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | rusqlite 0.39 + rusqlite_migration 2.5 versions confirmed from crates.io 2026-03-26 (direct codebase inspection); sqlx rejection rationale confirmed via GitHub issue #3926; bundled feature behavior documented |
| Features | HIGH | Four core operations from cross-competitor analysis (IronClaw, OpenClaw, Anthropic memory tool, Agent Zero); IronClaw PostgreSQL-vs-SQLite decision confirmed via primary source (GitHub issue #19) |
| Architecture | HIGH | Direct codebase inspection of /home/wb/dev/rightclaw/crates/; component boundaries follow existing patterns (rightskills, rightcron); skill pattern confirmed via system-skill-pattern source |
| Pitfalls | HIGH | MINJA NeurIPS 2025 is primary published research; SQLite WAL/locking from official SQLite docs; OpenClaw issue #26949 is a live bug report; append-only trigger pattern from Instant-SQLite-Audit-Trail |

**Overall confidence:** HIGH

### Gaps to Address

- **FTS5 vs. LIKE decision:** ARCHITECTURE.md chose plain `LIKE` for v2.3 to reduce complexity. STACK.md and FEATURES.md assumed FTS5 from day one. Resolution: include FTS5 virtual table and sync triggers in V1 schema (to avoid costly retrofitting), but have the skill use LIKE queries in v2.3. Skill switches to FTS5 queries in v2.4 without a schema migration.

- **Stack version discrepancy:** STACK.md recommends rusqlite 0.38 + rusqlite_migration 2.4 + tokio-rusqlite 0.7. ARCHITECTURE.md (direct codebase inspection, same date) recommends 0.39 + 2.5 + no tokio-rusqlite. Use ARCHITECTURE.md versions. Verify compatibility matrix before first Cargo.toml edit.

- **Eviction columns in V1 schema:** PITFALLS.md flags unbounded growth as a critical pitfall and recommends `expires_at` + `importance` columns in V1 schema. ARCHITECTURE.md schema does not include them. Resolution: add these columns to V1 schema during Phase 1 even if eviction logic ships in v2.4.

- **Injection scanning implementation:** Research identifies the attack class (MINJA, Unit 42, OWASP ASI06) but practical Rust implementation is not documented. Defer to Phase 3 with a dedicated research pass.

- **`sqlite3` binary availability in sandbox:** The skill calls `sqlite3` via bash. The default sandbox config does not exclude it. Doctor check should verify `sqlite3` is on PATH before agents start. Behavior on macOS (built-in) vs. Linux (package manager) differs — add to doctor checks.

## Sources

### Primary (HIGH confidence)
- Direct codebase inspection: `/home/wb/dev/rightclaw/crates/` (2026-03-26)
- [rusqlite 0.39.0 on crates.io](https://crates.io/crates/rusqlite) — version, bundled feature, FTS5 support confirmed 2026-03-26
- [rusqlite_migration 2.5.0 on crates.io](https://crates.io/crates/rusqlite_migration) — version, rusqlite 0.39 compatibility confirmed 2026-03-26
- [MINJA: Memory INJection Attack — NeurIPS 2025](https://openreview.net/forum?id=QVX6hcJ2um) — 95%+ injection success rate via query-only memory poisoning
- [SQLite WAL Mode](https://www.sqlite.org/wal.html) — WAL mode, checkpoint behavior, recovery on restart
- [SQLite File Locking and Concurrency V3](https://sqlite.org/lockingv3.html) — SQLITE_BUSY behavior, busy_timeout
- [Anthropic Memory Tool docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool) — official memory_20250818 tool API
- [IronClaw "Why Postgres?" issue #19](https://github.com/nearai/ironclaw/issues/19) — confirms PostgreSQL vs. SQLite decision
- [sqlx + rusqlite semver hazard — sqlx GitHub issue #3926](https://github.com/launchbadge/sqlx/issues/3926) — libsqlite3-sys conflict confirmed

### Secondary (MEDIUM confidence)
- [deepwiki.com/nearai/ironclaw](https://deepwiki.com/nearai/ironclaw) — memory system architecture
- [Memory & Search OpenClaw deepwiki](https://deepwiki.com/openclaw/openclaw/3.4.3-memory-and-search) — SQLite FTS5 + sqlite-vec pattern
- [OpenClaw Issue #26949](https://github.com/openclaw/openclaw/issues/26949) — MEMORY.md double injection live bug
- [The System Skill Pattern](https://www.shruggingface.com/blog/the-system-skill-pattern) — SKILL.md + sqlite3 bash approach
- [Palo Alto Unit 42: Indirect Prompt Injection](https://unit42.paloaltonetworks.com/indirect-prompt-injection-poisons-ai-longterm-memory/) — session summarization exploit via injected memory
- [Instant SQLite Audit Trail](https://github.com/simon-weber/Instant-SQLite-Audit-Trail) — trigger-based immutability pattern
- [rusqlite_migration async example](https://github.com/cljoly/rusqlite_migration/blob/master/examples/async/src/main.rs) — call_unwrap pattern

### Tertiary (LOW confidence)
- [Rust ORMs in 2026 comparison](https://aarambhdevhub.medium.com/rust-orms-in-2026-diesel-vs-sqlx-vs-seaorm-vs-rusqlite-which-one-should-you-actually-use-706d0fe912f3) — single blog source, context only
- [OpenClaw vs IronClaw comparison](https://clawchemy.xyz/blog/openclaw-vs-ironclaw-which-ai-agent-framework-is-best) — feature matrix context

---
*Research completed: 2026-03-26*
*Ready for roadmap: yes*
