# Requirements: RightClaw

**Defined:** 2026-03-26
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, orchestrated by a single CLI command.

## v2.3 Requirements (Memory System)

### Database Foundation

- [x] **DB-01**: `rightclaw up` creates per-agent `memory.db` (WAL mode + busy_timeout=5000ms) if absent
- [x] **DB-02**: V1 schema is append-only — memories table with `content, tags, stored_by, created_at, deleted_at, expires_at, importance`; SQLite triggers block UPDATE/DELETE
- [x] **DB-03**: FTS5 virtual table included in V1 schema for full-text search
- [x] **DB-04**: Schema migrations use rusqlite_migration 2.5 (user_version pragma); `to_latest()` called on every DB open

### Memory Skill

- [x] **SKILL-01**: Agent can store a memory via `/remember` — provenance (agent name, source_tool) auto-recorded
- [x] **SKILL-02**: Agent can look up memories via `/recall` — tag/keyword lookup
- [x] **SKILL-03**: Agent can full-text search memories via `/search` — FTS5 BM25 ranking
- [x] **SKILL-04**: Agent can soft-delete a memory via `/forget` — entry excluded from recall/search; audit row preserved
- [x] **SKILL-05**: `rightmemory` skill installed as built-in by `install_builtin_skills()` on every `rightclaw up`

### CLI Inspection

- [x] **CLI-01**: `rightclaw memory list <agent>` shows paginated memory table with timestamps
- [x] **CLI-02**: `rightclaw memory search <agent> <query>` uses same FTS5 index as skill
- [x] **CLI-03**: `rightclaw memory delete <agent> <id>` hard-deletes entry (operator bypass of soft-delete)
- [x] **CLI-04**: `rightclaw memory stats <agent>` shows DB size, entry count, oldest/newest entry

### Security

- [x] **SEC-01**: `/remember` scans entry for prompt injection patterns before writing; rejects on match
- [x] **SEC-02**: No code path writes to `MEMORY.md` from skill or CLI — strict separation enforced
- [x] **SEC-03**: Memory recall is always on-demand — never auto-injected into system prompt

### Doctor

- [x] **DOCTOR-01**: `rightclaw doctor` warns (non-fatal) when `sqlite3` binary is absent from PATH

## Future Requirements

### v2.4 Candidates

- **MEM-F01**: Semantic/vector search via sqlite-vec extension — deferred; FTS5 covers v2.3 use case
- **MEM-F02**: Cross-agent memory sharing via named shared DB — explicitly out of scope for v2.3
- **MEM-F03**: Memory eviction policy (TTL, importance threshold) — `expires_at`/`importance` columns in schema, logic deferred
- **MEM-F04**: `rightclaw memory export <agent>` — dump to JSON/CSV for portability
- **SEC-F01**: Injection scanning using regex pattern library — Phase 3 candidate per research

## Out of Scope

| Feature | Reason |
|---------|--------|
| Cross-agent memory sharing | Per-agent isolation is core design; sharing deferred to v2.4+ MCP approach |
| Semantic/vector search | sqlite-vec adds ops complexity; FTS5 BM25 covers agent recall use case |
| Auto-injection into system prompt | Security risk (MINJA attack); on-demand recall is correct pattern |
| MCP memory server | Future option; skill-based sqlite3 approach sufficient for v2.3 |
| Memory sync / backup | Out of scope; agent HOME backup is user's responsibility |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| DB-01 | Phase 16 | Complete |
| DB-02 | Phase 16 | Complete |
| DB-03 | Phase 16 | Complete |
| DB-04 | Phase 16 | Complete |
| SEC-02 | Phase 16 | Complete |
| SEC-03 | Phase 16 | Complete |
| DOCTOR-01 | Phase 16 | Complete |
| SKILL-01 | Phase 17 | Complete |
| SKILL-02 | Phase 17 | Complete |
| SKILL-03 | Phase 17 | Complete |
| SKILL-04 | Phase 17 | Complete |
| SKILL-05 | Phase 17 | Complete |
| SEC-01 | Phase 17 | Complete |
| CLI-01 | Phase 18 | Complete |
| CLI-02 | Phase 18 | Complete |
| CLI-03 | Phase 18 | Complete |
| CLI-04 | Phase 18 | Complete |

**Coverage:**
- v2.3 requirements: 17 total
- Mapped to phases: 17
- Unmapped: 0 ✓

---
*Requirements defined: 2026-03-26*
*Last updated: 2026-03-26 — traceability populated by roadmapper*
