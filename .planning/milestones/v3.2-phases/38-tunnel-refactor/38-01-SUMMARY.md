---
phase: 38-tunnel-refactor
plan: 01
subsystem: infra
tags: [cloudflare-tunnel, config, yaml, serde, migration]

# Dependency graph
requires: []
provides:
  - "New TunnelConfig struct with tunnel_uuid/credentials_file/hostname fields"
  - "GlobalConfig, read_global_config, write_global_config with new YAML format"
  - "Migration error for old token-only configs with re-run hint"
  - "Removed JWT base64 decode method and base64 import"
affects:
  - 38-02  # cmd_init reads TunnelConfig fields
  - 38-03  # cmd_up uses TunnelConfig fields

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "RawTunnelConfig keeps legacy token field as #[serde(default)] for migration detection only — never stored back to disk"
    - "Migration error: credentials_file.is_empty() || tunnel_uuid.is_empty() triggers re-run hint"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/config.rs

key-decisions:
  - "TunnelID read directly from credentials JSON field — no JWT decode needed, struct holds UUID directly"
  - "Old token: field retained in RawTunnelConfig as #[serde(default)] to avoid parse panic, used only for migration error detection"
  - "migration error triggers when credentials_file or tunnel_uuid is empty (covers: token-only, partial new format, empty config)"

patterns-established:
  - "Serde deserialization uses Raw* structs for migration; Real structs never derived Deserialize"

requirements-completed: [TUNL-01]

# Metrics
duration: 8min
completed: 2026-04-05
---

# Phase 38 Plan 01: TunnelConfig Credentials-File Refactor Summary

**Replaced token/JWT TunnelConfig with credentials-file struct (tunnel_uuid + credentials_file + hostname), removing base64 JWT decode entirely and adding backward-compat migration error for old token-only configs.**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-04-05T00:00:00Z
- **Completed:** 2026-04-05T00:08:00Z
- **Tasks:** 1 (TDD: RED then GREEN)
- **Files modified:** 1

## Accomplishments
- New `TunnelConfig` struct with `tunnel_uuid: String`, `credentials_file: PathBuf`, `hostname: String`
- `read_global_config` returns migration error with re-run hint for old token-only configs
- `write_global_config` emits `tunnel_uuid:`/`credentials_file:`/`hostname:` (no `token:`)
- Removed `base64` import and `tunnel_uuid()` JWT decode method entirely
- 6 new TDD tests covering all plan requirements, all 264 rightclaw package tests pass

## Task Commits

1. **Task 1: Replace TunnelConfig struct** - `007e190` (feat)

## Files Created/Modified
- `crates/rightclaw/src/config.rs` - New TunnelConfig struct, GlobalConfig, read/write config, migration error, 6 new tests replacing all JWT-decode tests

## Decisions Made
- TunnelID read directly from credentials JSON `TunnelID` field — no JWT decode needed; storing UUID directly in struct eliminates the entire base64 path
- `RawTunnelConfig` retains legacy `token` field as `#[serde(default)]` to avoid parse panic on old configs; migration error triggered by empty `credentials_file` or `tunnel_uuid` (covers all pre-Phase-38 config formats)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `TunnelConfig` contract established for Plan 02 (cmd_init) and Plan 03 (cmd_up)
- `GlobalConfig`, `read_global_config`, `write_global_config` APIs are stable

---
*Phase: 38-tunnel-refactor*
*Completed: 2026-04-05*
