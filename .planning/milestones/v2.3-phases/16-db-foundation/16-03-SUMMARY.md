---
phase: 16-db-foundation
plan: "03"
subsystem: memory-wiring
tags: [sqlite, doctor, cmd_up, memory, db-foundation]
dependency_graph:
  requires: [16-01, 16-02]
  provides: [DB-01-complete, DOCTOR-01-complete]
  affects: [rightclaw-cli/cmd_up, rightclaw/doctor]
tech_stack:
  added: []
  patterns: [inline-Warn-override, step-10-agent-loop]
key_files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/doctor.rs
decisions:
  - "sqlite3 check uses inline Warn override pattern (not a helper fn) — matches RESEARCH.md Pattern 5, consistent with git Warn pattern from Phase 9"
  - "fix field for sqlite3 is None via check_binary(name, None) — no override needed for fix, only status"
metrics:
  duration_seconds: 90
  tasks_completed: 2
  files_modified: 2
  completed_date: "2026-03-26"
requirements: [DB-01, DOCTOR-01]
---

# Phase 16 Plan 03: Wire open_db + sqlite3 Doctor Check Summary

Wire `open_db` into `cmd_up` step 10 and add sqlite3 Warn check to `run_doctor` — pure integration connecting Plan 01's memory module to the runtime.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Wire open_db into cmd_up step 10 | 101df25 | crates/rightclaw-cli/src/main.rs |
| 2 | Add sqlite3 Warn check to run_doctor | 0a89db1 | crates/rightclaw/src/doctor.rs |

## What Was Built

**Task 1:** Inserted step 10 into the per-agent loop in `cmd_up`. After `settings.local.json` scaffold (step 9), calls `rightclaw::memory::open_db(&agent.path)` with a fatal `miette!` error if DB creation fails. Error format matches the established pattern: `"failed to open memory database for '{agent}': {e:#}"`.

**Task 2:** Added sqlite3 binary check to `run_doctor()` using inline Warn override pattern. Calls `check_binary("sqlite3", None)` (no fix hint), then overrides status to `Warn` when binary is absent — never `Fail`. Two tests added: `run_doctor_includes_sqlite3_check` (verifies presence + Pass/Warn + no fix) and `sqlite3_check_is_warn_not_fail_when_absent` (verifies the override logic directly). All 23 doctor tests pass.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None.

## Verification Results

All plan verification checks pass:

1. `cargo build --workspace` — exits 0
2. `cargo test --workspace` — 19 passed, 1 pre-existing failure (`test_status_no_running_instance` — documented known issue in PROJECT.md)
3. `rg "rightclaw::memory::open_db" crates/rightclaw-cli/src/main.rs` — 1 match
4. `rg "sqlite3" crates/rightclaw/src/doctor.rs` — appears in run_doctor and tests
5. `rg "memory_path" crates/` — zero matches
6. `rg "MEMORY\.md" crates/rightclaw/src/codegen/system_prompt.rs` — zero matches
7. `rg "You are starting\." crates/rightclaw/src/codegen/system_prompt.rs` — 1 match
8. `rg "USING fts5" crates/rightclaw/src/memory/sql/v1_schema.sql` — 1 match
9. `rg "RAISE.ABORT" crates/rightclaw/src/memory/sql/v1_schema.sql` — 2 matches

## Self-Check: PASSED

Files exist:
- `crates/rightclaw-cli/src/main.rs` — FOUND
- `crates/rightclaw/src/doctor.rs` — FOUND

Commits exist:
- `101df25` — FOUND (feat(16-03): wire open_db into cmd_up step 10)
- `0a89db1` — FOUND (feat(16-03): add sqlite3 Warn check to run_doctor)
