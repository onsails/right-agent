---
phase: 29-sandbox-dependency-fix
plan: 01
subsystem: sandbox
tags: [ripgrep, sandbox, nix, settings-json, devenv]

requires:
  - phase: 06-sandbox-configuration
    provides: generate_settings function and settings.json codegen
provides:
  - sandbox.failIfUnavailable: true in all generated settings.json
  - sandbox.ripgrep.command injection with resolved system rg path
  - USE_BUILTIN_RIPGREP corrected to "0" in CC subprocess invocations
  - pkgs.ripgrep in devenv.nix
affects: [sandbox-configuration, bot-telegram, cron-runtime]

tech-stack:
  added: []
  patterns: [rg_path resolution once in cmd_up before per-agent loop, conditional sandbox field injection]

key-files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/init.rs
    - crates/bot/src/telegram/worker.rs
    - crates/bot/src/cron.rs
    - devenv.nix

key-decisions:
  - "D-01: generate_settings gains rg_path: Option<PathBuf> — keeps settings.rs pure (no IO)"
  - "D-04: failIfUnavailable: true unconditional — even with --no-sandbox"
  - "D-08: All 4 fix sites committed atomically to prevent broken intermediate state"

patterns-established:
  - "Resolve external tool paths once in cmd_up, pass to codegen functions"

requirements-completed: [SBOX-01, SBOX-02, SBOX-03, SBOX-04]

duration: 4min
completed: 2026-04-02
---

# Phase 29 Plan 01: Sandbox Dependency Fix Summary

**Fix CC sandbox silent disable in nix by injecting system rg path, failIfUnavailable flag, correcting USE_BUILTIN_RIPGREP polarity, and adding ripgrep to devenv**

## Performance

- **Duration:** 4 min
- **Started:** 2026-04-02T19:45:46Z
- **Completed:** 2026-04-02T19:49:20Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments
- generate_settings now accepts rg_path parameter and injects sandbox.ripgrep.command when system rg is available
- sandbox.failIfUnavailable: true added unconditionally to all generated settings.json -- sandbox failures are now fatal, not silent
- USE_BUILTIN_RIPGREP corrected from "1" (use vendored, broken in nix) to "0" (use system rg) in both worker.rs and cron.rs
- pkgs.ripgrep added to devenv.nix ensuring rg is in PATH for all dev sessions
- 3 new tests covering failIfUnavailable, ripgrep injection, and ripgrep omission

## Task Commits

Both tasks committed atomically per D-08 requirement:

1. **Task 1+2: Sandbox dependency detection fix** - `6313653` (fix)

## Files Created/Modified
- `crates/rightclaw/src/codegen/settings.rs` - Added rg_path parameter, failIfUnavailable, ripgrep.command injection
- `crates/rightclaw/src/codegen/settings_tests.rs` - Updated 11 existing calls to 4-arg, added 3 new tests
- `crates/rightclaw-cli/src/main.rs` - which::which("rg") resolution before per-agent loop
- `crates/rightclaw/src/init.rs` - Pass None as rg_path for template generation
- `crates/bot/src/telegram/worker.rs` - USE_BUILTIN_RIPGREP "1" -> "0" with documentation
- `crates/bot/src/cron.rs` - USE_BUILTIN_RIPGREP "1" -> "0" with documentation
- `devenv.nix` - Added pkgs.ripgrep to packages list

## Decisions Made
- All decisions were locked by D-01 through D-08. No discretionary decisions needed.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All SBOX requirements complete
- Sandbox will now fail loudly instead of silently degrading in nix environments
- System ripgrep path resolved at rightclaw up time and injected per-agent

---
*Phase: 29-sandbox-dependency-fix*
*Completed: 2026-04-02*

## Self-Check: PASSED
