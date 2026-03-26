# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- 🚧 **v2.3 Memory System** - Phases 16-18 (in progress)

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

### v2.3 Memory System (In Progress)

**Milestone Goal:** Give each agent a per-agent SQLite-backed memory store — persistent across restarts, queryable via a built-in skill, and inspectable from the CLI.

- [x] **Phase 16: DB Foundation** - Per-agent SQLite module with append-only schema, WAL mode, migrations, security invariants, and doctor check (completed 2026-03-26)
- [ ] **Phase 17: Memory Skill** - `rightmemory` built-in skill with /remember /recall /search /forget and injection scanning
- [ ] **Phase 18: CLI Inspection** - `rightclaw memory` subcommand with list, search, delete, stats

## Phase Details

### Phase 16: DB Foundation
**Goal**: Every agent has a correctly-structured, safe SQLite memory database ready for use on first `rightclaw up`
**Depends on**: Phase 15 (v2.2 complete)
**Requirements**: DB-01, DB-02, DB-03, DB-04, SEC-02, SEC-03, DOCTOR-01
**Success Criteria** (what must be TRUE):
  1. Running `rightclaw up` on a new agent creates `memory.db` in the agent dir with WAL mode and 5000ms busy_timeout active
  2. Attempting `UPDATE` or `DELETE` on `memory_events` raises SQLite ABORT — the audit trail cannot be mutated
  3. FTS5 virtual table exists in the schema from day one; `user_version` pragma reflects correct migration version
  4. No code path in `cmd_up` or any skill touches `MEMORY.md` — verified by grep/test
  5. `rightclaw doctor` warns (non-fatal) when `sqlite3` binary is absent from PATH
**Plans**: 3 plans

Plans:
- [x] 16-01-PLAN.md — memory module: rusqlite deps, error type, V1 SQL schema, open_db(), 9 unit tests
- [x] 16-02-PLAN.md — dead code removal: memory_path from AgentDef, MEMORY.md scan, stale default start_prompt
- [x] 16-03-PLAN.md — integration: open_db wired into cmd_up step 10, sqlite3 Warn check in doctor

### Phase 17: Memory Skill
**Goal**: Agents can store, retrieve, search, and forget memories via built-in slash commands
**Depends on**: Phase 16
**Requirements**: SKILL-01, SKILL-02, SKILL-03, SKILL-04, SKILL-05, SEC-01
**Success Criteria** (what must be TRUE):
  1. Agent session has `/remember`, `/recall`, `/search`, and `/forget` commands available via the `rightmemory` built-in skill installed on every `rightclaw up`
  2. `/remember` records `stored_by` (agent name) and `source_tool` provenance automatically — no manual input required
  3. `/forget <id>` excludes the entry from all subsequent `/recall` and `/search` results while preserving the audit row in `memory_events`
  4. `/remember` rejects entries matching prompt injection patterns and returns an error message — the write is not persisted
**Plans**: TBD

### Phase 18: CLI Inspection
**Goal**: Operators can inspect, query, and manage any agent's memory database from the terminal without entering an agent session
**Depends on**: Phase 16
**Requirements**: CLI-01, CLI-02, CLI-03, CLI-04
**Success Criteria** (what must be TRUE):
  1. `rightclaw memory list <agent>` prints a paginated table of memories with timestamps and stored_by provenance
  2. `rightclaw memory search <agent> <query>` returns results ranked by the same FTS5 index used by the skill
  3. `rightclaw memory delete <agent> <id>` hard-deletes an entry (operator bypass of soft-delete) with a confirmation prompt
  4. `rightclaw memory stats <agent>` reports DB size on disk, total entry count, and oldest/newest entry timestamps
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 16 → 17 → 18

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 16. DB Foundation | v2.3 | 3/3 | Complete    | 2026-03-26 |
| 17. Memory Skill | v2.3 | 0/? | Not started | - |
| 18. CLI Inspection | v2.3 | 0/? | Not started | - |
