---
phase: 40-wire-cloudflared-into-process-compose
plan: 01
subsystem: infra
tags: [cloudflared, process-compose, minijinja, tunnel, which]

# Dependency graph
requires:
  - phase: 39-cloudflared-auto-tunnel
    provides: cloudflared-start.sh script generation and cloudflared_script_path variable in cmd_up
  - phase: 38-tunnel-refactor
    provides: TunnelConfig struct and global config reading
provides:
  - generate_process_compose with cloudflared_script: Option<&Path> 4th parameter
  - process-compose.yaml.j2 conditional cloudflared process block with on_failure restart
  - cmd_up pre-flight which::which("cloudflared") check gated on TunnelConfig presence
  - Two new tests covering with/without tunnel cases (20 total in process_compose_tests.rs)
affects: [cmd_up, process-compose-template, cloudflared-tunnel, agent-launch]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Optional process injection: None maps to Jinja2 null (falsy), suppresses conditional block cleanly"
    - "Pre-flight gate: which::which check before file generation prevents stale artifacts on missing binary"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/process_compose.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - templates/process-compose.yaml.j2
    - crates/rightclaw-cli/src/main.rs

key-decisions:
  - "working_dir for cloudflared = script.parent().parent() (scripts/ -> home/) — matches rightclaw home dir"
  - "minijinja serializes Option::None as null which is falsy — no explicit None check needed in template"
  - "Pre-flight fires before any file writes so no stale artifacts are left when binary is absent"

patterns-established:
  - "4-arg generate_process_compose: all call sites pass None as trailing arg when no tunnel configured"

requirements-completed: [TUNL-02]

# Metrics
duration: 12min
completed: 2026-04-05
---

# Phase 40 Plan 01: Wire Cloudflared into Process-Compose Summary

**cloudflared spawned as persistent process-compose entry via conditional Jinja2 block, wired from cmd_up with pre-flight PATH check**

## Performance

- **Duration:** ~12 min
- **Started:** 2026-04-05T13:00:00Z
- **Completed:** 2026-04-05T13:12:00Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments

- Extended `generate_process_compose` with `cloudflared_script: Option<&Path>` — builds `CloudflaredEntry` struct from script path (working_dir = parent of scripts/ dir)
- Added conditional `{% if cloudflared %}` block to process-compose.yaml.j2 with on_failure restart, backoff 5s, max_restarts 10, signal 15, timeout 30s
- Removed `let _ = cloudflared_script_path` suppression in cmd_up; now passes `cloudflared_script_path.as_deref()` to codegen
- Added `which::which("cloudflared")` pre-flight check in cmd_up, gated on `tunnel.is_some()`, before any file writes
- Updated all 18 existing test call sites from 3-arg to 4-arg form; added 2 new cloudflared tests (20 total, all passing)

## Task Commits

1. **Task 1: Add CloudflaredEntry, extend signature, template, tests** - `a978e65` (feat)
2. **Task 2: Wire pre-flight check and passthrough in cmd_up** - `665fc4e` (feat)

## Files Created/Modified

- `crates/rightclaw/src/codegen/process_compose.rs` - Added CloudflaredEntry struct, updated signature to 4-arg, build cf_entry, pass to render context
- `crates/rightclaw/src/codegen/process_compose_tests.rs` - Updated 18 existing call sites to 4-arg; added 2 new cloudflared tests
- `templates/process-compose.yaml.j2` - Added conditional cloudflared process block after agents loop
- `crates/rightclaw-cli/src/main.rs` - Added pre-flight which check, removed suppression, passed script path to codegen

## Decisions Made

- `working_dir` for cloudflared process = `script.parent().parent()` (scripts/ -> rightclaw home) — this is the natural working dir for the cloudflared process alongside its config file
- minijinja serializes `Option::None` as `null` which is falsy in Jinja2 — the `{% if cloudflared %}` guard suppresses the block correctly with no extra template logic
- Pre-flight check placed before file generation block to ensure no stale artifacts when cloudflared is absent from PATH

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `rightclaw up` now spawns cloudflared as a process-compose process when TunnelConfig is present
- The full tunnel lifecycle (generate config -> write script -> spawn process) is complete
- Phase 39 UAT gap (cloudflared script suppressed) is closed

---
*Phase: 40-wire-cloudflared-into-process-compose*
*Completed: 2026-04-05*
