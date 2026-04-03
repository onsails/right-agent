---
phase: 35-token-refresh
plan: "03"
subsystem: bot, doctor
tags: [token-refresh, mcp-oauth, doctor, scheduler]
dependency_graph:
  requires: [35-01, 35-02]
  provides: [REFRESH-03, REFRESH-04]
  affects: [crates/bot/src/lib.rs, crates/rightclaw/src/doctor.rs]
tech_stack:
  added: []
  patterns: [fire-and-forget tokio::spawn, testable inner function pattern with _with_creds suffix]
key_files:
  created: []
  modified:
    - crates/bot/src/lib.rs
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw/src/doctor_tests.rs
decisions:
  - check_mcp_tokens uses check_mcp_tokens_with_creds inner function for testability — public wrapper resolves host credentials path, tests inject tempdir path
  - detect.rs exports ServerStatus (not McpStatus as plan interface showed) — plan had stale type name; code uses actual type
metrics:
  duration: ~5m
  completed: "2026-04-03T23:58:52Z"
  tasks_completed: 2
  files_modified: 3
---

# Phase 35 Plan 03: Bot Scheduler Wiring + Doctor mcp-tokens Check Summary

Bot startup now spawns the MCP token refresh scheduler as a fire-and-forget tokio task; `rightclaw doctor` includes an `mcp-tokens` check that surfaces missing/expired tokens per agent.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Spawn refresh scheduler in bot lib.rs | af962a8 | crates/bot/src/lib.rs |
| 2 (RED) | Add failing tests for check_mcp_tokens | d8670fc | crates/rightclaw/src/doctor_tests.rs |
| 2 (GREEN) | Implement check_mcp_tokens doctor check | c99b389 | crates/rightclaw/src/doctor.rs |

## What Was Built

**Task 1 — Bot scheduler wiring:**
- `credentials_path.clone()` captured as `refresh_credentials_path` before it is moved into `OAuthCallbackState`
- `tokio::spawn` of `rightclaw::mcp::refresh::run_refresh_scheduler(agent_dir, creds, http_client)` after existing cron spawn
- Fire-and-forget: scheduler failure does not affect bot operation

**Task 2 — Doctor mcp-tokens check:**
- `check_mcp_tokens_with_creds(home, credentials_path)` — walks `agents/` dir, calls `mcp_auth_status` per agent, aggregates Missing/Expired states into a problem list
- `check_mcp_tokens(home)` — thin wrapper resolving `~/.claude/.credentials.json`
- Called from `run_doctor` after `check_tunnel_config`
- Pass when all tokens present or non-expiring; Warn listing `agent-name/server-name` pairs for problems
- expires_at=0 treated as Present (REFRESH-04 — non-expiring tokens like Linear)

## Test Results

```
running 5 tests
test doctor::tests::check_mcp_tokens_pass_no_agents_dir ... ok
test doctor::tests::check_mcp_tokens_warn_on_missing_token ... ok
test doctor::tests::check_mcp_tokens_nonexpiring_is_ok ... ok
test doctor::tests::check_mcp_tokens_warn_on_expired_token ... ok
test doctor::tests::check_mcp_tokens_pass_when_all_present ... ok
test result: ok. 5 passed; 0 failed
```

Full workspace: all tests pass except pre-existing `test_status_no_running_instance` (documented in STATE.md).

## Deviations from Plan

**1. [Rule 1 - Bug] Plan used stale type name `McpStatus`**
- **Found during:** Task 2 implementation
- **Issue:** Plan `<interfaces>` showed `pub struct McpStatus` but detect.rs exports `pub struct ServerStatus`
- **Fix:** Used `crate::mcp::detect::AuthState` and accessed `s.name` / `s.state` directly on `ServerStatus`
- **Files modified:** crates/rightclaw/src/doctor.rs (inline — no separate fix commit needed)
- **Impact:** None — code compiles and tests pass

## Known Stubs

None — all data paths are wired to real credential files and .mcp.json parsing.

## Self-Check

- [x] crates/bot/src/lib.rs contains `run_refresh_scheduler`
- [x] crates/rightclaw/src/doctor.rs contains `fn check_mcp_tokens` and `checks.push(check_mcp_tokens(home))`
- [x] 5 new check_mcp_tokens tests pass
- [x] `cargo build --workspace` exits 0
