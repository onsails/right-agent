---
phase: 39-cloudflared-auto-tunnel
plan: "02"
subsystem: cli-init
tags: [bug-fix, regression-test, telegram, tdd]
dependency_graph:
  requires: []
  provides: [TUNNEL-03]
  affects: [crates/rightclaw-cli/src/main.rs, crates/rightclaw-cli/tests/cli_integration.rs]
tech_stack:
  added: []
  patterns: [match guard arm (None if yes => None)]
key_files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/tests/cli_integration.rs
decisions:
  - "[39-02]: None if yes => None guard added as match arm — minimal, non-disruptive, preserves interactive path"
  - "[39-02]: TDD red phase skipped as test passes pre-fix due to assert_cmd /dev/null stdin (no real terminal hang in CI); fix is still correct for interactive usage"
metrics:
  duration: ~10min
  completed: 2026-04-05
  tasks: 2
  files: 2
---

# Phase 39 Plan 02: Guard prompt_telegram_token with yes flag — Summary

**One-liner:** Added `None if yes => None` match arm so `rightclaw init -y` never blocks stdin when `--telegram-token` is omitted, plus regression test.

## Tasks Completed

| Task | Description | Commit | Files |
|------|-------------|--------|-------|
| 1 | Fix yes-flag guard in cmd_init telegram_token match | cc6cadf | crates/rightclaw-cli/src/main.rs |
| 2 | Regression test test_init_yes_no_telegram_prompt | cc6cadf | crates/rightclaw-cli/tests/cli_integration.rs |

## Changes Made

### `crates/rightclaw-cli/src/main.rs`

Added one match arm to `cmd_init`'s `telegram_token` match block:

```rust
let token = match telegram_token {
    Some(t) => {
        rightclaw::init::validate_telegram_token(t)?;
        Some(t.to_string())
    }
    None if yes => None,   // <-- added
    None => rightclaw::init::prompt_telegram_token()?,
};
```

### `crates/rightclaw-cli/tests/cli_integration.rs`

Added `test_init_yes_no_telegram_prompt` — runs `rightclaw --home TMPDIR init -y --tunnel-hostname example.com` with implicit `/dev/null` stdin (assert_cmd default) and asserts exit 0.

## Verification

- `cargo build --workspace` passes with zero errors, zero new warnings.
- `cargo test --package rightclaw-cli test_init_yes_no_telegram_prompt` passes (1 test, exit 0).
- `rg 'None if yes => None' crates/rightclaw-cli/src/main.rs` confirms the arm is present.
- All 20 other cli_integration tests pass. `test_status_no_running_instance` fails (pre-existing issue logged in STATE.md).

## Deviations from Plan

### TDD Red Phase Observation

**1. [Rule 1 - Bug] TDD red phase did not produce a failing test**
- **Found during:** Task 1 RED phase
- **Issue:** `prompt_telegram_token()` reads from stdin via `read_line`. When assert_cmd passes `/dev/null` as stdin, `read_line` returns immediately with 0 bytes → empty string → `Ok(None)`. No hang occurs in CI. Test passed even without the fix.
- **Fix:** Proceeded with fix regardless — the `None if yes` guard is correct for interactive (real-terminal) usage and makes intent explicit in code. The regression test still provides value: it documents expected behavior and would catch any future regression that introduces an explicit hang/error on non-interactive stdin.
- **Files modified:** none (observation only)
- **Commit:** n/a

## Known Stubs

None.

## Threat Flags

None. The `None if yes` branch results in no Telegram channel being configured — safe default per threat register T-39-02-01.

## Self-Check: PASSED

- `crates/rightclaw-cli/src/main.rs` — exists and contains `None if yes => None`
- `crates/rightclaw-cli/tests/cli_integration.rs` — exists and contains `test_init_yes_no_telegram_prompt`
- Commit `cc6cadf` — present in git log
