---
phase: 24-system-prompt-codegen
plan: "02"
subsystem: cli
tags: [system-prompt, codegen, shell-wrapper-removal, cmd_up, cmd_pair]
dependency_graph:
  requires: [24-01 (generate_system_prompt API)]
  provides: [cmd_up writes system-prompt.txt, cmd_pair uses --system-prompt-file]
  affects: [rightclaw-cli/src/main.rs]
tech_stack:
  added: []
  patterns: [generate-then-write, exec-replacement-writes-own-files]
key_files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/bot/src/lib.rs
decisions:
  - "D-10/D-11: cmd_up now writes agent_dir/.claude/system-prompt.txt via generate_system_prompt; no run/<agent>-prompt.md written"
  - "D-12: process-compose template still references wrapper_path (stale until Phase 26)"
  - "cmd_pair writes system-prompt.txt itself before exec for standalone correctness"
metrics:
  duration_seconds: 155
  completed_date: "2026-03-31"
  tasks_completed: 2
  files_modified: 2
---

# Phase 24 Plan 02: CLI Call Sites Update Summary

**One-liner:** Wire `generate_system_prompt` into `cmd_up` and `cmd_pair` — system-prompt.txt now written to `agent_dir/.claude/` on every launch, shell wrappers and intermediate prompt files eliminated.

## Tasks Completed

| # | Name | Commit | Files |
|---|------|--------|-------|
| 1 | Replace wrapper codegen block in cmd_up loop | 95f4550 | main.rs (cmd_up loop) |
| 2 | Update cmd_pair — write system-prompt.txt, switch to --system-prompt-file | 95f4550 | main.rs (cmd_pair) |

## What Was Built

**cmd_up changes:**
- Removed entire `generate_combined_prompt` + `generate_wrapper` + `set_permissions` block (lines ~426-456)
- Added `generate_system_prompt(agent)` call placed AFTER `create_dir_all(&claude_dir)` (guarantees dir exists)
- Writes to `agent.path/.claude/system-prompt.txt`
- No `run/<agent>-prompt.md` or `run/<agent>.sh` written to disk

**cmd_pair changes:**
- Removed `run_dir` creation and old combined-prompt write
- Added `create_dir_all(&claude_dir)` + `generate_system_prompt(agent)` + write to `system-prompt.txt`
- Changed `--append-system-prompt-file` to `--system-prompt-file`
- Function is now self-sufficient for standalone invocations (no prior `rightclaw up` required)

**Cleanup:**
- Removed unused `use std::os::unix::fs::PermissionsExt` import
- Prefixed `debug` parameter with `_debug` (no longer used after wrapper removal)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] start_prompt: None in crates/bot/src/lib.rs**
- **Found during:** Task 1, during cargo build --workspace
- **Issue:** `crates/bot/src/lib.rs:68` had `start_prompt: None` in an `AgentConfig` struct literal. Plan 01 removed `start_prompt` from `AgentConfig` but missed this file (it was outside `crates/rightclaw/`).
- **Fix:** Removed the `start_prompt: None` line from the fallback AgentConfig literal in bot/src/lib.rs
- **Files modified:** crates/bot/src/lib.rs
- **Commit:** 95f4550 (bundled in same commit)

## Known Stubs

- `process-compose` template still passes `wrapper_path` (stale shell wrapper path). Intentional per D-12 — Phase 26 will update the template for direct claude invocation.

## Verification

```
cargo build --workspace: 0 errors, 0 warnings
cargo test --workspace: 19 passed, 1 failed (test_status_no_running_instance — pre-existing)
rg "generate_combined_prompt|generate_wrapper" crates/rightclaw-cli/src/main.rs → 0 results
rg "append-system-prompt-file" crates/rightclaw-cli/src/main.rs → 0 results
rg "system-prompt.txt" crates/rightclaw-cli/src/main.rs → 6 matches (both call sites)
rg "generate_system_prompt" crates/rightclaw-cli/src/main.rs → 2 matches
```

## Self-Check: PASSED

- crates/rightclaw-cli/src/main.rs: FOUND and modified
- crates/bot/src/lib.rs: FOUND and modified
- Commit 95f4550: FOUND in git log
