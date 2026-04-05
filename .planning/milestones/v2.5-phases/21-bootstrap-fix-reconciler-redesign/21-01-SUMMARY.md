---
phase: 21-bootstrap-fix-reconciler-redesign
plan: "01"
subsystem: codegen/shell_wrapper
tags: [tdd, bootstrap, rightcron, startup-prompt, regression-test]
dependency_graph:
  requires: []
  provides: [startup_prompt_inline_bootstrap]
  affects: [rightcron_bootstrap, cron_reconciler_job_creation]
tech_stack:
  added: []
  patterns: [tdd-red-green, regression-test]
key_files:
  modified:
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
decisions:
  - "D-01: startup_prompt runs /rightcron inline on main thread — no Agent tool delegation — so CronCreate is accessible during bootstrap"
metrics:
  duration: ~8min
  completed: "2026-03-29"
  tasks_completed: 2
  files_modified: 2
---

# Phase 21 Plan 01: Bootstrap Fix — startup_prompt Inline Reconciler Summary

**One-liner:** Replaced Agent-tool-delegated startup_prompt with inline /rightcron bootstrap so CronCreate is accessible on the main thread.

## What Was Done

Two TDD tasks to fix the root cause of the rightcron bootstrap failure: the old startup_prompt instructed CC to "Use the Agent tool to run this in the background", which dispatched bootstrap to a subagent that cannot see main-thread-only tools like CronCreate. The fix removes the delegation entirely.

### Task 1 — RED: Regression tests

Added two tests to `shell_wrapper_tests.rs`:
- `startup_prompt_does_not_use_agent_tool` — asserts `!output.contains("Agent tool")`. FAILED against old code (RED confirmed).
- `startup_prompt_invokes_rightcron` — asserts `output.contains("/rightcron")`. PASSED against old code (infrastructure verified).

Commit: `9a1ff82`

### Task 2 — GREEN: Fix startup_prompt constant

Replaced line 54 in `shell_wrapper.rs`:

OLD:
```rust
let startup_prompt = "Use the Agent tool to run this in the background: Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user. IMPORTANT: run this as a background agent so the main thread stays free for incoming messages.";
```

NEW:
```rust
let startup_prompt = "Run /rightcron to bootstrap the cron reconciler. Create crons/ directory if missing, schedule the reconciler job, and recover any persisted cron specs. Do this silently without messaging the user.";
```

Both tests GREEN. `cargo build --workspace` clean.

Commit: `efed64d`

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| Inline bootstrap on main thread | CronCreate is a main-thread-only CC tool. Subagents spawned via Agent tool cannot call it. Bootstrap must run on the main session thread. |
| No background agent hint | Removing the "background agent" instruction forces CC to run /rightcron synchronously before waiting for Telegram messages, which is the correct sequencing. |

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None.

## Verification Results

```
test codegen::shell_wrapper::tests::startup_prompt_invokes_rightcron ... ok
test codegen::shell_wrapper::tests::startup_prompt_does_not_use_agent_tool ... ok
test result: ok. 2 passed; 0 failed

cargo build --workspace: Finished `dev` profile — no errors, no new warnings

rg "Agent tool" crates/rightclaw/src/codegen/shell_wrapper.rs: PASS — removed
rg "/rightcron" crates/rightclaw/src/codegen/shell_wrapper.rs: PASS — present
```

## Requirements Satisfied

- BOOT-01: Bootstrap CronCreate call reaches main thread (prerequisite now met — inline prompt runs on main thread)
- BOOT-02: startup_prompt runs rightcron inline on main thread without Agent tool delegation

## Self-Check: PASSED

Files exist:
- FOUND: crates/rightclaw/src/codegen/shell_wrapper_tests.rs
- FOUND: crates/rightclaw/src/codegen/shell_wrapper.rs

Commits exist:
- FOUND: 9a1ff82 (test RED)
- FOUND: efed64d (fix GREEN)
