---
phase: 24-system-prompt-codegen
plan: "01"
subsystem: codegen
tags: [system-prompt, codegen, cleanup, shell-wrapper-removal]
dependency_graph:
  requires: []
  provides: [generate_system_prompt, AgentConfig-without-start_prompt]
  affects: [rightclaw-cli (Plan 02 fixes call sites)]
tech_stack:
  added: []
  patterns: [raw-content-concat, optional-file-skip]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/system_prompt.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/claude_json.rs
    - crates/rightclaw/src/codegen/telegram.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
  deleted:
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - templates/agent-wrapper.sh.j2
decisions:
  - "D-03 fallback: raw content concatenation (no template engine needed for system prompt)"
  - "D-04: all four identity files in canonical order IDENTITY→SOUL→USER→AGENTS"
  - "D-06: no hardcoded sections (Startup Instructions, rightcron, Communication, BOOTSTRAP removed)"
  - "D-09/D-10: shell_wrapper fully deleted including template"
  - "start_prompt removed with deny_unknown_fields — YAML with that field now fails to parse (fail-fast)"
metrics:
  duration_seconds: 196
  completed_date: "2026-03-31"
  tasks_completed: 2
  files_modified: 9
  files_deleted: 3
---

# Phase 24 Plan 01: System Prompt Codegen Rewrite Summary

**One-liner:** Replace `generate_combined_prompt` + shell wrapper pipeline with `generate_system_prompt` — raw content concatenation of IDENTITY/SOUL/USER/AGENTS files with no hardcoded sections.

## Tasks Completed

| # | Name | Commit | Files |
|---|------|--------|-------|
| 1 | Rewrite system_prompt.rs + new tests | 9bc192b | system_prompt.rs, system_prompt_tests.rs, mod.rs |
| 2 | Delete shell_wrapper, remove start_prompt, update mod.rs + types.rs | 7d9bb14 | 9 modified, 3 deleted |

## What Was Built

`generate_system_prompt(&AgentDef) -> miette::Result<String>`:
- Reads IDENTITY.md (required — Err if missing/unreadable)
- Optionally reads SOUL.md, USER.md, AGENTS.md (silently skipped if path is None or file missing)
- Joins present sections with `"\n\n---\n\n"`
- Zero hardcoded content — system prompt is 100% agent-owned file content

Shell wrapper pipeline removed:
- `shell_wrapper.rs` deleted (was: jinja2 template rendering, startup_prompt hardcoded string)
- `shell_wrapper_tests.rs` deleted
- `templates/agent-wrapper.sh.j2` deleted
- `generate_wrapper` removed from mod.rs exports

`AgentConfig.start_prompt` removed:
- YAML with `start_prompt` key now fails to parse (deny_unknown_fields enforcement)
- All struct literals in tests and codegen updated (settings_tests.rs, process_compose_tests.rs, claude_json.rs, telegram.rs, init.rs)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Additional start_prompt references in codegen test files**
- **Found during:** Task 2, after updating types.rs
- **Issue:** `start_prompt: None` / `start_prompt: Some(...)` in settings_tests.rs, process_compose_tests.rs, claude_json.rs, telegram.rs — would cause compile errors
- **Fix:** Removed all occurrences (sed + manual edit)
- **Files modified:** settings_tests.rs (3 lines), process_compose_tests.rs (1 line), claude_json.rs (1 line), telegram.rs (1 line)
- **Commit:** 7d9bb14

## Known Stubs

None — generate_system_prompt is fully wired and functional. CLI call sites remain broken until Plan 02 (expected state per plan spec).

## Verification

```
cargo test -p rightclaw --lib: 222 passed, 0 failed
rg "generate_combined_prompt|generate_wrapper|start_prompt" crates/rightclaw/src/ → 0 results
```

CLI will fail to build until Plan 02 fixes `rightclaw-cli/src/main.rs` call sites — this is the expected state at end of Plan 01.

## Self-Check: PASSED

- crates/rightclaw/src/codegen/system_prompt.rs: FOUND
- crates/rightclaw/src/codegen/system_prompt_tests.rs: FOUND
- crates/rightclaw/src/codegen/mod.rs: FOUND (exports generate_system_prompt, no shell_wrapper)
- shell_wrapper.rs: MISSING (correctly deleted)
- Commits 9bc192b and 7d9bb14: FOUND in git log
