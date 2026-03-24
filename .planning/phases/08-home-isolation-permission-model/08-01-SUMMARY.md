---
phase: 08-home-isolation-permission-model
plan: 01
subsystem: infra
tags: [shell-wrapper, home-isolation, claude-json, credentials, symlink, env-forwarding]

requires:
  - phase: 06-native-sandbox
    provides: per-agent .claude/settings.json codegen foundation
  - phase: 07-doctor-apparmor
    provides: bubblewrap smoke test and dependency validation

provides:
  - Shell wrapper sets HOME to agent directory before exec (D-01)
  - Shell wrapper forwards 6 identity/auth env vars before HOME override (D-02, D-03)
  - generate_agent_claude_json: per-agent .claude.json with hasTrustDialogAccepted (D-05, D-06)
  - create_credential_symlink: agent .claude/.credentials.json -> host credentials (D-07, D-08)
  - rightclaw init writes per-agent .claude.json instead of host ~/.claude.json (D-06)
  - pre_trust_directory() removed -- host ~/.claude.json writes eliminated

affects: [rightclaw-up, agent-launch, headless-operation, oauth-auth]

tech-stack:
  added: [dirs crate added to rightclaw-cli dependencies]
  patterns:
    - "Resolve host_home via dirs::home_dir() BEFORE per-agent loop (before any HOME manipulation)"
    - "Read-modify-write pattern for .claude.json preserves existing CC-written fields"
    - "Idempotent symlink creation: remove_file then symlink on each cmd_up run"
    - "Warn (not error) on missing host credentials -- ANTHROPIC_API_KEY as fallback"

key-files:
  created:
    - crates/rightclaw/src/codegen/claude_json.rs
  modified:
    - templates/agent-wrapper.sh.j2
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw/src/init.rs

key-decisions:
  - "HOME override placed AFTER all env var captures in wrapper -- avoids ~ expansion pointing to agent dir"
  - "host_home resolved once before per-agent loop, not inside it -- ensures real home dir regardless of env"
  - "pre_trust_directory() removed entirely (not just commented) -- D-06 locks direction as agent-local writes"
  - "init.rs builds AgentDef inline for trust generation -- avoids changing init function signature"

patterns-established:
  - "TDD for shell wrapper changes: write failing tests, run to confirm RED, implement, confirm GREEN"
  - "All codegen modules use read-modify-write for JSON config files (preserve existing CC state)"

requirements-completed: [HOME-01, HOME-02, HOME-03, HOME-04, PERM-01, PERM-02]

duration: 15min
completed: 2026-03-24
---

# Phase 8 Plan 01: Home Isolation and Permission Model Summary

**Shell wrapper sets HOME to agent dir with git/SSH/API key forwarding; per-agent .claude.json trust generation and credential symlink wired into cmd_up and init**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-03-24T22:00:00Z
- **Completed:** 2026-03-24T22:15:00Z
- **Tasks:** 2 (TDD)
- **Files modified:** 7

## Accomplishments

- Shell wrapper now sets `export HOME="{{ working_dir }}"` after capturing all identity env vars
- `GIT_CONFIG_GLOBAL`, `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, `ANTHROPIC_API_KEY` all forwarded with `:-` fallback for `set -u` compatibility
- New `codegen::claude_json` module with `generate_agent_claude_json()` and `create_credential_symlink()`
- `rightclaw up` now generates `.claude.json` and credential symlink per agent on every launch
- `rightclaw init` writes agent-local `.claude.json` instead of polluting host `~/.claude.json` (D-06)
- `pre_trust_directory()` removed -- host `~/.claude.json` writes eliminated

## Task Commits

1. **Task 1 (RED): shell wrapper tests** - `5519cea` (test)
2. **Task 1 (GREEN): shell wrapper template** - `3c9e3f3` (feat)
3. **Task 2: claude_json module** - `42ddd7d` (feat)
4. **Task 2: wiring in cmd_up and init** - `e5f3525` (feat)

## Files Created/Modified

- `templates/agent-wrapper.sh.j2` - Added env var capture block + `export HOME="{{ working_dir }}"` before exec
- `crates/rightclaw/src/codegen/claude_json.rs` - New module: generate_agent_claude_json + create_credential_symlink + 8 tests
- `crates/rightclaw/src/codegen/mod.rs` - Added claude_json module and re-exports
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - 6 new tests for HOME override and env forwarding
- `crates/rightclaw-cli/src/main.rs` - host_home resolution + per-agent .claude.json + symlink calls in cmd_up
- `crates/rightclaw-cli/Cargo.toml` - Added dirs workspace dependency
- `crates/rightclaw/src/init.rs` - Replaced pre_trust_directory() with generate_agent_claude_json(); removed pre_trust_directory fn

## Decisions Made

- **Ordering in shell wrapper:** env var captures MUST appear before `export HOME=` because after HOME override, any `~` expansion resolves to agent dir not host home. Tests verify ordering.
- **host_home resolved once before loop:** If HOME env var were changed inside the loop, subsequent `dirs::home_dir()` calls could return the agent dir. Resolved once before any potential side effects.
- **Remove pre_trust_directory() entirely:** D-06 is a one-way decision (agent-local writes are strictly better). Keeping dead code would be misleading.
- **init.rs builds AgentDef inline:** The `init_rightclaw_home` function creates `agents_dir` directly, building a minimal AgentDef inline is correct. No signature changes needed.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added dirs dependency to rightclaw-cli**
- **Found during:** Task 2 (wiring cmd_up)
- **Issue:** `dirs::home_dir()` used in main.rs but `dirs` not in rightclaw-cli/Cargo.toml
- **Fix:** Added `dirs = { workspace = true }` to rightclaw-cli/Cargo.toml
- **Files modified:** crates/rightclaw-cli/Cargo.toml
- **Verification:** `cargo build --workspace` succeeds
- **Committed in:** e5f3525 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking dependency)
**Impact on plan:** Necessary missing dependency. No scope creep.

## Issues Encountered

None -- plan executed smoothly. The `generate_settings` function referenced in the plan's interfaces section doesn't exist as a codegen module (settings.json is written directly in init.rs), but this had no impact -- the new calls were placed correctly after the wrapper write in cmd_up.

## Known Stubs

None -- all functionality is fully wired.

## User Setup Required

None -- no external service configuration required.

## Next Phase Readiness

- HOME isolation infrastructure complete for Phase 8 Plan 02
- Agents launched by `rightclaw up` will have:
  - HOME pointing to their agent directory
  - Forwarded git/SSH identity
  - Agent-local `.claude.json` with trust entries
  - Credential symlink to host OAuth tokens
- Warning displayed when no host OAuth credentials (ANTHROPIC_API_KEY required in that case)

---
*Phase: 08-home-isolation-permission-model*
*Completed: 2026-03-24*
