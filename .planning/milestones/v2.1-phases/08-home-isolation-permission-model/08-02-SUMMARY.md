---
phase: 08-home-isolation-permission-model
plan: 02
subsystem: infra
tags: [sandbox, home-isolation, settings-json, allow-read, deny-read, absolute-paths, integration-tests]

requires:
  - phase: 08-01
    provides: generate_agent_claude_json and create_credential_symlink codegen functions
  - phase: 06-native-sandbox
    provides: generate_settings() foundation and SandboxOverrides struct

provides:
  - generate_settings() accepts host_home parameter and uses absolute denyRead paths (HOME-05)
  - SandboxOverrides.allow_read field for user-defined read permissions
  - allowRead array in generated settings.json with agent path as default
  - Credential symlink creation wired into rightclaw init (not just cmd_up)
  - Integration test suite for HOME isolation security: home_isolation.rs

affects: [rightclaw-up, rightclaw-init, agent-sandbox-config, settings-codegen]

tech-stack:
  added: []
  patterns:
    - "host_home resolved once at function entry before any HOME manipulation (applies to both cmd_up and init_rightclaw_home)"
    - "denyRead built dynamically from host_home.join() calls -- never tilde literals"
    - "allowRead defaults to agent path alone, user allow_read overrides extend (not replace)"
    - "Belt + suspenders: deny entire host HOME in denyRead, allowRead exceptions for agent path"

key-files:
  created:
    - crates/rightclaw-cli/tests/home_isolation.rs
  modified:
    - crates/rightclaw/src/agent/types.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/init.rs
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "generate_settings() takes host_home: &Path -- callers are responsible for resolving it before HOME manipulation"
  - "denyRead denies entire host HOME (trailing slash) as belt -- allowRead[agent_path] creates exception"
  - "create_credential_symlink added to init so agent is fully OAuth-ready after init, not just after up"
  - "host_home moved to function-level in init_rightclaw_home -- eliminates second dirs::home_dir() call"

patterns-established:
  - "TDD: write failing tests, confirm RED, implement, confirm GREEN"
  - "Integration tests use --home flag to isolate filesystem, HOME env var to control host_home resolution"

requirements-completed: [HOME-05]

duration: 15min
completed: 2026-03-24
---

# Phase 8 Plan 02: HOME Isolation Path Hardening Summary

**Absolute denyRead paths via host_home parameter, allowRead for agent dir, SandboxOverrides.allow_read, and integration tests covering Plan 01 artifacts**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-03-24T22:00:00Z
- **Completed:** 2026-03-24T22:15:57Z
- **Tasks:** 2 (both TDD)
- **Files modified:** 5 + 1 created

## Accomplishments

- `generate_settings()` now takes `host_home: &Path` and builds absolute denyRead paths (no more `~/.ssh` → agent dir resolution)
- Added `SandboxOverrides.allow_read` field for user-defined additional read permissions in `agent.yaml`
- Generated `settings.json` now includes `allowRead` array with agent absolute path (required since agent dir is inside denied host HOME)
- `rightclaw init` now calls `create_credential_symlink()` so agent is OAuth-ready immediately after init
- 6 integration tests in `home_isolation.rs` covering sandbox path assertions and Plan 01 artifacts

## Task Commits

1. **Task 1 (RED+GREEN): SandboxOverrides.allow_read + absolute denyRead paths** - `9646067` (feat)
2. **Task 2 (RED+GREEN): HOME isolation integration tests + credential symlink in init** - `fec68be` (feat)

## Files Created/Modified

- `crates/rightclaw/src/agent/types.rs` - Added `allow_read: Vec<String>` field to SandboxOverrides; new tests
- `crates/rightclaw/src/codegen/settings.rs` - Removed DEFAULT_DENY_READ; added host_home parameter; dynamic absolute denyRead; allowRead with agent path
- `crates/rightclaw/src/codegen/settings_tests.rs` - Updated all calls to pass Path::new("/home/user"); added includes_allow_read_with_agent_path, deny_read_uses_absolute_paths_not_tilde, merges_user_allow_read_overrides tests; updated includes_deny_read_security_defaults to expect absolute paths
- `crates/rightclaw/src/init.rs` - Moved host_home resolution to function-level; added create_credential_symlink call; updated generate_settings call with host_home
- `crates/rightclaw-cli/src/main.rs` - Updated generate_settings call to pass &host_home
- `crates/rightclaw-cli/tests/home_isolation.rs` - New: 6 integration tests + 2 #[ignore] scaffold tests

## Decisions Made

- **create_credential_symlink in init**: Plan only specified testing that init produces the artifact, but the artifact wasn't created by init — only by cmd_up. Adding the symlink creation to init makes agents immediately ready for OAuth. Consistent with the principle that init prepares a complete, runnable agent.
- **host_home at function-level in init.rs**: The inner block `{}` scoping was unnecessary complexity. Moving host_home before the settings block means one call to dirs::home_dir() serves both generate_settings and create_credential_symlink.
- **allowRead includes entire agent path**: Since denyRead now denies the full host HOME directory (belt approach), agent path must be explicitly allowed or agent cannot read its own files.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] create_credential_symlink not called by init (test would never pass)**
- **Found during:** Task 2 (integration test for init_warns_when_host_creds_missing)
- **Issue:** Test expected `rightclaw init` to produce warning about missing credentials, but `create_credential_symlink` was only called by `cmd_up`. The test premise was correct (init should fully prepare agent) but the implementation lagged.
- **Fix:** Added `create_credential_symlink(&trust_agent, &host_home)` call in `init_rightclaw_home()`, moved `host_home` resolution to function-level to avoid duplicate `dirs::home_dir()` calls.
- **Files modified:** `crates/rightclaw/src/init.rs`
- **Verification:** All 6 integration tests pass including `init_warns_when_host_creds_missing`
- **Committed in:** fec68be (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug — missing call)
**Impact on plan:** Necessary fix — init was incomplete without credential symlink setup. Aligns with plan intent.

## Issues Encountered

- `test_status_no_running_instance` in cli_integration.rs continues to fail (pre-existing, noted in PROJECT.md). Unrelated to this plan.

## Known Stubs

None -- all functionality is fully wired.

## User Setup Required

None -- no external service configuration required.

## Next Phase Readiness

- Phase 8 is complete: HOME isolation, permission model, sandbox hardening are all wired end-to-end
- Agents launched via `rightclaw up` have: absolute denyRead paths, allowRead for agent dir, HOME pointing to agent dir, forwarded env vars, agent-local .claude.json, credential symlink
- `rightclaw init` now fully prepares an agent including credential symlink setup

---
*Phase: 08-home-isolation-permission-model*
*Completed: 2026-03-24*

## Self-Check: PASSED

- SUMMARY.md exists: FOUND
- home_isolation.rs exists: FOUND
- allow_read in types.rs: FOUND
- host_home parameter in settings.rs: FOUND
- Commit 9646067 (Task 1): FOUND
- Commit fec68be (Task 2): FOUND
