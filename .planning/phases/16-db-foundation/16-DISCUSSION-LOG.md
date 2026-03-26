# Phase 16: DB Foundation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-26
**Phase:** 16-db-foundation
**Areas discussed:** DB creation failure, Schema topology, SEC-02 enforcement, MEMORY.md cleanup (discovered during discussion)

---

## DB Creation Failure

| Option | Description | Selected |
|--------|-------------|----------|
| Non-fatal warn + continue | Consistent with git init pattern — agent starts without memory, operator sees warning | |
| Fatal error — block startup | Guarantees memory.db invariant before agents launch, same as settings.json | ✓ |

**User's choice:** Fatal error
**Notes:** User explicitly chose the stricter invariant. Error message follows settings.json pattern: `"failed to open memory database for '{agent}': {err:#}"`.

---

## Schema Topology

| Option | Description | Selected |
|--------|-------------|----------|
| Single table + soft-delete | `memories` with `deleted_at` column; simple queries, one table | |
| Two tables: memories + memory_events | Proper event sourcing; append-only audit log with ABORT triggers | ✓ |

**User's choice:** Two tables (memories + memory_events)
**Notes:** User asked for explanation before deciding. After understanding the trade-off (query simplicity vs. architectural correctness), chose correctness. Reasoning: "retrofitting event sourcing later is painful." Key point: `/forget` is the only mutation, so single-table covers all actual operations — but two-table is more correct and extensible.

---

## SEC-02 Enforcement

| Option | Description | Selected |
|--------|-------------|----------|
| Grep test in CI | Unit test scanning .rs files for cross-writes between memory.db and MEMORY.md | |
| Passive — code review only | Trust convention, no automated check | |
| Type-level enforcement | Separate MemoryStore / AgentFiles types that cannot cross-reference | |
| Dropped — architecture enforces | No shared code paths exist between the two systems | ✓ |

**User's choice:** Dropped
**Notes:** Multi-turn discussion. User asked "what's the purpose of this test?" — needed explanation of the MINJA attack vector. After discussion, concluded: (1) the memory skill is bash, not Rust, so type-level enforcement doesn't cover the main threat; (2) the grep test only guards against something unlikely to happen by accident; (3) architectural separation is the real enforcement. Requirement dropped.

---

## MEMORY.md Cleanup (discovered during discussion)

Not a pre-identified gray area — emerged from user questioning MEMORY.md's role in the codebase.

| Option | Description | Selected |
|--------|-------------|----------|
| Keep MEMORY.md references | Leave existing default start_prompt and memory_path field | |
| Remove MEMORY.md references + fold into Phase 16 | Remove dead memory_path field, change default start_prompt to "You are starting." | ✓ |

**User's choice:** Fold cleanup into Phase 16
**Notes:** User asked "do we have MEMORY.md anywhere in the system now? because we should not." Investigation revealed: (1) `memory_path` in AgentDef is dead code — discovered but never used; (2) default start_prompt in system_prompt.rs:16 tells agents to "Read your MEMORY.md to restore context." IronClaw analysis confirmed MEMORY.md is CC-native and SQLite is on-demand only — no reason to manage MEMORY.md ourselves. User agreed to fold cleanup into Phase 16.

---

## Claude's Discretion

- FTS5 content table design (triggers for sync) — standard rusqlite_migration pattern, no user input needed
- `rusqlite 0.39` bundled feature — locked by research, no discussion needed
- Module file structure (`error.rs`, `migrations.rs`, `store.rs`, `mod.rs`) — standard Rust module layout

## Deferred Ideas

- Phase 17: Update default start_prompt to reference `/recall` once skill exists
- Phase 17: SEC-01 injection scanning — needs research pass before implementation
