---
phase: 11-env-var-injection
plan: "01"
subsystem: codegen
tags: [env-injection, shell-wrapper, tdd, quoting]
dependency_graph:
  requires: []
  provides: [AgentConfig.env, shell_single_quote_escape, env_exports]
  affects: [agent-wrapper.sh.j2, generate_wrapper]
tech_stack:
  added: []
  patterns: [single-quote bash escaping, minijinja context injection]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - templates/agent-wrapper.sh.j2
    - crates/rightclaw/src/codegen/claude_json.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs
    - crates/rightclaw/src/codegen/telegram.rs
decisions:
  - "Single-quote escaping via replace('\\'', \"'\\\\''\"): safe for $, backticks, spaces without shell expansion"
  - "env_exports built as Vec<String> in Rust before template rendering — pre-formatted export lines"
  - "env: block positioned between ANTHROPIC_API_KEY capture and export HOME= override (D-03)"
  - "Agent.env defaults to empty HashMap via #[serde(default)] — no extra exports when unset"
metrics:
  duration: "~10min"
  completed: "2026-03-25T22:45:30Z"
  tasks_completed: 2
  files_modified: 9
requirements: [ENV-01, ENV-02, ENV-03]
requirements-completed: [ENV-01, ENV-02, ENV-03]
---

# Phase 11 Plan 01: Env Var Injection Summary

**One-liner:** Per-agent env vars declared in agent.yaml are single-quote escaped and injected between identity captures and HOME override in the generated wrapper.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Write failing quoting tests (RED) | 5a589a4 | shell_wrapper_tests.rs |
| 2 | Implement env field, injection, template (GREEN) | 242ff6e | types.rs, shell_wrapper.rs, agent-wrapper.sh.j2, 6 other test files |

## What Was Built

- `AgentConfig.env: HashMap<String, String>` with `#[serde(default)]` — zero-friction addition to agent.yaml
- `shell_single_quote_escape()` private helper — replaces `'` with `'\''` for safe bash single-quoting
- `env_exports: Vec<String>` built in `generate_wrapper()` as pre-formatted `export KEY='value'` lines
- Template block in `agent-wrapper.sh.j2`: emitted only when env_exports is non-empty, positioned after ANTHROPIC_API_KEY and before HOME override

## Test Coverage

6 new `wrapper_env_*` tests:

- `wrapper_env_basic` — basic key=value with spaces works
- `wrapper_env_single_quote_escape` — single-quote in value escaped as `'\''`
- `wrapper_env_special_chars` — `$`, backticks treated as literals
- `wrapper_env_before_home` — position ordering verified
- `wrapper_no_env_no_exports` — no extra exports when env: absent
- `wrapper_env_empty_value` — empty string produces `export KEY=''`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Worktree stale — rebased onto current master**

- **Found during:** Initial setup
- **Issue:** Worktree branch `worktree-agent-ac9f860e` was at `5ea70d1` (pre-Phase-8), missing identity captures + HOME override template changes from Phases 8-11
- **Fix:** `git rebase master` — fast-forward rebase, no conflicts
- **Impact:** Template now has correct structure for env injection

**2. [Rule 1 - Bug] `rposition` requires `ExactSizeIterator` — str::Lines doesn't implement it**

- **Found during:** Task 1 test writing
- **Issue:** `output.lines().rposition(...)` fails to compile — `Lines<'_>` is not `ExactSizeIterator`
- **Fix:** Collect lines to `Vec<&str>` first, then use `rposition` on the vec
- **Commit:** 5a589a4

**3. [Rule 3 - Blocking] 6 additional AgentConfig struct initializations in other test/source files**

- **Found during:** Task 2 compilation
- **Issue:** Adding `env` field to `AgentConfig` caused E0063 (missing field) in `claude_json.rs`, `process_compose_tests.rs`, `settings_tests.rs`, `system_prompt_tests.rs`, `telegram.rs`
- **Fix:** Added `env: std::collections::HashMap::new()` to each struct initialization
- **Files modified:** claude_json.rs, process_compose_tests.rs, settings_tests.rs, system_prompt_tests.rs, telegram.rs
- **Commit:** 242ff6e

## Known Stubs

None — all env: values flow through to wrapper generation. Template injection is fully wired.

## Pre-existing Failures

- `test_status_no_running_instance` in `rightclaw-cli` integration tests — pre-existing HTTP error (documented in MEMORY.md), unrelated to this plan.

## Self-Check: PASSED

- `pub env: HashMap<String, String>` in types.rs: FOUND
- `env_exports` in shell_wrapper.rs: FOUND
- `env_exports` in agent-wrapper.sh.j2: FOUND
- Task 1 commit 5a589a4: FOUND
- Task 2 commit 242ff6e: FOUND
- All 6 wrapper_env_* tests pass: CONFIRMED (cargo test -p rightclaw shell_wrapper → 19 passed)
