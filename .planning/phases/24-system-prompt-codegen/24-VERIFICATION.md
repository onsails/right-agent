---
phase: 24-system-prompt-codegen
verified: 2026-03-31T00:00:00Z
status: passed
score: 9/9 must-haves verified
re_verification: false
human_verification:
  - test: "rightclaw up with a real agent dir writes system-prompt.txt"
    expected: "agent_dir/.claude/system-prompt.txt contains IDENTITY content followed by separator-joined optional files"
    why_human: "Requires a real agent directory layout and process-compose environment to invoke cmd_up end-to-end"
  - test: "rightclaw replay passes --system-prompt-file to the claude invocation"
    expected: "claude binary receives --system-prompt-file flag pointing to .claude/system-prompt.txt"
    why_human: "exec() replaces the process; cannot observe flag in integration test without process tracing"
---

# Phase 24: System Prompt Codegen Verification Report

**Phase Goal:** Replace the combined-prompt + shell-wrapper codegen pipeline with a lean generate_system_prompt that writes raw identity-file content to agent_dir/.claude/system-prompt.txt; remove deprecated start_prompt field from AgentConfig; deliver USER.md template and update AGENTS.md with operational guidance.
**Verified:** 2026-03-31
**Status:** PASSED
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | generate_system_prompt(&AgentDef) returns concatenated identity-file content in canonical order | VERIFIED | system_prompt.rs implements exact spec; 7 tests pass covering all ordering cases |
| 2 | Absent SOUL/USER/AGENTS files silently skipped; missing IDENTITY.md returns Err | VERIFIED | system_prompt_tests.rs: absent_optional_paths_are_silently_skipped, missing_identity_returns_err_with_path, soul_path_set_but_file_deleted_is_silently_skipped all pass |
| 3 | AgentConfig has no start_prompt field — YAML with it fails to parse | VERIFIED | rg "start_prompt" crates/ returns zero results; field removed from struct and all test literals |
| 4 | shell_wrapper.rs, shell_wrapper_tests.rs, templates/agent-wrapper.sh.j2 do not exist | VERIFIED | All three paths return "No such file or directory" |
| 5 | mod.rs exports generate_system_prompt, no generate_combined_prompt or generate_wrapper | VERIFIED | mod.rs has pub use system_prompt::generate_system_prompt; no other prompt exports present |
| 6 | rightclaw up writes agent_dir/.claude/system-prompt.txt for every agent | VERIFIED | main.rs calls generate_system_prompt then fs::write(..., "system-prompt.txt") in the per-agent loop, after create_dir_all(&claude_dir) |
| 7 | No shell wrapper files are written on rightclaw up | VERIFIED | All generate_wrapper call sites removed; no wrapper path or set_permissions calls remain |
| 8 | cmd_replay writes system-prompt.txt and passes --system-prompt-file flag | VERIFIED | Second call site in main.rs creates .claude/ dir, writes system-prompt.txt, then passes --system-prompt-file arg |
| 9 | rightclaw init creates USER.md; AGENTS.md contains operational guidance | VERIFIED | init.rs has DEFAULT_USER const, ("USER.md", DEFAULT_USER) in files array; AGENTS.md contains "Communication", "reply MCP tool", "rightcron" |

**Score:** 9/9 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| crates/rightclaw/src/codegen/system_prompt.rs | generate_system_prompt() producing raw-content concat | VERIFIED | 46 lines, correct implementation; no hardcoded sections |
| crates/rightclaw/src/codegen/system_prompt_tests.rs | Tests for new behavior | VERIFIED | 7 tests covering all specified cases, all pass |
| crates/rightclaw/src/codegen/mod.rs | Re-exports without shell_wrapper or generate_combined_prompt | VERIFIED | Exports generate_system_prompt, no deprecated exports |
| crates/rightclaw/src/agent/types.rs | AgentConfig without start_prompt | VERIFIED | Zero start_prompt occurrences in crates/ |
| crates/rightclaw-cli/src/main.rs | cmd_up and cmd_replay using generate_system_prompt | VERIFIED | Two call sites confirmed; writes system-prompt.txt; uses --system-prompt-file |
| templates/right/USER.md | Placeholder for user preferences | VERIFIED | Exists; contains 3-line placeholder comment per spec |
| templates/right/AGENTS.md | Operational guidance (rightcron + communication) | VERIFIED | Contains "Communication", "reply MCP tool", "rightcron", "/rightcron skill" |
| crates/rightclaw/src/init.rs | Creates USER.md on rightclaw init | VERIFIED | DEFAULT_USER const, files array entry, test assertion added |

Deleted (confirmed absent):
- crates/rightclaw/src/codegen/shell_wrapper.rs — MISSING (correct)
- crates/rightclaw/src/codegen/shell_wrapper_tests.rs — MISSING (correct)
- templates/agent-wrapper.sh.j2 — MISSING (correct)

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| codegen/mod.rs | codegen/system_prompt.rs | pub use system_prompt::generate_system_prompt | WIRED | Pattern confirmed in mod.rs line 16 |
| main.rs cmd_up loop | agent_dir/.claude/system-prompt.txt | generate_system_prompt + fs::write | WIRED | Call present after create_dir_all(&claude_dir) |
| main.rs cmd_replay | --system-prompt-file flag | Command::arg | WIRED | .arg("--system-prompt-file") confirmed; no --append-system-prompt-file remains |
| init.rs | templates/right/USER.md | include_str! macro + files array | WIRED | include_str!("../../../templates/right/USER.md") confirmed |

---

### Data-Flow Trace (Level 4)

Not applicable — phase produces file-writing functions (codegen), not UI components rendering dynamic data.

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Library tests (228) pass | cargo test -p rightclaw --lib | 228 passed, 0 failed | PASS |
| Workspace builds | cargo build --workspace | Finished dev profile, 0 errors | PASS |
| init tests pass (19) | cargo test -p rightclaw --lib init | 19 passed, 0 failed | PASS |
| system_prompt tests all pass | cargo test -p rightclaw --lib codegen::system_prompt | Included in 228 passed | PASS |

Note: test_status_no_running_instance in CLI integration tests fails with HTTP error vs. "No running instance" message — this is a pre-existing issue documented in project MEMORY.md and is unrelated to phase 24.

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| PROMPT-01 | 24-01, 24-02, 24-03 | rightclaw up generates agent_dir/.claude/system-prompt.txt by concatenating present files; absent files silently skipped | SATISFIED | generate_system_prompt in system_prompt.rs; fs::write in main.rs cmd_up loop; USER.md in init; REQUIREMENTS.md marked [x] |
| PROMPT-02 | 24-02 | claude -p invocations pass --system-prompt-file agent_dir/.claude/system-prompt.txt | SATISFIED | --system-prompt-file flag in cmd_replay confirmed; REQUIREMENTS.md marked [x] |
| PROMPT-03 | 24-01 | codegen/shell_wrapper.rs removed — shell wrappers no longer generated | SATISFIED | shell_wrapper.rs, shell_wrapper_tests.rs, agent-wrapper.sh.j2 all absent; zero generate_wrapper references remain |

Note: REQUIREMENTS.md marks PROMPT-03 as "Pending" in the tracking table and the checkbox is unchecked. The code state is fully complete — all three shell wrapper files are gone and zero references remain. This is a documentation inconsistency in REQUIREMENTS.md, not a code gap.

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | None found | — | — |

No TODOs, stubs, empty implementations, or hardcoded data found in phase-touched files.

---

### Human Verification Required

#### 1. End-to-end cmd_up writes system-prompt.txt

**Test:** Run rightclaw up against a real agent directory containing IDENTITY.md, SOUL.md, and USER.md. Inspect agent_dir/.claude/system-prompt.txt.
**Expected:** File exists with IDENTITY content, "\n\n---\n\n" separator, SOUL content, "\n\n---\n\n", USER content. No "Startup Instructions", "rightcron", or "BOOTSTRAP" text unless those strings appear in the source identity files.
**Why human:** Requires process-compose and a provisioned agent directory; cannot invoke cmd_up in unit tests.

#### 2. cmd_replay passes --system-prompt-file to claude

**Test:** Trace a rightclaw replay invocation and observe the exec'd claude command line.
**Expected:** --system-prompt-file /path/to/agent/.claude/system-prompt.txt is present in the args; --append-system-prompt-file is absent.
**Why human:** exec() replaces the process — the flag is visible only in process tracing, not test output.

---

### Gaps Summary

No gaps. All 9 observable truths verified against the actual codebase. All artifacts exist, are substantive, and are correctly wired. The workspace builds cleanly and 228 library tests pass. REQUIREMENTS.md has a minor documentation inconsistency (PROMPT-03 tracking row shows "Pending") that does not reflect an implementation gap — the code is complete.

---

_Verified: 2026-03-31_
_Verifier: Claude (gsd-verifier)_
