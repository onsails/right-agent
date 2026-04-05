---
phase: 39-cloudflared-auto-tunnel
plan: 01
subsystem: infra
tags: [cloudflared, tunnel, cli, tdd, serde_json, which, dirs]

# Dependency graph
requires:
  - phase: 38-tunnel-refactor
    provides: TunnelConfig struct, write_global_config, check_tunnel_credentials_file in doctor.rs
provides:
  - Auto-detect/create Named Tunnel via cert.pem + cloudflared tunnel list/create
  - detect_cloudflared_cert_with_home + cloudflared_credentials_path_for_home testable helpers
  - find_tunnel_by_name, create_tunnel, route_dns, prompt_yes_no, prompt_hostname helpers
  - --tunnel-name / --tunnel-hostname / -y CLI args replacing --tunnel-credentials-file
affects: [cmd_init, doctor, config, cloudflared-integration]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "_with_home testable variant pattern: helper fn takes explicit home: &Path for unit testing, public wrapper calls dirs::home_dir()"
    - "cloudflared flag placement: --loglevel error is tunnel-level flag, must precede list/create/route subcommand"
    - "TunnelListEntry with no deny_unknown_fields: forward-compatible JSON parsing from cloudflared"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/config.rs

key-decisions:
  - "Phase 39: cert.pem at ~/.cloudflared/ as login signal — absent means skip tunnel setup with info message, not error"
  - "Phase 39: credentials_file in TunnelConfig points to ~/.cloudflared/<uuid>.json directly — no copy to ~/.rightclaw/tunnel/"
  - "Phase 39: tunnel_uuid_from_credentials_file removed entirely — UUID now from tunnel list/create JSON output"
  - "Phase 39: route_dns is non-fatal — CNAME may already exist or zone may differ; log warn and continue"
  - "Phase 39: -y without --tunnel-hostname returns Err when cert.pem present — prevents silent CI hang"

patterns-established:
  - "_with_home testable variant: fn detect_cloudflared_cert_with_home(home: &Path) -> bool; fn detect_cloudflared_cert() wraps it with dirs::home_dir()"
  - "cloudflared subprocess invocation: args=[tunnel, --loglevel, error, <subcommand>, ...]; check output.status.success() before parsing stdout"

requirements-completed: [TUNL-01]

# Metrics
duration: 25min
completed: 2026-04-05
---

# Phase 39 Plan 01: Cloudflared Auto-Tunnel Summary

**Zero-touch Named Tunnel setup via cert.pem detection — rightclaw init auto-detects or creates cloudflared tunnel using tunnel list/create JSON API, removing the manual --tunnel-credentials-file UX from Phase 38**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-04-05T02:15:00Z
- **Completed:** 2026-04-05T02:40:00Z
- **Tasks:** 2 (TDD RED + GREEN)
- **Files modified:** 3

## Accomplishments

- Replaced `--tunnel-credentials-file PATH` with `--tunnel-name NAME` (default: "rightclaw") + `-y/--yes` non-interactive flag
- Implemented auto-detect/create flow: cert.pem absent → info message skip; cert.pem present → find or create Named Tunnel by name
- Removed `tunnel_uuid_from_credentials_file` function and its 3 tests; added 6 new TDD tests (all passing)
- Credentials file now referenced directly at `~/.cloudflared/<uuid>.json` — no copy to `~/.rightclaw/tunnel/` needed
- Updated doctor.rs and config.rs fix hints to reference `--tunnel-name NAME`

## Task Commits

1. **Task 1: Write failing tests for auto-tunnel logic** - `c1dc53a` (test)
2. **Task 2: Implement auto-tunnel — replace Phase 38 UX** - `e54712b` (feat)

## Files Created/Modified

- `/home/wb/dev/rightclaw/crates/rightclaw-cli/src/main.rs` - New CLI args, cmd_init auto-tunnel logic, cloudflared helper fns, updated tests
- `/home/wb/dev/rightclaw/crates/rightclaw/src/doctor.rs` - Updated check_tunnel_credentials_file fix hint
- `/home/wb/dev/rightclaw/crates/rightclaw/src/config.rs` - Updated migration hint in read_global_config

## Decisions Made

- **cert.pem as login signal:** `~/.cloudflared/cert.pem` presence is the authoritative indicator that `cloudflared login` has been run. Absent = skip tunnel setup with info message (not error), enabling users without cloudflared to use rightclaw init normally.
- **No credentials file copy:** Phase 38 copied credentials to `~/.rightclaw/tunnel/<uuid>.json`. Phase 39 references `~/.cloudflared/<uuid>.json` directly — cloudflared already places it there with correct permissions.
- **_with_home testable pattern:** All path-dependent helpers take an explicit `home: &Path` parameter for unit testability; public-facing wrappers call `dirs::home_dir()` and delegate.
- **`--loglevel error` placement:** Must precede the subcommand (`tunnel --loglevel error list`), not follow it (`tunnel list --loglevel error` fails). Verified against live cloudflared 2026.3.0.
- **route_dns non-fatal:** DNS CNAME record may already exist (error 1003) or zone may differ. Failure is logged as warn and execution continues — matching Phase 38 `|| true` behavior.

## Deviations from Plan

None — plan executed exactly as written. TDD RED phase yielded 2 failing tests (not 5-6 as estimated) because the `TunnelListEntry` serde parsing tests passed immediately since the struct definition was already correct in the stub. The plan's "at least 5 of 6" criterion was advisory; the RED state was genuine (2 stubs returned wrong values).

## Issues Encountered

None.

## User Setup Required

None — no external service configuration required. Users need `cloudflared login` pre-run for tunnel setup; rightclaw init gracefully skips tunnel config when cert.pem is absent.

## Next Phase Readiness

- Auto-tunnel init complete; `rightclaw init` now zero-touch for users with cloudflared login
- `rightclaw up` cloudflared block unchanged — reads `TunnelConfig.credentials_file` directly from `~/.cloudflared/<uuid>.json`
- No blockers

---
*Phase: 39-cloudflared-auto-tunnel*
*Completed: 2026-04-05*
