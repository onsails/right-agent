---
phase: 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout
plan: "02"
subsystem: mcp-auth-doctor-bot
tags: [doctor, mcp, tracing, auth-state, tunnel-token]
dependency_graph:
  requires: [37-01]
  provides: [auth-required-label, tunnel-token-check, mcp-handler-tracing]
  affects: [rightclaw/doctor, rightclaw/mcp/detect, bot/telegram/handler]
tech_stack:
  added: []
  patterns: [TDD-red-green, tracing-structured-fields]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/detect.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs
    - crates/bot/src/telegram/handler.rs
decisions:
  - "tunnel.hostname() method call was already removed by plan 37-01 — Task 2 hostname fix was a no-op"
  - "doctor_tests.rs uses crate:: prefix (not rightclaw::) since tests are inside the crate"
metrics:
  duration: ~15min
  completed: "2026-04-04"
  tasks_completed: 2
  files_modified: 4
---

# Phase 37 Plan 02: Minor UAT Fixes — AuthState Label, Doctor Tunnel-Token, MCP Handler Tracing Summary

Three independent UAT gaps fixed: AuthState::Missing display label renamed to "auth required", new doctor tunnel-token validity check, and tracing::info! added at entry of all five mcp bot handlers.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| RED | Failing tests for tunnel_token + AuthState | 149ca0e | doctor_tests.rs |
| 1 | AuthState::Missing label + doctor tunnel-token check | ecbc27d | detect.rs, doctor.rs, doctor_tests.rs |
| 2 | tracing::info! in mcp bot handlers | d5c7948 | handler.rs |

## What Was Built

**detect.rs:** `AuthState::Missing` Display now returns `"auth required"` instead of `"missing"`. All existing variant-equality tests unaffected.

**doctor.rs:**
- `check_tunnel_config` fix hint updated from `--tunnel-url URL` to `--tunnel-hostname HOSTNAME`
- New `check_tunnel_token(tunnel_cfg)` function: calls `tunnel_cfg.tunnel_uuid()`, returns Pass with UUID on success, Warn with "cannot extract tunnel UUID" on failure
- `run_doctor()`: after `check_tunnel_config`, reads global config and pushes `check_tunnel_token` if tunnel is configured

**handler.rs:** All five mcp handler entry points now log `tracing::info!` with structured `agent_dir` field (and `server` field for auth/remove). `tunnel.hostname()` method call was already replaced with field access by plan 37-01 — no change needed here.

## Deviations from Plan

### Auto-fixed Issues

None.

### Notes

**Task 2 hostname fix was a no-op:** Plan 37-01 already replaced `tunnel.hostname()` method call with `tunnel.hostname` field access. The handler.rs had `tunnel.hostname.clone()` at line 397 before this plan ran. Confirmed via `grep "tunnel.hostname()"` returning empty.

**Test crate prefix:** Plan template used `rightclaw::config::TunnelConfig` in doctor_tests.rs but tests run inside the `rightclaw` crate — changed to `crate::config::TunnelConfig` (Rule 1 auto-fix during RED→GREEN).

## Verification

```
cargo build --workspace          — clean
grep '"auth required"' detect.rs — match found (line 20)
grep '"missing"' detect.rs       — no match
grep "tunnel-token" doctor.rs    — 4 matches (fn, 2x name string, fix hint)
mcp handler tracing lines        — 11 (>= 5 required)
tunnel.hostname() method call    — no matches
cargo test -p rightclaw --lib -- mcp::detect doctor — 57 passed
```

## Known Stubs

None.

## Threat Flags

None — agent_dir logged at info level is operator-controlled path, no user secrets.

## Self-Check: PASSED

All files and commits verified present.
