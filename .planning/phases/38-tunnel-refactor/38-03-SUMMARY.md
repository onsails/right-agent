---
phase: 38-tunnel-refactor
plan: 03
subsystem: infra
tags: [cloudflare-tunnel, doctor, codegen, tdd]

# Dependency graph
requires:
  - "38-01: TunnelConfig with credentials_file/tunnel_uuid fields"
provides:
  - "doctor check_tunnel_credentials_file — verifies credentials file path exists on disk"
  - "CloudflaredCredentials struct + generate_cloudflared_config with credentials param"
  - "cloudflared-config.yml.j2 template (tunnel:/credentials-file: conditional)"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Tunnel doctor check uses credentials_file.exists() — no JWT decode, pure filesystem check"
    - "generate_cloudflared_config accepts Option<&CloudflaredCredentials> — always Some in Phase 38+ usage"

key-files:
  created:
    - crates/rightclaw/src/codegen/cloudflared.rs
    - crates/rightclaw/src/codegen/cloudflared_tests.rs
    - templates/cloudflared-config.yml.j2
  modified:
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs
    - crates/rightclaw/src/codegen/mod.rs

key-decisions:
  - "cloudflared.rs was deleted in f37e9da mega-cleanup commit — restored as part of this plan (Rule 3: missing referenced file)"
  - "check_tunnel_config fix hint not updated — function doesn't exist in codebase (was never added to this branch)"

patterns-established:
  - "Tunnel doctor checks are credentials-file-only — no JWT decode path remains"

requirements-completed: [TUNL-01]

# Metrics
duration: 12min
completed: 2026-04-05
---

# Phase 38 Plan 03: Doctor + Cloudflared Codegen Update Summary

**Updated doctor.rs with credentials-file existence check replacing JWT token check; restored cloudflared.rs + template deleted in prior cleanup, adding two Phase 38 credential tests.**

## Performance

- **Duration:** ~12 min
- **Started:** 2026-04-05T01:32:00Z
- **Completed:** 2026-04-05T01:44:03Z
- **Tasks:** 2 (Task 1 TDD, Task 2 create+test)
- **Files modified:** 3 modified, 3 created

## Accomplishments

- Added `check_tunnel_credentials_file()` to `doctor.rs` — Pass when file exists at stored path, Warn when missing with fix hint pointing to `--tunnel-credentials-file`
- Wired into `run_doctor()` — fires only when tunnel is configured in global config
- 3 TDD tests: pass-when-exists, warn-when-missing, no-tunnel-token-check-in-run-doctor
- Restored `codegen/cloudflared.rs` with `CloudflaredCredentials` struct and `generate_cloudflared_config(agents, hostname, Option<&CloudflaredCredentials>)`
- Restored `templates/cloudflared-config.yml.j2` with conditional `tunnel:`/`credentials-file:` block
- Restored `cloudflared_tests.rs` with 8 pre-existing tests + 2 new Phase 38 tests: `credentials_embedded_when_provided` and `no_credentials_section_when_none`
- Registered `pub mod cloudflared` in `codegen/mod.rs`
- All 277 rightclaw package tests pass; 1 pre-existing CLI integration failure (`test_status_no_running_instance`) unrelated to this plan

## Task Commits

1. **Task 1: doctor.rs tunnel credentials check** — `b192d33` (feat)
2. **Task 2: cloudflared.rs + tests + template** — `4434824` (feat)

## Files Created/Modified

- `crates/rightclaw/src/doctor.rs` — `check_tunnel_credentials_file()` + `run_doctor()` call
- `crates/rightclaw/src/doctor_tests.rs` — 3 new TDD tests
- `crates/rightclaw/src/codegen/cloudflared.rs` — `CloudflaredCredentials` struct + `generate_cloudflared_config`
- `crates/rightclaw/src/codegen/cloudflared_tests.rs` — 10 tests (8 regression + 2 new)
- `crates/rightclaw/src/codegen/mod.rs` — added `pub mod cloudflared`
- `templates/cloudflared-config.yml.j2` — conditional tunnel/credentials-file block

## Decisions Made

- cloudflared.rs and its template were deleted in the f37e9da mega-cleanup (which also pruned the OAuth flow, MCP creds, and 80+ other files). Since the plan depends on these files, recreated them from git history (Rule 3 deviation).
- `check_tunnel_config` fix hint update was a no-op — that function doesn't exist in the current codebase and was not reintroduced.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] cloudflared.rs missing from codebase**
- **Found during:** Task 2
- **Issue:** `crates/rightclaw/src/codegen/cloudflared.rs`, its test file, and `templates/cloudflared-config.yml.j2` were all deleted in the f37e9da docs/cleanup commit that ran alongside the 38-01 feat commit. The plan assumed these files existed and only needed test additions.
- **Fix:** Restored all three files from git history (`git show 3331d12:...`). Added the two new Phase 38 tests on top of the restored test file.
- **Files modified:** cloudflared.rs, cloudflared_tests.rs, cloudflared-config.yml.j2, mod.rs
- **Commit:** 4434824

**2. [Rule 3 - No-op] check_tunnel_config fix hint not updated**
- **Found during:** Task 1
- **Issue:** Plan said to update `check_tunnel_config` fix hint string, but that function doesn't exist in this codebase. No action taken — the fix hint the plan specified is already present in `check_tunnel_credentials_file` fix string.
- **Fix:** No change needed.

## Known Stubs

None.

## Threat Flags

None — `check_tunnel_credentials_file` prints credentials file path in detail output. T-38-08 in the plan's threat model: accepted (user-owned path, `rightclaw doctor` requires shell access to run).

## Self-Check

Files created/modified:
- FOUND: crates/rightclaw/src/doctor.rs
- FOUND: crates/rightclaw/src/doctor_tests.rs
- FOUND: crates/rightclaw/src/codegen/cloudflared.rs
- FOUND: crates/rightclaw/src/codegen/cloudflared_tests.rs
- FOUND: templates/cloudflared-config.yml.j2

Commits:
- FOUND: b192d33 (feat(38-03): doctor.rs)
- FOUND: 4434824 (feat(38-03): cloudflared.rs)

## Self-Check: PASSED
